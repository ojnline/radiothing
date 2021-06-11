#![allow(unused)]

mod device;
mod worker;
// mod memory_recycler;

use device::{DeviceBoundCommand, DeviceManager, GuiBoundCommand};
use qt::QHBoxLayout;
use qt_charts::{
    cpp_core::CppBox,
    qt_core::{AlignmentFlag, QRectF, QTimer, SlotOfBool},
    qt_gui::{q_image::Format, q_painter::RenderHint, QColor, QIcon, QImage, QPixmap},
    *,
};
use qt_widgets::{
    self as qt, q_size_policy::Policy, QApplication, QGridLayout, QGroupBox, QPushButton,
    QVBoxLayout, QWidget,
};
use qt_widgets::{cpp_core::Ptr, q_layout::SizeConstraint, QLabel};
use qt_widgets::{
    qt_core::{qs, QBox, SlotNoArgs},
    QCheckBox, QDoubleSpinBox, QFormLayout, QSpinBox, QTextEdit,
};
use rustfft::{num_complex::Complex64, Fft, FftPlanner};
use soapysdr::{Args, Device};
use std::{
    borrow::Borrow,
    cell::{Cell, RefCell},
    f64::consts::FRAC_PI_2,
    ops::Range,
    rc::Rc,
    sync::Arc,
};
use worker::{FinishedMaybe, Worker};

const SAMPLE_COUNT: usize = 256;

struct Radio {
    device: Option<Device>,
    receive_channel: Option<usize>,
    args: Vec<Args>,
}

impl Radio {
    fn new() -> Self {
        Self {
            device: None,
            receive_channel: None,
            args: Vec::new(),
        }
    }
    fn get_data(&mut self, buffer: &mut [Complex64]) {
        for (i, s) in buffer.iter_mut().enumerate() {
            // s.re = (i as f64 * 5.0).sin()
            fn sin_sum(n: usize, f: f64) -> f64 {
                (1..=n).into_iter().map(|i| (i as f64 * f).sin()).sum()
            }
            let i = i as f64 * 0.2;
            let r = sin_sum(6, i) * 0.5;

            *s = Complex64::new(r, 0.0);
        }
    }
}

struct DeviceGroup {
    group: QBox<QGroupBox>,
    combo_box: QBox<qt::QComboBox>,
    auto_select: QBox<QCheckBox>,
    entry: QBox<qt::QLineEdit>,
    row_widget: QBox<QWidget>,
    b1: QBox<qt::QPushButton>,
    b2: QBox<qt::QPushButton>,
    b3: QBox<qt::QPushButton>,

    device: Rc<RefCell<DeviceManager>>,
    devices: Option<Vec<String>>,
}

