mod decode;
pub mod device;
pub mod settings;

use device::{DeviceBoundCommand, DeviceError, DeviceManager, GuiBoundEvent, ValueRanges};
use qt_charts::{
    cpp_core::CppBox,
    qt_core::{AlignmentFlag, CheckState, QRectF, QTimer, SlotOfBool, SlotOfInt},
    qt_gui::{q_image::Format, q_painter::RenderHint, QColor, QGuiApplication, QImage, QPixmap},
    *,
};
use qt_widgets::{
    self as qt, QApplication, QComboBox, QGridLayout, QGroupBox, QHBoxLayout, QLineEdit,
    QPushButton, QVBoxLayout, QWidget,
};
use qt_widgets::{cpp_core::Ptr, q_layout::SizeConstraint, QLabel};
use qt_widgets::{
    qt_core::{qs, QBox, SlotNoArgs},
    QCheckBox, QDoubleSpinBox, QFormLayout, QTextEdit,
};
use rustfft::{num_complex::Complex, num_traits::Zero, Fft, FftNum, FftPlanner};
use std::{
    borrow::Borrow,
    cell::{Cell, RefCell},
    error::Error,
    fmt::{Debug, Formatter},
    ops::Range,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

use crate::{
    device::{ReceiverState, RxFormat},
    settings::Field,
};

// crash on BadState, ignore WorkerPoisoned because it will be handled in the next iteration
// previously the code ws just unwrapping the result which enabled a race condition when the worker thread has just closed
// obviously on a bad state we want to crash regardless but a panic is a much nicer error
fn handle_send_result(result: Result<(), DeviceError>) {
    match result {
        Err(DeviceError::BadState) => {
            panic!("Application is in the wrong state, this is a fatal error, shutting down");
        }
        Err(DeviceError::WorkerPoisoned) | Ok(()) => {}
    }
}

const SAMPLE_COUNT: usize = 256;

#[allow(unused)]
struct DeviceGroup {
    group: QBox<QGroupBox>,
    combo_box: QBox<QComboBox>,
    auto_select: QBox<QCheckBox>,
    filter: QBox<QLineEdit>,
    row_widget: QBox<QWidget>,
    b1: QBox<QPushButton>,
    b2: QBox<QPushButton>,
    b3: QBox<QPushButton>,

    device: Rc<DeviceManager>,
    settings: Rc<AppSettings>,
}

impl DeviceGroup {
    unsafe fn new(
        device: Rc<DeviceManager>,
        settings: Rc<AppSettings>,
    ) -> (Rc<DeviceGroup>, Ptr<QGroupBox>) {
        let layout = QVBoxLayout::new_0a();
        let group = QGroupBox::new();
        group.set_title(&qs("Device"));
        group.set_layout(&layout);

        let auto_select = QCheckBox::new();
        auto_select.set_text(&qs("Auto select device"));
        auto_select.set_checked(settings.auto_select_device);
        layout.add_widget(&auto_select);

        let entry = QLineEdit::new();
        entry.set_placeholder_text(&qs("Device filter"));
        entry.set_text(&qs(&settings.device_filter));

        let combo_box = QComboBox::new_0a();

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
        handle_send_result(device.send_command(DeviceBoundCommand::RefreshDevices {
            args: settings.device_filter.clone(),
        }));

        let s = Rc::new(Self {
            group,
            combo_box,
            filter: entry,
            row_widget,
            b1,
            b2,
            b3,
            auto_select,

            settings,
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
            device: _,
            ..
        } = self.borrow();

        let s = self.clone();
        auto_select
            .clicked()
            .connect(&SlotOfBool::new(group, move |checked| {
                s.filter.set_enabled(!checked);
                s.combo_box.set_enabled(!checked);
                // start the refreshing pingpong
                // FIXME clicking the refresh button from code is a very poorly searchable pattern
                s.b1.click();
            }));

        let s = self.clone();
        combo_box
            .current_index_changed()
            .connect(&SlotNoArgs::new(group, move || {
                let enabled = (s.combo_box.count() != 0) && !s.device.get_device_valid();
                s.b2.set_enabled(enabled);
            }));

        let s = self.clone();
        b1.clicked().connect(&SlotNoArgs::new(group, move || {
            // only send refresh request when the last one has finished
            if !s.device.get_refreshing_devices() {
                let filter = s.filter.text().to_std_string();

                handle_send_result(
                    s.device
                        .send_command(DeviceBoundCommand::RefreshDevices { args: filter }),
                );
            }
        }));

        let s = self.clone();
        b2.clicked().connect(&SlotNoArgs::new(group, move || {
            if s.combo_box.count() != 0 {
                let index = s.combo_box.current_index();

                handle_send_result(s.device.send_command(DeviceBoundCommand::CreateDevice {
                    index: index as usize,
                }));

                s.b2.set_enabled(false);
                s.b3.set_enabled(true);
            }
        }));

        let s = self.clone();
        b3.clicked().connect(&SlotNoArgs::new(group, move || {
            handle_send_result(s.device.send_command(DeviceBoundCommand::DestroyDevice));

            s.b3.set_enabled(false);
        }));
    }
    unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
        match event.as_ref().unwrap() {
            GuiBoundEvent::WorkerReset => {
                // self.combo_box.clear(); // it's not very ergonomic to make me click refresh every time the worker crashes
                self.b2.set_enabled(false);
                self.b3.set_enabled(false);

                // force refresh the devices because the worker thread lost it's list of them
                self.b1.click();
            }
            GuiBoundEvent::RefreshedDevices { list } => {
                self.combo_box.clear();

                for name in list {
                    self.combo_box.add_item_q_string(&qs(name.as_str()));
                }

                if self.auto_select.is_checked() {
                    // if there were no devices found, search again
                    // FIXME would be better to do this less frequently
                    if list.is_empty() {
                        self.b1.click();
                        return;
                    }

                    // try to find the exact device as was selected previously
                    if !self.settings.device.is_empty() {
                        if let Some((i, _)) = list
                            .iter()
                            .enumerate()
                            .find(|(_, s)| **s == self.settings.device)
                        {
                            self.combo_box.set_current_index(i as i32);
                            self.b2.click();
                            return;
                        }
                    }

                    // just select the first device
                    self.combo_box.set_current_index(0);
                    self.b2.click();
                }
            }
            _ => (),
        };
    }
    unsafe fn populate_settings(&self, settings: &mut AppSettings) {
        let AppSettings {
            auto_select_device,
            device_filter,
            device,
            ..
        } = settings;

        *auto_select_device = self.auto_select.is_checked();

        *device_filter = self.filter.text().to_std_string();

        *device = match self.combo_box.count() {
            0 => "".to_string(),
            _ => self.combo_box.current_text().to_std_string(),
        }
    }
}

const DATA_REQUESTS_IN_FLIGHT: usize = 8;

enum Samplerate {
    Ranges(QBox<QDoubleSpinBox>),
    Values(QBox<QComboBox>),
}

struct ReceiveGroup {
    automatic_update: QBox<QCheckBox>,
    frequency: QBox<QDoubleSpinBox>,
    // most devices provide only a set of valid values for samplerate
    // some are able to cover a range though, :(
    samplerate: RefCell<Samplerate>,
    gain: QBox<QDoubleSpinBox>,
    // some devices, for example RTL-SDR, do not allow setting bandwidth
    // currently it set if the valid bandwith range returned by the device is empty
    bandwidth_available: Cell<bool>,
    automatic_gain: QBox<QCheckBox>,
    automatic_dc_offset: QBox<QCheckBox>,
    apply_btn: QBox<QPushButton>,

    group: QBox<QGroupBox>,
    form_layout: QBox<QFormLayout>,

    value_ranges: RefCell<Option<ValueRanges>>,
    device: Rc<DeviceManager>,
}

impl ReceiveGroup {
    unsafe fn new(
        device: Rc<DeviceManager>,
        settings: Rc<AppSettings>,
    ) -> (Rc<Self>, Ptr<QGroupBox>) {
        let group = QGroupBox::new();
        group.set_title(&qs("Receive"));

        let v = QVBoxLayout::new_0a();

        let automatic_update = QCheckBox::new();
        automatic_update.set_checked(settings.auto_update);
        automatic_update.set_text(&qs("Automatic update"));
        v.add_widget(&automatic_update);

        let form_layout = QFormLayout::new_0a();
        v.add_layout_1a(&form_layout);
        group.set_layout(&v);

        let frequency = QDoubleSpinBox::new_0a();
        frequency.set_suffix(&qs(" MHz"));
        frequency.set_value(settings.frequency);
        form_layout.add_row_q_string_q_widget(&qs("Frequency"), &frequency);

        let samplerate = QDoubleSpinBox::new_0a();
        samplerate.set_suffix(&qs(" MSps"));
        samplerate.set_value(settings.samplerate);
        form_layout.add_row_q_string_q_widget(&qs("Samplerate"), &samplerate);

        let gain = QDoubleSpinBox::new_0a();
        gain.set_suffix(&qs(" dB"));
        gain.set_value(settings.gain);
        form_layout.add_row_q_string_q_widget(&qs("Gain"), &gain);

        let automatic_gain = QCheckBox::new();
        automatic_gain.set_checked(settings.automatic_gain);
        form_layout.add_row_q_string_q_widget(&qs("Automatic gain"), &automatic_gain);

        let automatic_dc_offset = QCheckBox::new();
        automatic_dc_offset.set_checked(settings.automatic_dc_offset);
        form_layout.add_row_q_string_q_widget(&qs("Automatic DC offset"), &automatic_dc_offset);

        let apply_btn = QPushButton::new();
        apply_btn.set_text(&qs("Apply"));
        v.add_widget(&apply_btn);

        let ptr = group.as_ptr();
        let s = Rc::new(Self {
            automatic_update,
            samplerate: RefCell::new(Samplerate::Ranges(samplerate)),
            frequency,
            bandwidth_available: Cell::new(true),
            gain,
            automatic_gain,
            automatic_dc_offset,
            apply_btn,
            group,
            form_layout,

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

        let samplerate = match &*self.samplerate.borrow() {
            Samplerate::Ranges(spinbox) => spinbox.value(),
            // in the case of only discreet values being available, minimum==maximum
            // simply get it from the Range minimum
            Samplerate::Values(combox) => {
                self.value_ranges.borrow().as_ref().unwrap().samplerate
                    [combox.current_index() as usize]
                    .minimum
            }
        };

        let state = ReceiverState {
            // TODO channel is hardcoded for now, it seems it is not too useful to be able to specify it, at least on my device
            channel: 0,
            samplerate: samplerate * MIL,
            frequency: self.frequency.value() * MIL,
            // set bandwidth to 75% of samplerate, seems to work fine for OsmoSDR
            // https://github.com/osmocom/gr-osmosdr/blob/e5bee0820f493d2ff048ba4ed18be4d0c7976a87/lib/soapy/soapy_sink_c.cc#L297
            // hopefully the driver is fine with rounding it to an available value, it is possible to be more smart about it
            bandwidth: if self.bandwidth_available.get() {
                samplerate * MIL * 0.75
            } else {
                0.0
            },
            gain: self.gain.value(),
            automatic_gain: self.automatic_gain.is_checked(),
            automatic_dc_offset: self.automatic_dc_offset.is_checked(),
        };

        handle_send_result(
            self.device
                .send_command(DeviceBoundCommand::SetReceiver(state)),
        );

        // this is the first point in program execution where the receiver get actually configured ad is usable,
        // therefore here we check if there are any existing requests (the configuration was only updated,
        // not configured for the first time) and if not we inject them to be pingponged between this thread and the worker
        if self.device.get_data_requests_in_flight() == 0 {
            for _ in 0..DATA_REQUESTS_IN_FLIGHT {
                let command = DeviceBoundCommand::RequestData {
                    data: FftData::new(1024),
                };

                handle_send_result(self.device.send_command(command));
            }
        }
    }
    unsafe fn init(self: &Rc<Self>) {
        let Self {
            automatic_update,
            frequency,
            gain,
            automatic_gain,
            automatic_dc_offset,
            apply_btn,
            group,
            ..
        } = self.borrow();

        let s = self.clone();
        automatic_update
            .state_changed()
            .connect(&SlotOfInt::new(group, move |state| {
                let enabled = state == CheckState::Unchecked.into() && s.device.get_device_valid();
                s.apply_btn.set_enabled(enabled);

                if state == CheckState::Checked.into() {
                    s.values_changed();
                }
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

                        drop(ranges);

                        if s.automatic_update.is_checked() {
                            s.values_changed();
                        }
                    }));
            };
        }

        // setup_values_changed! {samplerate, std::iter::IntoIterator::into_iter};
        setup_values_changed! {frequency, std::iter::IntoIterator::into_iter};
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
    unsafe fn handle_event(self: &Rc<Self>, event: &mut Option<GuiBoundEvent>) {
        {
            let enabled = self.device.get_device_valid();
            self.group.set_enabled(enabled);
        }

        match event.as_ref().unwrap() {
            // it is incredibly ugly to be doing this replacement here and everytime the device changes
            // but this wouldn't be a gui project without bad code
            GuiBoundEvent::DeviceCreated { channels_info } => {
                let mut ranges = channels_info[0].ranges.clone();

                // everything is in megahertz or megasamples/second
                const MIL: f64 = 1_000_000.0;

                self.form_layout.remove_row_int(1);

                // the device supports only discreet samplerates
                if ranges.samplerate[0].minimum == ranges.samplerate[0].maximum {
                    let combox = QComboBox::new_0a();

                    for range in &ranges.samplerate {
                        let label = format!("{} MSps", range.minimum / MIL);
                        combox.add_item_q_string(&qs(label));
                    }

                    let s = self.clone();
                    combox
                        .current_index_changed()
                        .connect(&SlotNoArgs::new(&combox, move || {
                            if s.automatic_update.is_checked() {
                                s.values_changed();
                            }
                        }));

                    self.form_layout.insert_row_int_q_string_q_widget(
                        1,
                        &qs("Samplerate"),
                        &combox,
                    );
                    self.samplerate.replace(Samplerate::Values(combox));
                } else {
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

                    let spinbox = QDoubleSpinBox::new_0a();
                    spinbox.set_range(min / MIL, max / MIL);

                    let s = self.clone();
                    spinbox
                        .editing_finished()
                        .connect(&SlotNoArgs::new(&spinbox, move || {
                            let ranges = s.value_ranges.borrow_mut();
                            let r = &ranges.as_ref().unwrap().samplerate;

                            match &*s.samplerate.borrow() {
                                Samplerate::Ranges(spinbox) => clamp_value(&spinbox, &mut r.iter()),
                                Samplerate::Values(_) => unreachable!(),
                            }

                            drop(ranges);

                            if s.automatic_update.is_checked() {
                                s.values_changed();
                            }
                        }));

                    self.form_layout.insert_row_int_q_string_q_widget(
                        1,
                        &qs("Samplerate"),
                        &spinbox,
                    );
                    self.samplerate.replace(Samplerate::Ranges(spinbox));
                }

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

                self.bandwidth_available.set(!ranges.bandwidth.is_empty());

                let min = ranges.gain.minimum;
                let max = ranges.gain.maximum;
                self.gain.set_range(min, max);

                fn scale_to_mega(ranges: &mut Vec<soapysdr::Range>) {
                    ranges.iter_mut().for_each(|s| {
                        s.minimum /= MIL;
                        s.maximum /= MIL;
                        s.step /= MIL
                    });
                }

                // scale the ranges so that they match the displayed units
                scale_to_mega(&mut ranges.samplerate);
                scale_to_mega(&mut ranges.frequency);
                scale_to_mega(&mut ranges.bandwidth);

                self.value_ranges.replace(Some(ranges));
            }
            _ => (),
        }
    }
    unsafe fn populate_settings(&self, settings: &mut AppSettings) {
        let AppSettings {
            auto_update,
            frequency,
            samplerate,
            gain,
            automatic_gain,
            automatic_dc_offset,
            ..
        } = settings;

        // TODO deduplicate this from values_changed()
        *auto_update = self.automatic_update.is_checked();

        *frequency = self.frequency.value();
        *samplerate = match &*self.samplerate.borrow() {
            Samplerate::Ranges(spinbox) => spinbox.value(),
            // in the case of only discreet values being available, minimum==maximum
            // simply get it from the Range minimum
            Samplerate::Values(combox) => {
                self.value_ranges.borrow().as_ref().unwrap().samplerate
                    [combox.current_index() as usize]
                    .minimum
            }
        };
        *gain = self.gain.value();
        *automatic_gain = self.automatic_gain.is_checked();
        *automatic_dc_offset = self.automatic_dc_offset.is_checked();
    }
}

// a helper function for ReceiveGroup to clamp the configured parameters to valid ranges
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

#[allow(unused)]
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

    unsafe fn clear(&self) {
        self.series.clear();
        self.pixlabel.clear();
    }
}

#[allow(unused)]
struct OutputGroup {
    group: QBox<QGroupBox>,
    grid: QBox<QGridLayout>,
    signal: ShittySpectogram,
    spectrum: ShittySpectogram,
    text_edit: QBox<QTextEdit>,

    device: Rc<DeviceManager>,
}

impl OutputGroup {
    unsafe fn new(
        device: Rc<DeviceManager>,
        settings: Rc<AppSettings>,
    ) -> (Rc<Self>, Ptr<QGroupBox>) {
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
            GuiBoundEvent::DeviceCreated { .. } => {
                self.signal.clear();
                self.spectrum.clear();
                self.text_edit.clear();
            }
            GuiBoundEvent::DecodedChars { data: _ } => todo!(),
            GuiBoundEvent::DataReady { data } => {
                unsafe fn coloring_fn(f: f64) -> CppBox<QColor> {
                    QColor::from_rgb_f_3a(f.ln() * 0.2, 0.0, 0.0)
                }

                let len = data.get_output().len();
                let averaged_signal = data
                    .get_input()
                    .chunks(4)
                    .map(|chunks| chunks.iter().map(|c| c.re).sum::<RxFormat>() as f64 / 4.0);
                let averaged_spectrum = data.get_output()[0..(len / 2 + 1)]
                    .chunks(4)
                    .map(|chunks| chunks.iter().map(|c| c.re).sum::<RxFormat>() as f64 / 4.0);

                self.signal.add_new_data(averaged_signal, coloring_fn);
                self.spectrum.add_new_data(averaged_spectrum, coloring_fn);

                if self.device.get_receiver_valid() {
                    match event.take().unwrap() {
                        GuiBoundEvent::DataReady { data } => handle_send_result(
                            self.device
                                .send_command(DeviceBoundCommand::RequestData { data }),
                        ),
                        _ => unreachable!(),
                    };
                }
            }
            _ => (),
        }
    }
}

