use std::{
    cell::RefCell,
    error::Error,
    fmt::Display,
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
    usize,
};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use rustfft::num_complex::Complex;
use soapysdr::{Args, Device, Direction::Rx, Range, RxStream};

use crate::FftData;

#[derive(Clone, Debug)]
pub enum DeviceBoundCommand {
    DestroyDevice, // FIXME is this neccessary
    CreateDevice { index: usize },
    RefreshDevices { args: String },
    SetReceiver(ReceiverState),
    RequestData { data: FftData<RxFormat> },
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

#[derive(Clone, Debug, PartialEq)]
pub struct ReceiverState {
    pub channel: usize,
    pub samplerate: f64,
    pub frequency: f64,
    pub bandwidth: f64,
    pub gain: f64,
    pub automatic_gain: bool,
    pub automatic_dc_offset: bool,
}

#[derive(Debug)]
pub struct ChannelInfo {
    pub ranges: ValueRanges,
    pub info: Vec<(String, String)>, // (key, value)
}

#[derive(Clone, Debug)]
pub struct ValueRanges {
    pub samplerate: Vec<Range>,
    pub frequency: Vec<Range>,
    pub bandwidth: Vec<Range>,
    pub gain: Range,
}
#[derive(Clone, Debug)]
pub enum DeviceError {
    BadState,
    WorkerPoisoned,
}
impl Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::BadState => writeln!(f, "The application is in a bad state."),
            DeviceError::WorkerPoisoned => writeln!(f, "The receive thread has panicked."),
        }
    }
}
impl Error for DeviceError {}

#[derive(Clone, Debug)]
pub struct WorkerPoisoned;
impl Display for WorkerPoisoned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "The receive thread has panicked.")
    }
}
impl Error for WorkerPoisoned {}

struct InnerDeviceManager {
    pub(crate) thread: ManuallyDrop<JoinHandle<()>>,
    pub(crate) sender: ManuallyDrop<Sender<DeviceBoundCommand>>,
    pub(crate) receive_enable_flag: Arc<AtomicBool>,
    pub(crate) receiver: ManuallyDrop<Receiver<GuiBoundEvent>>,

    pub(crate) device_valid: bool,
    pub(crate) receiver_valid: bool,
    pub(crate) refreshing_devices: bool,
    pub(crate) data_requests_in_flight: usize,
}

impl InnerDeviceManager {
    fn new() -> Self {
        let (gui_sender_channel, gui_receive_channel) = crossbeam_channel::unbounded();
        let (device_sender_channel, device_receive_channel) = crossbeam_channel::unbounded();

        let receive_enable_flag = Arc::new(AtomicBool::new(false));
        let receive_enable_flag_c = receive_enable_flag.clone();

        let thread = thread::Builder::new()
            .name("Worker thread".to_owned())
            .spawn(move || {
                let worker = DeviceWorker {
                    receive_stream_active: false,
                    receive_enable_flag,
                    receiver: device_receive_channel,
                    sender: gui_sender_channel,
                    available_devices: None,
                    device: None,
                    receive_state: None,
                    receive_stream: None,
                    working_memory: Box::new([Default::default(); RECEIVE_SIZE]),
                };

                worker.process();
            })
            .unwrap();

        Self {
            thread: ManuallyDrop::new(thread),
            sender: ManuallyDrop::new(device_sender_channel),
            receive_enable_flag: receive_enable_flag_c,
            receiver: ManuallyDrop::new(gui_receive_channel),

            device_valid: false,
            receiver_valid: false,
            refreshing_devices: false,
            data_requests_in_flight: 0,
        }
    }
    fn send_command(&mut self, command: DeviceBoundCommand) -> Result<(), DeviceError> {
        match &command {
            DeviceBoundCommand::DestroyDevice => {
                if self.device_valid {
                    self.device_valid = false;
                    self.receiver_valid = false;
                } else {
                    return Err(DeviceError::BadState);
                }
            }
            DeviceBoundCommand::CreateDevice { .. } => {
                if self.device_valid {
                    return Err(DeviceError::BadState);
                } else {
                    self.device_valid = true;
                }
            }
            &DeviceBoundCommand::RequestData { .. } => self.data_requests_in_flight += 1,
            DeviceBoundCommand::RefreshDevices { .. } => self.refreshing_devices = true,
            DeviceBoundCommand::SetReceiver(_) => self.receiver_valid = true,
        }

        self.sender
            .send(command)
            .map_err(|_| DeviceError::WorkerPoisoned)
    }
    fn try_receive(&mut self) -> Result<Option<GuiBoundEvent>, WorkerPoisoned> {
        let event = self.receiver.try_recv();

        if let Ok(event) = event.as_ref() {
            match event {
                GuiBoundEvent::RefreshedDevices { .. } => self.refreshing_devices = false,
                GuiBoundEvent::DataReady { .. } => self.data_requests_in_flight -= 1,
                _ => (),
            }
        }

        match event {
            Ok(event) => return Ok(Some(event)),
            Err(TryRecvError::Disconnected) => return Err(WorkerPoisoned),
            Err(TryRecvError::Empty) => return Ok(None),
        }
    }
}