impl DeviceGroup {
    unsafe fn new(device: Rc<RefCell<DeviceManager>>) -> (Rc<DeviceGroup>, Ptr<QGroupBox>) {
        let layout = QVBoxLayout::new_0a();
        let group = QGroupBox::new();
        group.set_title(&qs("Device"));
        group.set_layout(&layout);

        let auto_select = QCheckBox::new();
        auto_select.set_text(&qs("Auto select device"));
        layout.add_widget(&auto_select);

        let entry = qt::QLineEdit::new();
        entry.set_placeholder_text(&qs("Device filter"));

        let combo_box = qt::QComboBox::new_0a();

        let row_widget = QWidget::new_0a();
        let row_layout = qt::QHBoxLayout::new_1a(&row_widget);
        let b1 = QPushButton::from_q_string(&qs("Refresh"));
        let b2 = QPushButton::from_q_string(&qs("Start"));
        let b3 = QPushButton::from_q_string(&qs("Stop"));
        b2.set_enabled(false);
        b3.set_enabled(false);

        row_layout.add_widget(&b1);
        row_layout.add_widget(&b2);
        row_layout.add_widget(&b3);
        row_layout.add_stretch_0a();

        layout.add_widget(&entry);
        layout.add_widget(&combo_box);
        layout.add_widget(&row_widget);

        layout.set_size_constraint(SizeConstraint::SetFixedSize);

        let ptr = group.as_ptr();

        let s = Rc::new(Self {
            group,
            combo_box,
            entry,
            row_widget,
            b1,
            b2,
            b3,
            auto_select,
            // radio: RefCell::new(Radio::new()),
            device,
            devices: None,
        });

        s.init();

        (s, ptr)
    }
    unsafe fn init(self: &Rc<Self>) {
        let Self {
            group,
            auto_select,
            combo_box,
            b1,
            b2,
            b3,
            device,
            ..
        } = self.borrow();

        let s = self.clone();
        auto_select
            .clicked()
            .connect(&SlotOfBool::new(group, move |checked| {
                s.entry.set_enabled(!checked);
                s.combo_box.set_enabled(!checked);
            }));

        let s = self.clone();
        combo_box
            .current_index_changed()
            .connect(&SlotNoArgs::new(group, move || {
                let enabled =
                    (s.combo_box.count() != 0) && !RefCell::borrow(&s.device).get_device_valid();
                s.b2.set_enabled(enabled)
            }));

        let s = self.clone();
        b1.clicked().connect(&SlotNoArgs::new(group, move || {
            // only send refresh request when the last one has finished
            if !RefCell::borrow(&s.device).get_refreshing_devices() {
                let filter = s.entry.text().to_std_string();

                RefCell::borrow(&s.device)
                    .send_command(DeviceBoundCommand::RefreshDevices { args: filter });
            }
        }));

        let s = self.clone();
        b2.clicked().connect(&SlotNoArgs::new(group, move || {
            if s.combo_box.count() != 0 {
                let index = s.combo_box.current_index();

                RefCell::borrow(&s.device).send_command(DeviceBoundCommand::CreateDevice {
                    index: index as usize,
                });

                s.b2.set_enabled(false);
                s.b3.set_enabled(true);
            }
        }));

        let s = self.clone();
        b3.clicked().connect(&SlotNoArgs::new(group, move || {
            s.b2.set_enabled(true);
            s.b3.set_enabled(false);

            RefCell::borrow(&s.device).send_command(DeviceBoundCommand::DestroyDevice);
        }));

        b1.click();
    }
    unsafe fn handle_event(&self, event: &GuiBoundCommand) -> bool {
        match event {
            GuiBoundCommand::DeviceCreated { channels_info } => true,
            GuiBoundCommand::DeviceDestroyed => true,
            // GuiBoundCommand::Error { desc, fatal } => false,
            GuiBoundCommand::RefreshedDevices { list } => {
                self.combo_box.clear();

                for name in list {
                    self.combo_box.add_item_q_string(&qs(name.as_str()));
                }

                true
            }
            // GuiBoundCommand::DecodedChars { data } => false,
            // GuiBoundCommand::DataReady { time_domain, frequency_domain } => todo!(),
            _ => false,
        }
    }
}

struct ReceiveGroup {
    samplerate: QBox<QDoubleSpinBox>,
    frequency: QBox<QDoubleSpinBox>,
    bandwidth: QBox<QSpinBox>,
    gain: QBox<QDoubleSpinBox>,
    automatic_gain: QBox<QCheckBox>,
    automatic_dc_offset: QBox<QCheckBox>,

    group: QBox<QGroupBox>,
}

impl ReceiveGroup {
    unsafe fn new() -> (Rc<Self>, Ptr<QGroupBox>) {
        let group = QGroupBox::new();
        group.set_title(&qs("Receive"));

        let layout = QFormLayout::new_0a();
        // layout.set_size_constraint(SizeConstraint::SetFixedSize);
        group.set_layout(&layout);

        let samplerate = QDoubleSpinBox::new_0a();
        layout.add_row_q_string_q_widget(&qs("Samplerate"), &samplerate);

        let frequency = QDoubleSpinBox::new_0a();
        layout.add_row_q_string_q_widget(&qs("Frequency"), &frequency);

        let bandwidth = QSpinBox::new_0a();
        layout.add_row_q_string_q_widget(&qs("Bandwidth"), &bandwidth);

        let gain = QDoubleSpinBox::new_0a();
        layout.add_row_q_string_q_widget(&qs("Gain"), &gain);

        let automatic_gain = QCheckBox::new();
        layout.add_row_q_string_q_widget(&qs("Automatic gain"), &automatic_gain);

        let automatic_dc_offset = QCheckBox::new();
        layout.add_row_q_string_q_widget(&qs("Automatic DC offset"), &automatic_dc_offset);

        let ptr = group.as_ptr();
        let s = Rc::new(Self {
            samplerate,
            frequency,
            bandwidth,
            gain,
            automatic_gain,
            automatic_dc_offset,
            group,
        });

        (s, ptr)
    }
}

