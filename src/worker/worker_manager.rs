use std::{
    cell::RefCell,
    collections::BinaryHeap,
    error::Error,
    fmt::Display,
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use soapysdr::Range;

use crate::worker::worker::DeviceWorker;

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

struct ScheduledCommandEntry {
    command: DeviceBoundCommand,
    trigger_time: u64,
}

impl Ord for ScheduledCommandEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.trigger_time.cmp(&other.trigger_time).reverse()
    }
}

impl PartialOrd for ScheduledCommandEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.trigger_time.cmp(&other.trigger_time).reverse())
    }
}

impl PartialEq for ScheduledCommandEntry {
    fn eq(&self, other: &Self) -> bool {
        self.trigger_time.eq(&other.trigger_time)
    }
}

impl Eq for ScheduledCommandEntry {}

struct InnerDeviceManager {
    pub(crate) thread: ManuallyDrop<JoinHandle<()>>,
    pub(crate) sender: ManuallyDrop<Sender<DeviceBoundCommand>>,
    pub(crate) receive_enable_flag: Arc<AtomicBool>,
    pub(crate) receiver: ManuallyDrop<Receiver<GuiBoundEvent>>,

    pub(crate) device_valid: bool,
    pub(crate) receiver_valid: bool,
    pub(crate) decoder_valid: bool,
    pub(crate) refreshing_devices: bool,
    pub(crate) data_requests_in_flight: usize,

    pub(crate) start_time: Instant,
    pub(crate) scheduled_commands: BinaryHeap<ScheduledCommandEntry>,
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
            decoder_valid: false,
            refreshing_devices: false,
            data_requests_in_flight: 0,

            start_time: Instant::now(),
            scheduled_commands: BinaryHeap::new(),
        }
    }
    fn check_state_by_command(&self, command: &DeviceBoundCommand) -> Result<(), DeviceError> {
        macro_rules! check_state {
            ($cond:expr) => {
                if !$cond {
                    return Err(DeviceError::BadState);
                }
            };
        }

        match command {
            DeviceBoundCommand::DestroyDevice => {
                check_state!(self.device_valid);
            }
            DeviceBoundCommand::CreateDevice { .. } => {
                check_state!(!self.device_valid);
            }
            DeviceBoundCommand::RefreshDevices { .. } => {}
            DeviceBoundCommand::SetReceiver(_) => {
                check_state!(self.device_valid);
            }
            DeviceBoundCommand::RequestData { .. } => {
                check_state!(self.device_valid);
                check_state!(self.receiver_valid);
            }
            DeviceBoundCommand::SetDecoder { .. } => {
                check_state!(self.device_valid);
                check_state!(self.receiver_valid);
            }
        }

        Ok(())
    }
    fn modify_state_by_command(&mut self, command: &DeviceBoundCommand) {
        match command {
            DeviceBoundCommand::DestroyDevice => {
                self.device_valid = false;
                self.receiver_valid = false;
                self.decoder_valid = false;
            }
            DeviceBoundCommand::CreateDevice { .. } => self.device_valid = true,
            DeviceBoundCommand::RequestData { .. } => self.data_requests_in_flight += 1,
            DeviceBoundCommand::RefreshDevices { .. } => self.refreshing_devices = true,
            DeviceBoundCommand::SetReceiver(_) => self.receiver_valid = true,
            DeviceBoundCommand::SetDecoder { .. } => self.decoder_valid = true,
        }
    }
    fn modify_state_by_received_event(&mut self, event: &GuiBoundEvent) {
        match event {
            // this event is not sent by the device
            GuiBoundEvent::WorkerReset => unreachable!(),
            GuiBoundEvent::DeviceCreated { .. } => self.device_valid = true,
            GuiBoundEvent::DeviceDestroyed => self.device_valid = false,
            GuiBoundEvent::RefreshedDevices { .. } => self.refreshing_devices = false,
            GuiBoundEvent::DataReady { .. } => self.data_requests_in_flight -= 1,
            GuiBoundEvent::Error(_) => {}
            GuiBoundEvent::DecodedChars { .. } => {}
        }
    }
    /// Returns the earliest time in ms for a next command to send
    fn poll_scheduled_commands(&mut self) -> u64 {
        let current = self.start_time.elapsed().as_millis() as u64;
        while let Some(next) = self.scheduled_commands.peek() {
            if next.trigger_time < current {
                let command = self.scheduled_commands.pop().unwrap().command;

                match self.send_command(command) {
                    Ok(_) => {}
                    // only a debug as this is not necessarily an error, this can happen if the device was closed while a RequestData was scheduled
                    Err(_) => {
                        log::debug!("Scheduled command tried to send at invalid worker state.")
                    }
                }
            } else {
                return next.trigger_time - current;
            }
        }

        // currently nothing is scheduled, default delay of 5 ms, this technically limits the lowest delay that can be expected to 5 ms since
        // this function needs to be called to process any commands that were enqueued in the meantime, this is fine for me
        return 5;
    }
    fn send_command(&mut self, command: DeviceBoundCommand) -> Result<(), DeviceError> {
        self.check_state_by_command(&command)?;
        self.modify_state_by_command(&command);

        self.sender
            .send(command)
            .map_err(|_| DeviceError::WorkerPoisoned)
    }
    fn schedule_command(&mut self, command: DeviceBoundCommand, delay_ms: u64) {
        let trigger_time = self.start_time.elapsed().as_millis() as u64 + delay_ms;
        self.scheduled_commands.push(ScheduledCommandEntry {
            command,
            trigger_time,
        });
    }
    fn try_receive(&mut self) -> Result<Option<GuiBoundEvent>, WorkerPoisoned> {
        let event = self.receiver.try_recv();

        if let Ok(event) = event.as_ref() {
            self.modify_state_by_received_event(event);
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
    pub fn poll_scheduled_commands(&self) -> u64 {
        self.0.borrow_mut().poll_scheduled_commands()
    }
    pub fn schedule_command(&self, command: DeviceBoundCommand, delay_ms: u64) {
        self.0.borrow_mut().schedule_command(command, delay_ms)
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
