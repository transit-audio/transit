//! Control-side transport admission, port naming and cancellation cutoffs.
use super::*;
use std::collections::BTreeMap;

/// One port opening's ordered input, delivered only to control.
#[derive(Clone, Debug)]
pub struct ButtonEvent {
    pub device: String,
    pub generation: u64,
    pub raw: transit_midi_io::TransportEnvelope,
}

#[derive(Default)]
pub(super) struct Buttons {
    active: bool,
    profiles: transit_model::TransportProfiles,
    sending: Vec<MidiOutPort>,
    cutoffs: BTreeMap<u64, u64>,
    sent: Vec<(u64, bool, Vec<transit_model::MidiSource>)>,
}

impl Buttons {
    fn admits(&self, key: &PortKey, generation: u64, seq: u64) -> bool {
        self.enabled(key) && seq >= self.cutoffs.get(&generation).copied().unwrap_or(0)
    }
    pub(super) fn enabled(&self, key: &PortKey) -> bool {
        self.active
            && self.profiles.get(&key.name).is_none_or(|p| p.transport_buttons)
            && !self.sending.iter().any(|out| transit_model::same_midi_device(key.backend.as_str(), &key.name, &key.id, out))
    }
}

impl Native {
    pub(crate) fn set_transport_learn(&self, on: bool) {
        self.worker.set_transport_learn(on);
    }
    pub(crate) fn retain_buttons(&self, events: &mut Vec<ButtonEvent>) {
        events.retain(|event| {
            self.inputs.iter().any(|input| {
                !input.closed
                    && !input.lost
                    && input.key.name == event.device
                    && input.attached.as_ref().is_some_and(|(id, _)| *id == event.generation && self.buttons.admits(&input.key, *id, event.raw.seq))
            })
        });
    }

    pub(crate) fn configure_buttons(&mut self, profiles: transit_model::TransportProfiles, sending: Vec<MidiOutPort>) {
        if self.buttons.active && self.buttons.profiles == profiles && self.buttons.sending == sending {
            return;
        }
        // Cut off even delayed publications, before changing the guard or switch.
        for input in &self.inputs {
            if let Some((id, boundary)) = &input.attached {
                self.buttons.cutoffs.insert(*id, boundary.cutoff());
            }
        }
        self.buttons.profiles = profiles;
        self.buttons.active = true;
        self.buttons.sending = sending;
        self.sync_buttons();
    }

    pub(crate) fn sync_buttons(&mut self) {
        let wanted: Vec<_> = self
            .inputs
            .iter()
            .filter_map(|input| {
                let (id, _) = input.attached.as_ref()?;
                let sources = if self.buttons.enabled(&input.key) {
                    self.buttons
                        .profiles
                        .get(&input.key.name)
                        .map(|p| {
                            p.mappings
                                .iter()
                                .filter(|m| {
                                    matches!(m.target.control, transit_model::Control::Transport(_))
                                        && m.source.port == input.key.name
                                        && m.validate().is_ok()
                                })
                                .map(|m| m.source.clone())
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                Some((*id, self.buttons.enabled(&input.key), sources))
            })
            .collect();
        if wanted != self.buttons.sent && self.worker.set_buttons(wanted.clone()).is_ok() {
            for input in &self.inputs {
                if let Some((id, boundary)) = &input.attached {
                    let sources: Vec<_> = wanted
                        .iter()
                        .find(|(port, _, _)| port == id)
                        .into_iter()
                        .flat_map(|(_, _, sources)| sources)
                        .map(|s| transit_midi_io::TransportSource {
                            channel: s.channel,
                            note: s.kind == transit_model::SourceKind::Note,
                            number: s.number,
                        })
                        .collect();
                    boundary.set_transport_enabled(self.buttons.enabled(&input.key));
                    boundary.set_transport_sources(&sources);
                    if self.buttons.sent.iter().any(|(port, _, _)| port == id) {
                        self.buttons.cutoffs.insert(*id, boundary.cutoff());
                    }
                }
            }
            self.buttons.sent = wanted;
        }
    }

    pub(crate) fn cancel_buttons(&mut self) {
        for input in &self.inputs {
            if let Some((id, boundary)) = &input.attached {
                self.buttons.cutoffs.insert(*id, boundary.cutoff());
            }
        }
    }

    pub(crate) fn take_buttons(&mut self) -> (bool, Vec<ButtonEvent>) {
        let (mut lost, events) = self.worker.take_transport();
        for input in &self.inputs {
            if let Some((_, boundary)) = &input.attached {
                let input_lost = boundary.take_transport_loss();
                lost |= input_lost && self.buttons.enabled(&input.key);
            }
        }
        if lost {
            self.cancel_buttons();
            return (true, Vec::new());
        }
        let mut out = Vec::new();
        for (id, raw) in events {
            let Some(input) = self.inputs.iter().find(|i| !i.closed && !i.lost && i.attached.as_ref().is_some_and(|(port, _)| *port == id)) else {
                continue;
            };
            if !self.buttons.admits(&input.key, id, raw.seq) {
                continue;
            }
            out.push(ButtonEvent { device: input.key.name.clone(), generation: id, raw });
        }
        // Preserve each opening's shared sequence; no ordering between devices is promised.
        out.sort_by_key(|event| (event.generation, event.raw.seq));
        (false, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_buttons_are_suspended_by_effective_same_device_transport_send() {
        let input = PortKey { backend: transit_midi_io::BackendId::Fake, name: "Controller".into(), ordinal: 0, id: String::new() };
        let mut buttons = Buttons { active: true, ..Default::default() };
        assert!(buttons.enabled(&input));
        buttons.sending.push(MidiOutPort { backend: "fake".into(), name: "Controller".into(), ..Default::default() });
        assert!(!buttons.enabled(&input), "even a stopped device cannot echo Start into transit");
        buttons.sending.clear();
        assert!(buttons.enabled(&input), "clock-only sending is not an effective Transport output");
        buttons.profiles.entry(input.name.clone()).or_default().transport_buttons = false;
        assert!(!buttons.enabled(&input));
    }

    #[test]
    fn guard_and_switch_cutoffs_refuse_delayed_offers_but_allow_fresh_openings() {
        let input = PortKey { backend: transit_midi_io::BackendId::Fake, name: "Controller".into(), ordinal: 0, id: String::new() };
        let mut buttons = Buttons { active: true, ..Default::default() };
        assert!(buttons.admits(&input, 1, 9));
        buttons.sending.push(MidiOutPort { backend: "fake".into(), name: "Controller".into(), ..Default::default() });
        buttons.cutoffs.insert(1, 10);
        assert!(!buttons.admits(&input, 1, 10));
        buttons.sending.clear();
        assert!(!buttons.admits(&input, 1, 9), "a delayed echo cannot escape after send is disabled");
        assert!(buttons.admits(&input, 1, 10));
        buttons.profiles.entry(input.name.clone()).or_default().transport_buttons = false;
        assert!(!buttons.admits(&input, 1, 11));
        buttons.cutoffs.insert(1, 12);
        buttons.profiles.entry(input.name.clone()).or_default().transport_buttons = true;
        assert!(!buttons.admits(&input, 1, 11));
        assert!(buttons.admits(&input, 1, 12));
        assert!(buttons.admits(&input, 2, 0), "a reconnected opening has its own sequence domain");
    }
}
