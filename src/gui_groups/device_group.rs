use std::borrow::Borrow;
use std::rc::Rc;

use qt_charts::qt_core::{SlotNoArgs, SlotOfBool};
use qt_widgets::cpp_core::Ptr;
use qt_widgets::q_layout::SizeConstraint;
use qt_widgets::qt_core::{qs, QBox};
use qt_widgets::{
    QCheckBox, QComboBox, QGroupBox, QHBoxLayout, QLineEdit, QPushButton, QVBoxLayout, QWidget,
};

use crate::app_settings::AppSettings;
use crate::worker::worker::{DeviceBoundCommand, GuiBoundEvent};
use crate::worker::worker_manager::DeviceManager;

use super::handle_send_result;

#[allow(unused)]
pub struct DeviceGroup {
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

const DEVICES_REFRESH_INTERVAL_MS: u64 = 1000;

impl DeviceGroup {
    pub unsafe fn new(
        device: Rc<DeviceManager>,
        settings: Rc<AppSettings>,
    ) -> (Rc<DeviceGroup>, Ptr<QGroupBox>) {
        let layout = QVBoxLayout::new_0a();
        let group = QGroupBox::new();
        group.set_title(&qs("Device"));
        group.set_layout(&layout);

        let auto_select = QCheckBox::new();
        auto_select.set_text(&qs("Auto select device"));
        auto_select.set_checked(settings.auto_device);
        layout.add_widget(&auto_select);

        let entry = QLineEdit::new();
        entry.set_placeholder_text(&qs("Device filter"));
        entry.set_text(&qs(&settings.device_filter));

        let combo_box = QComboBox::new_0a();

        let row_widget = QWidget::new_0a();
        let row_layout = QHBoxLayout::new_1a(&row_widget);
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
                if checked {
                    s.b1.click();
                }
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

            s.b2.set_enabled(true);
            s.b3.set_enabled(false);
        }));
    }
    pub unsafe fn handle_event(&self, event: &mut Option<GuiBoundEvent>) {
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

                if self.device.get_device_valid() {
                    return;
                }

                self.b2.set_enabled(!list.is_empty());

                if self.auto_select.is_checked() {
                    if list.is_empty() && !self.device.get_refreshing_devices() {
                        let filter = self.filter.text().to_std_string();
                        self.device.schedule_command(
                            DeviceBoundCommand::RefreshDevices { args: filter },
                            DEVICES_REFRESH_INTERVAL_MS,
                        );
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
    pub unsafe fn populate_settings(&self, settings: &mut AppSettings) {
        let AppSettings {
            auto_device: auto_select_device,
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
