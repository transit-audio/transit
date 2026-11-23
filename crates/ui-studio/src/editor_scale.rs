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

/// One saved entry of `UiState::editor_scale`. An entry this build cannot read (hand-edited, or
/// written by a newer build) is kept as it is on disk and saved back unchanged, and that plug-in
/// reads as the default: one bad entry never drops the others (#153 review).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum SavedScale {
    /// A setting this build reads.
    Valid(EditorScale),
    /// Anything else, kept as it was.
    Unreadable(serde_json::Value),
}

impl StudioApp {
    /// The Scale setting of the plug-in `plugin` on this machine (the default for an entry this
    /// build cannot read).
    #[must_use]
    pub fn editor_scale_of(&self, plugin: &str) -> EditorScale {
        match self.ui.editor_scale.get(plugin) {
            Some(SavedScale::Valid(s)) => *s,
            Some(SavedScale::Unreadable(v)) => {
                log::debug!("plug-in editor scale: {plugin}: unreadable entry {v} ignored");
                EditorScale::default()
            }
            None => EditorScale::default(),
        }
    }

    /// Stores `value` as the Scale setting of `plugin` and writes that one entry into `ui.json`
    /// now: the file's `editor_scale` map is read, the entry set (or removed for the default) and
    /// every other entry and key kept as they are on disk, not replaced from this process's copy.
    /// Known limit: two transit processes writing `ui.json` at the same moment can still lose one
    /// of the two writes (the read and the write are not one step across processes).
    pub fn set_editor_scale(&mut self, plugin: &str, value: EditorScale) {
        if self.editor_scale_of(plugin) == value && self.ui.editor_scale.contains_key(plugin) != value.is_default() {
            return;
        }
        if value.is_default() {
            self.ui.editor_scale.remove(plugin);
        } else {
            self.ui.editor_scale.insert(plugin.to_owned(), SavedScale::Valid(value));
        }
        log::info!("plug-in editor scale: {plugin}: {value:?} (saved for this machine)");
        if std::env::var_os("TRANSIT_NO_PREFS").is_some() {
            return;
        }
        let Some(dir) = self.autosave_dir.as_deref().and_then(std::path::Path::parent) else { return };
        let entry = (!value.is_default()).then(|| json!(value));
        let saved = crate::prefs_file::merge_with(&dir.join("ui.json"), &self.ui, "editor_scale", |obj| {
            let map = obj.entry("editor_scale").or_insert_with(|| json!({}));
            if !map.is_object() {
                *map = json!({});
            }
            if let Some(m) = map.as_object_mut() {
                match &entry {
                    Some(v) => m.insert(plugin.to_owned(), v.clone()),
                    None => m.remove(plugin),
                };
            }
        });
        if let Err(e) = saved {
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
        assert_eq!(back.editor_scale.get(ID), Some(&SavedScale::Valid(pick)));
        // Back to the default: the entry goes.
        app.set_editor_scale(ID, EditorScale::default());
        assert_eq!(read(&path), json!({"unrelated": 42, "editor_scale": {}}));
        assert!(app.ui.editor_scale.is_empty());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    /// #153 review, item 7: one entry this build cannot read does not drop the others at load,
    /// is saved back as it was, reads as the default; and a pick writes only its own entry into
    /// the map on disk (an entry another writer added since this process loaded stays).
    #[test]
    fn a_bad_entry_keeps_the_others_and_a_pick_merges_into_the_map_on_disk() {
        let file = json!({"editor_scale": {"clap:bad": {"step": 333}, ID: {"step": 75, "mode": null}}});
        let ui: crate::UiState = serde_json::from_value(file.clone()).unwrap();
        assert_eq!(ui.editor_scale.len(), 2, "both entries kept");
        assert_eq!(serde_json::to_value(&ui).unwrap()["editor_scale"], file["editor_scale"], "saved back as they were");
        let dir = transit_test_support::TestDir::new("editor-scale-merge");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ui.json");
        let mut app = crate::tests::app();
        app.ui = ui;
        app.autosave_dir = Some(dir.join("Autosave"));
        assert_eq!(app.editor_scale_of("clap:bad"), EditorScale::default());
        assert_eq!(app.editor_scale_of(ID), EditorScale { step: ScaleStep::Percent(75), mode: None });
        // Another writer added an entry since this process loaded.
        std::fs::write(&path, r#"{"editor_scale":{"clap:other":{"step":200,"mode":null},"clap:bad":{"step":333}}}"#).unwrap();
        app.set_editor_scale(ID, EditorScale { step: ScaleStep::Percent(150), mode: None });
        assert_eq!(
            read(&path)["editor_scale"],
            json!({"clap:other": {"step": 200, "mode": null}, "clap:bad": {"step": 333}, ID: {"step": 150, "mode": null}})
        );
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
        assert_eq!(back.editor_scale.get(ID), Some(&SavedScale::Valid(pick)));
        let session = serde_json::to_string(app.engine.session()).unwrap();
        assert!(!session.contains("editor_scale") && !session.contains("windows_stretch"), "not in the session");
        assert!(!app.engine.is_dirty(), "picking a scale is no document change");
    }
}
