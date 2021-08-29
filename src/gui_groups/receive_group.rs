use std::{
    borrow::Borrow,
    cell::{Cell, RefCell},
    rc::Rc,
};

use qt_charts::qt_core::{qs, CheckState, QBox, SlotNoArgs, SlotOfInt};
use qt_widgets::{
    cpp_core::Ptr, q_form_layout::FieldGrowthPolicy, QCheckBox, QComboBox, QDoubleSpinBox,
    QFormLayout, QGroupBox, QPushButton, QVBoxLayout,
};

use crate::{
    app_settings::AppSettings,
    device::{DeviceBoundCommand, DeviceManager, GuiBoundEvent, ReceiverState, ValueRanges},
    gui_groups::handle_send_result,
};

enum Samplerate {
    Ranges(QBox<QDoubleSpinBox>),
    Values(QBox<QComboBox>),
}

pub struct ReceiveGroup {
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
    current_values: Cell<Option<ReceiverState>>,
    device: Rc<DeviceManager>,
    settings: Rc<AppSettings>,
}

impl ReceiveGroup {
    pub unsafe fn new(
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

        let form = QFormLayout::new_0a();
        // form.set_label_alignment(AlignmentFlag::AlignLeft.into());
        form.set_field_growth_policy(FieldGrowthPolicy::AllNonFixedFieldsGrow);
        v.add_layout_1a(&form);
        group.set_layout(&v);

        let frequency = QDoubleSpinBox::new_0a();
        frequency.set_suffix(&qs(" MHz"));
        // start with practically unlimited range so that the following set_value isn't accidentally rounded
        // the correct range is later set when the actual Device is created and queried for ranges
        // TODO maybe leave the range uncapped this way and rely only on the clamp_value function
        frequency.set_range(0.0, 10000.0);
        frequency.set_value(settings.frequency);
        form.add_row_q_string_q_widget(&qs("Frequency"), &frequency);

        let samplerate = QDoubleSpinBox::new_0a();
        samplerate.set_suffix(&qs(" MSps"));
        samplerate.set_range(0.0, 10000.0);
        samplerate.set_value(settings.samplerate);
        form.add_row_q_string_q_widget(&qs("Samplerate"), &samplerate);

        let gain = QDoubleSpinBox::new_0a();
        gain.set_suffix(&qs(" dB"));
        gain.set_range(0.0, 10000.0);
        gain.set_value(settings.gain);
        form.add_row_q_string_q_widget(&qs("Gain"), &gain);

        let automatic_gain = QCheckBox::new();
        automatic_gain.set_checked(settings.automatic_gain);
        form.add_row_q_string_q_widget(&qs("Automatic gain"), &automatic_gain);

        let automatic_dc_offset = QCheckBox::new();
        automatic_dc_offset.set_checked(settings.automatic_dc_offset);
        form.add_row_q_string_q_widget(&qs("Automatic DC offset"), &automatic_dc_offset);

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
            form_layout: form,

            value_ranges: RefCell::new(None),
            current_values: Cell::new(None),
            device,
            settings,
        });

        s.group.set_enabled(false);
        s.init();

        (s, ptr)
    }
    unsafe fn update_receiver_configuration(&self, force: bool) {
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

        // nothing changed, this is possible because this function is called on editing_finished signal from qt
        // this signal gets sent if for example you click into the value field of a spinbox and then focus something else
        // without actually changing anything

        // Cell is repr(transparent) so it is valid to compare it with a value of the inner Type
        // Cell<Option<T>> -> *const Option<T> -> &Option<T> -> Option<&T>
        if Some(&state) == (&*self.current_values.as_ptr()).as_ref() && force == false {
            return;
        } else {
            self.current_values.set(Some(state.clone()));
        }

        handle_send_result(
            self.device
                .send_command(DeviceBoundCommand::SetReceiver(state)),
        );
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
                    s.update_receiver_configuration(false);
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
                            s.update_receiver_configuration(false);
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
                s.update_receiver_configuration(false);
            }
        });

        automatic_gain.state_changed().connect(&checkbox_slot);
        automatic_dc_offset.state_changed().connect(&checkbox_slot);

        let s = self.clone();
        apply_btn
            .clicked()
            .connect(&SlotNoArgs::new(group, move || {
                s.update_receiver_configuration(false);
            }));
    }
    pub unsafe fn handle_event(self: &Rc<Self>, event: &mut Option<GuiBoundEvent>) {
        match event.as_ref().unwrap() {
            // it is incredibly ugly to be doing this replacement here and everytime the device changes
            // but this wouldn't be a gui project without bad code
            GuiBoundEvent::DeviceCreated { channels_info } => {
                let mut ranges = channels_info[0].ranges.clone();

                // everything is in megahertz or megasamples/second
                const MIL: f64 = 1_000_000.0;

                // remove the samplerate widget, it will be replaced later
                self.form_layout.remove_row_int(1);

                // the device supports only discreet samplerates
                if ranges.samplerate[0].minimum == ranges.samplerate[0].maximum {
                    let combox = QComboBox::new_0a();

                    // the index of the samplerate loaded from AppSettings
                    // if it is not found, fall back to 0
                    let mut set_samplerate_index = 0;

                    for (i, range) in ranges.samplerate.iter().enumerate() {
                        let label = format!("{} MSps", range.minimum / MIL);
                        combox.add_item_q_string(&qs(label));

                        if range.minimum / MIL == self.settings.samplerate {
                            // index is found
                            set_samplerate_index = i;
                        }
                    }

                    combox.set_current_index(set_samplerate_index as i32);

                    let s = self.clone();
                    combox
                        .current_index_changed()
                        .connect(&SlotNoArgs::new(&combox, move || {
                            if s.automatic_update.is_checked() {
                                s.update_receiver_configuration(false);
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

                    spinbox.set_value(self.settings.samplerate);

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
                                s.update_receiver_configuration(false);
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

                log::debug!("Receiver value ranges: {:#?}", ranges);

                self.value_ranges.replace(Some(ranges));

                // make sure that the device has "some" receive stream configured, if the values lead to a timeout error or similar, the sample retrieval will be paused
                self.update_receiver_configuration(true);

                self.group.set_enabled(true);
            }
            GuiBoundEvent::DeviceDestroyed => {
                self.group.set_enabled(false);
            }
            _ => (),
        }
    }
    pub unsafe fn populate_settings(&self, settings: &mut AppSettings) {
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
