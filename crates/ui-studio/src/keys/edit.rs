//! The piano roll's note edits as engine commands: one command per open clip under one merge key,
//! so every gesture is one undo step (#329).

use super::*;

impl StudioApp {
    /// Change notes (old value, new value) as one undo step, each found again before its edit.
    pub fn keys_edit(&mut self, pairs: &[(Note, Note)]) -> Result<Vec<Note>, String> {
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let olds: Vec<Note> = pairs.iter().map(|(a, _)| *a).collect();
        // Each note's clip is fixed before any edit: after one edit a note of one clip can equal
        // another clip's old value, so only the search within the clip runs again per edit.
        let owners: Vec<Option<(ClipId, i64)>> = all.owners(&olds).into_iter().map(|p| p.map(|p| (p.clip, p.off))).collect();
        self.keys_refuse_frozen(&owners.iter().flatten().map(|(c, _)| *c).collect::<Vec<_>>())?;
        let key = self.keys_merge_key();
        let mut out = Vec::new();
        for ((old, new), owner) in pairs.iter().zip(owners) {
            if old == new {
                out.push(*new);
                continue;
            }
            let Some((clip, off)) = owner else { continue };
            let local = clip_view(self.engine.session(), clip).map(|v| v.notes).unwrap_or_default();
            let Some(i) = resolve(&local, &[Note { start: old.start - off, ..*old }]).first().copied() else { continue };
            let start = new.start - off;
            self.keys_exec(
                "midi.note_edit",
                json!({"clip": clip.0, "index": i, "pitch": new.pitch, "start_ticks": start, "length_ticks": new.length, "velocity": new.velocity}),
                &key,
            )?;
            out.push(*new);
        }
        Ok(out)
    }

    /// Move the selection by `ticks` and `semitones` (clamped so no note leaves its clip's start or
    /// the pitch range), one command per clip, one undo step.
    pub fn keys_move(&mut self, ticks: i64, semitones: i64) -> Result<(), String> {
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let sel = self.keys.selection.clone();
        if self.keys_scale().enabled {
            let degrees = if matches!(semitones, -12 | 12) { semitones.signum() * 7 } else { semitones };
            let pairs = self.scale_move(&sel, ticks, degrees, None);
            self.keys.selection = self.scale_apply_move(&pairs)?;
            return Ok(());
        }
        let (dt, ds) = clamp_move(&sel, all.local_min(&sel), ticks, semitones);
        let groups = all.groups(&sel);
        if groups.is_empty() || (dt == 0 && ds == 0) {
            return Ok(());
        }
        self.keys_refuse_frozen(&groups.iter().map(|(p, _)| p.clip).collect::<Vec<_>>())?;
        let key = self.keys_merge_key();
        for (part, idx) in groups {
            self.keys_exec("midi.notes_move", json!({"clip": part.clip.0, "indices": idx, "ticks": dt, "semitones": ds}), &key)?;
        }
        self.keys.selection = sel.iter().map(|n| shifted(n, dt, ds)).collect();
        Ok(())
    }

    /// Delete the selected notes, one command per clip, one undo step.
    pub fn keys_delete(&mut self) -> Result<(), String> {
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let groups = all.groups(&self.keys.selection);
        if groups.is_empty() {
            self.keys.selection.clear();
            return Ok(());
        }
        self.keys_refuse_frozen(&groups.iter().map(|(p, _)| p.clip).collect::<Vec<_>>())?;
        self.keys.selection.clear();
        let key = self.keys_merge_key();
        for (part, idx) in groups {
            self.keys_exec("midi.note_delete", json!({"clip": part.clip.0, "indices": idx}), &key)?;
        }
        Ok(())
    }

    /// Ctrl+D: copies of the selection right after it (its span rounded up to the grid), each in
    /// its source's clip.
    pub fn keys_duplicate(&mut self) -> Result<(), String> {
        let sel = self.keys.selection.clone();
        let (Some(a), Some(b)) = (sel.iter().map(|n| n.start).min(), sel.iter().map(Note::end).max()) else { return Ok(()) };
        let g = self.keys_step();
        let by = ((b - a + g - 1) / g).max(1) * g;
        let pairs: Vec<(Note, Note)> = sel.iter().map(|n| (*n, shifted(n, by, 0))).collect();
        self.keys.selection = self.keys_add_copies(&pairs)?;
        Ok(())
    }

    /// Quick apply with saved strength/swing to the current piano-roll grid.
    pub fn keys_quantize(&mut self) -> Result<(), String> {
        self.keys_apply_quantize(self.keys_step())
    }

    /// Ctrl+Up/Down: transpose the selection (all notes when none or all are selected:
    /// `event.transpose`; otherwise `midi.notes_transpose`).
    pub fn keys_transpose(&mut self, semitones: i64) -> Result<(), String> {
        let Some(clip) = self.keys.clip else { return Err("no clip".into()) };
        let Some(all) = self.keys_all() else { return Err("no clip".into()) };
        let notes = all.notes.clone();
        let sel = self.keys.selection.clone();
        let whole = sel.is_empty() || resolve(&notes, &sel).len() == notes.len();
        if self.keys_scale().enabled {
            let degrees = if matches!(semitones, -12 | 12) { semitones.signum() * 7 } else { semitones };
            let pairs = self.scale_move(if whole { &notes } else { &sel }, 0, degrees, None);
            let edited = self.scale_apply_move(&pairs)?;
            if !sel.is_empty() {
                self.keys.selection = edited;
            }
            return Ok(());
        }
        if !whole {
            return self.keys_pitch_edit("midi.notes_transpose", json!({"semitones": semitones}));
        }
        let key = self.keys_merge_key();
        self.ensure_clip_selected(clip);
        let clips: Vec<u64> = all.parts.iter().map(|p| p.clip.0).collect();
        self.keys_exec("event.transpose", json!({"clips": clips, "semitones": semitones}), &key)?;
        if !sel.is_empty() {
            self.keys.selection = self.keys_all().map(|a| a.notes).unwrap_or_default();
        }
        Ok(())
    }
}
