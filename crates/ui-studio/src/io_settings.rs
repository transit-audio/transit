//! The machine's I/O paths (ADR 0015), kept with the preferences in `ui.json`, per side since the
//! amendment for #431 (rule 8): output records in `UiState.io_maps`, one per audio system plus
//! output device (outputs, Main, measurements), and input records in `UiState.input_maps`, one per
//! audio system plus the identity of the capture endpoint actually opened. The engine holds one
//! composed table (`setup.io_map`); every change reaches the audio thread as a plan, never a
//! reopen, and is split again into the two records when it is kept.

use crate::StudioApp;
use transit_model::{InputMap, IoMap};

/// Records kept in the preferences, per side; the oldest go first.
const MAX_SAVED: usize = 64;

/// The input composed into the engine's table: the capture endpoint's identity (the key of its
/// record and of its measurements), its display name, and its own channel count (without the
/// System Audio pair; it may exceed the table's 32-channel cap, which the page says).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInput {
    pub identity: String,
    pub name: String,
    pub channels: usize,
    /// A JACK virtual device's ordered port names at this open (the port-mapping guard, amendment
    /// for #431, rule 5); empty for every other input.
    pub ports: Vec<String>,
}

impl OpenInput {
    pub(crate) fn of(r: &transit_playback::record::Recorder) -> Self {
        Self {
            identity: r.input_identity.id.clone(),
            name: r.input_identity.name.clone(),
            channels: r.device_channels(),
            ports: r.input_ports.clone(),
        }
    }

    fn width(&self) -> u16 {
        u16::try_from(self.channels).unwrap_or(u16::MAX)
    }
}

/// The output record for a device that just opened: its saved one with the output count the
/// stream has (output paths that no longer fit are dropped), else the defaults `opened` holds. The
/// input half is not looked at here ([`compose`]).
pub fn adopt(saved: &[IoMap], opened: &IoMap) -> IoMap {
    let mut map = saved.iter().rfind(|m| m.key() == opened.key()).cloned().unwrap_or_else(|| opened.clone());
    map.reconcile_outputs(opened.outputs);
    // Where System Audio sits in the live input is the open input's, never the saved table's.
    map.system_audio = opened.system_audio;
    map
}

/// The engine's table: `output`'s outputs, Main and measurements with the input half of `input`'s
/// record (rules 9 and 11, [`transit_model::adopt_input`]), or with no input half when no input
/// is open. Returns the input record to keep, when there is an input.
pub fn compose(output: &IoMap, input: Option<&OpenInput>, inputs: &[InputMap], outputs: &[IoMap]) -> (IoMap, Option<InputMap>) {
    match input {
        // An input without an identity (two devices share its name, cpal): this open's defaults,
        // no saved record read or written (amendment for #431, rule 8).
        Some(i) if i.identity.is_empty() => (transit_model::adopt_input(output, &[], &[], "", i.width()).0, None),
        Some(i) => {
            let (map, record) = transit_model::adopt_input_ports(output, inputs, outputs, &i.identity, i.width(), &i.ports);
            (map, Some(record))
        }
        None => (output.compose(None), None),
    }
}

/// Keep `map`'s output half as its output device's record (most recent last): an older build's
/// record under the same key keeps its input half exactly as it was read (rule 11).
pub fn remember(saved: &mut Vec<IoMap>, map: &IoMap) {
    let old = saved.iter().rfind(|m| m.key() == map.key()).cloned();
    let record = map.output_record(old.as_ref());
    saved.retain(|m| m.key() != record.key());
    saved.push(record);
    let excess = saved.len().saturating_sub(MAX_SAVED);
    saved.drain(..excess);
}

/// Keep an input record (most recent last).
pub fn remember_input(saved: &mut Vec<InputMap>, record: InputMap) {
    saved.retain(|r| r.key() != record.key());
    saved.push(record);
    let excess = saved.len().saturating_sub(MAX_SAVED);
    saved.drain(..excess);
}

