// #![windows_subsystem = "linux"]
#![allow(unused)]

mod worker;

use qt::QHBoxLayout;
use qt_charts::{
    cpp_core::CppBox,
    qt_core::{AlignmentFlag, QRectF, QTimer},
    qt_gui::{q_image::Format, q_painter::RenderHint, QColor, QImage, QPixmap},
    *,
};
use qt_widgets::qt_core::{qs, QBox, SlotNoArgs};
use qt_widgets::{
    self as qt, q_size_policy::Policy, QApplication, QGridLayout, QGroupBox, QPushButton,
    QVBoxLayout, QWidget,
};
use qt_widgets::{cpp_core::Ptr, q_layout::SizeConstraint, QLabel};
use rustfft::num_complex::Complex64;
use soapysdr::{Args, Device};
use std::{borrow::Borrow, cell::RefCell, f64::consts::FRAC_PI_2, ops::Range, rc::Rc};

const SAMPLE_WINDOW: usize = 64;

struct Radio {
    device: Option<Device>,
    args: Vec<Args>,
}

impl Radio {
    fn new() -> Self {
        Self {
            device: None,
            args: Vec::new(),
        }
    }

    // fn init_stream(&mut self, samplerate: usize) {
    //     if let Some(device) = self.device.as_ref() {
    //         device.
    //     }
    // }

    fn get_data(&mut self, buffer: &mut [Complex64]) {
        for (i, s) in buffer.iter_mut().enumerate() {
            // s.re = (i as f64 * 5.0).sin()
            fn sin_sum(n: usize, f: f64) -> f64 {
                (1..=n).into_iter().map(|i| (i as f64 * f).sin()).sum()
            }
            let i = i as f64 * 0.2;
            let n = 3;
            let r = sin_sum(n, i);
            let i = sin_sum(n, i + FRAC_PI_2);
            *s = Complex64::new(r, i);
        }
    }
}

struct DeviceGroup {
    group: QBox<QGroupBox>,
    combo_box: QBox<qt::QComboBox>,
    entry: QBox<qt::QLineEdit>,
    row_widget: QBox<QWidget>,
    b1: QBox<qt::QPushButton>,
    b2: QBox<qt::QPushButton>,
    b3: QBox<qt::QPushButton>,
    radio: RefCell<Radio>,
}

impl DeviceGroup {
    unsafe fn new() -> (Rc<DeviceGroup>, Ptr<QGroupBox>) {
        let layout = QVBoxLayout::new_0a();
        let group = QGroupBox::new();
        group.set_title(&qs("Device"));
        group.set_layout(&layout);
        // group.set_size_policy_2a(Policy::Fixed, Policy::MinimumExpanding);

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

        // layout.add_spacing(50);

        layout.set_size_constraint(SizeConstraint::SetFixedSize);
        // layout.set_direction(Direction::Down);
        // layout.set_alignment_q_layout_q_flags_alignment_flag(&layout, AlignmentFlag::AlignTop.into());
        // layout.add_stretch_0a();
        // layout.ali

        let ptr = group.as_ptr();

        let s = Rc::new(Self {
            group,
            combo_box,
            entry,
            row_widget,
            b1,
            b2,
            b3,
            radio: RefCell::new(Radio::new()),
        });

        s.init();

        (s, ptr)
    }
    unsafe fn init(self: &Rc<Self>) {
        let Self {
            group,
            combo_box,
            b1,
            b2,
            b3,
            ..
        } = self.borrow();

        let s = self.clone();
        combo_box
            .current_index_changed()
            .connect(&SlotNoArgs::new(group, move || {
                let enabled = (s.combo_box.count() != 0) && s.radio.borrow().device.is_none();
                s.b2.set_enabled(enabled)
            }));

        let s = self.clone();
        b1.clicked().connect(&SlotNoArgs::new(group, move || {
            let filter = s.entry.text();

            let args = soapysdr::enumerate(filter.to_std_string().as_str()).unwrap();

            s.combo_box.clear();

            for arg in &args {
                s.combo_box
                    .add_item_q_string(&qs(arg.get("label").unwrap_or("")));
            }

            s.radio.borrow_mut().args = args;
        }));

        let s = self.clone();
        b2.clicked().connect(&SlotNoArgs::new(group, move || {
            fn clone_args(a: &Args) -> Args {
                let mut c = Args::new();
                for (k, v) in a {
                    c.set(k, v)
                }
                c
            }

            if s.combo_box.count() != 0 {
                s.b2.set_enabled(false);
                s.b3.set_enabled(true);

                let backend = &mut s.radio.borrow_mut();

                drop(backend.device.take()); // drop the previous device first so that the connection gets closed

                let arg_clone = clone_args(&backend.args[s.combo_box.current_index() as usize]);
                let device = Device::new(arg_clone).unwrap();

                backend.device = Some(device);
            }
        }));

        let s = self.clone();
        b3.clicked().connect(&SlotNoArgs::new(group, move || {
            s.b2.set_enabled(true);
            s.b3.set_enabled(false);

            let device_ref = &mut s.radio.borrow_mut().device;

            drop(device_ref.take());
        }));

        b1.click();
    }
}

