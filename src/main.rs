use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::{path::PathBuf, rc::Rc};

use app_settings::{AppSettings, DEFAULT_SETTINGS};
use gui_groups::decode_group::DecodeGroup;
use gui_groups::habhub_group::HabhubGroup;
use gui_groups::{
    device_group::DeviceGroup, output_group::OutputGroup, receive_group::ReceiveGroup,
};
use qt_charts::qt_core::{QTimer, SlotNoArgs};
use qt_widgets::{qt_core::QBox, QApplication, QHBoxLayout, QVBoxLayout, QWidget};

use rustfft::{num_complex::Complex, num_traits::Zero, Fft, FftNum, FftPlanner};
use worker::worker::GuiBoundEvent;
use worker::worker_manager::DeviceManager;

pub mod app_settings;
pub mod decoder;
pub mod dsp;
pub mod gui_groups;
pub mod settings;
pub mod worker;

pub const SAMPLE_COUNT: usize = 256;

pub const DATA_REQUESTS_IN_FLIGHT: usize = 1;

#[allow(unused)]
struct App {
    root: QBox<QWidget>,
    v_layout_left: QBox<QVBoxLayout>,
    v_layout_right: QBox<QVBoxLayout>,
    device_group: Rc<DeviceGroup>,
    receive_group: Rc<ReceiveGroup>,
    decode_group: Rc<DecodeGroup>,
    output_group: Rc<OutputGroup>,
    habhub_group: Rc<HabhubGroup>,

    device: Rc<DeviceManager>,
    settings: Rc<AppSettings>,
    save_path: Option<PathBuf>,
}

impl App {
    unsafe fn new() -> Self {
        let (settings, save_path) = app_settings::get_settings();
        let settings = Rc::new(settings);

        let device = Rc::new(DeviceManager::new());

        let root = QWidget::new_0a();

        // this timer runs the scheduled device command
        // these exist because it is very useful to ensure that a command is sent at some point but not eventually
        // at this time only to limit the rate at which the commands are exchanged - the automatic device starting would spam the requests a lot otherwise
        // and rate of RequestData sending would depend on other events being sent through the channel
        let timer = QTimer::new_1a(&root);
        timer.set_interval(5);
        timer.set_single_shot(false);
        let timer_ptr = timer.as_ptr();
        let d = device.clone();
        timer
            .timeout()
            .connect(&SlotNoArgs::new(timer_ptr, move || {
                let next = d.poll_scheduled_commands();
                // the timer gets the how long it will take for the next earliest command to be "ready"
                // and then sets it as its interval
                timer.set_interval(next as i32);
            }));
        timer_ptr.start_0a();

        let h_layout = QHBoxLayout::new_1a(&root);

        // LEFT
        let v_layout_left = QVBoxLayout::new_0a();
        h_layout.add_layout_1a(&v_layout_left);

        let (device_group, group) = DeviceGroup::new(device.clone(), settings.clone());
        v_layout_left.add_widget(group);

        let (receive_group, group) = ReceiveGroup::new(device.clone(), settings.clone());
        v_layout_left.add_widget(group);

        let (decode_group, group) = DecodeGroup::new(device.clone(), settings.clone());
        v_layout_left.add_widget(group);

        v_layout_left.add_stretch_0a();

        // MIDDLE
        let (output_group, group) = OutputGroup::new(device.clone());
        h_layout.add_widget(group);

        // RIGHT
        let v_layout_right = QVBoxLayout::new_0a();
        h_layout.add_layout_1a(&v_layout_right);

        let (habhub_group, group) = HabhubGroup::new(device.clone(), settings.clone());
        v_layout_right.add_widget(group);

        v_layout_right.add_stretch_0a();

        root.show();

        Self {
            root,
            v_layout_left,
            v_layout_right,
            device_group,
            receive_group,
            decode_group,
            output_group,
            habhub_group,

            device,
            settings,
            save_path,
        }
    }
    unsafe fn handle_event(&self, mut event: GuiBoundEvent) {
        let mut event = Some(event);

        macro_rules! chain_handle_events {
            ($event:ident, $($handler:expr),+) => {
                $(
                    if $event.is_some() {
                        $handler.handle_event(&mut $event);
                    } else { return }
                )+
            }
        }

        chain_handle_events! {event, self.device_group, self.receive_group, self.decode_group, self.output_group};
    }
    unsafe fn reset_worker(&self) {
        self.device.reset();

        let event = GuiBoundEvent::WorkerReset;
        self.handle_event(event);
    }
    unsafe fn collect_settings(&self) -> AppSettings {
        let mut settings = DEFAULT_SETTINGS;
        self.device_group.populate_settings(&mut settings);
        self.receive_group.populate_settings(&mut settings);

        settings
    }
}

