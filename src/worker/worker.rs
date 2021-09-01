use super::worker_manager::ReceiverState;
use crate::{
    decoder::Decoder,
    dsp::fir_filter::FirFilter,
    worker::worker_manager::{ChannelInfo, ValueRanges},
    FftData,
};

use std::{
    error::Error,
    fmt::Display,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
    usize,
};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use rustfft::num_complex::Complex;
use soapysdr::{Args, Device, Direction::Rx, RxStream};

#[derive(Debug)]
pub enum DeviceBoundCommand {
    DestroyDevice, // FIXME is this neccessary
    CreateDevice { index: usize },
    RefreshDevices { args: String },
    SetReceiver(ReceiverState),
    RequestData { data: FftData<RxFormat> },
    SetDecoder { decoder: Box<dyn Decoder> },
}
#[derive(Debug)]
pub enum GuiBoundEvent {
    WorkerReset,
    DeviceCreated { channels_info: Vec<ChannelInfo> },
    DeviceDestroyed,
    Error(soapysdr::Error),
    RefreshedDevices { list: Vec<String> },
    DecodedChars { data: String }, // TODO
    DataReady { data: FftData<RxFormat> },
}

#[derive(Debug)]
enum DeviceWorkerError {
    MainThreadTerminated,
    SoapyError(soapysdr::Error),
    WorkerError(&'static str),
    DecoderError(&'static str),
}

impl Display for DeviceWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceWorkerError::MainThreadTerminated => {
                writeln!(f, "The main thread has terminated before the worker.")
            }
            DeviceWorkerError::SoapyError(e) => writeln!(f, "SoapySDR Error: {}", e),
            DeviceWorkerError::WorkerError(e) => writeln!(f, "Worker Error: {}", e),
            DeviceWorkerError::DecoderError(e) => writeln!(f, "Decoder Error: {}", e),
        }
    }
}

impl Error for DeviceWorkerError {}

impl From<soapysdr::Error> for DeviceWorkerError {
    fn from(error: soapysdr::Error) -> Self {
        DeviceWorkerError::SoapyError(error)
    }
}

impl<T> From<crossbeam_channel::SendError<T>> for DeviceWorkerError {
    fn from(_: crossbeam_channel::SendError<T>) -> Self {
        DeviceWorkerError::MainThreadTerminated
    }
}

impl From<&'static str> for DeviceWorkerError {
    fn from(error: &'static str) -> Self {
        DeviceWorkerError::WorkerError(error)
    }
}

const INITIAL_RECEIVE_SIZE: usize = 16 * 1024;
const RECEIVE_TIMEOUT_US: i64 = 200_000; // 200 miliseconds
pub type RxFormat = f32;

pub struct DeviceWorker {
    // this is an atomic bool rather than a message in the channel because there may be multiple data requests queued at a time
    // this was mostly implemented to quickly react to
    pub(crate) receive_enable_flag: Arc<AtomicBool>,
    pub(crate) receive_stream_active: bool,

    pub(crate) receiver: Receiver<DeviceBoundCommand>,
    pub(crate) sender: Sender<GuiBoundEvent>,

    pub(crate) available_devices: Option<Vec<Args>>,
    pub(crate) device: Option<Device>,

    pub(crate) receive_state: Option<ReceiverState>,
    pub(crate) receive_stream: Option<RxStream<Complex<RxFormat>>>,

    pub(crate) decoder: Option<Box<dyn Decoder>>,

    pub(crate) working_memory: Vec<Complex<RxFormat>>,
    pub(crate) memory_receive_offset: usize,

    // TODO expand the caching mechanism or throw it away
    pub(crate) decimation_fir_cache: Vec<(u32, Rc<FirFilter>)>,
}

// TODO receive mtu() counts of data for maximum efficiency

