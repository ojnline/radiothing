use std::borrow::Borrow;
use std::cell::Cell;
use std::{ops::Range, rc::Rc};

use crate::device::{DeviceBoundCommand, DeviceManager, GuiBoundEvent};
use crate::gui_groups::handle_send_result;
use crate::{FftData, DATA_REQUESTS_IN_FLIGHT, SAMPLE_COUNT};

use qt_charts::{
    qt_core::{AlignmentFlag, QVectorOfQPointF, SlotNoArgs},
    qt_gui::{q_font_database::SystemFont, q_painter::RenderHint, QFontDatabase},
    QChart, QChartView, QLineSeries, QValueAxis,
};
use qt_widgets::{
    cpp_core::Ptr,
    q_size_policy::Policy,
    q_style::StandardPixmap,
    qt_core::{qs, QBox},
    QApplication, QGridLayout, QGroupBox, QPushButton, QTextEdit,
};
use rustfft::num_complex::Complex32;

#[allow(unused)]
struct SingleSeriesGraph {
    chart: QBox<QChart>,
    view: QBox<QChartView>,
    series: QBox<QLineSeries>,

    x_axis: QBox<QValueAxis>,
    y_axis: QBox<QValueAxis>,
    y_scale: Cell<f32>,
}

impl SingleSeriesGraph {
    unsafe fn new(
        x: Range<f64>,
        y: f64,
        x_label: &str,
        y_label: &str,
        title: &str,
        y_axis_show_labels: bool,
        show_markers: bool,
        grid_visible: bool,
    ) -> Self {
        let chart = QChart::new_0a();

        if title.is_empty() {
            let margins = chart.margins();
            margins.set_top((-margins.top() as f64 / 1.5) as i32);
            chart.set_margins(&margins);
        } else {
            chart.set_title(&qs(title));
        }

        let x_axis = QValueAxis::new_0a();
        let y_axis = QValueAxis::new_0a();

        let point_size = x_axis.labels_font().point_size();
        let mono_font = QFontDatabase::system_font(SystemFont::FixedFont);
        mono_font.set_point_size(point_size);

        x_axis.set_range(x.start, x.end);
        x_axis.set_title_text(&qs(x_label));
        x_axis.set_labels_font(&mono_font);

        y_axis.set_range(-y, y);
        y_axis.set_title_text(&qs(y_label));
        y_axis.set_labels_font(&mono_font);

        chart.add_axis(&x_axis, AlignmentFlag::AlignBottom.into());
        chart.add_axis(&y_axis, AlignmentFlag::AlignLeft.into());

        let series = QLineSeries::new_0a();
        chart.add_series(&series);
        // no x-axis is set and the series is empty so it seems that the series defaults to range 0..1
        series.attach_axis(&y_axis);

        if !y_axis_show_labels {
            y_axis.set_label_format(&qs(" "));
        }

        if !show_markers {
            chart.legend().markers_0a().iter().for_each(|m| {
                let m = m.as_ref().unwrap().as_ref().unwrap();
                m.set_visible(false);
            });
        }

        x_axis.set_grid_line_visible_1a(grid_visible);
        y_axis.set_grid_line_visible_1a(grid_visible);

        let view = QChartView::from_q_chart(&chart);
        view.set_render_hint_1a(RenderHint::Antialiasing);
        view.set_size_policy_2a(Policy::MinimumExpanding, Policy::MinimumExpanding);

        Self {
            chart,
            view,
            series,

            x_axis,
            y_axis,
            y_scale: Cell::new(y.abs() as f32),
        }
    }

    // fill the QLineSeries in the graph with the entirety of y_samples
    //  x is always scaled from 0..1
    //  the imaginary part is discarded

