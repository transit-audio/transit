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
    /// The notes Quantize works on: the selection, or every open clip's notes when none is
    /// selected; grouped by clip with their local indices.
    fn quantize_groups(&self, all: &Composite) -> Vec<(Part, Vec<usize>)> {
        if self.keys.selection.is_empty() {
            all.parts.iter().map(|p| (p.clone(), (0..p.count).collect())).collect()
        } else {
            all.groups(&self.keys.selection)
        }
    }

    /// Where the notes would land, in the roll's ticks (each clip quantized in its own ticks).
    pub(super) fn keys_quantize_preview(&self) -> Vec<Note> {
        let Some(all) = self.keys_all() else { return Vec::new() };
        let mut out = Vec::new();
        for (part, indices) in self.quantize_groups(&all) {
            let mut local: Vec<Note> = indices.iter().map(|i| all.notes[part.base + i]).map(|n| Note { start: n.start - part.off, ..n }).collect();
            transit_midi::ops::quantize(&mut local, &self.ui.keys.quantize.options());
            out.extend(local.into_iter().map(|n| Note { start: n.start + part.off, ..n }));
        }
        out
    }

    /// Quantize, one `event.quantize` per open clip, one undo step.
    pub(super) fn keys_apply_quantize(&mut self, grid: i64) -> Result<(), String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let groups: Vec<_> = self.quantize_groups(&all).into_iter().filter(|(_, idx)| !idx.is_empty()).collect();
        if groups.is_empty() {
            return Ok(());
        }
        self.keys_refuse_frozen(&groups.iter().map(|(p, _)| p.clip).collect::<Vec<_>>())?;
        let prefs = self.ui.keys.quantize.clone();
        let key = self.keys_merge_key();
        self.ensure_clip_selected(clip);
        let mut selection = Vec::new();
        for (part, indices) in groups {
            let result = self.keys_exec(
                "event.quantize",
                json!({
                    "clip": part.clip.0, "indices": indices, "grid": grid.clamp(1, PPQ * 16),
                    "strength": prefs.strength.min(100), "swing": prefs.swing.min(100), "quantize_ends": prefs.ends,
                }),
                &key,
            )?;
            if !self.keys.selection.is_empty() {
                let notes: Vec<Note> = serde_json::from_value(result["selected_notes"].clone()).map_err(|e| e.to_string())?;
                selection.extend(notes.into_iter().map(|n| Note { start: n.start + part.off, ..n }));
            }
        }
        if !self.keys.selection.is_empty() {
            self.keys.selection = selection;
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
        ui.label(match (app.keys.selection.is_empty(), app.keys.others.is_empty()) {
            (false, _) => "Selected notes only",
            (true, true) => "All notes in this clip",
            (true, false) => "All notes in the open clips",
        });
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
        crate::chrome::field(ui, |ui| {
            crate::chrome_pick::Combo::new("quantize_grid", label).show(ui, |ui| {
                for (label, ticks) in GRIDS {
                    crate::chrome_pick::choice_value(ui, &mut prefs.grid_ticks, ticks, label);
                }
            })
        });
        ui.add(egui::Slider::new(&mut prefs.strength, 0..=100).text("Strength %"));
        ui.add(egui::Slider::new(&mut prefs.swing, 0..=100).text("Swing %"));
        crate::chrome::checkbox(ui, &mut prefs.ends, "Also quantize note ends");
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
