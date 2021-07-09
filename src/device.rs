use core::f32;
use std::{
    cell::{Cell, RefCell},
    error::Error,
    iter::FromIterator,
    mem::ManuallyDrop,
    ops::Deref,
    sync::{
        atomic::{
            AtomicBool,
            Ordering::{Acquire, Release},
        },
        mpsc::{channel, Receiver, Sender, TryRecvError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
    usize,
};

use rustfft::{
    num_complex::{Complex, Complex64},
    Fft, FftPlanner,
};
use soapysdr::{Args, Device, Direction::Rx, Range, RxStream};

use crate::FftData;

#[derive(Clone, Debug)]
pub enum DeviceBoundCommand {
    DestroyDevice,
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
    Error { desc: String, fatal: bool },
    RefreshedDevices { list: Vec<String> },
    DecodedChars { data: String },
    DataReady { data: FftData<RxFormat> },
}

#[derive(Clone, Debug)]
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
    pub bandwidth: Vec<Range>,
    pub frequency: Vec<Range>,
    pub gain: Range,
}
#[derive(Debug)]
pub enum DeviceError {
    BadState,
    WorkerPoisoned,
}

struct InnerDeviceManager {
    thread: JoinHandle<()>,
    sender: Sender<DeviceBoundCommand>,
    receiver: Receiver<GuiBoundEvent>,

    device_valid: bool,
    receiver_valid: bool,
    refreshing_devices: bool,
    get_data_requests_in_flight: usize,
}

impl InnerDeviceManager {
    fn new() -> Self {
        let (gui_sender_channel, gui_receive_channel) = channel();
        let (device_sender_channel, device_receive_channel) = channel();

        let thread = thread::Builder::new()
        .name("Worker thread".to_owned())
        .spawn(move || {
            let worker = DeviceWorker {
                receiver: device_receive_channel,
                sender: gui_sender_channel,
                available_devices: None,
                device: None,
                receive_state: None,
                receive_stream: None,
                valid_count: 0,
                working_memory: Box::new([Default::default(); RECEIVE_SIZE]),
                fft_planner: FftPlanner::new(),
            };

            worker.process();
        }).unwrap();

        Self {
            thread,
            sender: device_sender_channel,
            receiver: gui_receive_channel,

            device_valid: false,
            receiver_valid: false,
            refreshing_devices: false,
            get_data_requests_in_flight: 0,
        }
    }
    fn get_device_valid(&mut self) -> bool {
        self.device_valid
    }
    fn get_receiver_valid(&mut self) -> bool {
        self.receiver_valid
    }
    fn get_refreshing_devices(&mut self) -> bool {
        self.refreshing_devices
    }
    fn get_data_requests_in_flight(&mut self) -> usize {
        self.get_data_requests_in_flight
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
            &DeviceBoundCommand::RequestData { .. } => self.get_data_requests_in_flight += 1,
            DeviceBoundCommand::RefreshDevices { .. } => self.refreshing_devices = true,
            DeviceBoundCommand::SetReceiver(_) => self.receiver_valid = true,
            _ => (),
        }

        self.sender
            .send(command)
            .map_err(|_| DeviceError::WorkerPoisoned)
    }
    fn try_receive(&mut self) -> Result<Option<GuiBoundEvent>, DeviceError> {
        let event = self.receiver.try_recv();

        if let Ok(event) = event.as_ref() {
            match event {
                GuiBoundEvent::RefreshedDevices { .. } => self.refreshing_devices = false,
                GuiBoundEvent::DataReady { .. } => self.get_data_requests_in_flight -= 1,
                _ => (),
            }
        }

        match event {
            Ok(event) => return Ok(Some(event)),
            Err(TryRecvError::Disconnected) => return Err(DeviceError::WorkerPoisoned),
            Err(TryRecvError::Empty) => return Ok(None),
        }
    }
}

pub struct DeviceManager(RefCell<InnerDeviceManager>);

impl DeviceManager {
    pub fn new() -> Self {
        Self(RefCell::new(InnerDeviceManager::new()))
    }
    pub fn get_device_valid(&self) -> bool {
        self.0.borrow_mut().get_device_valid()
    }
    pub fn get_receiver_valid(&self) -> bool {
        self.0.borrow_mut().get_receiver_valid()
    }
    pub fn get_refreshing_devices(&self) -> bool {
        self.0.borrow_mut().get_refreshing_devices()
    }
    pub fn get_data_requests_in_flight(&self) -> usize {
        self.0.borrow_mut().get_data_requests_in_flight()
    }
    pub fn send_command(&self, command: DeviceBoundCommand) -> Result<(), DeviceError> {
        self.0.borrow_mut().send_command(command)
    }
    pub fn try_receive(&self) -> Result<Option<GuiBoundEvent>, DeviceError> {
        self.0.borrow_mut().try_receive()
    }

    pub fn reset(&self) {
        *self.0.borrow_mut() = InnerDeviceManager::new();
    }
}