struct ShittySpectogram {
    graph_width: Cell<u32>,
    graph_height: Cell<u32>,
    frequency_samples: Cell<u32>,
    spectogram_history_count: Cell<u32>,
    recreate_image: Cell<bool>,

    widget: QBox<QWidget>,
    chart: QBox<QChart>,
    view: QBox<QChartView>,
    series: QBox<QLineSeries>,
    history_image: RefCell<CppBox<QImage>>,
    pixlabel: QBox<QLabel>,
}

impl ShittySpectogram {
    unsafe fn new(
        graph_width: u32,
        graph_height: u32,
        frequency_samples: u32,
        spectogram_history_count: u32,
        x_range: Range<f64>,
        y_range: Range<f64>,
    ) -> (Self, Ptr<QWidget>) {
        let layout = QVBoxLayout::new_0a();
        let margin = layout.margin();
        layout.set_size_constraint(SizeConstraint::SetMaximumSize);
        layout.set_spacing(0);
        layout.set_margin(0);

        let widget = QWidget::new_0a();
        // widget.set_size_policy_2a(Policy::Fixed, Policy::MinimumExpanding);
        widget.set_layout(&layout);

        let chart = QChart::new_0a();

        let x_axis = QValueAxis::new_0a();
        x_axis.set_range(x_range.start, x_range.end);
        let y_axis = QValueAxis::new_0a();
        y_axis.set_range(y_range.start, y_range.end);
        y_axis.set_label_format(&qs(" "));

        chart.add_axis(&x_axis, AlignmentFlag::AlignTop.into());
        chart.add_axis(&y_axis, AlignmentFlag::AlignLeft.into());

        let series = QLineSeries::new_0a();
        chart.add_series(&series);
        series.attach_axis(&x_axis);
        series.attach_axis(&y_axis);

        chart.legend().markers_0a().iter().for_each(|m| {
            let m = m.as_ref().unwrap().as_ref().unwrap();
            m.set_visible(false);
        });

        let graph_width_f = graph_width as f64;
        let graph_height_f = graph_height as f64;

        let graph_width_i = graph_width as i32;
        let graph_height_i = graph_height as i32;

        let y_space = 20.0;
        chart.set_plot_area(&QRectF::from_4_double(
            0.0,
            y_space,
            graph_width_f,
            graph_height_f,
        ));

        let chart_view = QChartView::from_q_chart(&chart);
        chart_view.set_render_hint_1a(RenderHint::Antialiasing);
        chart_view.set_minimum_size_2a(graph_width_i, graph_height_i + y_space as i32);
        chart.set_background_roundness(0.0);
        layout.add_widget(&chart_view);

        let pixmap = QPixmap::from_2_int(frequency_samples as i32, spectogram_history_count as i32); // from_q_string(&qs("./bbb.jpg"));
        pixmap.fill_1a(&QColor::from_rgb_3a(0, 0, 0));

        let pixlabel = QLabel::new();
        pixlabel.set_pixmap(&pixmap);
        pixlabel.set_contents_margins_4a(margin, 0, margin, margin);
        pixlabel.set_scaled_contents(true);
        pixlabel.set_fixed_width(graph_width_i);
        layout.add_widget(&pixlabel);
        layout.add_stretch_0a();

        let history_image = QImage::from_2_int_format(
            frequency_samples as i32,
            spectogram_history_count as i32,
            Format::FormatRGB32,
        );
        history_image.fill_uint(0);

        let ptr = widget.as_ptr();
        let s = Self {
            widget,
            chart,
            view: chart_view,
            series,
            graph_width: Cell::new(graph_width),
            graph_height: Cell::new(graph_height),
            frequency_samples: Cell::new(frequency_samples),
            spectogram_history_count: Cell::new(spectogram_history_count),

            history_image: RefCell::new(history_image),
            pixlabel,
            recreate_image: Cell::new(false),
        };

        (s, ptr)
    }

    unsafe fn set_sample_count(&self, count: u32) {
        self.recreate_image.set(true);
        self.frequency_samples.set(count);
    }

    unsafe fn set_history_count(&self, count: u32) {
        self.recreate_image.set(true);
        self.spectogram_history_count.set(count);
    }