    // the safety of this is dubious at best but should work
    pub unsafe fn update_series(
        &self,
        y_samples: &[Complex32],
        fit_y: bool,
        smoothing_factor: f32,
        proportional_margin: f32,
    ) {
        if y_samples.len() < 2 {
            return;
        }

        self.view.set_updates_enabled(false);

        if fit_y {
            let y_scale = self.y_scale.get();

            let mut abs_max = 0.0f32;
            for s in y_samples {
                abs_max = abs_max.max(s.re.abs());
            }

            abs_max += abs_max * proportional_margin;
            let new_y_scale = y_scale * smoothing_factor + abs_max * (1.0 - smoothing_factor);

            self.y_axis
                .set_range(-new_y_scale as f64, new_y_scale as f64);
            self.y_scale.set(new_y_scale);
        }

        // QVector, like most Qt containers, is implicitly shared which allows us to update all the data at once in this roundabout way
        let vector = self.series.points_vector();
        {
            let empty = QVectorOfQPointF::new_0a();
            // remove the shared reference held by the series, otherwise resize() and more importantly data() would reallocate even though the size is the same and the old data is discarded
            // look at the beautiful code here https://code.woboq.org/qt5/include/qt/QtCore/qvector.h.html#_ZN7QVector4dataEv
            self.series.replace_q_vector_of_q_point_f(&empty);
        }

        // qt reallocates vectors even thought the previous size is larger than the requested size
        // (this has now been changed but not backported)
        if y_samples.len() as i32 > vector.size() {
            vector.resize(y_samples.len() as i32);
        }

        // PointF source is here https://code.woboq.org/qt5/qtbase/src/corelib/tools/qpoint.h.html#QPointF::xp
        // this is horrible hacking to be able to write the vector memory without calling qt functions which cannot be inlined
        // due to cpp not having a stable abi, it is impossible to soundly bind field access so the field offsets are computed here

        // with common sense, PointF should always have a stride of 16 bytes (2 doubles)
        // but such speculation on memory layout is so horribly evil and not guaranteed
        let pointf_stride = {
            let ptr0 = vector.at(0).as_raw_ptr() as *const u8;
            let ptr1 = vector.at(1).as_raw_ptr() as *const u8;

            ptr1.offset_from(ptr0)
        };

        let data_ptr = vector.data().as_mut_raw_ptr();

        // most likely offset from the base pointer by 0 bytes
        let x0 = (*data_ptr).rx() as *mut u8;
        // most likely offset from the base pointer by 8 bytes
        let y0 = (*data_ptr).ry() as *mut u8;

        // dbg!(pointf_stride);
        // dbg!(x0.offset_from(data_ptr as *const u8));
        // dbg!(y0.offset_from(data_ptr as *const u8));

        let d_x = 1.0 / (y_samples.len() as f64);
        let mut x = 0.0;

        for (i, y) in y_samples.iter().enumerate() {
            let y = y.re as f64;

            (x0.offset(i as isize * pointf_stride) as *mut ::std::os::raw::c_double).write(x);
            (y0.offset(i as isize * pointf_stride) as *mut ::std::os::raw::c_double).write(y);

            x += d_x;
        }

        self.view.set_updates_enabled(true);

        self.series.replace_q_vector_of_q_point_f(&vector);
    }
}

#[allow(unused)]
pub struct OutputGroup {
    group: QBox<QGroupBox>,
    run: QBox<QPushButton>,
    run_state: Cell<bool>,
    grid: QBox<QGridLayout>,
    signal: SingleSeriesGraph,
    spectrum: SingleSeriesGraph,
    text_edit: QBox<QTextEdit>,

    device: Rc<DeviceManager>,
}