// TODO the fft can be owned by the worker since the fft length is static
pub struct FftData<T: FftNum> {
    fft: Arc<dyn Fft<T>>,
    input: Box<[Complex<T>]>,
    output: Box<[Complex<T>]>,
    scratch: Box<[Complex<T>]>,
}

impl<T: FftNum> FftData<T> {
    pub fn new(len: usize) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(len);
        // let scratch = fft.get_outofplace_scratch_len();
        let scratch = fft.get_outofplace_scratch_len();

        let input = vec![Complex::zero(); len].into_boxed_slice();
        let output = vec![Complex::zero(); len].into_boxed_slice();
        let scratch = vec![Complex::zero(); scratch].into_boxed_slice();

        Self {
            fft,
            input,
            output,
            scratch,
        }
    }
    pub fn get_input(&self) -> &[Complex<T>] {
        &self.input
    }
    pub fn get_input_mut(&mut self) -> &mut [Complex<T>] {
        &mut self.input
    }
    pub fn get_output(&self) -> &[Complex<T>] {
        &self.output
    }

    pub fn process(&mut self) {
        self.fft.process_outofplace_with_scratch(
            &mut self.input,
            &mut self.output,
            &mut self.scratch,
        );
    }
}

impl<T: FftNum> Clone for FftData<T> {
    fn clone(&self) -> Self {
        let input = vec![Complex::zero(); self.input.len()].into_boxed_slice();
        let output = vec![Complex::zero(); self.output.len()].into_boxed_slice();
        let scratch = vec![Complex::zero(); self.scratch.len()].into_boxed_slice();

        Self {
            fft: self.fft.clone(),
            input,
            output,
            scratch,
        }
    }
}

impl<T: FftNum> Debug for FftData<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // it is useless to print many-thousand-long arrays
        f.debug_struct("FftData").finish()
    }
}

fn main() {
    use std::io::Write;

    env_logger::builder()
        .format(|buf, record| {
            // needed for minimum width format to work correctly without allocation
            let level = match record.level() {
                log::Level::Error => "[ERROR]",
                log::Level::Warn => "[WARN]",
                log::Level::Info => "[INFO]",
                log::Level::Debug => "[DEBUG]",
                log::Level::Trace => "[TRACE]",
            };
            writeln!(buf, "{:5} {}", level, record.args())
        })
        .target(env_logger::Target::Stderr)
        .init();

    soapysdr::configure_logging();

    QApplication::init(|qapp| unsafe {
        let app = Rc::new(App::new());

        let timer = QTimer::new_1a(&app.root);
        timer.set_interval(16);

        let a = app.clone();
        timer.timeout().connect(&SlotNoArgs::new(&timer, move || {
            let start = std::time::Instant::now();
            loop {
                let event = a.device.try_receive();

                match event {
                    Ok(Some(GuiBoundEvent::Error(e))) => {
                        log::error!("Device encountered an error: {}", e);
                        a.device.set_receive_enabled(false);
                        a.output_group.set_run(false);
                    }
                    Ok(Some(event)) => {
                        a.handle_event(event);
                    }
                    Err(_) => {
                        log::error!("The receiver worker thread has panicked, resetting worker");

                        a.reset_worker();
                        a.device.set_receive_enabled(false);
                        a.output_group.set_run(false);
                    }
                    // break if all the queued messages have been processed
                    Ok(None) => break,
                }

                // break if processing took too long
                if start.elapsed() > std::time::Duration::from_millis(5) {
                    break;
                }
            }
        }));

        qapp.about_to_quit()
            .connect(&SlotNoArgs::new(qapp, move || {
                if let Some(path) = &app.save_path {
                    let settings = app.collect_settings();
                    let string = settings.pretty_serialize();

                    std::fs::write(path, string).unwrap();
                }
            }));

        timer.start_0a();

        QApplication::exec()
    })
}
