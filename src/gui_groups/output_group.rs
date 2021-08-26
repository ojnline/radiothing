use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use qt_charts::{
    cpp_core::CppBox,
    qt_core::{AlignmentFlag, QRectF},
    qt_gui::{q_image::Format, q_painter::RenderHint, QColor, QImage, QPixmap},
    QChart, QChartView, QLineSeries, QValueAxis,
};
use qt_widgets::{
    cpp_core::Ptr,
    q_layout::SizeConstraint,
    qt_core::{qs, QBox},
    QGridLayout, QGroupBox, QLabel, QTextEdit, QVBoxLayout, QWidget,
};

use crate::device::{DeviceBoundCommand, DeviceManager, GuiBoundEvent};
use crate::gui_groups::handle_send_result;
use crate::SAMPLE_COUNT;

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
pub struct OutputGroup {
    group: QBox<QGroupBox>,
    grid: QBox<QGridLayout>,
    signal: ShittySpectogram,
    spectrum: ShittySpectogram,
    text_edit: QBox<QTextEdit>,

    device: Rc<DeviceManager>,
}

impl OutputGroup {
    pub unsafe fn new(device: Rc<DeviceManager>) -> (Rc<Self>, Ptr<QGroupBox>) {
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
    pub unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
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

                const G: f32 = 20.0;
                let averaged_signal = data.get_input().iter().map(|s| (G * s.re) as f64);
                let averaged_spectrum = data.get_output().iter().map(|s| (G * s.re) as f64);
                // let averaged_signal = data
                //     .get_input()
                //     .chunks(16)
                //     .map(|chunks| chunks.iter().map(|c| c.re).sum::<RxFormat>() as f64 / 16.0);
                // let averaged_spectrum = data.get_output()[0..(len / 2 + 1)]
                //     .chunks(16)
                //     .map(|chunks| chunks.iter().map(|c| c.re).sum::<RxFormat>() as f64 / 16.0);

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