#[derive(Clone, Debug)]
struct AppSettings {
    auto_select_device: bool,
    device_filter: String,
    device: String,

    auto_update: bool,
    frequency: f64,
    samplerate: f64,
    gain: f64,
    automatic_gain: bool,
    automatic_dc_offset: bool,
}

impl AppSettings {
    fn pretty_serialize(&self) -> String {
        let AppSettings {
            auto_select_device,
            device_filter,
            device,
            auto_update,
            frequency,
            samplerate,
            gain,
            automatic_gain,
            automatic_dc_offset,
            ..
        } = self.clone();

        format!(
            r#"auto_device = {:8}      # if true, the application tries to immediatelly select a device without user input
device_filter = {:8}    # the "args" used to filter the SoapySDR devices, for example 'driver=RTLSDR' or 'hardware=R820T' 
device = {:8}           # the 'label' field of the device used last time, auto_select_device first tries to find a device with this label

auto_update = {:8}      # whether to update the receiver configuration immediatelly after a value is changed

    # values of the different configuration options
    frequency = {} # MHz
    samplerate = {} # MSps
    gain = {} # dB
    automatic_gain = "{}"
    automatic_dc_offset = "{}""#,
            // the data is first formatted into a string before being interpolated into the main string
            // so that the minimum width-format is correct
            format!("\"{}\"", auto_select_device),
            format!("\"{}\"", device_filter),
            format!("\"{}\"", device),
            format!("\"{}\"", auto_update),
            frequency,
            samplerate,
            gain,
            automatic_gain,
            automatic_dc_offset,
        )
    }
}

