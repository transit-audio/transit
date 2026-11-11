//! The bench's gestures (README, Slices view and its round 2 decisions; ADR 0041), as intents the
//! view sends on completion (never per frame), and the previews drawn while a gesture is under
//! way. The previews use the model's own marker rules (`Markers::legal_add`, `legal_move`,
//! `move_to` and `slicing::snap_to_zero_crossing`, the functions the engine's command calls), so
//! the ghost turns `too close` exactly where the add would be refused and a drag shows where the
//! move will land; there is no second implementation.
//!
//! - **The bench under the flag strip:** a press inside a slice selects it and auditions it while
//!   the button is held (the view's held audition, as Space does; released with the button, when
//!   the pointer leaves the window, the window loses focus or closes); a double-click adds a
//!   marker at the pointer, exactly one (the maintainer's decision of 2026-11-11, #469). Neither
//!   acts while a menu, a modal or a dialog is open or a field has the keyboard, and a click whose
//!   press was held off that way never counts as the first click of a double-click.
//! - **The flag strip** acts on single clicks and swallows the second click of a double-click, so
//!   a double-click never removes and then selects; the one exception is a double-click on the
//!   same slice's cell, which renames the slice in place.
//! - **Add:** a double-click on the bench under the flag strip. **Remove:** a click on a flag.
//!   **Move:** drag a Manual handle (by the distance dragged, once past the toolkit's drag
//!   threshold; a click on a handle sends nothing and plays nothing); Escape, or the window losing
//!   focus, cancels and nothing is sent; one move per completed drag.
//!   **Select:** a click on a slice's cell, or a press on the bench inside the slice.

use super::state::{Loop, Mode};
use transit_model::SourceAudio;
use transit_model::slicing;

/// What a completed gesture asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// Add a marker at the frame under the pointer.
    Add(u64),
    /// Move the marker with this id toward the frame under the pointer.
    Move {
        id: u32,
        frame: u64,
    },
    /// Remove the marker with this id (at this index of the showing set).
    Remove {
        id: u32,
        index: usize,
    },
    /// Audition the slice at this index while the button is held (a press on the bench; the view
    /// already selected it).
    Audition(usize),
    /// Start renaming the slice at this index.
    Rename(usize),
    /// Commit a typed name for the marker with this id.
    Label {
        id: u32,
        text: String,
    },
    Mode(Mode),
    Snap(bool),
    /// The empty bench's Load….
    Load,
}

/// What a click landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Body,
    Flag(usize),
    Cell(usize),
}

/// What to do with a click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Act on it (remove or select; on the bench the press already acted).
    Single,
    /// The second click of a double-click on the bench: add a marker.
    Add,
    /// The second click of a double-click on the same cell: rename.
    Rename,
    /// The second click of any other double-click: nothing.
    Swallow,
}

/// A handle being dragged: the marker, where the drag asks it to go, and whether Escape (or the
/// window losing focus) canceled it. The drag is relative to where the press grabbed the handle
/// (`grab`), and asks for nothing until the pointer has traveled past the toolkit's drag threshold
/// (`moved`): a click on a handle never moves its marker.
#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub index: usize,
    pub id: u32,
    pub requested: u64,
    pub cancelled: bool,
    pub grab: egui::Pos2,
    pub moved: bool,
}

/// A slice's name being typed in its cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Renaming {
    pub index: usize,
    pub id: u32,
    pub text: String,
    pub focused: bool,
}

/// The bench's gesture state (UI state).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gestures {
    pub drag: Option<Drag>,
    /// The last click acted on: when (the frame's input time) and on what.
    pub last: Option<(f64, Target)>,
    pub renaming: Option<Renaming>,
    /// The last press on the bench came while a menu, a modal or a dialog was open or a field had
    /// the keyboard: it played nothing, and its click is not counted.
    pub press_held_off: bool,
}

impl Gestures {
    /// Sorts a click on `target` at `now` (`double`: egui counted it as the second click or later
    /// of a run, a double- or triple-click).
    pub fn click(&mut self, now: f64, double: bool, target: Target, window: f64) -> Act {
        let recent = self.last.filter(|(t, _)| now - t <= window);
        if double && let Some((_, first)) = recent {
            self.last = None;
            return match (first, target) {
                (Target::Cell(a), Target::Cell(b)) if a == b => Act::Rename,
                (Target::Body, Target::Body) => Act::Add,
                _ => Act::Swallow,
            };
        }
        self.last = Some((now, target));
        Act::Single
    }
}

/// The add preview under the pointer.
#[derive(Debug, Clone, PartialEq)]
pub struct Ghost {
    /// Where the add lands (snapped when the loop snaps).
    pub frame: u64,
    /// The frame under the pointer.
    pub raw: u64,
    /// The add would be refused (on a marker, at S or E).
    pub refused: bool,
    /// `+ 0.869 s` or `too close`.
    pub label: String,
}

/// The source's channels, for the snap search (none without decoded audio: the engine then
/// refuses a snapped edit, and the preview shows the requested frame).
fn channels(audio: Option<&SourceAudio>) -> Option<(Vec<&[f32]>, u32)> {
    let a = audio.filter(|a| !a.is_disk())?;
    Some((a.buffer.channels.iter().map(Vec::as_slice).collect(), a.buffer.sample_rate))
}

fn snapped(l: &Loop, audio: Option<&SourceAudio>, frame: u64, lo: u64, hi: u64) -> u64 {
    if !l.snap {
        return frame;
    }
    channels(audio).and_then(|(ch, rate)| slicing::snap_to_zero_crossing(&ch, frame, rate, lo, hi).ok()).unwrap_or(frame)
}

/// The ghost for an add at `raw`: the model's legal interval for the showing set, then the snap.
pub fn ghost(l: &Loop, audio: Option<&SourceAudio>, raw: u64) -> Ghost {
    match l.markers().legal_add(l.start, l.end, raw) {
        Ok((lo, hi)) => {
            let frame = snapped(l, audio, raw, lo, hi);
            Ghost { frame, raw, refused: false, label: format!("+ {:.3} s", l.seconds(frame as f64)) }
        }
        Err(_) => Ghost { frame: raw, raw, refused: true, label: "too close".to_string() },
    }
}

/// Where a drag of the marker at `index` toward `requested` lands, and its tooltip
/// (`1.412 s · snapped`, `0.871 s · clamped at marker 4`): the clamp first, then the snap inside
/// the legal interval, as the move command does. `None` for S.
pub fn drag_preview(l: &Loop, audio: Option<&SourceAudio>, index: usize, requested: u64) -> Option<(u64, String)> {
    let m = l.markers();
    let id = m.markers.get(index)?.id;
    let (lo, hi) = m.legal_move(l.start, l.end, id).ok()?;
    let mut moved = m.clone();
    let step = moved.move_to(l.start, l.end, id, requested).ok()?;
    let frame = snapped(l, audio, step.frame, lo, hi);
    let t = format!("{:.3} s", l.seconds(frame as f64));
    let tip = match step.clamped {
        Some(c) => format!("{t} · {c}"),
        None if l.snap => format!("{t} · snapped"),
        None => t,
    };
    Some((frame, tip))
}
