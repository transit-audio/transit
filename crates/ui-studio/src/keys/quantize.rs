//! Non-destructive quantize preview; only Apply changes the document.

use super::*;
use transit_midi::ops::QuantizeOptions;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct QuantizePrefs {
    pub strength: u8,
    pub swing: u8,
    pub ends: bool,
    pub grid_ticks: i64,
}

impl Default for QuantizePrefs {
    fn default() -> Self {
        Self { strength: 100, swing: 0, ends: false, grid_ticks: PPQ / 4 }
    }
}

impl QuantizePrefs {
    fn options(&self) -> QuantizeOptions {
        QuantizeOptions {
            grid_ticks: self.grid_ticks.clamp(1, PPQ * 16),
            strength: f32::from(self.strength.min(100)) / 100.0,
            swing: f32::from(self.swing.min(100)) / 100.0,
            quantize_ends: self.ends,
            ..Default::default()
        }
    }
}

impl StudioApp {
    fn quantize_indices(&self, notes: &[Note]) -> Vec<usize> {
        if self.keys.selection.is_empty() { (0..notes.len()).collect() } else { resolve(notes, &self.keys.selection) }
    }

    pub(super) fn keys_quantize_preview(&self) -> Vec<Note> {
        let notes = self.keys_notes();
        let mut selected: Vec<_> = self.quantize_indices(&notes).iter().map(|i| notes[*i]).collect();
        transit_midi::ops::quantize(&mut selected, &self.ui.keys.quantize.options());
        selected
    }

    pub(super) fn keys_apply_quantize(&mut self, grid: i64) -> Result<(), String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let indices = self.quantize_indices(&self.keys_notes());
        if indices.is_empty() {
            return Ok(());
        }
        let prefs = self.ui.keys.quantize.clone();
        let key = self.keys_merge_key();
        self.ensure_clip_selected(clip);
        let result = self.keys_exec(
            "event.quantize",
            json!({
                "clip": clip.0, "indices": indices, "grid": grid.clamp(1, PPQ * 16),
                "strength": prefs.strength.min(100), "swing": prefs.swing.min(100), "quantize_ends": prefs.ends,
            }),
            &key,
        )?;
        if !self.keys.selection.is_empty() {
            self.keys.selection = serde_json::from_value(result["selected_notes"].clone()).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

pub(super) fn window(app: &mut StudioApp, ctx: &egui::Context) {
    if !app.keys.quantize_open {
        return;
    }
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    crate::theme::modal_window(ctx, "Quantize notes").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label(if app.keys.selection.is_empty() { "All notes in this clip" } else { "Selected notes only" });
        let prefs = &mut app.ui.keys.quantize;
        prefs.strength = prefs.strength.min(100);
        prefs.swing = prefs.swing.min(100);
        prefs.grid_ticks = prefs.grid_ticks.clamp(1, PPQ * 16);
        const GRIDS: [(&str, i64); 6] =
            [("1/4", PPQ), ("1/8", PPQ / 2), ("1/16", PPQ / 4), ("1/32", PPQ / 8), ("1/8 triplet", PPQ / 3), ("1/16 triplet", PPQ / 6)];
        let label = GRIDS
            .iter()
            .find(|(_, ticks)| *ticks == prefs.grid_ticks)
            .map(|(label, _)| (*label).to_owned())
            .unwrap_or_else(|| format!("{} ticks", prefs.grid_ticks));
        egui::ComboBox::from_id_salt("quantize_grid").selected_text(label).show_ui(ui, |ui| {
            for (label, ticks) in GRIDS {
                ui.selectable_value(&mut prefs.grid_ticks, ticks, label);
            }
        });
        ui.add(egui::Slider::new(&mut prefs.strength, 0..=100).text("Strength %"));
        ui.add(egui::Slider::new(&mut prefs.swing, 0..=100).text("Swing %"));
        ui.checkbox(&mut prefs.ends, "Also quantize note ends");
        ui.small("Swing delays alternate grid lines: 100% = one third of the grid interval.");
        ui.small("Outlined notes preview the result. Nothing changes until Apply.");
        ui.horizontal(|ui| {
            let response = ui.button("Apply");
            app.keys.layout.quantize_apply = r4(response.rect);
            apply = response.clicked();
            let response = ui.button("Cancel");
            app.keys.layout.quantize_cancel = r4(response.rect);
            cancel = response.clicked();
        });
    });
    if apply {
        let _ = app.keys_apply_quantize(app.ui.keys.quantize.grid_ticks);
    }
    app.keys.quantize_open = open && !apply && !cancel;
}
