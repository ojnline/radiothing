#[derive(Clone, Copy)]
pub enum WindowKind {
    BlackmanHaris,
}

impl WindowKind {
    pub fn max_attenuation(&self) -> f64 {
        match *self {
            WindowKind::BlackmanHaris => 92.0,
        }
    }
    pub fn coefficients(&self, buf: &mut [f32]) {
        match *self {
            WindowKind::BlackmanHaris => blackman_haris(buf),
        }
    }
}

use std::f32::consts::PI;

fn cos(buf: &mut [f32], c0: f32, c1: f32, c2: f32, c3: f32) {
    let n = (buf.len() - 1) as f32;

    for i in 0..buf.len() {
        let x = i as f32 / n;

        let c =
            c0 - c1 * (2.0 * PI * x).cos() + c2 * (4.0 * PI * x).cos() - c3 * (6.0 * PI * x).cos();

        // this is cool because i is within 0..buf.len()
        unsafe {
            *buf.get_unchecked_mut(i) = c;
        }
    }
}

fn blackman_haris(buf: &mut [f32]) {
    cos(buf, 0.35874, 0.48829, 0.14128, 0.01168);
}
