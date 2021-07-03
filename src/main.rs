#![allow(unused)]

mod device;
mod worker;
// mod memory_recycler;

use device::{DeviceBoundCommand, DeviceError, DeviceManager, GuiBoundEvent, ValueRanges};
use qt::QHBoxLayout;
use qt_charts::{
    cpp_core::CppBox,
    qt_core::{AlignmentFlag, CheckState, QRectF, QTimer, SlotOfBool, SlotOfDouble, SlotOfInt},
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
use rustfft::{
    num_complex::{Complex, Complex64},
    num_traits::Zero,
    Fft, FftNum, FftPlanner,
};
use soapysdr::{Args, Device};
use std::{
    borrow::Borrow,
    cell::{Cell, RefCell},
    convert::TryInto,
    f64::consts::FRAC_PI_2,
    fmt::{Debug, Formatter},
    ops::Range,
    rc::Rc,
    sync::Arc,
};
use worker::{FinishedMaybe, Worker};

use crate::device::ReceiverState;

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
            todo!()
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

    device: Rc<DeviceManager>,
}

impl DeviceGroup {
    unsafe fn new(device: Rc<DeviceManager>) -> (Rc<DeviceGroup>, Ptr<QGroupBox>) {
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

        // send a refresh request once beforehand
        device.send_command(DeviceBoundCommand::RefreshDevices {
            args: String::new(),
        });

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
                let enabled = (s.combo_box.count() != 0) && !s.device.get_device_valid();
                s.b2.set_enabled(enabled)
            }));

        let s = self.clone();
        b1.clicked().connect(&SlotNoArgs::new(group, move || {
            // only send refresh request when the last one has finished
            if !s.device.get_refreshing_devices() {
                let filter = s.entry.text().to_std_string();

                s.device
                    .send_command(DeviceBoundCommand::RefreshDevices { args: filter })
                    .ok()
                    .unwrap();
            }
        }));

        let s = self.clone();
        b2.clicked().connect(&SlotNoArgs::new(group, move || {
            if s.combo_box.count() != 0 {
                let index = s.combo_box.current_index();

                s.device
                    .send_command(DeviceBoundCommand::CreateDevice {
                        index: index as usize,
                    })
                    .ok()
                    .unwrap();

                let command = DeviceBoundCommand::RequestData {
                    data: FftData::new(1024),
                };
                s.device.send_command(command.clone());
                s.device.send_command(command);

                s.b2.set_enabled(false);
                s.b3.set_enabled(true);
            }
        }));

        let s = self.clone();
        b3.clicked().connect(&SlotNoArgs::new(group, move || {
            s.b2.set_enabled(true);
            s.b3.set_enabled(false);

            s.device.send_command(DeviceBoundCommand::DestroyDevice);
        }));

        // b1.click();
    }
    unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
        match event.as_ref().unwrap() {
            GuiBoundEvent::WorkerReset => {
                self.combo_box.clear();
                self.b2.set_enabled(false);
                self.b3.set_enabled(false);
            }
            GuiBoundEvent::RefreshedDevices { list } => {
                self.combo_box.clear();

                for name in list {
                    self.combo_box.add_item_q_string(&qs(name.as_str()));
                }
            }
            _ => (),
        };
    }
}

struct ReceiveGroup {
    automatic_update: QBox<QCheckBox>,
    samplerate: QBox<QDoubleSpinBox>,
    frequency: QBox<QDoubleSpinBox>,
    bandwidth: QBox<QDoubleSpinBox>,
    gain: QBox<QDoubleSpinBox>,
    automatic_gain: QBox<QCheckBox>,
    automatic_dc_offset: QBox<QCheckBox>,
    apply_btn: QBox<QPushButton>,

    group: QBox<QGroupBox>,

    value_ranges: RefCell<Option<ValueRanges>>,
    device: Rc<DeviceManager>,
}

