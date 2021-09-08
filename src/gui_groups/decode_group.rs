use std::borrow::Borrow;
use std::cell::RefCell;
use std::rc::Rc;

use crate::app_settings::AppSettings;
use crate::decoder::Decoder;
use crate::worker::worker::{DeviceBoundCommand, GuiBoundEvent};
use crate::worker::worker_manager::DeviceManager;

use qt_charts::qt_core::{SlotNoArgs, SlotOfInt};
use qt_widgets::{
    cpp_core::Ptr,
    q_form_layout::FieldGrowthPolicy,
    qt_core::{qs, QBox},
    QComboBox, QDoubleSpinBox, QFormLayout, QFrame, QGroupBox, QSpinBox,
};
use qt_widgets::{QPushButton, QVBoxLayout, QWidget};

use super::handle_send_result;

const MODES: &[&str] = &["None", "Baudot"];

enum ModeConfig {
    None,
    Baudot {
        form: QBox<QFormLayout>,
        // frame: QBox<QFrame>,
        baudrate: QBox<QDoubleSpinBox>,
        stop_bits: QBox<QDoubleSpinBox>,
        freq_shift: QBox<QDoubleSpinBox>,
    },
}

impl ModeConfig {
    unsafe fn new_from_index(index: usize, settings: &AppSettings) -> (Self, QBox<QWidget>) {
        match index {
            0 => (Self::None, QWidget::new_0a()),
            1 => {
                let widget = QWidget::new_0a();
                let form = QFormLayout::new_0a();
                widget.set_layout(&form);

                // let frame = QFrame::new_0a();
                // frame.set_frame_shape(qt_widgets::q_frame::Shape::HLine);
                // frame.set_frame_shadow(qt_widgets::q_frame::Shadow::Sunken);
                // form.add_row_q_widget(&frame);
                // form.add_row_q_string_q_widget(&qs("AAAA"), &frame);

                let baudrate = QDoubleSpinBox::new_0a();
                baudrate.set_suffix(&qs(" Bd"));
                baudrate.set_range(0.0, 1000.0);
                // settings.baudratebaudrate.set_value()
                form.add_row_q_string_q_widget(&qs("Baudrate"), &baudrate);

                let stop_bits = QDoubleSpinBox::new_0a();
                stop_bits.set_suffix(&qs(" Bits"));
                form.add_row_q_string_q_widget(&qs("Stop bits"), &stop_bits);

                let freq_shift = QDoubleSpinBox::new_0a();
                freq_shift.set_suffix(&qs(" Hz"));
                freq_shift.set_range(0.0, 1000.0);
                form.add_row_q_string_q_widget(&qs("Frequency shift"), &freq_shift);

                let s = Self::Baudot {
                    form,
                    // frame,
                    baudrate,
                    stop_bits,
                    freq_shift,
                };

                (s, widget)
            }
            _ => panic!("Invalid index."),
        }
    }
    unsafe fn get_decoder(&self) -> Option<Decoder> {
        match self {
            ModeConfig::None => None,
            ModeConfig::Baudot {
                baudrate,
                stop_bits,
                freq_shift,
                ..
            } => Some(Decoder::new_baudot(
                baudrate.value() as f32,
                stop_bits.value() as f32,
                freq_shift.value() as f32,
            )),
        }
    }
    fn populate_settings(&self, settings: &mut AppSettings) {

    }
}

#[allow(unused)]
pub struct DecodeGroup {
    group: QBox<QGroupBox>,

    v_layout: QBox<QVBoxLayout>,
    mode_select: QBox<QComboBox>,
    mode_widget: RefCell<QBox<QWidget>>,
    mode_config: RefCell<ModeConfig>,
    apply_btn: QBox<QPushButton>,

    device: Rc<DeviceManager>,
    settings: Rc<AppSettings>,
}

impl DecodeGroup {
    pub unsafe fn new(
        device: Rc<DeviceManager>,
        settings: Rc<AppSettings>,
    ) -> (Rc<Self>, Ptr<QGroupBox>) {
        let group = QGroupBox::new();
        group.set_title(&qs("Decode"));

        let v_layout = QVBoxLayout::new_0a();
        group.set_layout(&v_layout);

        let form = QFormLayout::new_0a();
        form.set_field_growth_policy(FieldGrowthPolicy::AllNonFixedFieldsGrow);
        v_layout.add_layout_1a(&form);

        let mode_select = QComboBox::new_0a();
        for string in MODES {
            mode_select.add_item_q_string(&qs(string));
        }

        form.add_row_q_string_q_widget(&qs("Mode"), &mode_select);

        let index = MODES.iter().position(|name| *name == settings.decoder.as_str()).unwrap_or(0);

        let (mode_config, mode_widget) = ModeConfig::new_from_index(index);

        v_layout.add_widget(&mode_widget);

        let apply = QPushButton::from_q_string(&qs("Apply"));

        v_layout.add_widget(&apply);

        let ptr = group.as_ptr();
        let s = Rc::new(Self {
            group,
            device,
            settings,
            v_layout,
            mode_select,
            mode_config: RefCell::new(mode_config),
            mode_widget: RefCell::new(mode_widget),
            apply_btn: apply,
        });
        
        s.apply_btn.set_enabled(false);

        s.init();

        (s, ptr)
    }
    pub unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
        match event.as_ref().unwrap() {
            GuiBoundEvent::DeviceCreated { .. } => {
                if let Some(decoder) = self.mode_config.borrow().get_decoder() {
                    let command = DeviceBoundCommand::SetDecoder { decoder };

                    handle_send_result(self.device.send_command(command));

                }
                self.apply_btn.set_enabled(true);
            }
            GuiBoundEvent::DeviceDestroyed | GuiBoundEvent::WorkerReset => {
                self.apply_btn.set_enabled(false);
            }
            _ => {}
        };
    }
    unsafe fn init(self: &Rc<Self>) {
        let Self {
            group,
            apply_btn: apply,
            mode_select,
            ..
        } = &*self.borrow();

        let s = self.clone();
        mode_select
            .current_index_changed()
            .connect(&SlotOfInt::new(group, move |i| {
                let (mode_config, mode_widget) = ModeConfig::new_from_index(i as usize);
                s.v_layout
                    .replace_widget_2a(&*s.mode_widget.borrow(), &mode_widget);
                s.mode_widget.replace(mode_widget);
                s.mode_config.replace(mode_config);
            }));

        let s = self.clone();
        apply.clicked().connect(&SlotNoArgs::new(group, move || {
            if let Some(decoder) = s.mode_config.borrow().get_decoder() {
                let command = DeviceBoundCommand::SetDecoder { decoder };

                handle_send_result(s.device.send_command(command));
            }
        }));
    }
    pub unsafe fn populate_settings(&self, settings: &mut AppSettings) {
        self.mode_config.borrow().populate_settings(settings);
    }
}