    // resize and clear image
    unsafe fn recreate_image(&self) {
        let new_image = QImage::from_2_int_format(
            self.frequency_samples.get() as i32,
            self.spectogram_history_count.get() as i32,
            Format::FormatRGB32,
        );
        new_image.fill_uint(0);
        *self.history_image.borrow_mut() = new_image;
        self.recreate_image.set(false);
    }

    unsafe fn add_new_data(
        &self,
        data: impl ExactSizeIterator<Item = f64>,
        coloring_fn: unsafe fn(f64) -> CppBox<QColor>,
    ) {
        if self.frequency_samples.get() != data.len() as u32 {
            self.set_sample_count(data.len() as u32);
        }

        self.series.clear();
        let d_x = 1.0 / (data.len() as f64);

        let is_spectogram = self.spectogram_history_count.get() != 0;

        if is_spectogram {
            if self.recreate_image.get() {
                self.recreate_image();
            } else {
                // shift image data one row down
                let samples = self.frequency_samples.get() as usize;
                let row = self.history_image.borrow().scan_line_mut(0) as *mut u32;
                std::ptr::copy(
                    row,
                    row.add(samples),
                    samples * (self.spectogram_history_count.get() as usize - 1),
                );
            }

            let row = self.history_image.borrow().scan_line_mut(0) as *mut u32;
            for (i, p) in data.enumerate() {
                self.series.append_2_double(d_x * i as f64, p);

                let color = coloring_fn(p);
                row.add(i).write(color.rgb());
            }

            let pixmap = QPixmap::from_image_1a(&*self.history_image.borrow());
            self.pixlabel.set_pixmap(&pixmap);
        } else {
            for (i, p) in data.enumerate() {
                self.series.append_2_double(d_x * i as f64, p);
            }
        }
    }
}
struct OutputGroup {
    group: QBox<QGroupBox>,
    grid: QBox<QGridLayout>,
    signal: ShittySpectogram,
    spectrum: ShittySpectogram,
    text_edit: QBox<QTextEdit>,
}

impl OutputGroup {
    unsafe fn new() -> (Rc<Self>, Ptr<QGroupBox>) {
        let group = QGroupBox::new();
        let grid = QGridLayout::new_0a();

        grid.set_size_constraint(SizeConstraint::SetMaximumSize);
        group.set_layout(&grid);

        let signal = {
            let (graph, widget) =
                ShittySpectogram::new(400, 300, SAMPLE_COUNT as u32, 0, 0.0..1.0, -2.5..2.5);

            let layout = QVBoxLayout::new_0a();
            layout.add_widget(widget);

            let group = QGroupBox::new();
            group.set_layout(&layout);
            group.set_flat(true);
            group.set_title(&qs("Signal"));

            grid.add_widget_3a(&group, 0, 0);

            graph
        };

        let spectrum = {
            let (graph, widget) =
                ShittySpectogram::new(400, 300, SAMPLE_COUNT as u32, 40, 0.0..1.0, -10.0..150.0);

            let layout = QVBoxLayout::new_0a();
            layout.add_widget(widget);

            let group = QGroupBox::new();
            group.set_layout(&layout);
            group.set_flat(true);
            group.set_title(&qs("Spectrum"));

            grid.add_widget_3a(&group, 0, 1);

            graph
        };

        let text_edit = QTextEdit::new();
        text_edit.set_read_only(true);
        grid.add_widget_5a(&text_edit, 1, 0, 1, 2);

        let ptr = group.as_ptr();
        let s = Rc::new(Self {
            group,
            grid,
            signal,
            spectrum,
            text_edit,
        });

        (s, ptr)
    }
    unsafe fn handle_event(&self, event: &GuiBoundCommand) -> bool {
        todo!()
    }
}

struct App {
    root: QBox<QWidget>,
    v_layout: QBox<QVBoxLayout>,
    device_group: Rc<DeviceGroup>,
    receive_group: Rc<ReceiveGroup>,
    output_group: Rc<OutputGroup>,

    device: Rc<RefCell<DeviceManager>>,
}

impl App {
    unsafe fn new() -> Self {
        let device = Rc::new(RefCell::new(DeviceManager::new()));

        let root = QWidget::new_0a();
        let h_layout = QHBoxLayout::new_1a(&root);

        let v_layout = QVBoxLayout::new_0a();

        h_layout.add_layout_1a(&v_layout);

        let (device_group, group) = DeviceGroup::new(device.clone());
        v_layout.add_widget(group);

        let (receive_group, group) = ReceiveGroup::new();
        v_layout.add_widget(group);
        v_layout.add_stretch_0a();

        let (output_group, group) = OutputGroup::new();
        h_layout.add_widget(group);

        h_layout.add_stretch_0a();

        root.show();

        Self {
            root,
            device_group,
            receive_group,
            output_group,

            v_layout,
            device,
        }
    }
}

