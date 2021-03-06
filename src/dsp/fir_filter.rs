use num_traits::{Num, NumOps};

use super::window_functions::WindowKind;

pub struct FirFilter {
    taps: Box<[f32]>,
}

impl FirFilter {
    // construct the truncated ideal impulse response
    // [sin(x)/x for the low pass case]
    // this is a heavily modified function from https://github.com/gnuradio/gnuradio/blob/1f1733eb4489b48fb73509e7806df19e1c738092/gr-filter/lib/firdes.cc#L77
    pub fn new_lowpass(
        gain: f64,
        normalized_cutoff_freq: f64, // normalized frequency center of transition band
        normalized_transition_width: f64, // normalized frequency width of transition band
        window_kind: WindowKind,
    ) -> Self {
        let ntaps = min_tap_count(normalized_transition_width, window_kind);

        // here is first stored the window function which is then multiplied with the sinc function
        let mut buf = vec![0f32; ntaps].into_boxed_slice();
        window_kind.coefficients(&mut *buf);

        use std::f32::consts::PI as PI_f32;
        use std::f64::consts::PI as PI_f64;

        let m = (ntaps as isize - 1) / 2;
        let fw_t0 = (2.0 * PI_f64 * normalized_cutoff_freq) as f32;

        // this integer centering-at-0 was in the original function and I don't dare change it
        // I have no idea whether this makes suboptimal assembly or LLVM is magic
        for n in -m..=m {
            // this is cool because n is -m..=m
            // so 'n + m' is '0..=2m' which is '0..=n-1' because n is always odd 2*((odd - 1) / 2) == odd - 1
            // because it's integer range it's '0..n'    which familiarly touches the whole buffer without overrunning it
            let cr = unsafe { buf.get_unchecked_mut((n + m) as usize) };

            if n == 0 {
                *cr *= fw_t0 / PI_f32;
            } else {
                // a little algebra gets this into the more familiar sin(x)/x form
                *cr *= (n as f32 * fw_t0).sin() / (n as f32 * PI_f32);
            }
        }

        // find the factor to normalize the gain, fmax.
        // For low-pass, gain @ zero freq = 1.0

        let mut fmax = 0.0;

        // add the middle sample
        fmax += buf[m as usize];

        for i in 0..m as usize {
            // m is always < buf.len()
            let t = *unsafe { buf.get_unchecked_mut(i) };
            // both sinc and any window function are symmetric
            // so it is enough to add the one half two times
            fmax += 2.0 * t;
        }

        let normalized_gain = gain as f32 / fmax; // normalize

        for i in 0..buf.len() {
            // i is always < buf.len()
            unsafe {
                *buf.get_unchecked_mut(i) *= normalized_gain;
            };
        }

        Self { taps: buf }
    }
    /// # Safety
    ///
    /// `side_len = (filter_len - 1) / 2`
    ///
    /// It is sound for the `src` and `dst` pointers to alias over the range of the touched memory
    /// hovewer, `dst + side_len` must be lower than or equal to `src` or completely non-overlapping.
    /// Memory will be read from src and then written to dst such that the underlying memory can be completely reused.
    /// The expected use for this is to save `side_len` elements the end of the src memory at (or less if decimation resulted in a weird number remaining)
    /// and then copy it to the beggining when new data is available.
    ///
    /// returns the number of elements that should be saved at the end, in general the value returned will be `len % filter.len()`
    pub unsafe fn apply<T: Num + NumOps<f32> + Copy>(
        &self,
        src: *const T,
        dst: *mut T,
        len: usize,
        decimation: u32,
    ) -> usize {
        let taps = self.taps.len();

        let end = src.add(len);

        let mut src = src;
        // each iteration touches ..taps so in each iteration
        // src + taps <= src + len   # subtract taps
        // src <= src + len - taps   # right side is constant which is good
        let src_end = src.offset(len as isize - taps as isize);
        let mut dst = dst;

        while src <= src_end {
            let mut acc = T::zero();

            // this just computes the scalar product (dot product) of a part of the input data and the filter
            for tap_i in 0..taps {
                let tap = *src.add(tap_i);
                acc = acc + tap * self.taps[tap_i];
            }

            *dst = acc;

            src = src.add(decimation as usize);
            dst = dst.add(1);
        }

        // imaaaagine decimation 10, taps 5
        // xxxxxxxxx|xxxxxxxxx|xxxxxxxxx|xxxxxxxxx|xxxxxxxxx|xx.......|
        // -----     -----     -----     -----     -----     ^^---  it is neccessary to save just the last two elements

        end.offset_from(src).max(0) as usize
    }
    pub fn len(&self) -> usize {
        self.taps.len()
    }
}

// also taken from gnuradio
fn min_tap_count(normalized_transition_width: f64, window_kind: WindowKind) -> usize {
    let a = window_kind.max_attenuation();

    // Fred Harris' rule-of-thumb for estimating filter order
    let mut ntaps = (a / (22.0 * normalized_transition_width)) as usize;

    // ensure ntaps is odd
    if ntaps % 2 == 0 {
        ntaps += 1;
    }

    ntaps
}
