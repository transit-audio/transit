//! Per-clip scale controls and pitch-row mapping. Live MIDI is never transformed here.

use super::*;
use soundcraft_midi::scale::{Scale, ScaleMode};

impl StudioApp {
    pub(super) fn scale_apply_move(&mut self, pairs: &[(Note, Note)]) -> Result<Vec<Note>, String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let old: Vec<_> = pairs.iter().map(|(a, _)| *a).collect();
        let indices = resolve(&self.keys_notes(), &old);
        if indices.len() != pairs.len() {
            return Err("notes changed during move".into());
        }
        let ticks = pairs.first().map_or(0, |(a, b)| b.start - a.start);
        let pitches: Vec<_> = pairs.iter().map(|(_, b)| b.pitch).collect();
        let key = self.keys_merge_key();
        self.keys_exec("midi.notes_move", json!({"clip": clip.0, "indices": indices, "pitches": pitches, "ticks": ticks}), &key)?;
        Ok(pairs.iter().map(|(_, b)| *b).collect())
    }

    pub(super) fn keys_scale(&self) -> ScaleMode {
        self.keys.clip.and_then(|id| self.engine.session().find_clip(id)).map(|(_, c)| c.scale_mode).unwrap_or_default()
    }

    pub(super) fn keys_scale_set(&mut self, params: Value) -> Result<(), String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let mut params = params;
        params["clip"] = json!(clip.0);
        let key = self.keys_merge_key();
        self.keys_exec("midi.scale_mode", params, &key)?;
        Ok(())
    }

    pub(super) fn scale_entry(&self, pitch: u8, chromatic: bool) -> u8 {
        let mode = self.keys_scale();
        if mode.enabled && !chromatic { mode.key.nearest(pitch) } else { pitch }
    }

    /// Scale-mode move of a selection: one shared degree step for the whole group (see
    /// `KeyScale::transpose_group`), clamped or refused as a group. `anchor` is the grabbed
    /// note; keyboard moves pass `None` and the lowest note leads.
    pub(super) fn scale_move(&self, notes: &[Note], ticks: i64, degrees: i64, anchor: Option<u8>) -> Vec<(Note, Note)> {
        let mode = self.keys_scale();
        let min = notes.iter().map(|n| n.start).min().unwrap_or(0);
        let ticks = ticks.max(-min);
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        let lowest = pitches.iter().copied().min().unwrap_or(0);
        let anchor = anchor.filter(|a| pitches.contains(a)).unwrap_or(lowest);
        let moved = if degrees == 0 { pitches } else { mode.key.transpose_group(&pitches, anchor, degrees) };
        notes.iter().zip(moved).map(|(n, pitch)| (*n, Note { start: n.start.saturating_add(ticks), pitch, ..*n })).collect()
    }
}

pub(super) fn controls(app: &mut StudioApp, ui: &mut Ui, layout: &mut KeysLayout) {
    let mode = app.keys_scale();
    let mut enabled = mode.enabled;
    let response = ui.checkbox(&mut enabled, "Scale mode");
    layout.pitch_menu.push(MenuItem::at(ui, "scale_mode".into(), "Scale mode".into(), response.rect));
    if response.changed() {
        let _ = app.keys_scale_set(json!({"enabled": enabled}));
    }
    let mut fold = mode.fold;
    let response = ui.checkbox(&mut fold, "Fold to scale");
    layout.pitch_menu.push(MenuItem::at(ui, "scale_fold".into(), "Fold to scale".into(), response.rect));
    if response.changed() {
        let _ = app.keys_scale_set(json!({"fold": fold}));
    }
    const ROOTS: [&str; 12] = ["C", "C# / Db", "D", "D# / Eb", "E", "F", "F# / Gb", "G", "G# / Ab", "A", "A# / Bb", "B"];
    let mut key = mode.key;
    key.root %= 12;
    let before = key;
    egui::ComboBox::from_id_salt("scale_root").selected_text(ROOTS[usize::from(key.root)]).show_ui(ui, |ui| {
        for (root, label) in ROOTS.iter().enumerate() {
            ui.selectable_value(&mut key.root, root as u8, *label);
        }
    });
    ui.horizontal(|ui| {
        ui.selectable_value(&mut key.scale, Scale::Major, "Major");
        ui.selectable_value(&mut key.scale, Scale::Minor, "Natural minor");
    });
    if key != before {
        let _ = app.keys_scale_set(json!({"root": key.root, "scale": key.scale}));
    }
    ui.small("Shift: chromatic entry and dragging (temporarily unfolds).");
    ui.small("Live MIDI always plays and records exactly as struck.");
}

/// Reusable row visibility mapping: folding hides empty rows, never existing notes.
#[derive(Clone, Copy)]
pub(super) struct Rows {
    visible: [bool; 128],
    ranks: [u8; 128],
    pitches: [u8; 128],
    count: u8,
}

impl Rows {
    pub(super) fn new(mode: ScaleMode, notes: &[Note], live: &[u8; 128], chromatic: bool) -> Self {
        let mut visible = [true; 128];
        if mode.enabled && mode.fold && !chromatic {
            for (pitch, show) in visible.iter_mut().enumerate() {
                *show = mode.key.contains(pitch as u8) || live[pitch] > 0;
            }
            for note in notes {
                if let Some(show) = visible.get_mut(usize::from(note.pitch)) {
                    *show = true;
                }
            }
        }
        let (mut ranks, mut pitches, mut count) = ([0; 128], [0; 128], 0u8);
        for (pitch, show) in visible.iter().enumerate() {
            ranks[pitch] = count;
            if *show {
                pitches[usize::from(count)] = pitch as u8;
                count += 1;
            }
        }
        Self { visible, ranks, pitches, count }
    }

    pub(super) fn contains(&self, pitch: u8) -> bool {
        self.visible[usize::from(pitch.min(127))]
    }

    pub(super) fn last_rank(&self) -> f32 {
        f32::from(self.count.saturating_sub(1))
    }

    pub(super) fn rank(&self, pitch: f32) -> f32 {
        let whole = pitch.floor().clamp(0.0, 127.0) as usize;
        f32::from(self.ranks[whole]) + pitch - whole as f32
    }

    pub(super) fn pitch(&self, rank: f32) -> f32 {
        let whole = rank.floor() as i32;
        if (0..i32::from(self.count)).contains(&whole) {
            return f32::from(self.pitches[whole as usize]) + rank - whole as f32;
        }
        let last = f32::from(self.pitches[usize::from(self.count.saturating_sub(1))]);
        if rank < 0.0 { rank } else { last + rank - self.last_rank() }
    }
}
