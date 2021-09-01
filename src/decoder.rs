use std::{any::Any, fmt::Debug, rc::Rc};

use crate::{dsp::{fir_filter::FirFilter, rtty_decode}, worker::worker::{DeviceWorker, GuiBoundEvent}};

pub type DecoderResult<T> = Result<T, &'static str>;

pub trait Decoder: Send + Any + Debug {
    fn init(
        &mut self,
        worker: &mut DeviceWorker,
        prev: Option<Box<dyn Decoder>>,
    ) -> DecoderResult<()>;
    fn configuration_changed(&mut self, worker: &mut DeviceWorker) -> DecoderResult<()>;
    fn process(&mut self, worker: &mut DeviceWorker) -> DecoderResult<()>;
}

#[derive(Debug)]
pub struct BaudotDecoder {
    baudrate: f32,
    stop_bits: f32,
    // these are reclaimed from the previous BaudotDecoder if there was any
    letters: bool,
    leftover_bits: Vec<bool>,
}

impl BaudotDecoder {
    pub fn new(baudrate: f32, stop_bits: f32) -> Self {
        Self {
            baudrate,
            stop_bits,
            letters: true,
            leftover_bits: Vec::new(),
        }
    }
}

impl Decoder for BaudotDecoder {
    fn init(
        &mut self,
        worker: &mut DeviceWorker,
        prev: Option<Box<dyn Decoder>>,
    ) -> DecoderResult<()> {
        // TODO actually setup decimation
        Ok(())
    }

    fn configuration_changed(&mut self, worker: &mut DeviceWorker) -> DecoderResult<()> {
        // TODO actually recompute the decimation filter
        Ok(())
    }

    fn process(&mut self, worker: &mut DeviceWorker) -> DecoderResult<()> {

        let samplerate = worker.receive_state.as_mut().unwrap().samplerate as f32;

        // todo actually restore the previous samples
        let (string, _, _) = unsafe {
            // println!("AAAAA");
            let a = rtty_decode::decode(worker.working_memory.as_ptr(), worker.working_memory.len(), worker.working_memory.as_mut_ptr() as *mut bool, 0, self.baudrate, samplerate, &mut self.letters);
            // println!("BBBB");
            a
        };

        let _ = worker.sender.send(GuiBoundEvent::DecodedChars { data: string });

        Ok(())
    }
}
