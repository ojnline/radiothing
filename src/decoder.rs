use std::{any::Any, fmt::Debug, mem::size_of, ops::Add, rc::Rc};

use num_traits::Zero;
use rustfft::num_complex::Complex;

use crate::{
    dsp::{
        fir_filter::FirFilter, multistage_fir::MultistageFir, rtty_decode,
        window_functions::WindowKind,
    },
    worker::worker::{DeviceWorker, GuiBoundEvent, RxFormat},
};

pub type DecoderResult<T> = Result<T, &'static str>;

#[derive(Debug)]
pub enum Decoder {
    BaudotDecoder {
        baudrate: f32,
        stop_bits: f32,
        shift: f32,
        // these are reclaimed from the previous BaudotDecoder if there was any
        letters: bool,
        leftover_bits: Vec<bool>,
        // relevant after init on worker
        decim: u32,
    },
}

impl Decoder {
    pub fn init(&mut self, _worker: &mut DeviceWorker, prev: Option<Self>) -> DecoderResult<()> {
        // I really like bikeshedding macros
        macro_rules! reclaim_fields {
            ($($variant:path$ (,$field:ident)*;)+) => {
                match self {
                    $(
                        $variant { $($field,)* ..} => {
                            // this results in a load of ifs but it would be huge pain to do otherwise
                            // this will most likely get optimized out because of the unreachable_unchecked()
                            // [0]
                            if let Some($variant {..}) = prev {
                                $(
                                    match prev {
                                        Some($variant { $field: placeholder, ..}) => *$field = placeholder,
                                        _ => unsafe {
                                            // the condition was already checked in the higher if [0]
                                            // this is only syntax hacking
                                            std::hint::unreachable_unchecked()
                                        }
                                    }
                                )*
                            }
                        }
                    )+
                }
            }
        }

        reclaim_fields! {
            Decoder::BaudotDecoder, letters, leftover_bits;
        }

        Ok(())
    }

    pub fn configuration_changed(
        &mut self,
        worker: &mut DeviceWorker,
        _during_init: bool,
    ) -> DecoderResult<()> {
        let state = worker.receive_state.as_ref().unwrap();

        match self {
            Decoder::BaudotDecoder {
                shift,
                baudrate,
                decim,
                ..
            } => {
                let target_samplerate = (*baudrate as f64 * 16.0).max(1.0);
                let factor = (state.samplerate / target_samplerate) as u32;

                let cutoff = *shift as f64 / (2.0 * state.samplerate);
                let (filter, factor) = MultistageFir::new_multistage_decim_precise(
                    factor,
                    WindowKind::BlackmanHaris,
                    &mut worker.decimation_fir_cache,
                    cutoff,
                    0.1,
                );

                worker
                    .working_memory
                    .resize(worker.mtu + filter.min_buffer_reserve(), Complex::zero());
                worker.memory_receive_offset = worker
                    .memory_receive_offset
                    .max(filter.min_buffer_reserve());

                worker.current_fir_filter = Some(filter);
                *decim = factor;
            }
        }
        Ok(())
    }

    pub fn process(&mut self, worker: &mut DeviceWorker) -> DecoderResult<()> {
        match self {
            Decoder::BaudotDecoder {
                baudrate,
                stop_bits,
                letters,
                leftover_bits,
                ..
            } => {
                let samplerate = worker.receive_state.as_mut().unwrap().samplerate as f32;

                let filter = worker.current_fir_filter.as_mut().unwrap();
                let (start, count) = filter.apply(
                    &mut worker.working_memory[..worker.memory_received_count],
                    worker.memory_receive_offset,
                );

                if leftover_bits.len() * size_of::<bool>() > start * size_of::<Complex<RxFormat>>()
                {
                    // integer division which rounds up
                    // taken from https://stackoverflow.com/questions/17944/how-to-round-up-the-result-of-integer-division
                    // size of the bits data in the original vector type
                    let bits_len_as_complex = (leftover_bits.len() * size_of::<bool>() - 1)
                        / size_of::<Complex<RxFormat>>()
                        + 1;
                    let min_len = bits_len_as_complex + count;

                    if worker.working_memory.len() < min_len {
                        worker.working_memory.resize(min_len, Complex::zero())
                    }

                    unsafe {
                        let buf = worker.working_memory.as_mut_ptr();
                        let src = buf.add(start);
                        let dst = buf.add(bits_len_as_complex);
                        std::ptr::copy(src, dst, count);
                    }
                }

                unsafe {
                    let dst = worker.working_memory.as_mut_ptr() as *mut bool;
                    std::ptr::copy_nonoverlapping(
                        leftover_bits.as_mut_ptr(),
                        dst,
                        leftover_bits.len(),
                    );
                }

                let (string, _, _) = unsafe {
                    rtty_decode::decode(
                        worker.working_memory.as_ptr(),
                        worker.working_memory.len(),
                        worker.working_memory.as_mut_ptr() as *mut bool,
                        leftover_bits.len(),
                        *baudrate,
                        *stop_bits,
                        samplerate,
                        letters,
                    )
                };

                if !string.is_empty() {
                    let _ = worker
                        .sender
                        .send(GuiBoundEvent::DecodedChars { data: string });
                }
            }
        }

        Ok(())
    }

    pub fn new_baudot(baudrate: f32, stop_bits: f32, shift: f32) -> Self {
        Self::BaudotDecoder {
            baudrate,
            stop_bits,
            shift,
            letters: true,
            leftover_bits: Vec::new(),
            decim: 0,
        }
    }
}