impl OutputGroup {
    pub unsafe fn new(device: Rc<DeviceManager>) -> (Rc<Self>, Ptr<QGroupBox>) {
        let group = QGroupBox::new();
        let grid = QGridLayout::new_0a();

        group.set_size_policy_2a(Policy::Expanding, Policy::Expanding);
        group.set_layout(&grid);

        // the axis ranges are meaningless because they are overridden after the correct signals are bound in init()
        let signal = SingleSeriesGraph::new(0.0..1.0, 0.1, "ms", "", "Signal", true, false, true);
        grid.add_widget_3a(&signal.view, 0, 0);

        // the axis ranges are meaningless because they are overridden after the correct signals are bound in init()
        let spectrum =
            SingleSeriesGraph::new(0.0..1.0, 0.1, "Hz", "", "Spectrum", true, false, true);
        grid.add_widget_3a(&spectrum.view, 0, 1);

        let text_edit = QTextEdit::new();
        text_edit.set_read_only(true);
        grid.add_widget_5a(&text_edit, 1, 0, 1, 2);

        let run = QPushButton::new();
        set_run_button_icon(&run, false);
        run.set_icon_size(&(&*run.icon_size() * 1.5));
        run.set_flat(true);
        run.set_size_policy_2a(Policy::Fixed, Policy::Fixed);

        grid.add_widget_6a(&run, 2, 0, 1, 2, AlignmentFlag::AlignCenter.into());

        let ptr = group.as_ptr();
        let s = Rc::new(Self {
            group,
            run,
            run_state: Cell::new(false),
            grid,
            signal,
            spectrum,
            text_edit,

            device,
        });

        s.init();

        (s, ptr)
    }
    unsafe fn init(self: &Rc<Self>) {
        let Self { group, run, .. } = self.borrow();

        let s = self.clone();
        // FIXME deduplicate this from handle_event
        run.clicked().connect(&SlotNoArgs::new(group, move || {
            let run = !s.run_state.get();
            s.run_state.set(run);

            let enabled =
                run == true && s.device.get_device_valid() && s.device.get_receiver_valid();

            set_run_button_icon(&s.run, enabled);

            if enabled {
                s.device.set_receive_enabled(true);

                for _ in 0..(DATA_REQUESTS_IN_FLIGHT
                    .saturating_sub(s.device.get_data_requests_in_flight()))
                {
                    let command = DeviceBoundCommand::RequestData {
                        data: FftData::new(SAMPLE_COUNT),
                    };

                    handle_send_result(s.device.send_command(command));
                }
            }
        }));
    }
    pub unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
        match event.as_ref().unwrap() {
            GuiBoundEvent::DeviceCreated { .. } => {
                // self.signal.clear();
                // self.spectrum.clear();
                // self.text_edit.clear();

                if self.run_state.get() && self.device.get_receiver_valid() {
                    self.device.set_receive_enabled(true);

                    for _ in 0..(DATA_REQUESTS_IN_FLIGHT
                        .saturating_sub(self.device.get_data_requests_in_flight()))
                    {
                        let command = DeviceBoundCommand::RequestData {
                            data: FftData::new(SAMPLE_COUNT),
                        };

                        handle_send_result(self.device.send_command(command));
                    }
                }
            }
            GuiBoundEvent::DecodedChars { data: _ } => todo!(),
            GuiBoundEvent::DataReady { data } => {
                if !(self.device.get_receiver_valid() && self.run_state.get()) {
                    return;
                }

                let signal = data.get_input();
                let spectrum = data.get_output();

                self.signal.update_series(signal, true, 0.9, 0.2);
                self.spectrum.update_series(spectrum, true, 0.9, 0.2);

                match event.take().unwrap() {
                    GuiBoundEvent::DataReady { data } => handle_send_result(
                        self.device
                            .send_command(DeviceBoundCommand::RequestData { data }),
                    ),
                    _ => unreachable!(),
                };
            }
            GuiBoundEvent::DeviceDestroyed | GuiBoundEvent::WorkerReset => {
                self.set_run(false);
            }
            _ => (),
        }
    }
    pub unsafe fn set_run(&self, run: bool) {
        self.run.set_checked(run);
        set_run_button_icon(&self.run, run);
        self.run_state.set(run);
    }
}

unsafe fn set_run_button_icon(button: &QPushButton, state: bool) {
    let icon = match state {
        true => QApplication::style().standard_icon_1a(StandardPixmap::SPMediaPause),
        false => QApplication::style().standard_icon_1a(StandardPixmap::SPMediaPlay),
    };

    button.set_icon(&icon);
}
