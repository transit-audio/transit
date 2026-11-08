//! Explicit selected-note pitch edits and per-clip scale controls.

use super::*;

impl StudioApp {
    /// `command` on the selected notes, one command per open clip, one undo step.
    pub(super) fn keys_pitch_edit(&mut self, command: &str, mut params: Value) -> Result<(), String> {
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let groups = all.groups(&self.keys.selection);
        if groups.is_empty() {
            return Ok(());
        }
        self.keys_refuse_frozen(&groups.iter().map(|(p, _)| p.clip).collect::<Vec<_>>())?;
        let key = self.keys_merge_key();
        let mut selection = Vec::new();
        for (part, indices) in groups {
            params["clip"] = json!(part.clip.0);
            params["indices"] = json!(indices);
            let result = self.keys_exec(command, params.clone(), &key)?;
            let notes: Vec<Note> = serde_json::from_value(result["notes"].clone()).map_err(|e| e.to_string())?;
            selection.extend(notes.into_iter().map(|n| Note { start: n.start + part.off, ..n }));
        }
        self.keys.selection = selection;
        Ok(())
    }

    /// Snap only the selected notes to the chosen scale, in one undo step.
    pub fn keys_constrain(&mut self) -> Result<(), String> {
        let key = self.keys_scale().key;
        self.keys_pitch_edit("midi.notes_constrain", json!({"root": key.root % 12, "scale": key.scale}))
    }
}

pub(super) fn menu(app: &mut StudioApp, ui: &mut Ui, layout: &mut KeysLayout) {
    let selected = app.keys_all().is_some_and(|all| !resolve(&all.notes, &app.keys.selection).is_empty());
    let (up, down) = if app.keys_scale().enabled { ("Up a scale degree", "Down a scale degree") } else { ("Up a semitone", "Down a semitone") };
    for (label, delta) in [(up, 1), (down, -1), ("Up an octave", 12), ("Down an octave", -12)] {
        let response = ui.add_enabled(selected, egui::Button::new(label));
        layout.pitch_menu.push(MenuItem::at(ui, label.into(), label.into(), response.rect));
        if response.clicked() {
            let _ = app.keys_transpose(delta);
            ui.close();
        }
    }
    ui.separator();
    scale::controls(app, ui, layout);
    let response = ui.add_enabled(selected, egui::Button::new("Constrain selected notes"));
    layout.pitch_menu.push(MenuItem::at(ui, "constrain".into(), "Constrain selected notes".into(), response.rect));
    if response.clicked() {
        let _ = app.keys_constrain();
        ui.close();
    }
    ui.small("Nearest scale tone; equal distances go down.");
}
