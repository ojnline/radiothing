use std::rc::Rc;

use qt_widgets::{
    cpp_core::Ptr,
    q_size_policy::Policy,
    qt_core::{qs, QBox},
    QCheckBox, QFormLayout, QGroupBox, QLineEdit,
};

use crate::app_settings::AppSettings;
use crate::device::{DeviceManager, GuiBoundEvent};

pub enum Mode {
    Baudot {
        baudrate: f32,
        stop_bits: f32,
        decimation: usize,
        freq_shift: f32,
    },
}

#[allow(unused)]
pub struct HabhubGroup {
    group: QBox<QGroupBox>,

    device: Rc<DeviceManager>,
    settings: Rc<AppSettings>,
}

impl HabhubGroup {
    pub unsafe fn new(
        device: Rc<DeviceManager>,
        settings: Rc<AppSettings>,
    ) -> (Rc<Self>, Ptr<QGroupBox>) {
        let group = QGroupBox::new();
        group.set_size_policy_2a(Policy::Fixed, Policy::Fixed);

        group.set_title(&qs("Habhub"));

        let form = QFormLayout::new_0a();
        group.set_layout(&form);

        let listener_callsign = QLineEdit::new();
        form.add_row_q_string_q_widget(&qs("Listener callsign"), &listener_callsign);

        let habhub_send = QCheckBox::new();

        form.add_row_q_string_q_widget(&qs("Habhub send"), &habhub_send);

        let ptr = group.as_ptr();
        let s = Rc::new(Self {
            group,
            device,
            settings,
        });

        (s, ptr)
    }
    unsafe fn init(self: &Rc<Self>) {}
    pub unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
        match event.as_ref().unwrap() {
            _ => (),
        };
    }
    pub unsafe fn populate_settings(&self, settings: &mut AppSettings) {}
}