impl Drop for InnerDeviceManager {
    fn drop(&mut self) {
        // first ensure that both of the channels close
        // on the worker thread this makes it exit it's toplevel function
        unsafe {
            ManuallyDrop::drop(&mut self.receiver);
            ManuallyDrop::drop(&mut self.sender);
        }

        let thread = unsafe { ManuallyDrop::take(&mut self.thread) };

        // after the thread has exited it can be joined
        let _ = thread.join();
    }
}

pub struct DeviceManager(RefCell<InnerDeviceManager>);
impl DeviceManager {
    pub fn new() -> Self {
        Self(RefCell::new(InnerDeviceManager::new()))
    }
    pub fn get_device_valid(&self) -> bool {
        self.0.borrow().device_valid
    }
    pub fn get_receiver_valid(&self) -> bool {
        self.0.borrow().receiver_valid
    }
    pub fn get_refreshing_devices(&self) -> bool {
        self.0.borrow().refreshing_devices
    }
    pub fn get_data_requests_in_flight(&self) -> usize {
        self.0.borrow().data_requests_in_flight
    }
    pub fn send_command(&self, command: DeviceBoundCommand) -> Result<(), DeviceError> {
        self.0.borrow_mut().send_command(command)
    }
    pub fn try_receive(&self) -> Result<Option<GuiBoundEvent>, WorkerPoisoned> {
        self.0.borrow_mut().try_receive()
    }

    pub fn set_receive_enabled(&self, enabled: bool) {
        self.0
            .borrow()
            .receive_enable_flag
            .store(enabled, Ordering::SeqCst);
    }

    pub fn reset(&self) {
        let ptr = self.0.as_ptr();

        unsafe {
            ptr.drop_in_place();
            let new = InnerDeviceManager::new();
            ptr.write(new);
        }
    }
}

const RECEIVE_SIZE: usize = 16 * 1024;
const RECEIVE_TIMEOUT_US: i64 = 200_000; // 200 miliseconds
pub type RxFormat = f32;

#[derive(Clone, Debug)]
enum DeviceWorkerError {
    MainThreadTerminated,
    SoapyError(soapysdr::Error),
    WorkerError(&'static str),
}

impl Display for DeviceWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceWorkerError::MainThreadTerminated => {
                writeln!(f, "The main thread has terminated before the worker.")
            }
            DeviceWorkerError::SoapyError(e) => writeln!(f, "SoapySDR Error: {}", e),
            DeviceWorkerError::WorkerError(e) => writeln!(f, "Worker Error: {}", e),
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

struct DeviceWorker {
    // this is an atomic bool rather than a message in the channel because there may be multiple data requests queued at a time
    // this was mostly implemented to quickly react to
    receive_enable_flag: Arc<AtomicBool>,
    receive_stream_active: bool,

    receiver: Receiver<DeviceBoundCommand>,
    sender: Sender<GuiBoundEvent>,

    available_devices: Option<Vec<Args>>,
    device: Option<Device>,

    receive_state: Option<ReceiverState>,
    receive_stream: Option<RxStream<Complex<RxFormat>>>,

    // TODO revamp the way data is read and decoded
    working_memory: Box<[Complex<RxFormat>; RECEIVE_SIZE]>,
}

impl DeviceWorker {
    fn error_process(&mut self) -> Result<(), DeviceWorkerError> {
        fn clone_args(a: &Args) -> Args {
            let mut c = Args::new();
            for (k, v) in a {
                c.set(k, v)
            }
            c
        }

        loop {
            // this will block until there is a command available, this is hopefully implemented with the proper primitives so the thread won't spin the cpu endlessly
            let event = match self.receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(DeviceWorkerError::MainThreadTerminated)
                }
            };

            // if let Some(event) = &event {
            //     dbg!(event);
            // }

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
                    }
                    DeviceBoundCommand::RequestData { mut data } => {
                        assert!(self.receive_stream.is_some());
                        assert!(data.get_input().len() <= self.working_memory.len());

                        let len = data.get_input().len();
                        data.get_input_mut()
                            .copy_from_slice(&mut self.working_memory[0..len]);
                        data.process();

                        self.sender.send(GuiBoundEvent::DataReady { data })?;
                    }
                }
            }

            if self.receive_stream.is_some() && self.receive_stream_active {
                let _read_count = self
                    .receive_stream
                    .as_mut()
                    .unwrap()
                    .read(&mut [self.working_memory.as_mut()], RECEIVE_TIMEOUT_US)?;
            }
        }
    }
    fn process(mut self) {
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
