//! MIDI transport buttons: all decoding and command dispatch runs on UI/control.

use crate::StudioApp;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use transit_model::{Control, SourceKind, TransportControl, TransportPrefs};
use transit_playback::ButtonEvent;
use transit_time::Timecode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Play,
    Continue,
    Stop,
    FastForward,
    Rewind,
    Strobe,
    Exit,
    Pause,
    Locate(i64),
    Record,
    Cycle,
    ReturnToZero,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::Continue => "Continue",
            Self::Stop => "Stop",
            Self::FastForward => "Fast Forward",
            Self::Rewind => "Rewind",
            Self::Strobe | Self::Record => "Record",
            Self::Exit => "Record Exit",
            Self::Pause => "Pause",
            Self::Locate(_) => "Locate",
            Self::Cycle => "Cycle",
            Self::ReturnToZero => "Return to Zero",
        }
    }
}

impl From<TransportControl> for Action {
    fn from(t: TransportControl) -> Self {
        match t {
            TransportControl::Play => Self::Play,
            TransportControl::Stop => Self::Stop,
            TransportControl::Record => Self::Record,
            TransportControl::Cycle => Self::Cycle,
            TransportControl::ReturnToZero => Self::ReturnToZero,
            TransportControl::Pause => Self::Pause,
        }
    }
}

#[derive(Default)]
pub(crate) struct State {
    pub retained: i64,
    pub take_locate: Option<i64>,
    pub resume_record_at: Option<i64>,
    pub record_pass_pushes: Option<u64>,
    pub deferred: Vec<transit_engine::TransportRequest>,
    pub waiting: Vec<ButtonEvent>,
    recovering: bool,
    pub dispatching: bool,
    emitted: BTreeMap<String, u64>,
    held: BTreeSet<(u64, String, u8, u8, u8)>,
    prefs: transit_model::TransportProfiles,
    player_generation: u32,
}

impl State {
    fn line(&mut self, device: &str, stamp: u64, text: String) -> Option<String> {
        if self.emitted.get(device).is_some_and(|last| stamp.saturating_sub(*last) < 1_000_000_000) {
            return None;
        }
        self.emitted.insert(device.into(), stamp);
        Some(text)
    }

    fn learned(&mut self, event: &ButtonEvent, prefs: &TransportPrefs) -> Option<Action> {
        let bytes = event.raw.message();
        let [status, number, value] = *bytes else { return None };
        let kind = match status & 0xf0 {
            0x80 | 0x90 => SourceKind::Note,
            0xb0 => SourceKind::Cc,
            _ => return None,
        };
        let mapping = prefs.mappings.iter().find(|m| {
            m.source.port == event.device && m.source.kind == kind && m.source.number == number && m.source.channel.is_none_or(|c| c == status & 15)
        })?;
        let Control::Transport(target) = mapping.target.control else { return None };
        let key = (event.generation, event.device.clone(), status & 15, u8::from(kind == SourceKind::Cc), number);
        let high = if kind == SourceKind::Cc { value >= 64 } else { status & 0xf0 == 0x90 && value > 0 };
        if high {
            self.held.insert(key).then_some(target.into())
        } else {
            self.held.remove(&key);
            None
        }
    }
}

/// Complete length and seven-bit validation happens before reading command fields.
pub(crate) fn decode(bytes: &[u8], id: u8, session: &transit_model::Session) -> Option<Action> {
    match bytes {
        [0xfa] => return Some(Action::Play),
        [0xfb] => return Some(Action::Continue),
        [0xfc] => return Some(Action::Stop),
        [0xf2, lo, hi] if *lo < 128 && *hi < 128 => {
            let ticks = 240 * (i64::from(*lo) + 128 * i64::from(*hi));
            return Some(Action::Locate(session.tempo.tick_to_samples(ticks, session.sample_rate)));
        }
        _ => {}
    }
    if !matches!(bytes.len(), 6 | 13)
        || bytes.first() != Some(&0xf0)
        || bytes.last() != Some(&0xf7)
        || bytes[1..bytes.len() - 1].iter().any(|b| *b >= 128)
        || bytes[1] != 0x7f
        || (bytes[2] != 0x7f && (id > 126 || bytes[2] != id))
        || bytes[3] != 6
    {
        return None;
    }
    if bytes.len() == 6 {
        return match bytes[4] {
            1 => Some(Action::Stop),
            2 | 3 => Some(Action::Play),
            4 => Some(Action::FastForward),
            5 => Some(Action::Rewind),
            6 => Some(Action::Strobe),
            7 => Some(Action::Exit),
            9 => Some(Action::Pause),
            _ => None,
        };
    }
    if bytes[4..7] != [0x44, 6, 1] || bytes[11] > 99 {
        return None;
    }
    let tc = Timecode {
        negative: false,
        hours: i64::from(bytes[7] & 31),
        minutes: i64::from(bytes[8]),
        seconds: i64::from(bytes[9]),
        frames: i64::from(bytes[10]),
    };
    let rate = session.frame_rate;
    if tc.hours > 23
        || tc.minutes > 59
        || tc.seconds > 59
        || tc.frames >= rate.nominal()
        || (rate.is_drop() && tc.minutes % 10 != 0 && tc.seconds == 0 && tc.frames < if rate.nominal() >= 60 { 4 } else { 2 })
    {
        return None;
    }
    Some(Action::Locate(tc.to_samples(rate, session.sample_rate).saturating_sub(session.timecode_start).max(0)))
}

