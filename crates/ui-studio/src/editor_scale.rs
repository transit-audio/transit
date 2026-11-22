//! The per-plug-in editor Scale setting (issue #153): Auto or a percentage, and the mode, per
//! plug-in id on this machine (`UiState::editor_scale` in `ui.json`), never in the session.
//!
//! The editor window's Scale menu (drawn on the plug-in host thread, `transit_rt::editor_window`)
//! reports a pick as `HeaderAction::Scale`; the app stores it for the plug-in id and writes it into
//! `ui.json` at once (no save step), and the next header the app sends carries it back to the
//! editor, which applies it at once where the plug-in allows and otherwise on the next open
//! (`transit_dsp::editor_scale`). The default (Auto, automatic mode) is not stored.

use crate::StudioApp;
use serde_json::json;
use transit_dsp::EditorScale;

impl StudioApp {
    /// The Scale setting of the plug-in `plugin` on this machine.
    #[must_use]
    pub fn editor_scale_of(&self, plugin: &str) -> EditorScale {
        self.ui.editor_scale.get(plugin).copied().unwrap_or_default()
    }

    /// Stores `value` as the Scale setting of `plugin` and writes it into `ui.json` now, keeping
    /// every other saved key. The default removes the entry.
    pub fn set_editor_scale(&mut self, plugin: &str, value: EditorScale) {
        if self.editor_scale_of(plugin) == value {
            return;
        }
        if value.is_default() {
            self.ui.editor_scale.remove(plugin);
        } else {
            self.ui.editor_scale.insert(plugin.to_owned(), value);
        }
        log::info!("plug-in editor scale: {plugin}: {value:?} (saved for this machine)");
        if std::env::var_os("TRANSIT_NO_PREFS").is_some() {
            return;
        }
        let Some(dir) = self.autosave_dir.as_deref().and_then(std::path::Path::parent) else { return };
        let saved = json!(self.ui.editor_scale);
        if let Err(e) = crate::prefs_file::merge_keys(&dir.join("ui.json"), &self.ui, "editor_scale", vec![("editor_scale", saved)]) {
            self.status = format!("The editor's scale is set, but it could not be saved: {e}");
        }
    }

    /// Sends the header (with the Scale setting) to each editor that is wanted but not open yet,
    /// before the frame asks it to open: the editor needs its setting when it opens (whether its
    /// window is stretched is fixed then), and the open and the header go through the same queue
    /// on the plug-in host thread, in the order they are sent.
    pub(crate) fn send_headers_before_open(&mut self) {
        for key in self.ui.plugin_windows.clone() {
            if self.player.as_ref().is_none_or(|p| p.editor_open(key.0, key.1)) {
                continue;
            }
            if let Some(header) = self.editor_header(key)
                && let Some(p) = &self.player
            {
                let _ = p.set_editor_header(key.0, key.1, &header);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::Value;
    use transit_dsp::{ScaleMode, ScaleStep};

    const ID: &str = "vst3:0123456789ABCDEF0123456789ABCDEF";

    fn read(path: &std::path::Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// Picked, written at once into `ui.json` with every other key kept; the default removes the
    /// entry; a fresh app reading the file gets the setting back.
    #[test]
    fn a_pick_is_written_at_once_and_reads_back_per_plugin_id() {
        let dir = transit_test_support::TestDir::new("editor-scale");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui.json");
        std::fs::write(&path, r#"{"unrelated":42}"#).unwrap();
        let mut app = crate::tests::app();
        app.autosave_dir = Some(dir.join("Autosave"));
        let pick = EditorScale { step: ScaleStep::Percent(150), mode: Some(ScaleMode::WindowsStretch) };
        app.set_editor_scale(ID, pick);
        assert_eq!(read(&path), json!({"unrelated": 42, "editor_scale": {ID: {"step": 150, "mode": "windows_stretch"}}}));
        assert_eq!(app.editor_scale_of(ID), pick);
        assert_eq!(app.editor_scale_of("clap:other"), EditorScale::default(), "keyed by plug-in id");
        // What the app loads at the next start (the file's own keys over the defaults).
        let back: crate::UiState = serde_json::from_value(read(&path)).unwrap();
        assert_eq!(back.editor_scale.get(ID), Some(&pick));
        // Back to the default: the entry goes.
        app.set_editor_scale(ID, EditorScale::default());
        assert_eq!(read(&path), json!({"unrelated": 42, "editor_scale": {}}));
        assert!(app.ui.editor_scale.is_empty());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    /// The setting is a machine preference: it round-trips through `UiState` (an older file
    /// without it reads empty) and never enters the session.
    #[test]
    fn the_setting_round_trips_with_the_preferences_and_stays_out_of_the_session() {
        let older: crate::UiState = serde_json::from_value(json!({})).unwrap();
        assert!(older.editor_scale.is_empty());
        let mut app = crate::tests::app();
        let pick = EditorScale { step: ScaleStep::Percent(75), mode: None };
        app.set_editor_scale(ID, pick);
        let saved = serde_json::to_value(&app.ui).unwrap();
        let back: crate::UiState = serde_json::from_value(saved).unwrap();
        assert_eq!(back.editor_scale.get(ID), Some(&pick));
        let session = serde_json::to_string(app.engine.session()).unwrap();
        assert!(!session.contains("editor_scale") && !session.contains("windows_stretch"), "not in the session");
        assert!(!app.engine.is_dirty(), "picking a scale is no document change");
    }
}