impl ReceiveGroup {
    unsafe fn new(device: Rc<DeviceManager>) -> (Rc<Self>, Ptr<QGroupBox>) {
        let group = QGroupBox::new();
        group.set_title(&qs("Receive"));

        let v = QVBoxLayout::new_0a();

        let automatic_update = QCheckBox::new();
        automatic_update.set_text(&qs("Automatic update"));
        v.add_widget(&automatic_update);

        let form_layout = QFormLayout::new_0a();
        v.add_layout_1a(&form_layout);
        // layout.set_size_constraint(SizeConstraint::SetFixedSize);
        group.set_layout(&v);

        let samplerate = QDoubleSpinBox::new_0a();
        samplerate.set_suffix(&qs(" MSps"));
        form_layout.add_row_q_string_q_widget(&qs("Samplerate"), &samplerate);
        
        let frequency = QDoubleSpinBox::new_0a();
        frequency.set_suffix(&qs(" MHz"));
        form_layout.add_row_q_string_q_widget(&qs("Frequency"), &frequency);
        
        let bandwidth = QDoubleSpinBox::new_0a();
        bandwidth.set_suffix(&qs(" MHz"));
        form_layout.add_row_q_string_q_widget(&qs("Bandwidth"), &bandwidth);
        
        let gain = QDoubleSpinBox::new_0a();
        gain.set_suffix(&qs(" dB"));
        form_layout.add_row_q_string_q_widget(&qs("Gain"), &gain);

        let automatic_gain = QCheckBox::new();
        form_layout.add_row_q_string_q_widget(&qs("Automatic gain"), &automatic_gain);

        let automatic_dc_offset = QCheckBox::new();
        form_layout.add_row_q_string_q_widget(&qs("Automatic DC offset"), &automatic_dc_offset);

        let apply_btn = QPushButton::new();
        apply_btn.set_text(&qs("Apply"));
        v.add_widget(&apply_btn);

        let ptr = group.as_ptr();
        let s = Rc::new(Self {
            automatic_update,
            samplerate,
            frequency,
            bandwidth,
            gain,
            automatic_gain,
            automatic_dc_offset,
            apply_btn,
            group,

            value_ranges: RefCell::new(None),
            device,
        });

        s.group.set_enabled(false);
        s.init();

        (s, ptr)
    }
    unsafe fn values_changed(&self) {
        // everything is in megahertz or megasamples/second
        const MIL: f64 = 1_000_000.0;

        let state = ReceiverState {
            // TODO channel is hardcoded for now, it seems it is not too useful to be able to specify it, at least on my device
            channel: 0,
            samplerate: self.samplerate.value() * MIL,
            frequency: self.frequency.value() * MIL,
            bandwidth: self.bandwidth.value() * MIL,
            gain: self.gain.value(),
            automatic_gain: self.automatic_gain.is_checked(),
            automatic_dc_offset: self.automatic_dc_offset.is_checked(),
        };

        self.device
            .send_command(DeviceBoundCommand::SetReceiver(state));
    }
    unsafe fn init(self: &Rc<Self>) {
        unsafe fn clamp_value(
            widget: &QBox<QDoubleSpinBox>,
            ranges: &mut dyn Iterator<Item = &soapysdr::Range>,
        ) {
            let mut val = widget.value();

            // this seems pretty robust however it is quite spaghetti so it is very much possible there is an off-by-one error
            let mut previous_edge = 0.0;
            let mut first = true;
            for range in ranges {
                // make sure to check against the start of the first range
                if first {
                    val = val.max(range.minimum);
                    first = false;
                }

                // if the value is inside the valid range, snap it a multiple of step
                if (range.minimum..=range.maximum).contains(&val) {
                    // if the step is 0 or some small value, this doesn't work
                    // let step = range.step.min(0.0001);
                    // val = (val / step).trunc() * step;

                    // it seems that the device round is itself which I assume is better
                }
                // if the value is between the previous maximum and the current minimum it is not a valid range, therefore we snap it to the previous maximum
                else if (previous_edge..=(range.minimum)).contains(&val) {
                    val = previous_edge;
                }
                previous_edge = range.maximum;
            }
            // make sure to also check the end of the last range
            val = val.min(previous_edge);

            widget.set_value(val);
        }

        let Self {
            automatic_update,
            samplerate,
            frequency,
            bandwidth,
            gain,
            automatic_gain,
            automatic_dc_offset,
            apply_btn,
            group,
            value_ranges,
            device,
        } = self.borrow();

        let s = self.clone();
        automatic_update
            .state_changed()
            .connect(&SlotOfInt::new(group, move |state| {
                let enabled = state == CheckState::Unchecked.into() && s.device.get_device_valid();

                s.apply_btn.set_enabled(enabled);
            }));

        // another extremely bikeshedded (bikeshad?) macro
        macro_rules! setup_values_changed {
            ($name:ident, $iter:path) => {
                let s = self.clone();
                $name
                    .editing_finished()
                    .connect(&SlotNoArgs::new(group, move || {
                        let ranges = s.value_ranges.borrow_mut();
                        let r = &ranges.as_ref().unwrap().$name;

                        clamp_value(&s.$name, &mut $iter(r));

                        if s.automatic_update.is_checked() {
                            s.values_changed();
                        }
                    }));
            };
        }

        setup_values_changed! {samplerate, std::iter::IntoIterator::into_iter};
        setup_values_changed! {frequency, std::iter::IntoIterator::into_iter};
        setup_values_changed! {bandwidth, std::iter::IntoIterator::into_iter};
        setup_values_changed! {gain, std::iter::once};

        let s = self.clone();
        let checkbox_slot = SlotNoArgs::new(group, move || {
            if s.automatic_update.is_checked() {
                s.values_changed();
            }
        });

        automatic_gain.state_changed().connect(&checkbox_slot);
        automatic_dc_offset.state_changed().connect(&checkbox_slot);

        let s = self.clone();
        apply_btn
            .clicked()
            .connect(&SlotNoArgs::new(group, move || {
                s.values_changed();
            }));
    }

    unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
        {
            let enabled = self.device.get_device_valid();
            self.group.set_enabled(enabled);
        }

