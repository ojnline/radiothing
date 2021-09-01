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
};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use soapysdr::Range;

use crate::{worker::worker::DeviceWorker};

use super::worker::{DeviceBoundCommand, GuiBoundEvent};

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
                let worker = DeviceWorker::new(
                    device_receive_channel,
                    gui_sender_channel,
                    receive_enable_flag,
                );

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
        match command {
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
            DeviceBoundCommand::RequestData { .. } => self.data_requests_in_flight += 1,
            DeviceBoundCommand::RefreshDevices { .. } => self.refreshing_devices = true,
            DeviceBoundCommand::SetReceiver(_) => self.receiver_valid = true,
            _ => {}
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
