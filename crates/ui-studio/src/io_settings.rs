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
}

impl OpenInput {
    fn of(r: &transit_playback::record::Recorder) -> Self {
        Self { identity: r.input_identity.id.clone(), name: r.input_identity.name.clone(), channels: r.device_channels() }
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
        Some(i) => {
            let (map, record) = transit_model::adopt_input(output, inputs, outputs, &i.identity, i.width());
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
    /// failed to open changes no record.
    pub(crate) fn sync_io(&mut self) {
        // A Player waiting to be reopened after a driver reset gets the table with its successor.
        let Some(player) = self.player.as_ref().filter(|p| !p.silent && p.reopen_cause().is_none()) else { return };
        let keep = !player.null_device();
        let input = self.open_input();
        let output = if self.io_adopt {
            Some(adopt(&self.ui.io_maps, &player.io_map()))
        } else if input.is_some() && input != self.io_input {
            // An input opened as its own device after the output (cpal: ALSA, CoreAudio), or
            // another one: the input half follows it; the output half is the engine's.
            Some(self.engine.io_map().compose(None))
        } else {
            None
        };
        if let Some(output) = output {
            let (map, record) = compose(&output, input.as_ref(), &self.ui.input_maps, &self.ui.io_maps);
            if let Some(record) = record.filter(|_| keep) {
                remember_input(&mut self.ui.input_maps, record);
            }
            self.engine.set_io_map(map);
            self.io_adopt = false;
            self.io_input = input;
        }
        if self.io_pushed == self.engine.io_revision() {
            return;
        }
        let map = self.engine.io_map().clone();
        let Some(player) = self.player.as_ref() else { return };
        if player.set_io(map.clone()).is_ok() {
            if keep {
                remember(&mut self.ui.io_maps, &map);
                remember_inputs_of(&mut self.ui.input_maps, &map);
            }
            self.io_pushed = self.engine.io_revision();
        }
    }
}

#[cfg(test)]
#[path = "io_settings_tests.rs"]
mod tests;