impl DeviceWorker {
    pub fn new(
        receiver: Receiver<DeviceBoundCommand>,
        sender: Sender<GuiBoundEvent>,
        receive_enable_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            receive_enable_flag,
            receive_stream_active: false,
            receiver,
            sender,
            available_devices: None,
            device: None,
            receive_state: None,
            receive_stream: None,
            decoder: None,
            working_memory: vec![Default::default(); INITIAL_RECEIVE_SIZE],
            memory_receive_offset: 0,
            decimation_fir_cache: Vec::new(),
        }
    }
    fn error_process(&mut self) -> Result<(), DeviceWorkerError> {
        fn clone_args(a: &Args) -> Args {
            let mut c = Args::new();
            for (k, v) in a {
                c.set(k, v)
            }
            c
        }

        loop {
            let event = match self.receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(DeviceWorkerError::MainThreadTerminated)
                }
            };

            let receive = self.receive_enable_flag.load(Ordering::SeqCst);

            // react to change in receive_enable_flag
            if let Some(stream) = self.receive_stream.as_mut() {
                match (receive, self.receive_stream_active) {
                    (true, false) => {
                        stream.activate(None)?;
                        self.receive_stream_active = true;
                    }
                    (false, true) => {
                        stream.deactivate(None)?;
                        self.receive_stream_active = false;
                    }
                    _ => {}
                }
            }

            if self.receive_stream.is_some() && self.receive_stream_active {
                let dst = &mut self.working_memory[self.memory_receive_offset..];
                let _read_count = self
                    .receive_stream
                    .as_mut()
                    .unwrap()
                    .read(&mut [dst], RECEIVE_TIMEOUT_US)?;
            }

            if let Some(event) = event {
                match event {
                    DeviceBoundCommand::CreateDevice { index } => {
                        assert!(self.device.is_none());
                        assert!(self.available_devices.is_some());

                        let args = clone_args(&self.available_devices.as_ref().unwrap()[index]);

                        log::info!("Creating device ({})", args);
                        let dev = Device::new(args)?;

                        let num_channels = dev.num_channels(Rx)?;
                        let mut channels_info = Vec::with_capacity(num_channels as usize);

                        for i in 0..dev.num_channels(Rx)? {
                            let info = dev
                                .channel_info(Rx, i)?
                                .into_iter()
                                .map(|(key, value)| (key.to_string(), value.to_string()))
                                .collect();

                            let ranges = ValueRanges {
                                samplerate: dev.get_sample_rate_range(Rx, i)?,
                                bandwidth: dev.bandwidth_range(Rx, i)?,
                                frequency: dev.frequency_range(Rx, i)?,
                                gain: dev.gain_range(Rx, i)?,
                            };

                            channels_info.push(ChannelInfo { ranges, info })
                        }

                        self.sender
                            .send(GuiBoundEvent::DeviceCreated { channels_info })?;
                        self.device = Some(dev);
                    }
                    DeviceBoundCommand::DestroyDevice => {
                        self.receive_enable_flag.store(false, Ordering::SeqCst);
                        self.receive_stream_active = false;
                        self.receive_stream = None;
                        self.receive_state = None;
                        self.device = None;
                        self.decoder = None;

                        self.sender.send(GuiBoundEvent::DeviceDestroyed)?;
                    }
                    DeviceBoundCommand::RefreshDevices { args } => {
                        let available = soapysdr::enumerate(args.as_str())?;
                        let names = available
                            .iter()
                            .map(|d| d.get("label").unwrap().to_owned())
                            .collect::<Vec<_>>();

                        // the refresh request is possibly sent very frequently if auto_select is true
                        // avoid spamming empty messages if there is nothing to report
                        if !names.is_empty() {
                            log::info!("Available devices: {:#?}", names);
                        }

                        self.available_devices = Some(available);

                        self.sender
                            .send(GuiBoundEvent::RefreshedDevices { list: names })?;
                    }
                    DeviceBoundCommand::SetReceiver(state) => {
                        assert!(self.device.is_some());

                        log::trace!("Configuring receiver:\n{:#?}", state);

                        let ReceiverState {
                            channel,
                            samplerate,
                            frequency,
                            bandwidth,
                            gain,
                            automatic_gain,
                            automatic_dc_offset,
                        } = state.clone();

                        // this is because changing channels after the device was created is unimplemented
                        // and would result in weirdness, currently it's fine as it is hardcoded on the other side to 0
                        assert!(channel == 0, "Currently channel is hardcoded as 0");

                        let dev = self.device.as_ref().unwrap();

                        // it is seemingly not neccessary to deactivate the stream before configuring
                        // I'm still going to keep this code here

                        // if self.receive_stream.is_some() && self.receive_stream_active {
                        //     // deactivate the stream before configuring
                        //     self.receive_stream.as_mut().unwrap().deactivate(None)?;
                        // }

                        // this is the first SetReceiver command after this Device was created
                        if self.receive_state.is_none() {
                            let antenna = dev
                                .antennas(Rx, channel)?
                                .pop()
                                .ok_or("No receiving antennas on device.")?; // I know it should be antennae

                            log::debug!("Selecting antenna '{}'", antenna);

                            dev.set_antenna(Rx, channel, antenna)?;

                            let stream = dev.rx_stream(&[channel])?;
                            self.receive_stream = Some(stream);
                        }

                        // compares the new state to the one currently set and if they differ (or the previous state is unset, this is why it's so ugly) run the block
                        macro_rules! if_differs {
                                ($($var:ident, $then:expr);+ $(;)?) => {
                                    $(
                                        if Some($var) != self.receive_state.as_ref().map(|s| s.$var) {
                                            $then;
                                        }
                                    );+
                                }
                            }

                        // this is the result of excessive bikeshedding
                        if_differs!(
                            automatic_gain, dev.set_gain_mode(Rx, channel, automatic_gain)?;
                            automatic_dc_offset, dev.set_dc_offset_mode(Rx, channel, automatic_dc_offset)?;
                            gain,       dev.set_gain(Rx, channel, gain)?;
                            frequency,  dev.set_frequency(Rx, channel, frequency, ())?; // FIXME are the args neccessary for anything?
                            samplerate, dev.set_sample_rate(Rx, channel, samplerate)?;
                            bandwidth,  dev.set_bandwidth(Rx, channel, bandwidth)?;
                        );

                        // if self.receive_stream_active {
                        //     self.receive_stream.as_mut().unwrap().activate(None)?;
                        // }

                        self.receive_state = Some(state);

                        // everyone loves the option dance (yes it's actually called that)
                        if let Some(mut decoder) = self.decoder.take() {
                            decoder
                                .configuration_changed(self)
                                .map_err(|e| DeviceWorkerError::DecoderError(e))?;

                            self.decoder = Some(decoder);
                        }
                    }
                    DeviceBoundCommand::RequestData { mut data } => {
                        assert!(self.receive_stream.is_some());
                        assert!(data.get_input().len() <= self.working_memory.len());

                        let len = data.get_input().len();
                        let offset = self.memory_receive_offset;
                        data.get_input_mut()
                            .copy_from_slice(&mut self.working_memory[offset..(len + offset)]);
                        data.process();

                        self.sender.send(GuiBoundEvent::DataReady { data })?;
                    }
                    DeviceBoundCommand::SetDecoder { mut decoder } => {
                        let prev = self.decoder.take();
                        decoder
                            .init(self, prev)
                            .map_err(|e| DeviceWorkerError::DecoderError(e))?;
                        self.decoder = Some(decoder);
                    }
                }
            }

            if self.receive_stream.is_some() && self.receive_stream_active {
                // println!("BBBBBB");
                // this horrible thing is needed to satisfy the borrowchecker
                // since Option<Box<_>> is just a pointer, it's very cheap to move even if it isn't optimized away
                if let Some(mut decoder) = self.decoder.take() {
                    decoder
                        .process(self)
                        .map_err(|e| DeviceWorkerError::DecoderError(e))?;

                    self.decoder = Some(decoder);
                }
            }
        }
    }
    pub fn process(mut self) {
        loop {
            let result = self.error_process();

            match result {
                Err(DeviceWorkerError::MainThreadTerminated) => return,
                Err(DeviceWorkerError::SoapyError(e)) => {
                    if let Err(_) = self.sender.send(GuiBoundEvent::Error(e)) {
                        return;
                    }
                }
                _ => unreachable!(), // the error_process() function only ever returns through null coalescing operators and as such always an error
            }

            // sleep some time so that the main thread has time to handle the error and possibly disable this thread
            // that can happen for example after an error because the receive stream is misconfigured with a wrong frequency
            // this is not a race condition hack per se it just keeps the worker from trigerring the error again before it is handled
            std::thread::sleep(std::time::Duration::from_millis(110));
        }
    }
}