        match event.as_ref().unwrap() {
            GuiBoundEvent::DeviceCreated { channels_info } => {
                let ranges = channels_info[0].ranges.clone();

                // everything is in megahertz or megasamples/second
                const MIL: f64 = 1_000_000.0;

                // NaN fun
                let min = ranges
                    .samplerate
                    .iter()
                    .map(|r| r.minimum)
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                let max = ranges
                    .samplerate
                    .iter()
                    .map(|r| r.maximum)
                    .max_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                self.samplerate.set_range(min / MIL, max / MIL);

                let min = ranges
                    .frequency
                    .iter()
                    .map(|r| r.minimum)
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                let max = ranges
                    .frequency
                    .iter()
                    .map(|r| r.maximum)
                    .max_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                self.frequency.set_range(min / MIL, max / MIL);

                let min = ranges
                    .bandwidth
                    .iter()
                    .map(|r| r.minimum)
                    .min_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                let max = ranges
                    .bandwidth
                    .iter()
                    .map(|r| r.maximum)
                    .max_by(|a, b| a.partial_cmp(b).unwrap())
                    .unwrap();
                self.bandwidth.set_range(min / MIL, max / MIL);

                let min = ranges.gain.minimum;
                let max = ranges.gain.maximum;
                self.gain.set_range(min, max);

                *self.value_ranges.borrow_mut() = Some(ranges);
            }
            _ => (),
        }
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

    device: Rc<DeviceManager>,
}

impl OutputGroup {
    unsafe fn new(device: Rc<DeviceManager>) -> (Rc<Self>, Ptr<QGroupBox>) {
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

            device,
        });

        (s, ptr)
    }
    unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
        match event.as_ref().unwrap() {
            GuiBoundEvent::DecodedChars { data } => todo!(),
            GuiBoundEvent::DataReady { data } => {
                unsafe fn coloring_fn(f: f64) -> CppBox<QColor> {
                    QColor::from_rgb_f_3a(f.ln() * 0.2, 0.0, 0.0)
                }

                let len = data.get_output().len();
                let averaged_signal = data
                    .get_input()
                    .chunks(4)
                    .map(|chunks| chunks.iter().map(|c| c.re).sum::<i16>() as f64 / 4.0);
                let averaged_spectrum = data.get_output()[0..(len / 2 + 1)]
                    .chunks(4)
                    .map(|chunks| chunks.iter().map(|c| c.re).sum::<i16>() as f64 / 4.0);

                self.signal.add_new_data(averaged_signal, coloring_fn);
                self.spectrum.add_new_data(averaged_spectrum, coloring_fn);

                if self.device.get_receiver_valid() {
                    match event.take().unwrap() {
                        GuiBoundEvent::DataReady { data } => self
                            .device
                            .send_command(DeviceBoundCommand::RequestData { data }),
                        _ => unreachable!(),
                    };
                }
            }
            _ => (),
        }
    }
}

struct App {
    root: QBox<QWidget>,
    v_layout: QBox<QVBoxLayout>,
    device_group: Rc<DeviceGroup>,
    receive_group: Rc<ReceiveGroup>,
    output_group: Rc<OutputGroup>,

    device: Rc<DeviceManager>,
}

impl App {
    unsafe fn new() -> Self {
        let device = Rc::new(DeviceManager::new());

        let root = QWidget::new_0a();
        let h_layout = QHBoxLayout::new_1a(&root);

        let v_layout = QVBoxLayout::new_0a();

        h_layout.add_layout_1a(&v_layout);

        let (device_group, group) = DeviceGroup::new(device.clone());
        v_layout.add_widget(group);

        let (receive_group, group) = ReceiveGroup::new(device.clone());
        v_layout.add_widget(group);
        v_layout.add_stretch_0a();

        let (output_group, group) = OutputGroup::new(device.clone());
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
    unsafe fn handle_event(&self, mut event: &mut Option<GuiBoundEvent>) {
        macro_rules! chain_handle_events {
            ($event:ident, $($handler:expr),+) => {
                $(
                    if $event.is_some() {
                        $handler.handle_event(&mut $event);
                    }
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
}

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
    QApplication::init(|_| unsafe {
        QApplication::set_window_icon(&QIcon::from_theme_1a(&qs("network-wireless-hotspot")));
        let mut app = App::new();
        let timer = QTimer::new_0a();
        timer.set_interval(100);

        timer.timeout().connect(&SlotNoArgs::new(&timer, move || {
            let mut event = app.device.try_receive();

            match event {
                Ok(Some(GuiBoundEvent::Error { desc, fatal })) => {
                    if fatal {
                        eprintln!(
                            "Device encountered a fatal error, resetting worker: \n\n{}\n\n",
                            desc
                        );
                        app.reset_worker();
                    } else {
                        eprintln!("Device encountered an error: \n\n{}\n\n", desc);
                    }
                }
                Ok(mut event) => {
                    app.handle_event(&mut event);
                }
                Err(DeviceError::BadState) => panic!(
                    "Application is in the wrong state, this is a fatal error, shutting down"
                ),
                Err(DeviceError::WorkerPoisoned) => {
                    eprintln!("The receiver worker thread has panicked, resetting worker");

                    app.reset_worker();
                }
            }
        }));

        timer.start_0a();
        QApplication::exec()
    })
}
