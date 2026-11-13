//! How a recording pass is cut into laps when the transport cycled while it recorded: one take
//! per lap, the newest playing (#25). An audio take and a MIDI take of the same pass are cut by
//! this one function, so their clips cover the same ranges (#567).
//!
//! Live and Logic agree on the shape: a pass started before or inside the cycle region records
//! its first stretch from where it started to the cycle end, then one take per full lap, each
//! lap lying over the cycle region, the newest on top and every earlier lap kept as a take.

use transit_time::{Range, Samples};

/// The most laps of one pass that become takes (#522). Placing and settling takes costs work
/// that grows with the square of the lap count per track, and each lap is a clip per recorded
/// track, so there is a limit; 10,000 is an engineering choice, not a measured safe maximum. Past
/// it the newest laps are kept (the newest plays) and the status line says how many earlier laps
/// were left out and where they remain. The window is the pass's, the same for every track.
pub const MAX_LAPS: i64 = 10_000;

/// How every note about laps left out begins, so the status line can find it among the
/// engine's messages.
pub const LEFT_OUT_NOTE: &str = "Cycle recording keeps the newest";

/// How every note about a last lap too short to be a take begins (#634).
pub const SHORT_LAP_NOTE: &str = "A last partial lap shorter than an eighth of the loop is not a take";

/// One lap of a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lap {
    /// Which lap this is, from 0, laps left out counted.
    pub index: i64,
    /// The offsets since the latch it holds.
    pub window: Range,
    /// Where its take lies on the timeline.
    pub range: Range,
}

/// The laps of a pass from its first kept lap on, and how many earlier laps were left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Laps {
    pub laps: Vec<Lap>,
    pub left_out: i64,
}

/// The laps of a pass that latched at `start`, `None` when it was not cut into laps (it played
/// straight through). With `pass`, it loop-recorded from the loop start in laps of that length.
/// Otherwise `wrap` is the loop the transport cycled over (the host's `take_cycle`): a pass that
/// started at or before its end and ran past it is cut at each wrap. `lead` (zero or less) is where
/// the first stretch starts (a pickup, #327).
///
/// `end` is the pass's end, in samples since the latch (Stop): it alone decides which laps are
/// left out past [`MAX_LAPS`], so every track of one pass keeps the same window. `last` is the last
/// offset this track holds material at: its laps run through it (a file a few frames longer than
/// the host's clock, say), never cut short of what it holds.
pub fn laps(start: Samples, pass: Option<Samples>, wrap: Option<Range>, lead: Samples, last: Samples, end: Samples) -> Option<Laps> {
    let reach = last.max(end - 1);
    if let Some(len) = pass.filter(|p| *p > 0) {
        let count = |to: Samples| (to.max(0) / len).saturating_add(1);
        // Pickups belong to the first lap, whose take starts early; later laps are whole.
        return Some(keep(count(end - 1), count(reach), |k| {
            let lead = if k == 0 { lead } else { 0 };
            Lap {
                index: k,
                window: Range::new(k.saturating_mul(len).saturating_add(lead), k.saturating_add(1).saturating_mul(len)),
                range: Range::new(start.saturating_add(lead), start.saturating_add(len)),
            }
        }));
    }
    let w = wrap.filter(|w| pass.is_none() && !w.is_empty() && start >= 0 && start < w.end)?;
    // The first stretch runs from the latch (or its first pickup) to the loop end, every later
    // one is a whole loop.
    let first = w.end - start;
    if reach < first {
        return None;
    }
    let count = |to: Samples| if to < first { 1 } else { ((to - first) / w.len()).saturating_add(2) };
    Some(keep(count(end - 1), count(reach), |k| {
        if k == 0 {
            Lap { index: 0, window: Range::new(lead, first), range: Range::new(start.saturating_add(lead), w.end) }
        } else {
            let from = first.saturating_add((k - 1).saturating_mul(w.len()));
            Lap { index: k, window: Range::new(from, from.saturating_add(w.len())), range: w }
        }
    }))
}

/// The laps from the pass's first kept one (the newest [`MAX_LAPS`] of the pass's `pass_total`)
/// through this track's `total`.
fn keep(pass_total: i64, total: i64, lap: impl Fn(i64) -> Lap) -> Laps {
    let left_out = (pass_total - MAX_LAPS).max(0);
    Laps { laps: (left_out..total.max(left_out)).map(lap).collect(), left_out }
}

/// The status line's note for `left_out` earlier laps of a cycle recording that are not takes;
/// `then` says where they remain (a clause starting with "; ", or a parenthesis).
pub fn left_out_note(left_out: i64, then: &str) -> String {
    let laps = if left_out == 1 { "1 earlier lap was".to_string() } else { format!("{left_out} earlier laps were") };
    format!("{LEFT_OUT_NOTE} {MAX_LAPS} laps as takes: {laps} left out{then}.")
}

/// A left-out note split into what every track of the pass shares (the count) and where this
/// track's laps remain (or a parenthesis: that they held nothing), for the host to say the count
/// once and list the places.
pub fn split_note(note: &str) -> (&str, Option<&str>) {
    let note = note.trim_end_matches('.');
    if let Some((head, tail)) = note.split_once("; ") {
        return (head, Some(tail));
    }
    match note.split_once(" (") {
        Some((head, inner)) => (head, Some(inner.trim_end_matches(')'))),
        None => (note, None),
    }
}

/// How the note begins that names a MIDI journal kept because the file of the laps left out
/// could not be written (#522): no cleanup removes that journal.
pub const KEPT_JOURNAL_NOTE: &str = "the MIDI journal is kept at";

/// The note naming the MIDI journal kept at `path`.
pub fn kept_journal_note(path: &std::path::Path) -> String {
    format!("{KEPT_JOURNAL_NOTE} {}", path.display())
}

/// The note for a last lap of `frames` frames (`seconds` long) at the end of the take file `file`
/// that is no take, being shorter than an eighth of the loop (#634): the frames stay in the file.
pub fn short_lap_note(frames: Samples, seconds: f64, file: &str) -> String {
    format!("{SHORT_LAP_NOTE}: the last {frames} frames ({:.1} ms) stay in {file}.", seconds * 1000.0)
}

#[cfg(test)]
#[path = "tests_take_laps.rs"]
mod tests;
