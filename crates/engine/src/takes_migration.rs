//! Restore pre-#25 cycle recordings on open, as one undoable edit.

use crate::Engine;
use transit_model::{Session, TakeStack, TrackId};

/// Summary of the load-time migration, also shown in the status line.
pub const NOTE: &str = "recorded with an earlier build now show their laps as takes";

fn migrate(s: &mut Session, track: TrackId, base: usize, stack: &TakeStack) -> Result<(), String> {
    super::materialize(s, track, stack);
    let t = s.track_mut(track).ok_or_else(|| "the track is gone".to_string())?;
    t.active_playlist = base;
    // Playlist order is recording order. Replay every lap, so a partial newest lap keeps
    // the preceding lap around it, using the same lossless cut/join operations as recording.
    for take in &stack.takes {
        for (_, clip) in &take.clips {
            super::activate_clip(s, track, *clip)?;
        }
    }
    Ok(())
}

/// After installing the loaded document and its saved snapshot, migrate eligible tracks.
/// No candidate means no document mutation, dirty flag, or history entry.
pub(crate) fn on_open(e: &mut Engine) {
    let mut candidates = Vec::new();
    let mut notes = Vec::new();
    for t in &e.session().tracks {
        if let Some((base, stack)) = t.legacy_take_stack() {
            if t.freeze.is_some() {
                notes.push(format!(
                    "track \"{}\": earlier-build takes were not migrated because the track is frozen; unfreeze, save, and reopen to migrate",
                    t.name
                ));
            } else {
                candidates.push((t.id, t.name.clone(), base, stack));
            }
        }
    }
    if !candidates.is_empty() {
        let before = e.session_arc();
        let mut migrated = (*before).clone();
        for (id, name, base, stack) in &candidates {
            if let Err(why) = migrate(&mut migrated, *id, *base, stack) {
                // Publish nothing on an unexpected refusal: keep every track as loaded.
                e.message(format!("takes were not migrated: track \"{name}\": {why}"));
                for note in notes {
                    e.message(note);
                }
                return;
            }
        }
        *e.session_mut() = migrated;
        e.push_undo("Migrate takes", before);
        notes.extend(candidates.iter().map(|(_, name, _, _)| format!("track \"{name}\": migrated earlier-build laps to numbered takes")));
        notes.push(format!("{} track(s) {NOTE}", candidates.len()));
    }
    for note in notes {
        e.message(note);
    }
}
