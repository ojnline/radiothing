use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::{path::PathBuf, rc::Rc};

use app_settings::{AppSettings, DEFAULT_SETTINGS};
use device::{DeviceManager, GuiBoundEvent};
use gui_groups::{
    device_group::DeviceGroup, output_group::OutputGroup, receive_group::ReceiveGroup,
};
use qt_charts::qt_core::{QTimer, SlotNoArgs};
use qt_widgets::{
    qt_core::{qs, QBox},
    QApplication, QGroupBox, QHBoxLayout, QLineEdit, QVBoxLayout, QWidget,
};
use rustfft::{num_complex::Complex, num_traits::Zero, Fft, FftNum, FftPlanner};

pub mod app_settings;
pub mod decode;
pub mod device;
pub mod gui_groups;
pub mod settings;

pub const SAMPLE_COUNT: usize = 256;

pub const DATA_REQUESTS_IN_FLIGHT: usize = 8;

#[allow(unused)]
struct App {
    root: QBox<QWidget>,
    v_layout: QBox<QVBoxLayout>,
    device_group: Rc<DeviceGroup>,
    receive_group: Rc<ReceiveGroup>,
    output_group: Rc<OutputGroup>,

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
        let h_layout = QHBoxLayout::new_1a(&root);

        let v_layout = QVBoxLayout::new_0a();

        h_layout.add_layout_1a(&v_layout);

        let (device_group, group) = DeviceGroup::new(device.clone(), settings.clone());
        v_layout.add_widget(group);

        let (receive_group, group) = ReceiveGroup::new(device.clone(), settings.clone());
        v_layout.add_widget(group);
        {
            let decode = QGroupBox::new();
            decode.set_title(&qs("Decode"));
            v_layout.add_widget(&decode);
        }
        v_layout.add_stretch_0a();

        let (output_group, group) = OutputGroup::new(device.clone());
        h_layout.add_widget(group);

        let v_layout_r = QVBoxLayout::new_0a();
        h_layout.add_layout_1a(&v_layout_r);

        {
            let habhub = QGroupBox::new();
            habhub.set_title(&qs("Habhub"));
            v_layout_r.add_widget(&habhub);

            let v = QVBoxLayout::new_0a();
            habhub.set_layout(&v);

            let listener_callsign = QLineEdit::new();
            v.add_widget(&listener_callsign);
        }

        h_layout.add_stretch_0a();

        root.show();

        Self {
            root,
            device_group,
            receive_group,
            output_group,
            v_layout,

            device,
            settings,
            save_path,
        }
    }
    unsafe fn handle_event(&self, mut event: &mut Option<GuiBoundEvent>) {
        macro_rules! chain_handle_events {
            ($event:ident, $($handler:expr),+) => {
                $(
                    if $event.is_some() {
                        $handler.handle_event(&mut $event);
                    } else { return }
                )+
            }
        }

        chain_handle_events! {event, self.device_group, self.receive_group, self.output_group};
    }
    unsafe fn reset_worker(&self) {
        self.device.reset();

        let mut event = Some(GuiBoundEvent::WorkerReset);
        self.handle_event(&mut event);
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
        .init();

    // FIXME
    // this is a bodge to fix qt from complaining about "QBasicTimer::start: QBasicTimer can only be used with threads started with QThread"
    // on application exit, apparently it often implies weird widget destruction order but leaving a reference here outside of QApplication::init
    // fixes it somehow?
    let mut keep_alive_outside_event_event_loop = None;

    QApplication::init(|qapp| unsafe {
        let app = Rc::new(App::new());
        keep_alive_outside_event_event_loop = Some(app.clone());

        let timer = QTimer::new_0a();
        timer.set_interval(100);

        let a = app.clone();
        timer.timeout().connect(&SlotNoArgs::new(&timer, move || {
            let event = a.device.try_receive();

            match event {
                Ok(Some(GuiBoundEvent::Error(e))) => {
                    log::error!("Device encountered an error: {}", e);
                }
                Ok(mut event) => {
                    a.handle_event(&mut event);
                }
                Err(_) => {
                    log::error!("The receiver worker thread has panicked, resetting worker");

                    a.reset_worker();
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
