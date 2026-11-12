//! How a recording pass is cut into laps when the transport cycled while it recorded: one take
//! per lap, the newest playing (#25). An audio take and a MIDI take of the same pass are cut by
//! this one function, so their clips cover the same ranges (#567).
//!
//! Live and Logic agree on the shape: a pass started before or inside the cycle region records
//! its first stretch from where it started to the cycle end, then one take per full lap, each
//! lap lying over the cycle region, the newest on top and every earlier lap kept as a take.

use transit_time::{Range, Samples};

/// The most laps one recording turns into takes (#522): ten thousand, which is over five hours
/// of a one-bar loop at 120 BPM and over 80 minutes of a one-beat loop. Placing the takes costs
/// time that grows with the square of the lap count on Stop, so there is a limit; past it the
/// newest laps are kept (the newest plays) and the status line says how many earlier laps were
/// left out. An audio take's file always keeps every lap.
pub const MAX_LAPS: i64 = 10_000;

/// How every note about laps left out begins, so the status line can find it among the
/// engine's messages.
pub const LEFT_OUT_NOTE: &str = "Cycle recording keeps the newest";

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

/// The laps of a pass, at most [`MAX_LAPS`] of them, and how many earlier laps were left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Laps {
    pub laps: Vec<Lap>,
    pub left_out: i64,
}

/// The laps of a pass that latched at `start`, `None` when it was not cut into laps (it played
/// straight through). With `pass`, it loop-recorded from the loop start in laps of that length.
/// Otherwise `wrap` is the loop the transport cycled over (the host's `take_cycle`): a pass that
/// started at or before its end and ran past it is cut at each wrap. `lead` (zero or less) is where
/// the first stretch starts (a pickup, #327); `last` is the last offset that holds material.
pub fn laps(start: Samples, pass: Option<Samples>, wrap: Option<Range>, lead: Samples, last: Samples) -> Option<Laps> {
    if let Some(len) = pass.filter(|p| *p > 0) {
        // Pickups belong to the first lap, whose take starts early; later laps are whole.
        let total = (last.max(0) / len).saturating_add(1);
        return Some(keep(total, |k| {
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
    if last < first {
        return None;
    }
    let total = ((last - first) / w.len()).saturating_add(2);
    Some(keep(total, |k| {
        if k == 0 {
            Lap { index: 0, window: Range::new(lead, first), range: Range::new(start.saturating_add(lead), w.end) }
        } else {
            let from = first.saturating_add((k - 1).saturating_mul(w.len()));
            Lap { index: k, window: Range::new(from, from.saturating_add(w.len())), range: w }
        }
    }))
}

/// The newest [`MAX_LAPS`] of `total` laps.
fn keep(total: i64, lap: impl Fn(i64) -> Lap) -> Laps {
    let left_out = (total - MAX_LAPS).max(0);
    Laps { laps: (left_out..total).map(lap).collect(), left_out }
}

/// The status line's note for `left_out` earlier laps of a cycle recording that are not takes;
/// `then` says what became of them (empty, or a clause starting with "; ").
pub fn left_out_note(left_out: i64, then: &str) -> String {
    let laps = if left_out == 1 { "1 earlier lap was".to_string() } else { format!("{left_out} earlier laps were") };
    format!("{LEFT_OUT_NOTE} {MAX_LAPS} laps as takes: {laps} left out{then}.")
}

#[cfg(test)]
#[path = "tests_take_laps.rs"]
mod tests;
