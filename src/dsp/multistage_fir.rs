use std::ops::Range;
use std::rc::Rc;

use num_traits::{Num, NumOps};

use super::fir_filter::FirFilter;
use super::window_functions::WindowKind;

struct MultistageFir<T: Num + NumOps<f32> + Copy> {
    stages: Vec<(u32, u32, Rc<FirFilter>)>, // (decimation, elements in prev_buffer, fir)

    prev_buffer: Vec<T>,
    min_buffer_reserve: usize,
}

const LOWPASS_TRANSITION_WIDTH: f64 = 0.05;

impl<T: Num + NumOps<f32> + Copy> MultistageFir<T> {
    // TODO possibly add a last stage that is computed depending on the factor so that it matches the requested one better
    fn multistage_decimation(
        decimation_factor: u32,
        window_kind: WindowKind,
        cache: &mut Vec<(u32, Rc<FirFilter>)>,
    ) -> (Self, u32) {
        let mut stages = Vec::new();

        let mut current_factor = 1;

        macro_rules! try_factor {
            ($factor:literal) => {
                let mut already_found: Option<usize> = None;
                while current_factor * $factor < decimation_factor {
                    current_factor *= $factor;

                    let fir = if let Some(found) = already_found {
                        cache[found].clone().1
                    } else {
                        if let Some(found) = cache.iter().position(|&(factor, _)| factor == current_factor) {
                            cache[found].clone().1
                        } else {
                            // before any stage the decimation is current_factor which has an effect of "squashing" time
                            // because transition_width is relative to the samplerate, keeping it constant would make it progressively tighter and tighter
                            // to counteract this, transition_width is multiplied with the factor

                            // while keeping it constant wouldn't be incorrect, it would lead to the same number of taps at every stage
                            // this way, less numbers have to be multiplied

                            let fir = Rc::new(FirFilter::new_lowpass(1.0, 0.5, LOWPASS_TRANSITION_WIDTH * current_factor as f64, window_kind));

                            already_found = Some(cache.len());
                            cache.push((current_factor, fir.clone()));

                            fir
                        }
                    };

                    let stage = ($factor, 0, fir);

                    stages.push(stage);
                }
            }
        }

        try_factor!(256);
        try_factor!(128);
        try_factor!(64);
        try_factor!(32);
        try_factor!(16);
        try_factor!(8);
        try_factor!(4);
        try_factor!(2);

        let buffer_len = stages.iter().map(|s| (s.2.len() - 1) / 2).sum();
        let buffer = vec![T::zero(); buffer_len];

        let min_buffer_reserve = stages
            .iter()
            .map(|s| (s.2.len() - 1) / 2)
            .max()
            .expect("stages must not be empty");

        let s = Self {
            stages,
            prev_buffer: buffer,
            min_buffer_reserve,
        };

        (s, current_factor)
    }
    fn add_stage(&mut self, fir: Rc<FirFilter>, decimation: u32) {
        self.min_buffer_reserve = self.min_buffer_reserve.max((fir.len() - 1) / 2);

        self.stages.push((decimation, 0, fir));
    }
    fn apply(&mut self, buffer: &mut [T], buffer_reserve_size: usize) -> Range<usize> {
        assert!(buffer_reserve_size >= self.min_buffer_reserve);

        // rust doesn't really document how valid _pointer_ aliasing is in relation to references
        // here we take a mutable reference so noone else can reference this
        // from here the original reference will not be used so that the compiler doesn't get any funny ideas
        // TODO run this through the miri validator
        let buf_start = buffer.as_mut_ptr();
        let buf_end = unsafe { buf_start.add(buffer.len()) };

        let mut work_buf = unsafe { buf_start.add(buffer_reserve_size) };
        let mut prev_buf = self.prev_buffer.as_mut_ptr();
        let mut elements_count = buffer.len() - buffer_reserve_size;

        for (decimation, prev_elements_ref, fir) in &mut self.stages {
            // for cleanliness
            let prev_elements = *prev_elements_ref as usize;
            let decimation = *decimation as usize;

            let side_len = (fir.len() - 1) / 2;

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
                // prepare
                if work_buf.sub(prev_elements) < buf_start {
                    let shifted = buf_end.sub(elements_count);
                    std::ptr::copy(work_buf, shifted, elements_count);
                    work_buf = shifted;
                }
                work_buf = work_buf.sub(prev_elements);

                // work
                std::ptr::copy_nonoverlapping(prev_buf, work_buf, prev_elements);

                let left = fir.apply(
                    work_buf,
                    work_buf,
                    elements_count + prev_elements,
                    decimation as u32,
                );

                elements_count = (elements_count + prev_elements - side_len) / decimation;

                std::ptr::copy_nonoverlapping(work_buf.add(elements_count - left), prev_buf, left);
                *prev_elements_ref = left as u32;

                // next
                prev_buf = prev_buf.add(side_len);
            }
        }

        let start = unsafe { work_buf.offset_from(buf_start) as usize };

        start..elements_count
    }
    fn min_buffer_reserve(&self) -> usize {
        self.min_buffer_reserve
    }
}