const RECEIVE_SIZE: usize = 16 * 1024;
const RECEIVE_TIMEOUT_US: i64 = 200_000;  // 200 miliseconds
pub type RxFormat = f32;

struct DeviceWorker {
    receiver: Receiver<DeviceBoundCommand>,
    sender: Sender<GuiBoundEvent>,

    available_devices: Option<Vec<Args>>,
    device: Option<Device>,

    receive_state: Option<ReceiverState>,
    receive_stream: Option<RxStream<Complex<RxFormat>>>,

    valid_count: usize,
    working_memory: Box<[Complex<RxFormat>; RECEIVE_SIZE]>,

    fft_planner: FftPlanner<f64>,
}

impl DeviceWorker {
    fn error_process(&mut self) -> Result<(), Box<dyn Error>> {
        fn clone_args(a: &Args) -> Args {
            let mut c = Args::new();
            for (k, v) in a {
                c.set(k, v)
            }
            c
        }

        loop {
            // this will block until there is a command available, this is hopefully implemented with the proper primitives so the thread won't spin the cpu endlessly
            let event = match self.receiver.recv() {
                Ok(command) => command,
                // the sender was closed
                Err(_) => {
                    return Ok(());
                }
            };

            match event {
                DeviceBoundCommand::CreateDevice { index } => {
                    assert!(self.device.is_none());
                    assert!(self.available_devices.is_some());

                    let args = clone_args(&self.available_devices.as_ref().unwrap()[index]);
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
                    self.receive_stream = None;
                    self.device = None;
                    self.valid_count = 0;

                    self.sender.send(GuiBoundEvent::DeviceDestroyed)?;
                }
                DeviceBoundCommand::RefreshDevices { args } => {
                    let available = soapysdr::enumerate(args.as_str())?;
                    let names = available
                        .iter()
                        .map(|d| d.get("label").unwrap().to_owned())
                        .collect();

                    self.available_devices = Some(available);

                    self.sender
                        .send(GuiBoundEvent::RefreshedDevices { list: names });
                }
                DeviceBoundCommand::SetReceiver(state) => {
                    assert!(self.device.is_some());

                    // compares the new state to the one currently set and if they differ (or the previous state is unset, this is why it's so ugly) run the block
                    macro_rules! if_differs {
                        ($($var:ident, $then:expr);+ $(;)?) => {
                            $(
                                if Some($var) != self.receive_state.as_ref().map(|s| s.$var) {
                                    $then;
                                    dbg!($var);
                                }
                            );+
                        }
                    }

                    let ReceiverState {
                        channel,
                        samplerate,
                        frequency,
                        bandwidth,
                        gain,
                        automatic_gain,
                        automatic_dc_offset,
                    } = state.clone();

                    let dev = self.device.as_ref().unwrap();

                    let mut antennas = dbg!(dev.antennas(Rx, channel)?);
                    dev.set_antenna(
                        Rx,
                        channel,
                        antennas.pop().expect("No receiving antennas on channel."),
                    )?;

                    // this is the result of excessive bikeshedding
                    if_differs!(
                        gain,       dev.set_gain(Rx, channel, gain)?;
                        frequency,  dev.set_frequency(Rx, channel, frequency, ())?; // FIXME are the args neccessary for anything?
                        samplerate, dev.set_sample_rate(Rx, channel, samplerate)?;
                        bandwidth,  dev.set_bandwidth(Rx, channel, bandwidth)?;
                        automatic_gain, dev.set_gain_mode(Rx, channel, automatic_gain)?;
                        automatic_dc_offset, dev.set_dc_offset_mode(Rx, channel, automatic_dc_offset)?;
                        channel, {
                            self.receive_stream = None;
                            let mut new_receiver = dev.rx_stream::<Complex<RxFormat>>(&[channel])?;
                            new_receiver.activate(None)?;
                            self.receive_stream = Some(new_receiver);
                        };
                    );
                }
                DeviceBoundCommand::RequestData { mut data } => {
                    assert!(self.receive_stream.is_some());
                    assert!(data.get_input().len() <= self.working_memory.len());
                    
                    let len = data.get_input().len();
                    data.get_input_mut()
                    .copy_from_slice(&mut self.working_memory[0..len]);
                    data.process();

                    self.sender.send(GuiBoundEvent::DataReady { data });
                }
            }

            if let Some(stream) = self.receive_stream.as_mut() {
                let read_count = stream.read(&mut [self.working_memory.as_mut()], RECEIVE_TIMEOUT_US)?;
                if read_count != RECEIVE_SIZE {
                    eprintln!("Reading timed out");
                }
                self.valid_count = read_count;
            }
        }
    }
    fn process(mut self) {
        loop {
            let result = self.error_process();
    
            if let Err(e) = result {
                let desc = format!("{}", e);
                self.sender.send(GuiBoundEvent::Error { desc, fatal: false });
            }
        }
    }
}