impl StudioApp {
    fn midi_transport_loss(&mut self) {
        if let Some(player) = &self.player {
            player.cancel_midi_buttons();
        }
        self.midi_transport.held.clear();
        self.midi_transport.waiting.clear();
        self.midi_transport.deferred.clear();
        self.midi_transport.take_locate = None;
        self.midi_transport.resume_record_at = None;
        self.midi_transport.recovering = true;
        self.stop_play();
        self.status = "MIDI transport input lost: stopped safely; press Play again".into();
    }

    pub(crate) fn midi_transport_feed(&mut self) {
        let Some(player) = &self.player else { return };
        let generation = player.midi_map_generation();
        if generation != self.midi_transport.player_generation || self.midi_transport.prefs != self.ui.midi_transport {
            self.midi_transport.held.clear();
            self.midi_transport.waiting.clear();
            self.midi_transport.player_generation = generation;
            self.midi_transport.prefs = self.ui.midi_transport.clone();
        }
        // Transport output is introduced by slice 1. Its effective outputs must be supplied here;
        // the Player admission API already applies the same-device guard before dispatch.
        player.configure_midi_buttons(self.ui.midi_transport.clone(), Vec::new());
        let (lost, events) = player.take_midi_buttons();
        player.retain_midi_buttons(&mut self.midi_transport.waiting);
        if lost {
            self.midi_transport_loss();
            return;
        }
        self.dispatch_midi_buttons(events);
    }

    pub(crate) fn dispatch_midi_buttons(&mut self, events: Vec<ButtonEvent>) {
        if self.midi_transport.recovering {
            // Discard every offer up to Stop acknowledgment, including delayed publication.
            if let Some(player) = &self.player {
                player.cancel_midi_buttons();
            }
            self.midi_transport.waiting.clear();
            if !self.midi_stop_pending() && !self.recording_active() {
                self.midi_transport.recovering = false;
            }
            return;
        }
        let mut ordered = std::mem::take(&mut self.midi_transport.waiting);
        ordered.extend(events);
        if ordered.len() > transit_playback::TRANSPORT_CAPACITY {
            self.midi_transport_loss();
            return;
        }
        let mut previous: BTreeMap<(String, u64), (Action, bool)> = BTreeMap::new();
        let mut running = self.is_playing()
            || self.play_pending()
            || self.count_in_state().is_some()
            || self.sim_count_in.is_some()
            || self.midi_transport.take_locate.is_some();
        let mut recording = self.recording_active() || self.midi_transport.take_locate.is_some();
        let mut events = ordered.into_iter();
        while let Some(event) = events.next() {
            if self.midi_stop_pending() {
                self.midi_transport.waiting.push(event);
                self.midi_transport.waiting.extend(events);
                break;
            }
            let prefs = self.ui.midi_transport.get(&event.device).cloned().unwrap_or_default();
            if !prefs.transport_buttons {
                continue;
            }
            let action =
                decode(event.raw.message(), prefs.mmc_device_id, self.engine.session()).or_else(|| self.midi_transport.learned(&event, &prefs));
            let Some(action) = action else { continue };
            let mmc = event.raw.bytes[0] == 0xf0;
            let prev = previous.insert((event.device.clone(), event.generation), (action, mmc));
            if matches!(action, Action::Play | Action::Stop) && prev.is_some_and(|(a, protocol)| a == action && protocol != mmc) {
                continue;
            }
            let (command, params) = match action {
                Action::Play if !running => ("transport.play", json!({"from": self.cursor()})),
                Action::Continue if !running => ("transport.play", json!({"from": self.midi_transport.retained})),
                Action::Stop if running || recording => ("transport.stop", json!({})),
                Action::Pause if running || recording => ("transport.pause", json!({})),
                Action::Strobe if !recording => ("transport.record", json!({})),
                Action::Exit if recording => ("transport.record", json!({})),
                Action::Record => ("transport.record", json!({})),
                Action::FastForward => ("transport.fast_forward", json!({"seconds": 1})),
                Action::Rewind => ("transport.rewind", json!({"seconds": 1})),
                Action::Locate(at)
                    if !self.midi_locate_pending(at) && at != if running { self.engine.transport.position } else { self.midi_transport.retained } =>
                {
                    ("transport.locate", json!({"at": at}))
                }
                Action::ReturnToZero => ("transport.rtz", json!({})),
                Action::Cycle => ("options.loop_playback", json!({"value": !self.engine.session().edit.loop_playback})),
                _ => continue,
            };
            let refused = command == "transport.record" && !recording && !self.engine.session().tracks.iter().any(|t| t.mixer.record_arm);
            self.midi_transport.dispatching = true;
            let result = self.run(command, params);
            self.midi_transport.dispatching = false;
            if result.is_err() {
                continue;
            }
            let was_recording = recording;
            // Apply each accepted request on control immediately. This includes Record's ordinary
            // arming/count-in/pre-roll/interlock path and updates pending state within this batch.
            self.handle_transport(0.0);
            if command == "transport.record" && !was_recording && !refused && !self.recording_active() {
                continue;
            }
            match action {
                Action::Stop | Action::Pause | Action::Exit => {
                    running = false;
                    recording = false;
                }
                Action::Play | Action::Continue => {
                    running = self.is_playing() || self.play_pending();
                }
                Action::Strobe | Action::Record => {
                    recording = self.recording_active();
                    running = self.is_playing() || self.play_pending() || self.sim_count_in.is_some();
                }
                _ => {}
            }
            let text = if refused {
                let text = format!("Record from {}: no armed track", event.device);
                self.record_hint = Some((self.last_frame.unwrap_or_default(), text.clone()));
                text
            } else {
                format!("Transport from {}: {}", event.device, action.label())
            };
            if let Some(line) = self.midi_transport.line(&event.device, event.raw.host_arrival_ns, text) {
                self.status = line;
            }
        }
    }
}

