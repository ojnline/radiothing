use std::{error::Error, iter::FromIterator, mem::ManuallyDrop, ops::Deref, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering::{Acquire, Release}}, mpsc::{Receiver, Sender, channel}}, thread::{self, JoinHandle}, usize};

use rustfft::{Fft, FftPlanner, num_complex::{Complex, Complex64}};
use soapysdr::{Args, Device, Direction::Rx, Range, RxStream};

pub enum DeviceBoundCommand {
    DestroyDevice,
    CreateDevice {
        index: usize
    },
    RefreshDevices {
        args: String
    },
    SetReceiver(ReceiverState),
    RequestData {
        len: usize,
        downsample: usize // how many samples to average together: 1 means no downsampling, 2 means halved data, ...
        // buffer: Arc<Mutex<Box<[Complex64]>>>
    }
}
pub enum GuiBoundCommand {
    DeviceCreated{channels_info: Vec<ChannelInfo>},
    DeviceDestroyed,
    Error{desc: String, fatal: bool},
    RefreshedDevices{list: Vec<String>},
    DecodedChars {
        data: String
    },
    DataReady {
        time_domain: Box<[Complex64]>,
        frequency_domain: Box<[Complex64]>
    }
}

#[derive(Clone)]
pub struct ReceiverState  {
    channel: usize,
    samplerate: f64,
    frequency: f64,
    bandwidth: f64,
    gain: f64,
    automatic_gain: bool,
    automatic_dc_offset: bool,
}

pub struct ChannelInfo {
    pub ranges: ValueRanges,
    pub info: Args
}
unsafe impl Send for ChannelInfo { }

pub struct ValueRanges {
    samplerate: Vec<Range>,
    bandwidth: Vec<Range>,
    frequency: Vec<Range>,
    gain: Range
}

pub struct DeviceManager {
    thread: JoinHandle<()>,
    sender: Sender<DeviceBoundCommand>,
    receiver: Receiver<GuiBoundCommand>,

    device_valid: bool,
    receiver_valid: bool,
}

impl DeviceManager {
    pub fn new() -> Self {
        let (gui_sender_channel, gui_receive_channel) = channel();
        let (device_sender_channel, device_receive_channel) = channel();

        let thread = thread::spawn(move || {
            let worker = DeviceWorker {
                receiver: device_receive_channel,
                sender: gui_sender_channel,
                device_args: None,
                device: None,
                receive_state: None,
                receive_stream: None,
                working_memory: None,
                fft_planner: FftPlanner::new(),
            };
            
            worker.process();
        });

        Self {
            thread,
            sender: device_sender_channel,
            receiver: gui_receive_channel,

            device_valid: false,
            receiver_valid: false
        }
    }
    pub fn get_device_valid(&mut self) -> bool {self.device_valid}
    pub fn get_receiver_valid(&mut self) -> bool {self.receiver_valid}
    pub fn send_command(&mut self, command: DeviceBoundCommand) -> Result<(), BadState>{
        let Self {device_valid, receiver_valid, ..} = self;

        match &command {
            DeviceBoundCommand::DestroyDevice => if *device_valid {
                *device_valid = false;
                *receiver_valid = false;
            } else {
                return Err(BadState);
            },
            DeviceBoundCommand::CreateDevice { .. } => if *device_valid {
                return Err(BadState);
            } else {
                *device_valid = true
            },
            DeviceBoundCommand::RefreshDevices { .. } => (),
            DeviceBoundCommand::SetReceiver(_) => *receiver_valid = true,
            DeviceBoundCommand::RequestData { .. } => (),
        }

        self.sender.send(command).unwrap();

        Ok(())
    }
    // pub fn receive_blocking(&self) -> Result<GuiBoundCommand, ()> {self.receiver.recv().map_err(|_| ())}
    pub fn try_receive(&self) -> Option<GuiBoundCommand> {
        self.receiver.try_recv().ok()
    }
}

pub struct BadState;

// const FFT_CACHE_MAX_CYCLES: usize = 2;
struct DeviceWorker {
    // stop_flag: Arc<AtomicBool>,

    receiver: Receiver<DeviceBoundCommand>,
    sender: Sender<GuiBoundCommand>,

    device_args: Option<Vec<Args>>,
    device: Option<Device>,

    receive_state: Option<ReceiverState>,
    receive_stream: Option<RxStream<Complex<i16>>>,

    working_memory: Option<Box<[u8]>>,

    fft_planner: FftPlanner<f64>,
    // fft_cache: Vec<(Box<dyn Fft<f64>>, usize, usize)> // (fft, size, cycles_unused)
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
                    assert!(self.device_args.is_none());

                    let args = clone_args(&self.device_args.as_ref().unwrap()[index]);
                    let dev = Device::new(args)?;

                    let num_channels = dev.num_channels(Rx)?;
                    let mut channels_info = Vec::with_capacity(num_channels as usize);

                    for i in 0..dev.num_channels(Rx)? {
                        let info = dev.channel_info(Rx, i)?;

                        let ranges = ValueRanges {
                            samplerate: dev.get_sample_rate_range(Rx, i)?,
                            bandwidth: dev.bandwidth_range(Rx, i)?,
                            frequency: dev.frequency_range(Rx, i)?,
                            gain: dev.gain_range(Rx, i)?,
                        };

                        channels_info.push(ChannelInfo {
                            ranges,
                            info
                        })
                    }

                    self.sender.send(GuiBoundCommand::DeviceCreated {
                        channels_info 
                    })?;
                    self.device = Some(dev);

                }
                DeviceBoundCommand::DestroyDevice => {
                    self.receive_stream = None;
                    self.device = None;
                    self.device_args = None;

                    self.sender.send(GuiBoundCommand::DeviceDestroyed)?;
                }
                DeviceBoundCommand::RefreshDevices{args} => {
                    let available = soapysdr::enumerate(args.as_str())?;
                    let names = available.iter().map(|d| d.get("label").unwrap().to_owned()).collect();

                    self.sender.send(GuiBoundCommand::RefreshedDevices {
                        list: names
                    });
                }
                DeviceBoundCommand::SetReceiver(state) => {
                    assert!(self.device.is_some()); 
                    
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

                    let ReceiverState { channel, samplerate, frequency, bandwidth, gain, automatic_gain, automatic_dc_offset } = state.clone();

                    let dev = self.device.as_ref().unwrap();

                    // this is a result of excessive bikeshedding
                    if_differs!(
                        frequency, dev.set_frequency(Rx, channel, frequency, ())?; // FIXME are the args neccessary for anything?
                        bandwidth, dev.set_bandwidth(Rx, channel, bandwidth)?;
                        gain,      dev.set_gain(Rx, channel, gain)?;
                        automatic_gain, dev.set_gain_mode(Rx, channel, automatic_gain)?;
                        automatic_dc_offset, dev.set_dc_offset_mode(Rx, channel, automatic_dc_offset)?;
                        channel, {
                            self.receive_stream = None;

                            let new_receiver = dev.rx_stream::<Complex<i16>>(&[channel])?;
                            self.receive_stream = Some(new_receiver);
                        };
                    );
                }
                DeviceBoundCommand::RequestData { len, downsample } => {}
            }
        }
    }
    fn process(mut self) {
        let result = self.error_process();

        if let Err(e) = result {
            let desc = format!("{}", e);
            self.sender.send(GuiBoundCommand::Error {
                desc,
                fatal: true
            });
        }
    }
}