const DEFAULT_SETTINGS: AppSettings = AppSettings {
    auto_select_device: false,
    device_filter: String::new(),
    device: String::new(),

    auto_update: false,
    frequency: 0.0,
    samplerate: 0.0,
    gain: 0.0,
    automatic_gain: false,
    automatic_dc_offset: false,
};

//                      (Settings, Save path)
fn get_settings() -> (AppSettings, Option<PathBuf>) {
    const HELP: &str = "\
Overview: Tool for receiving transmission from weather baloons.

Usage: radiothing [options]

Options:
--create-config       Write default config file to provided path and immediatelly exit, CWD if empty.
-c, --config          Path to configuration file and/or the path the config will be saved to,
                      by default the current working directory.
-i, --ignore-config   Ignore any configuration file, don't save upon exit either.
-s, --save-config     Path to save the configuration on program exit, by default same as path.
-h, --help            Print this help.
";

    let mut args = pico_args::Arguments::from_env();

    if args.contains(["-h", "--help"]) {
        print!("{}", HELP);
        std::process::exit(0);
    }

    if args.contains("--create-config") {
        let path = args
            .opt_value_from_str("--create-config")
            .unwrap()
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap()
                    .join("radiothing_config.txt")
            });

        log::info!(
            "Creating default configuration at '{}'",
            path.to_string_lossy()
        );

        match std::fs::write(&path, DEFAULT_SETTINGS.pretty_serialize()) {
            Err(e) => log::error!(
                "Error writing config to file at '{}': {}",
                path.to_string_lossy(),
                e
            ),
            Ok(_) => {}
        }

        std::process::exit(0);
    }

    if !args.contains(["-i", "--ignore-config"]) {
        let path = args
            .opt_value_from_str(["-c", "--config"])
            .unwrap()
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap()
                    .join("radiothing_config.txt")
            });

        let save_path =
            if let Some(save) = args.opt_value_from_str(["-s", "--save-config"]).unwrap() {
                Some(save)
            } else if args.contains(["-s", "--save-config"]) {
                Some(path.clone())
            } else {
                None
            };

        log::info!("Reading configuration file at '{}'", path.to_string_lossy());

        match &save_path {
            Some(path) => log::info!("Save path is '{}'", path.to_string_lossy()),
            None => log::info!("Will not save config"),
        }

        if path.is_file() {
            let string = match std::fs::read_to_string(&path) {
                Ok(string) => string,
                Err(e) => {
                    log::error!(
                        "Error reading config file at '{}': {}",
                        path.to_string_lossy(),
                        e
                    );
                    return (DEFAULT_SETTINGS, save_path);
                }
            };

            let (settings, errors) = settings::Settings::new(string.as_str());

            if !errors.is_empty() {
                let errors_string: String = errors.iter().map(|e| e.to_string() + "\n").collect();
                log::error!(
                    "Encountered errors while parsing settings, falling back to defaults:\n{}",
                    errors_string
                );

                // the parsed settings can't be assumed to be correct
                // fall back to the defaults but set save_config to false so that
                // we don't overwrite the bad settings file in case the error there is only minor
                return (DEFAULT_SETTINGS, None);
            } else {
                macro_rules! settings_from_settings {
                        ($($field:ident),* $(,)*) => {
                            AppSettings {
                                $(
                                    $field: settings.get(stringify!($field)).unwrap_or(DEFAULT_SETTINGS.$field),
                                )*
                            }
                        }
                    }

                let settings = settings_from_settings! {
                    auto_select_device,
                    device,
                    device_filter,
                    auto_update,
                    frequency,
                    samplerate,
                    gain,
                    automatic_gain,
                    automatic_dc_offset,
                };

                return (settings, save_path);
            }
        } else {
            log::error!("Config file at '{}' is not a file", path.to_string_lossy())
        }
    } else {
        log::info!("Ignoring config");
    }

    return (DEFAULT_SETTINGS, None);
}

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
        let (settings, save_path) = get_settings();
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
        v_layout.add_stretch_0a();

        let (output_group, group) = OutputGroup::new(device.clone(), settings.clone());
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