/// What the MIDI section drew, for the window's layout report: (text, key, rect).
pub(crate) type Drawn = Vec<(String, &'static str, egui::Rect)>;

pub(crate) fn settings(app: &mut StudioApp, ui: &mut egui::Ui) -> Drawn {
    let mut drawn = Drawn::new();
    let connected: BTreeSet<String> =
        app.player.as_ref().and_then(|p| p.midi_status().ok()).map(|s| s.inputs.into_iter().map(|p| p.key.name).collect()).unwrap_or_default();
    let names: BTreeSet<String> = connected.iter().cloned().chain(app.ui.midi_transport.keys().cloned()).collect();
    ui.heading("Transport from MIDI devices");
    if names.is_empty() {
        ui.weak("No MIDI inputs available");
    }
    let mut changed = false;
    for name in names {
        let prefs = app.ui.midi_transport.entry(name.clone()).or_default();
        ui.push_id(&name, |ui| {
            ui.label(if connected.contains(&name) { name.clone() } else { format!("{name} (disconnected)") });
            // The row is the field's height before anything is placed, so every element centers on it
            // (a row sized by its last, tallest widget leaves the earlier ones riding high).
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = crate::tokens::chrome::CONTROL_HEIGHT;
                ui.horizontal(|ui| {
                    let b = crate::chrome::checkbox(ui, &mut prefs.transport_buttons, "Transport buttons");
                    crate::help::hover(ui, &b, crate::help::Id::MidiTransportButtons);
                    changed |= b.changed();
                    drawn.push((name.clone(), "midi_transport_buttons", b.rect));
                    let label = ui.label("MMC device ID");
                    drawn.push((name.clone(), "midi_id_label", label.rect));
                    let id = crate::chrome::field(ui, |ui| ui.add(egui::DragValue::new(&mut prefs.mmc_device_id).range(0..=126)));
                    crate::help::hover(ui, &id, crate::help::Id::MidiMmcDeviceId);
                    changed |= id.changed();
                    drawn.push((name.clone(), "midi_id_field", id.rect));
                });
            });
        });
    }
    if changed {
        app.engine.set_midi_transport(app.ui.midi_transport.clone());
        if let Some(player) = &app.player {
            player.cancel_midi_buttons();
        }
        app.midi_transport.held.clear();
    }
    let note = ui.add(egui::Label::new("MMC Record Exit ends recording and stops playback; it does not punch out and continue playing.").wrap());
    drawn.push((String::new(), "midi_note", note.rect));
    ui.separator();
    ui.label("Learn a note or CC button for this machine:");
    for target in TransportControl::ALL {
        let b = ui.button(format!("Learn {}", target.label()));
        crate::help::hover(ui, &b, crate::help::Id::MidiTransportLearn);
        if b.clicked() {
            let _ = app.run("midi.learn", json!({"target": target.target().to_string(), "scope": "profile"}));
        }
    }
    drawn
}

#[cfg(test)]
mod tests;