// struct FftData {
//     fft: Arc<dyn Fft<f64>>,
//     input: Box<[Complex64]>,
//     output: Box<[Complex64]>,
//     scratch: Box<[Complex64]>,
// }

// impl FftData {
//     fn new(len: usize) -> Self {
//         let fft = FftPlanner::new().plan_fft_forward(len);
//         // let scratch = fft.get_outofplace_scratch_len();
//         let scratch = fft.get_outofplace_scratch_len();

//         let input = vec![Default::default(); len].into_boxed_slice();
//         let output = vec![Default::default(); len].into_boxed_slice();
//         let scratch = vec![Default::default(); scratch].into_boxed_slice();

//         Self {
//             fft,
//             input,
//             output,
//             scratch,
//         }
//     }
//     fn get_input(&self) -> &[Complex64] {
//         &self.input
//     }
//     fn get_input_mut(&mut self) -> &mut [Complex64] {
//         &mut self.input
//     }
//     fn get_output(&self) -> &[Complex64] {
//         &self.output
//     }

//     fn process(&mut self) {
//         self.fft.process_outofplace_with_scratch(
//             &mut self.input,
//             &mut self.output,
//             &mut self.scratch,
//         );
//     }
// }

// impl Clone for FftData {
//     fn clone(&self) -> Self {
//         let input = vec![Default::default(); self.input.len()].into_boxed_slice();
//         let output = vec![Default::default(); self.output.len()].into_boxed_slice();
//         let scratch = vec![Default::default(); self.scratch.len()].into_boxed_slice();

//         Self {
//             fft: self.fft.clone(),
//             input,
//             output,
//             scratch,
//         }
//     }
// }

fn main() {
    QApplication::init(|_| unsafe {
        QApplication::set_window_icon(&QIcon::from_q_string(&qs("./window-icon.svg")));
        let mut app = App::new();
        let timer = QTimer::new_0a();
        // let rand = Rc::new(RefCell::new(0.0));
        // let mut a: f64 = 1.0;
        // timer.set_interval(50);
        // let start = std::time::Instant::now();

        // let mut fft = None;
        // let mut task: Option<FinishedMaybe<FftData>> = None;

        timer.timeout().connect(&SlotNoArgs::new(&timer, move || {
            // unsafe fn color(f: f64) -> CppBox<QColor> {
            //     QColor::from_rgb_f_3a(f.ln() * 0.2, 0.0, 0.0)
            // };

            // let mut finished_task = None;

            // if let Some(task) = &mut task {
            //     match task.poll().ok().unwrap() {
            //         worker::Poll::Ready(t) => finished_task = Some(t),
            //         worker::Poll::Pending => (),
            //         _ => unimplemented!(),
            //     }
            // };

            // if finished_task.is_some() || task.is_none() {
            //     let new_fft = || FftData::new(SAMPLE_COUNT);
            //     let mut fft = fft.take().unwrap_or_else(new_fft);

            //     app.device.radio.borrow_mut().get_data(fft.get_input_mut());

            //     let new_task = app
            //         .worker
            //         .add_work(move || {
            //             fft.process();

            //             fft
            //         })
            //         .ok()
            //         .unwrap();

            //     task = Some(new_task);
            // }

            // if let Some(finished) = finished_task.take() {
            //     let iter = finished
            //         .get_output()
            //         .iter()
            //         .map(|c| (c.re * c.re + c.im * c.im).sqrt()).take(SAMPLE_COUNT/2+1);
            //     app.spectogram.add_new_data(iter, color);

            //     let iter = finished.get_input().iter().map(|c| c.re);
            //     app.graph.add_new_data(iter, color);

            //     fft = Some(finished);
            // }

            if let Some(event) = RefCell::borrow(&app.device).try_receive() {
                app.device_group.handle_event(&event);
                match event {
                    GuiBoundCommand::Error { desc, fatal } => {
                        todo!("TODO error handling: {}", desc)
                    }
                    _ => (),
                }
            }
        }));

        timer.start_0a();
        QApplication::exec()
    })
}
