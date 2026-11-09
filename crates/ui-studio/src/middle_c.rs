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
    /// convention. The host calls it once at startup, after loading `ui.json`.
    pub fn apply_middle_c(&mut self) {
        self.engine.set_message_middle_c(self.ui.middle_c);
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
