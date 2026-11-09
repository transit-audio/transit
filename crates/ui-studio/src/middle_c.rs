//! Options > Middle C (ADR 0051 decision 2): the name MIDI 60 carries in every transit window,
//! C3 (the default, as Logic and Live show it) or C4 (scientific pitch). A per-machine preference
//! in `ui.json`; never in the session or undo, and never read by a command's parser.
//!
//! `options.middle_c {value?: "C3" | "C4"}` is the one way to change it (the Options menu's radio
//! pair runs it): it saves `ui.json` and hands the setting to the engine's message convention in
//! the same step, before the next frame, so the screen and engine messages never disagree.

use crate::StudioApp;
use serde_json::{Value, json};
use transit_midi::MiddleC;

impl StudioApp {
    /// `options.middle_c {value?}`: sets the convention; without `value` it answers the setting.
    pub(crate) fn set_middle_c(&mut self, p: &Value) -> Result<Value, String> {
        if let Some(v) = p.get("value") {
            let want = v.as_str().and_then(MiddleC::from_label).ok_or("options.middle_c: value must be \"C3\" or \"C4\"")?;
            if want != self.ui.middle_c {
                self.ui.middle_c = want;
                self.save_middle_c();
            }
            self.apply_middle_c();
        }
        Ok(json!({"value": self.ui.middle_c}))
    }

    /// Hands the UI's setting to what names notes outside the frame: the engine's message
    /// convention and the MIDI Monitor's retained rows. The host calls it once at startup, after
    /// loading `ui.json`.
    pub fn apply_middle_c(&mut self) {
        self.engine.set_message_middle_c(self.ui.middle_c);
        self.midi_monitor.set_convention(self.ui.middle_c);
    }

    /// Writes `middle_c` into `ui.json` now, keeping every other saved key.
    fn save_middle_c(&mut self) {
        if std::env::var_os("TRANSIT_NO_PREFS").is_some() {
            return;
        }
        let Some(dir) = self.autosave_dir.as_deref().and_then(std::path::Path::parent) else { return };
        if let Err(e) = crate::prefs_file::merge_keys(&dir.join("ui.json"), &self.ui, "middle_c", vec![("middle_c", json!(self.ui.middle_c))]) {
            self.status = format!("Middle C is {}, but the setting could not be saved: {e}", self.ui.middle_c.label());
        }
    }
}

/// A parameter's value as the screen shows it (ADR 0051 decision 4): a note-labelled choice
/// (`ParamInfo::note_labels`) names entry `i` as MIDI `first_note + step * i` in the Middle C
/// setting (its octave number alone when `octave_only`); every other parameter reads
/// `ParamInfo::format`. Every UI site that shows a parameter's value text calls this; the
/// control channel's listing keeps the fixed C3 strings in `choices`.
pub(crate) fn param_text(p: &transit_dsp::ParamInfo, value: f32, middle_c: MiddleC) -> String {
    match entry_pitch(p, value) {
        Some((pitch, true)) => transit_midi::octave(pitch, middle_c).to_string(),
        Some((pitch, false)) => transit_midi::note_name(pitch, middle_c),
        None => p.format(value),
    }
}

/// Every entry of a choice as [`param_text`] names it, in order (a dropdown's list).
pub(crate) fn choice_texts(p: &transit_dsp::ParamInfo, middle_c: MiddleC) -> Vec<String> {
    (0..p.choices.len()).map(|i| param_text(p, i as f32, middle_c)).collect()
}

/// A typed value for a note-labelled choice: the entry whose note (or octave number, when
/// `octave_only`) the text names in the Middle C setting. `None` for any other parameter, or
/// when no entry matches.
pub(crate) fn parse_note_entry(p: &transit_dsp::ParamInfo, text: &str, middle_c: MiddleC) -> Option<f32> {
    let labels = p.note_labels?;
    let entries = (0..p.choices.len()).filter_map(|i| entry_pitch(p, i as f32).map(|(pitch, _)| (i, pitch)));
    let found = if labels.octave_only {
        let n: i32 = text.trim().parse().ok()?;
        entries.clone().find(|(_, pitch)| transit_midi::octave(*pitch, middle_c) == n)
    } else {
        let want = transit_midi::parse_note(text, middle_c, None)?;
        entries.clone().find(|(_, pitch)| *pitch == want)
    };
    found.map(|(i, _)| i as f32)
}

/// The MIDI pitch entry `value` of a note-labelled choice stands for, and whether it shows its
/// octave alone.
fn entry_pitch(p: &transit_dsp::ParamInfo, value: f32) -> Option<(u8, bool)> {
    let labels = p.note_labels.filter(|_| p.unit == transit_dsp::Unit::Choice)?;
    let index = p.clamp(value) as u32;
    let pitch = u32::from(labels.first_note) + u32::from(labels.step) * index;
    u8::try_from(pitch).ok().filter(|n| *n <= 127).map(|n| (n, labels.octave_only))
}