/// Keep `map`'s input half as its input's record, with the `defaults_for` the record already has
/// (it changes only when an input opens). Nothing when no input is composed in.
pub fn remember_inputs_of(saved: &mut Vec<InputMap>, map: &IoMap) {
    let defaults_for = saved.iter().rfind(|r| r.key() == (map.system.as_str(), map.input_device.as_str())).map_or(0, |r| r.defaults_for);
    if let Some(record) = map.input_record(defaults_for) {
        remember_input(saved, record);
    }
}

impl StudioApp {
    /// The input open now: the app's recorder, else the one the player holds (opened with the
    /// output). `None` when no input opened (not asked for yet, or it failed).
    pub(crate) fn open_input(&self) -> Option<OpenInput> {
        self.recorder.as_ref().map(OpenInput::of).or_else(|| self.player.as_ref()?.with_held_input(OpenInput::of))
    }

    /// Once per opened device, give the engine this machine's table for it, composed from the
    /// output's record and the open input's (ADR 0015, amendment for #431); again whenever another
    /// input (or the same one at another width) opens; afterwards pass every change of the
    /// engine's table to the audio thread and the preferences. Without a real device (`--no-audio`,
    /// the silent fallback) there is nothing to open or keep; the silent device of `--null-audio`
    /// gets its table, but it is not this machine's, so neither half is ever saved. An input that
    /// failed to open changes no record. An input the app opens itself is composed before it is
    /// attached ([`StudioApp::open_input_composed`]); this catches the input the player holds.
    pub(crate) fn sync_io(&mut self) {
        if self.io_player().is_none() {
            return;
        }
        let input = self.open_input();
        if self.io_adopt || (input.is_some() && input != self.io_input) {
            self.compose_io(input);
        } else {
            self.push_io();
        }
    }

    /// The Player a table goes to: a real one, not waiting to be reopened after a driver reset (it
    /// gets the table with its successor).
    fn io_player(&self) -> Option<&transit_playback::Player> {
        self.player.as_ref().filter(|p| !p.silent && p.reopen_cause().is_none())
    }

    /// Compose the engine's table for `input` (the output's record: the opened device's on a new
    /// open, else the engine's own output half) and pass it to the audio thread at once.
    pub(crate) fn compose_io(&mut self, input: Option<OpenInput>) {
        let Some(player) = self.io_player() else { return };
        let keep = !player.null_device();
        let output = if self.io_adopt { adopt(&self.ui.io_maps, &player.io_map()) } else { self.engine.io_map().compose(None) };
        let (map, record) = compose(&output, input.as_ref(), &self.ui.input_maps, &self.ui.io_maps);
        if let Some(record) = record.filter(|_| keep) {
            remember_input(&mut self.ui.input_maps, record);
        }
        self.engine.set_io_map(map);
        self.io_adopt = false;
        self.io_input = input;
        self.push_io();
    }

    /// Pass the engine's table to the audio thread and keep both halves, when it changed.
    fn push_io(&mut self) {
        if self.io_pushed == self.engine.io_revision() {
            return;
        }
        let map = self.engine.io_map().clone();
        let Some(player) = self.io_player() else { return };
        let keep = !player.null_device();
        if player.set_io(map.clone()).is_ok() {
            if keep {
                remember(&mut self.ui.io_maps, &map);
                remember_inputs_of(&mut self.ui.input_maps, &map);
            }
            self.io_pushed = self.engine.io_revision();
        }
    }

    /// Open the input and compose its saved table before it is attached (amendment for #431, rule
    /// 8; review of 652509f3, must-fix 3): there is no state in which capture feeds the plan, or a
    /// take is planned, while the engine's table lacks this input's half. One step, so no caller can
    /// attach first and compose later.
    pub(crate) fn open_input_composed(&mut self) -> Result<transit_playback::record::Recorder, String> {
        use crate::audio_connection::Connection;
        let cfg = self.ui.audio.clone();
        let player = self.player.as_ref().ok_or("no audio device")?;
        let r = Connection::open_input(player, &cfg)?;
        self.compose_io(Some(OpenInput::of(&r)));
        let player = self.player.as_ref().ok_or("no audio device")?;
        Connection::monitor(player, &r)?;
        Ok(r)
    }
}

#[cfg(test)]
#[path = "io_settings_tests.rs"]
mod tests;
