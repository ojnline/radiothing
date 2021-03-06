use std::ops::Range;
use std::rc::Rc;

use num_traits::{Num, NumOps};

use super::fir_filter::FirFilter;
use super::window_functions::WindowKind;

pub struct MultistageFir<T: Num + NumOps<f32> + Copy> {
    stages: Vec<(u32, u32, Rc<FirFilter>)>, // (decimation, elements in prev_buffer, fir)

    prev_buffer: Vec<T>,
    prev_buffer_needs_resize: bool,
    min_buffer_reserve: usize,
}

const LOWPASS_TRANSITION_WIDTH: f64 = 0.05;

impl<T: Num + NumOps<f32> + Copy> MultistageFir<T> {
    pub fn new() -> Self {
        Self {
            stages: Vec::new(),
            prev_buffer: Vec::new(),
            prev_buffer_needs_resize: false,
            min_buffer_reserve: 0,
        }
    }
    // TODO possibly add a last stage that is computed depending on the factor so that it matches the requested one better
    pub fn new_multistage_decim_imprecise_cached(
        decimation_factor: u32,
        window_kind: WindowKind,
        cache: &mut Vec<(u32, Rc<FirFilter>)>,
    ) -> (Self, u32) {
        let mut s = Self::new();

        let mut current_factor = 1;

        macro_rules! try_factor {
            ($factor:literal) => {
                let mut already_found: Option<&Rc<FirFilter>> = None;
                while current_factor * $factor < decimation_factor {
                    current_factor *= $factor;

                    let fir = if let Some(found) = already_found {
                        found.clone()
                    } else {
                        if let Some(found) = cache.iter().position(|&(factor, _)| factor == $factor)
                        {
                            cache[found].clone().1
                        } else {
                            let fir = Rc::new(FirFilter::new_lowpass(
                                1.0,
                                1.0 / $factor as f64,
                                LOWPASS_TRANSITION_WIDTH,
                                window_kind,
                            ));

                            let next_index = cache.len();
                            cache.push(($factor, fir.clone()));
                            already_found = Some(&cache[next_index].1);

                            fir
                        }
                    };

                    s.add_stage(fir, $factor);
                }
            };
        }

        try_factor!(256);
        try_factor!(128);
        try_factor!(64);
        try_factor!(32);
        try_factor!(16);
        // try_factor!(8);
        // try_factor!(4);
        // try_factor!(2);

        assert!(!s.stages.is_empty(), "stages must not be empty");

        (s, current_factor)
    }
    pub fn new_multistage_decim_precise(
        decimation_factor: u32,
        window_kind: WindowKind,
        cache: &mut Vec<(u32, Rc<FirFilter>)>,
        normalized_cutoff_freq: f64,
        normalized_transition_width: f64,
    ) -> (Self, u32) {
        let (mut filter, achieved_decimation) =
            Self::new_multistage_decim_imprecise_cached(decimation_factor, window_kind, cache);

        let remaining_decimation = decimation_factor / achieved_decimation;

        if remaining_decimation > 0 || normalized_cutoff_freq < 0.5 {
            // because transition_width is relative to the samplerate, keeping it constant would make it progressively tighter and tighter
            // to counteract this, transition_width is multiplied with the current decimation factor

            // while keeping it constant wouldn't be incorrect, it would lead to the same number of taps at every stage
            // this way, less numbers have to be multiplied

            // this is not done with the cached stages because it would make each one unique
            let fir = Rc::new(FirFilter::new_lowpass(
                1.0,
                normalized_cutoff_freq,
                normalized_transition_width,
                window_kind,
            ));
            filter.add_stage(fir, remaining_decimation.max(1));
        }

        (filter, achieved_decimation * remaining_decimation)
    }
    pub fn add_stage(&mut self, fir: Rc<FirFilter>, decimation: u32) {
        self.min_buffer_reserve = self.min_buffer_reserve.max(fir.len() - 1);

        self.stages.push((decimation, 0, fir));

        self.prev_buffer_needs_resize = true;
    }
    pub fn apply(&mut self, buffer: &mut [T], buffer_reserve_size: usize) -> (usize, usize) {
        assert!(buffer_reserve_size >= self.min_buffer_reserve);

        if self.prev_buffer_needs_resize {
            self.resize_prev_buffer();
        }

        // rust doesn't really document how valid _pointer_ aliasing is in relation to references
        // here we take a mutable reference so noone else can reference this
        // from here the original reference will not be used so that the compiler doesn't get any funny ideas
        // TODO run this through the miri validator (I think it validates pointers?)
        let buf_start = buffer.as_mut_ptr();
        let buf_end = unsafe { buf_start.add(buffer.len()) };

        let mut work_buf = unsafe { buf_start.add(buffer_reserve_size) };
        let mut prev_buf = self.prev_buffer.as_mut_ptr();
        let mut elements_count = buffer.len() - buffer_reserve_size;

        // if elements_count == 0 {
        //     return (buffer_reserve_size, 0);
        // }

        for (decimation_ref, prev_elements_ref, fir) in &mut self.stages {
            // for cleanliness
            let prev_elements = *prev_elements_ref as usize;
            let decimation = *decimation_ref;

            // this is the maximum value which can be left unrpocessed by the fir, simply because fir.len() would get processed
            let max_leftover = fir.len() - 1;

            //            prev_buffer[0]                |                         |      |                       |
            //                  |            side_len 0 |                         |      |      fresh data       |
            //                  | trash | prev_elements |                         |      |                       |
            //                  |.......|xxxxxxxxxxxxxxx|                         |......|xxxxxxxxxxxxxxxxx|xxxxx|  stage 0
            //                  |      [1]      ^                                       [4]                |-----|
            //     | side_len 1 |               |                                                             |
            //     |......xxxxxx| stage 1       -------------------------------------------------------------[3]
            //          |      [2]
            //          |
            //  <-------- and so on
            //
            // [0] - `prev_buffer` is the sum of all `side_len`s of the fir stages as that is the maximum count of elements than can be
            //       left over from the previous call of `FirFilter::apply()`. The data will be copied out at the start of stage iteration
            //       and new data back in after `FirFilter::apply()` is called.
            //
            // [1] - Start of the previous elements, they will be copied before [4] in the next iteration.
            //
            // [2] - left of [0] is a chunk of memory for the elements left over from the previous call, they are packed to the right and within range of side_len_0
            //       this pattern repeats for all of the stages, always side_len_$n left from the previous one.
            //
            // [3] - The left over samples - these will be copied to prev_buffer

            unsafe {
                // the leftover elements from the previous apply() need to be included at the beggining of the fresh data
                // check if there is space and if there isn't, move all the elements at the end of the buffer
                // realistically this should only happen once for most `MultistageFir`s
                if work_buf.sub(prev_elements) < buf_start {
                    let shifted = buf_end.sub(elements_count);
                    std::ptr::copy(work_buf, shifted, elements_count);
                    work_buf = shifted;
                }

                work_buf = work_buf.sub(prev_elements);
                // copy the leftover samples before the fresh ones
                std::ptr::copy_nonoverlapping(prev_buf, work_buf, prev_elements);
                // don't forget to adjust the count for the elements that were just copied in
                elements_count += prev_elements;

                let left = fir.apply(work_buf, work_buf, elements_count, decimation);

                // copy the new leftover samples back into prev_buffer
                std::ptr::copy_nonoverlapping(work_buf.add(elements_count - left), prev_buf, left);
                *prev_elements_ref = left as u32;

                // for each n=decimation samples, a new one is written out, pretty much just integer division
                elements_count = elements_count / decimation as usize;

                // shift the prev_buf to the next filter's frame at which the relevant data starts
                prev_buf = prev_buf.add(max_leftover);
            }
        }

        let start = unsafe { work_buf.offset_from(buf_start) as usize };

        (start, elements_count)
    }
    pub fn min_buffer_reserve(&self) -> usize {
        self.min_buffer_reserve
    }
    fn resize_prev_buffer(&mut self) {
        let prev_buffer_len = self.stages.iter().map(|s| s.2.len() - 1).sum();
        self.prev_buffer.resize(prev_buffer_len, T::zero());
        self.prev_buffer_needs_resize = false;
    }
}
