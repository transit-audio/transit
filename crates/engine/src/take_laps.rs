//! How a recording pass is cut into laps when the transport cycled while it recorded: one take
//! per lap, the newest playing (#25). An audio take and a MIDI take of the same pass are cut by
//! this one function, so their clips cover the same ranges (#567).
//!
//! Live and Logic agree on the shape: a pass started before or inside the cycle region records
//! its first stretch from where it started to the cycle end, then one take per full lap, each
//! lap lying over the cycle region, the newest on top and every earlier lap kept as a take.

use transit_time::{Range, Samples};

/// The most laps one cycle recording pass records (#522): the host ends the recording once the
/// pass completes this many laps, so every lap up to there is a take and nothing is left out.
/// Placing and settling takes costs work that grows with the square of the lap count per track,
/// and each lap is a clip per recorded track, so there is a limit; 10,000 is an engineering
/// choice, not a measured safe maximum.
///
/// The cut allows one lap window after these (zero-based lap 10,000: the partial lap a Stop at
/// the limit leaves), under the ordinary partial-lap rules and with no notice. A pass that
/// reaches lap 10,001 or later, by its end or its material (the host could not stop it in time,
/// or crash recovery of a pass that ran on), keeps laps 0 to 9,999 as takes and everything from lap 10,000 on as one
/// longer final take, said in [`over_limit_note`].
pub const MAX_LAPS: i64 = 10_000;

/// How the note begins that says a pass ran past [`MAX_LAPS`] and its last laps are one take.
pub const OVER_LIMIT_NOTE: &str = "Cycle recording ran past its limit of";

/// How every note about a last lap too short to be a take begins (#634).
pub const SHORT_LAP_NOTE: &str = "A last partial lap shorter than an eighth of the loop is not a take";

/// One lap of a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lap {
    /// Which lap this is, from 0.
    pub index: i64,
    /// The offsets since the latch it holds.
    pub window: Range,
    /// Where its take lies on the timeline.
    pub range: Range,
}

/// The laps of a pass, and how many laps its final take holds when the pass ran past
/// [`MAX_LAPS`] (0 when it did not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Laps {
    pub laps: Vec<Lap>,
    pub merged: i64,
}

/// The laps of a pass that latched at `start`, `None` when it was not cut into laps (it played
/// straight through). With `pass`, it loop-recorded from the loop start in laps of that length.
/// Otherwise `wrap` is the loop the transport cycled over (the host's `take_cycle`): a pass that
/// started at or before its end and ran past it is cut at each wrap. `lead` (zero or less) is where
/// the first stretch starts (a pickup, #327).
///
/// The laps run through the furthest of `last` (the last offset this track holds material at) and
/// `end - 1` (`end` is the pass's end in samples since the latch, the same for every track), so
/// every track of a pass decides from the same end whether the transport wrapped (review M1).
pub fn laps(start: Samples, pass: Option<Samples>, wrap: Option<Range>, lead: Samples, last: Samples, end: Samples) -> Option<Laps> {
    let reach = last.max(end - 1);
    if let Some(len) = pass.filter(|p| *p > 0) {
        // Pickups belong to the first lap, whose take starts early; later laps are whole.
        return Some(cut((reach.max(0) / len).saturating_add(1), reach, |k| {
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
    Some(cut(((reach - first) / w.len()).saturating_add(2), reach, |k| {
        if k == 0 {
            Lap { index: 0, window: Range::new(lead, first), range: Range::new(start.saturating_add(lead), w.end) }
        } else {
            let from = first.saturating_add((k - 1).saturating_mul(w.len()));
            Lap { index: k, window: Range::new(from, from.saturating_add(w.len())), range: w }
        }
    }))
}

/// How many laps of a pass that latched at `start` are complete once it has run `elapsed`
/// samples since the latch, laps included, as [`laps`] cuts them: the first lap runs from the
/// latch to the first wrap (one lap length with `pass`, else to the end of `wrap` even when the
/// pass started inside the loop), every later lap from one wrap to the next. The host ends a cycle
/// recording when this reaches [`MAX_LAPS`] (#522).
pub fn completed(start: Samples, pass: Option<Samples>, wrap: Option<Range>, elapsed: Samples) -> i64 {
    if let Some(len) = pass.filter(|p| *p > 0) {
        return elapsed.max(0) / len;
    }
    let Some(w) = wrap.filter(|w| pass.is_none() && !w.is_empty() && start >= 0 && start < w.end) else { return 0 };
    let first = w.end - start;
    if elapsed < first { 0 } else { (elapsed - first) / w.len() + 1 }
}

/// The `total` laps of a pass that reaches offset `reach`: the first [`MAX_LAPS`] as they are,
/// and one final lap after them (the partial lap a Stop at the limit leaves, under the usual
/// partial-lap rule). Past that, the final lap holds everything to `reach`, its take lying from
/// its lap start on for as long as it holds: no lap is ever dropped.
fn cut(total: i64, reach: Samples, lap: impl Fn(i64) -> Lap) -> Laps {
    let n = total.min(MAX_LAPS + 1);
    let mut laps: Vec<Lap> = (0..n).map(lap).collect();
    let mut merged = 0;
    if total > n
        && let Some(last) = laps.last_mut()
    {
        let to = reach.saturating_add(1);
        last.range = Range::new(last.range.start, last.range.start.saturating_add(to - last.window.start));
        last.window = Range::new(last.window.start, to);
        merged = total - MAX_LAPS;
    }
    Laps { laps, merged }
}

/// The note for a pass whose final take holds its last `merged` laps, past [`MAX_LAPS`].
pub fn over_limit_note(merged: i64) -> String {
    format!("{OVER_LIMIT_NOTE} {MAX_LAPS} laps: its last {merged} laps are one longer final take.")
}

/// The note for a last lap of `frames` frames (`seconds` long) at the end of the take file `file`
/// that is no take, being shorter than an eighth of the loop (#634): the frames stay in the file.
pub fn short_lap_note(frames: Samples, seconds: f64, file: &str) -> String {
    format!("{SHORT_LAP_NOTE}: the last {frames} frames ({:.1} ms) stay in {file}.", seconds * 1000.0)
}

#[cfg(test)]
#[path = "tests_take_laps.rs"]
mod tests;
