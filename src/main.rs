// #![windows_subsystem = "linux"]
#![allow(unused)]

use cpp_core::{Ptr, Ref, StaticUpcast};
use qt_core::{ContextMenuPolicy, QBox, QObject, QPoint, SlotNoArgs, SlotOfInt, qs, slot};
use qt_widgets::{
    self as qt,
    QAction, QApplication, QLineEdit, QMenu, QMessageBox, QPushButton, QTableWidget,
    QTableWidgetItem, QVBoxLayout, QWidget, SlotOfQPoint, SlotOfQTableWidgetItemQTableWidgetItem,
};
use std::{cell::{Cell, RefCell}, rc::Rc};
use soapysdr::{Device, Args};

struct Backend {
    device: Option<Device>,
    args: Vec<Args>
}

impl Backend {
    fn new() -> Self {
        Self  {
            device: None,
            args: Vec::new()
        }
    }
}

struct App {
    widget: QBox<QWidget>,
    combo_box: QBox<qt::QComboBox>,
    entry: QBox<qt::QLineEdit>,
    row_widget: QBox<QWidget>,
    b1: QBox<qt::QPushButton>,
    b2: QBox<qt::QPushButton>,
    b3: QBox<qt::QPushButton>,
    // table: QBox<QTableWidget>,
    // button: QBox<QPushButton>,
    backend: RefCell<Backend>
    
}

impl StaticUpcast<QObject> for App {
    unsafe fn static_upcast(ptr: Ptr<Self>) -> Ptr<QObject> {
        ptr.widget.as_ptr().static_upcast()
    }
}

impl App {
    fn refresh_devices(self: &Rc<Self>) {println!("Refresh")}
    fn start_b(self: &Rc<Self>) {println!("Start")}
    fn stop_b(self: &Rc<Self>) {println!("Stop")}
    fn new() -> Rc<App> {
        unsafe {
            let widget = QWidget::new_0a();
            let layout = QVBoxLayout::new_1a(&widget);

            let entry = qt::QLineEdit::new();
            entry.set_placeholder_text(&qs("Device filter"));
            // entry.set_maximum_width(100);

            let combo_box = qt::QComboBox::new_0a();

            let row_widget = QWidget::new_0a();
            let row = qt::QHBoxLayout::new_1a(&row_widget);
            let b1 = QPushButton::from_q_string(&qs("Refresh"));
            let b2 = QPushButton::from_q_string(&qs("Start"));
            let b3 = QPushButton::from_q_string(&qs("Stop"));
            b2.set_enabled(false);
            b3.set_enabled(false);
            // row_widget.set_size_policy_2a(qt::q_size_policy::Policy::Minimum, qt::q_size_policy::Policy::Fixed);
            // // b1.set_size_policy_2a(qt::q_size_policy::Policy::Fixed, qt::q_size_policy::Policy::Fixed);
            // // b2.set_size_policy_2a(qt::q_size_policy::Policy::Fixed, qt::q_size_policy::Policy::Fixed);
            // // b3.set_size_policy_2a(qt::q_size_policy::Policy::Fixed, qt::q_size_policy::Policy::Fixed);
            // b1.set_maximum_width(70);
            // b2.set_maximum_width(50);
            // b3.set_maximum_width(50);
            row.add_widget(&b1);
            row.add_widget(&b2);
            row.add_widget(&b3);
            row.add_stretch_0a();

            layout.add_widget(&entry);
            layout.add_widget(&combo_box);
            layout.add_widget(&row_widget);
            
            layout.add_spacing(50);

            layout.add_stretch_0a();
            widget.show();

            // let b1_ = b1.as_ptr();
            // let b2_ = b2.as_ptr();
            // let b3_ = b3.as_ptr();

            let s = Rc::new(Self {
                widget,
                combo_box,
                entry,
                row_widget,
                b1,
                b2,
                b3,
                backend: RefCell::new(Backend::new())
            });

            // let s_ = s.clone();
            // b1_.clicked().connect(&SlotNoArgs::new(b1_, move || {
            //     s_.borrow_mut().refresh_devices();
            // }));
                                                        
            // let s_ = s.clone();
            // b2_.clicked().connect(&SlotNoArgs::new(b2_, move || {
            //     s_.borrow_mut().start_b();
            // }));
                        
            // let s_ = s.clone();
            // b3_.clicked().connect(&SlotNoArgs::new(b3_, move || {
            //     s_.borrow_mut().stop_b();
            // }));

            s
        }
        
    }

    unsafe fn init(self: Rc<Self>) {
        let Self {
            widget,
            combo_box,
            b1,
            b2,
            b3,
            ..
        } = &*self;

        let s = self.clone();
        combo_box.current_index_changed().connect(&SlotNoArgs::new(widget, move || {
            let enabled = (s.combo_box.count() != 0) && s.backend.borrow().device.is_none();
            s.b2.set_enabled(enabled)
        }));

        let s = self.clone();
        b1.clicked().connect(&SlotNoArgs::new(widget, move || {
            let filter = s.entry.text();

            let args = soapysdr::enumerate(filter.to_std_string().as_str()).unwrap();

            s.combo_box.clear();

            for arg in &args {
                s.combo_box.add_item_q_string(&qs(arg.get("label").unwrap_or("")));
            }

            s.backend.borrow_mut().args = args;
        }));

        let s = self.clone();
        b2.clicked().connect(&SlotNoArgs::new(widget, move || {
            
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

                let backend = &mut s.backend.borrow_mut(); 

                drop(backend.device.take()); // drop the previous device first so that the connection gets closed

                let arg_clone = clone_args(&backend.args[s.combo_box.current_index() as usize]);
                let device = Device::new(arg_clone).unwrap();

                backend.device = Some(device);
            }
        }));

        let s = self.clone();
        b3.clicked().connect(&SlotNoArgs::new(widget, move || {
            s.b2.set_enabled(true);
            s.b3.set_enabled(false);

            let device_ref = &mut s.backend.borrow_mut().device; 

            drop(device_ref.take());
        }));

        b1.click();
    }
}

fn main() {
    QApplication::init(|_| unsafe {
        let app = App::new();
        app.init();
        QApplication::exec()
    })
}