struct ShittySpectogram {
    graph_width: u32,
    graph_height: u32,
    frequency_samples: u32,
    spectogram_history_count: u32,
    widget: QBox<QWidget>,
    chart: QBox<QChart>,
    view: QBox<QChartView>,
    series: QBox<QLineSeries>,
    // series: QBox<QSplineSeries>,
    history_image: CppBox<QImage>,
    pixlabel: QBox<QLabel>,
    recreate_image: bool,
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
        layout.set_spacing(0);
        layout.set_margin(0);

        // let group = QGroupBox::new();
        // group.set_title(&qs("Graph"));
        // group.set_layout(&layout);

        let widget = QWidget::new_0a();
        widget.set_size_policy_2a(Policy::Fixed, Policy::Fixed);
        widget.set_layout(&layout);

        let chart = QChart::new_0a();

        let x_axis = QValueAxis::new_0a();
        x_axis.set_range(x_range.start, x_range.end);
        let y_axis = QValueAxis::new_0a();
        y_axis.set_range(y_range.start, y_range.end);
        // y_axis.set_visible_1a(false);
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

        // chart.set_plot_area(&QRectF::from_4_double(25.0, 25.0, 400.0, 300.0));
        // let margin = layout.spacing();
        // let margin = chart.margins().left();
        // let x_space = chart.margins().left() as f64;
        // let bottom_padding = 0.0;
        // let x_space = margin as f64;
        let y_space = 20.0;
        chart.set_plot_area(&QRectF::from_4_double(
            0.0,
            y_space,
            graph_width_f,  /*  - x_space * 2.0 */
            graph_height_f, /* - bottom_padding */
        ));

        // let m = chart.margins().left();
        // m.set_left(0);
        // m.set_bottom(0);
        // chart.set_margins(&m);
        // chart.set_contents_margins_4a(0.0, 0.0, 0.0, 0.0);
        // let g = chart.plot_area();
        // chart.set_plot_area(&QRectF::from_4_double(g.left(), g.right(), 400.0, 300.0));

        let chart_view = QChartView::from_q_chart(&chart);
        chart_view.set_render_hint_1a(RenderHint::Antialiasing);
        // chart_view.resize_2a(graph_width_i + margin, graph_height_i+y_space as i32);
        chart_view.set_minimum_size_2a(graph_width_i, graph_height_i + y_space as i32);
        // chart_view.set_contents_margins_4a(margin, margin, margin, 0);
        chart.set_background_roundness(0.0);
        // chart_view.set_maximum_size_2a(400, 300);
        // chart_view.set_window_title(&qs("Charts example"));
        // chart_view.as_ptr().static_upcast::<QWidget>()
        // chart_view.set_background_role(ColorRole::Dark);

        layout.add_widget(&chart_view);

        // chart_view.set_size_policy_2a(Policy::Minimum, Policy::Minimum);
        // let margins = chart_view.contents_margins();
        // margins.set_bottom(0);
        // chart_view.set_contents_margins_1a(&margins);

        let pixmap = QPixmap::from_2_int(frequency_samples as i32, spectogram_history_count as i32); // from_q_string(&qs("./bbb.jpg"));
        pixmap.fill_1a(&QColor::from_rgb_3a(0, 0, 0));

        let pixlabel = QLabel::new();
        pixlabel.set_pixmap(&pixmap);
        pixlabel.set_contents_margins_4a(margin, 0, margin, margin);
        pixlabel.set_scaled_contents(true);
        pixlabel.set_fixed_width(graph_width_i);
        layout.add_widget(&pixlabel);

        // let margins = label.contents_margins();
        // margins.set_top(0);
        // label.set_contents_margins_1a(&margins);

        // chart.set_contents_margins_4a(0.0, 0.0, 0.0, 0.0);
        // let margin = layout.margin();

        // layout.set_size_constraint(SizeConstraint::SetFixedSize);
        // layout.add_stretch_0a();

