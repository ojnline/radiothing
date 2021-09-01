use std::rc::Rc;

use crate::app_settings::AppSettings;
use crate::decoder::BaudotDecoder;
use crate::worker::worker::{DeviceBoundCommand, GuiBoundEvent};
use crate::worker::worker_manager::{DeviceManager};

use qt_widgets::{
    cpp_core::Ptr,
    q_form_layout::FieldGrowthPolicy,
    qt_core::{qs, QBox},
    QComboBox, QDoubleSpinBox, QFormLayout, QFrame, QGroupBox, QSpinBox,
};

use super::handle_send_result;

#[allow(unused)]
pub struct DecodeGroup {
    group: QBox<QGroupBox>,

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

        let form = QFormLayout::new_0a();
        // form.set_label_alignment(AlignmentFlag::AlignLeft.into());
        form.set_field_growth_policy(FieldGrowthPolicy::AllNonFixedFieldsGrow);

        group.set_layout(&form);
        {
            let decimation = QSpinBox::new_0a();
            form.add_row_q_string_q_widget(&qs("Decimation"), &decimation);
        }

        {
            let mode = QComboBox::new_0a();
            mode.add_item_q_string(&qs("Baudot"));

            form.add_row_q_string_q_widget(&qs("Mode"), &mode);
        }

        let frame = QFrame::new_0a();
        frame.set_frame_shape(qt_widgets::q_frame::Shape::HLine);
        frame.set_frame_shadow(qt_widgets::q_frame::Shadow::Sunken);
        form.add_row_q_widget(&frame);

        let baudrate = QDoubleSpinBox::new_0a();
        baudrate.set_suffix(&qs(" Bd"));
        form.add_row_q_string_q_widget(&qs("Baudrate"), &baudrate);

        let stop_bits = QDoubleSpinBox::new_0a();
        stop_bits.set_suffix(&qs(" Bits"));
        form.add_row_q_string_q_widget(&qs("Stop bits"), &stop_bits);

        let freq_shift = QDoubleSpinBox::new_0a();
        freq_shift.set_suffix(&qs(" Hz"));
        form.add_row_q_string_q_widget(&qs("Frequency shift"), &freq_shift);

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
            GuiBoundEvent::DeviceCreated {..} => {
                let decoder = Box::new(BaudotDecoder::new(50.0, 1.5));

                let command = DeviceBoundCommand::SetDecoder {
                    decoder,
                };

                // handle_send_result(self.device.send_command(command));
            },
            _ => {}
        };
    }
    pub unsafe fn populate_settings(&self, settings: &mut AppSettings) {}
}