        // layout.set_contents_margins_4a(margin, -bottom_padding as i32, margin, 0);

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
            graph_width,
            graph_height,
            frequency_samples,
            spectogram_history_count,

            history_image,
            pixlabel,
            recreate_image: false,
        };

        (s, ptr)
    }

    unsafe fn set_sample_count(&mut self, count: u32) {
        self.recreate_image = true;
        self.frequency_samples = count;
    }

    unsafe fn set_history_count(&mut self, count: u32) {
        self.recreate_image = true;
        self.spectogram_history_count = count;
    }

    // resize and clear image
    unsafe fn recreate_image(&mut self) {
        let new_image = QImage::from_2_int_format(
            self.frequency_samples as i32,
            self.spectogram_history_count as i32,
            Format::FormatRGB32,
        );
        new_image.fill_uint(0);
        self.history_image = new_image;
    }

    unsafe fn add_new_data(
        &mut self,
        data: impl ExactSizeIterator<Item = f64>,
        coloring_fn: unsafe fn(f64) -> CppBox<QColor>,
    ) {
        if self.frequency_samples != data.len() as u32 {
            self.set_sample_count(data.len() as u32);
        }

        self.series.clear();
        let d_x = 1.0 / (data.len() as f64);

        let is_spectogram = self.spectogram_history_count != 0;

        if is_spectogram {
            let row = self.history_image.scan_line_mut(0) as *mut u32;

            if self.recreate_image {
                self.recreate_image();
            } else {
                // shift image data one row down
                let samples = self.frequency_samples as usize;
                std::ptr::copy(
                    row,
                    row.add(samples),
                    samples * (self.spectogram_history_count as usize - 1),
                );
            }

            for (i, p) in data.enumerate() {
                self.series.append_2_double(d_x * i as f64, p);

                let color = coloring_fn(p);
                row.add(i).write(color.rgb());
            }

            let pixmap = QPixmap::from_image_1a(&self.history_image);
            self.pixlabel.set_pixmap(&pixmap);
        } else {
            for (i, p) in data.enumerate() {
                self.series.append_2_double(d_x * i as f64, p);
            }
        }
    }
    // unsafe fn set_new_data(self: &Rc<Self>, data: impl ExactSizeIterator<Item = f64>) {
    //     self.series.clear();

    //     // let width = self.view.width();

    //     let d_x = 1.0 / (data.len() as f64);

    //     let mut x = 0.0;
    //     for p in data {
    //         self.series.append_2_double(x, p);
    //         x += d_x;
    //     }
    // }
}

// struct WipSpectogram {
//     scene: QBox<QGraphicsScene>,
//     view: QBox<QGraphicsView>
// }

// impl WipSpectogram {
//     unsafe fn new() -> (Rc<Self>, Ptr<QGraphicsView>) {
//         let scene = QGraphicsScene::new();

//         // scene.set_scene_rect_4a(-100, , w, h);
//         // let pen = QPen::new();
//         // pen.set_color(&QColor::from_3_int(0, 0, 0));
//         // scene.add_line_5a(0.0, 0.0, 20.0, 20.0, &pen);
//         // let text = scene.add_text_1a(&qs("Ass"));
//         // QGraphicsTextItem::static_upcast(text.as_ptr());
//         // text.as_ptr().static_upcast::<QGraphicsItem>().set_pos_2a(0.0, 0.0);
//         // // scene.add_line_4a(20.0, 50.0, 50.0, 200.0);
//         // // scene.add_rect_4a(100.0, 50.0, 60.0, 80.0);
//         // scene.add_ellipse_4a(200.0, 100.0, 80.0, 80.0);

//         let green = QBrush::from_global_color(GlobalColor::Green);
//         let blue = QBrush::from_global_color(GlobalColor::Blue);
//         let outline = QPen::from_q_color(&QColor::from_3_int(0, 0, 0));
//         outline.set_width(2);

//         let rectangle = scene.add_rect_6a(100.0, 0.0, 80.0, 100.0, &outline, &blue);

//         // addEllipse(x,y,w,h,pen,brush)
//         let ellipse = scene.add_ellipse_6a(0.0, 0.0, 20.0, 20.0, &outline, &green);

//         let text = scene.add_text_2a(&qs("Ass"), &QFont::from_q_string_int(&qs("Arial"), 20) );
//         // movable text
//         // text.as_ptr().static_upcast::<QGraphicsItem>().set_flag_1a(GraphicsItemFlag::ItemIsMovable);

//         let view = QGraphicsView::from_q_graphics_scene(&scene);
//         // view.set_focus_policy(FocusPolicy::NoFocus);

//         // view.set_minimum_size_2a(400, 300);
//         view.set_scene_rect_1a(&scene.items_bounding_rect());
//         scene.set_scene_rect_4a(0.0, 0.0, 400.0, 400.0);
//         // let g = view.map_to_scene_q_rect(&view.rect()).bounding_rect();
//         // scene.set_scene_rect_1a(&g);
//         // view.set_scene_rect_1a(&g);
//         // view.ensure_visible_q_rect_f(&g);
//         // view.set

//         std::mem::forget(green);
//         std::mem::forget(blue);
//         std::mem::forget(outline);
//         let ptr = view.as_ptr();
//         let s = Rc::new(Self{
//             scene,
//             view
//         });

//         (s, ptr)
//     }
// }

struct App {
    root: QBox<QWidget>,
    device: Rc<DeviceGroup>,
    v_layout: QBox<QVBoxLayout>,
    spectogram: ShittySpectogram,
    graph: ShittySpectogram,
}

impl App {
    unsafe fn new() -> Self {
        let root = QWidget::new_0a();
        let h_layout = QHBoxLayout::new_1a(&root);

        let v_layout = QVBoxLayout::new_0a();

        h_layout.add_layout_1a(&v_layout);

        let (device, group) = DeviceGroup::new();
        v_layout.add_widget(group);
        v_layout.add_stretch_0a();

        let group2 = QGroupBox::new();
        let layout2 = QGridLayout::new_0a();
        group2.set_layout(&layout2);
        h_layout.add_widget(&group2);

        let spectogram = {
            let (graph, widget) =
                ShittySpectogram::new(400, 300, SAMPLE_WINDOW as u32, 40, 0.0..1.0, -10.0..50.0);

            let layout = QVBoxLayout::new_0a();
            layout.add_widget(widget);

            let group = QGroupBox::new();
            group.set_layout(&layout);
            group.set_flat(true);
            group.set_title(&qs("Spectrum"));

            layout2.add_widget(&group);

            graph
        };

        let graph = {
            let (graph, widget) =
                ShittySpectogram::new(400, 300, SAMPLE_WINDOW as u32, 0, 0.0..0.1, -1.2..1.2);

            let layout = QVBoxLayout::new_0a();
            layout.add_widget(widget);

            let group = QGroupBox::new();
            group.set_layout(&layout);
            group.set_flat(true);
            group.set_title(&qs("Raw"));

            layout2.add_widget(&group);

            graph
        };

        h_layout.add_stretch_0a();

        root.show();

        Self {
            root,
            device,
            spectogram,
            graph,
            v_layout,
        }
    }
}

fn main() {
    QApplication::init(|_| unsafe {
        let mut app = App::new();
        let timer = QTimer::new_0a();
        // let rand = Rc::new(RefCell::new(0.0));
        let mut a: f64 = 1.0;
        timer.set_interval(50);
        let start = std::time::Instant::now();
        timer.timeout().connect(&SlotNoArgs::new(&timer, move || {
            // let mut hasher = DefaultHasher::new();
            // std::time::Instant::now().elapsed().hash(&mut hasher);
            // let seed = hasher.finish() as f64;
            // let offset = start.elapsed().as_secs_f64();

            unsafe fn color(f: f64) -> CppBox<QColor> {
                QColor::from_rgb_f_3a(f, 0.0, 0.0)
            };

            // let data = (0..64)
            //     .into_iter()
            //     .map(|i| {
            //         a += 0.001;

            //         if !a.is_finite() {
            //             a = 1.0;
            //         }

            //         // (i as f64) / 256.0
            //         ((i as f64) * seed % 5.0).sin()
            //         // (offset * 3.0 + (i as f64) / 10.0).sin()
            //     })
            //     .collect::<Vec<_>>();
            let mut data = Box::new([Complex64::new(0.0, 0.0); SAMPLE_WINDOW]);
            app.device.radio.borrow_mut().get_data(&mut *data);

            let iter = data.iter().map(|c| c.re);
            app.graph.add_new_data(iter, color);

            use rustfft::Fft;
            let fft = rustfft::FftPlanner::new().plan_fft_forward(SAMPLE_WINDOW);
            fft.process(&mut *data);

            let iter = data.iter().map(|c| (c.re * c.re + c.im * c.im).sqrt().ln());

            app.spectogram.add_new_data(iter, color);
            // spectogram.add_new_data(data, color);
        }));
        timer.start_0a();
        // QApplication::set_style_q_string(&qs("windows"));
        QApplication::exec()
    })
}
