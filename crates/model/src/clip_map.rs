//! One clip-to-source mapping (ADR 0056 decision 5): an audio clip reads source position
//! `x(c) = offset + s·(c − anchor)` at content time `c` (the clip time after the loop wrap), with
//! `s` the clip's signed effective rate. `offset` is the source frame read at clip time `anchor`.
//! Every edit that moves a clip's window over its source (a start trim, a split, the copy of a time
//! range, a take cut) changes `anchor`, never `offset`, so it is exact at any rate (#554).

use crate::{Clip, ClipContent};
use transit_time::{MAX_POSITION, Range, Samples};

/// The largest anchor magnitude (decision 5.5): the material invariant bounds every anchor a clip
/// can reach by an edit to ±2^46, so a stored anchor outside it comes only from a hand-edited file.
pub const MAX_ANCHOR: Samples = 1 << 46;

/// Frames from 2^40 on are not addressable (decision 5.5's `Fa`).
const MAX_FRAMES: u64 = 1 << 40;

/// The declared mapping arithmetic (decision 5.3): `d = c − anchor` in `i64`, then `d·s` rounded once,
/// then `offset + that` rounded once; no fused multiply-add. At `s = ±1` every step is exact.
#[inline]
pub(crate) fn map_pos(offset: Samples, anchor: Samples, s: f64, c: Samples) -> f64 {
    let d = c.saturating_sub(anchor);
    let scaled = d as f64 * s;
    offset as f64 + scaled
}

/// `stretch` as playback reads it: 1 when it is not a finite positive number.
#[inline]
pub(crate) fn playable_stretch(stretch: f64) -> f64 {
    if stretch.is_finite() && stretch > 0.0 { stretch } else { 1.0 }
}

impl Clip {
    /// The clip time at which `offset` is read (0 for MIDI and for every clip in normal form).
    pub fn anchor(&self) -> Samples {
        match self.content {
            ClipContent::Audio { anchor, .. } => anchor,
            ClipContent::Midi { .. } => 0,
        }
    }

    /// The signed effective rate `s` (decision 5.2): source frames per sample of clip time,
    /// `q = r / stretch` with no transpose yet (`r = 1`), one `f64` division. Never reversed yet.
    pub fn rate(&self) -> f64 {
        1.0 / playable_stretch(self.stretch)
    }

    /// The source position this clip reads at content time `c` (decision 5.3), the clip time after
    /// [`Clip::content_pos`]. An integer at `q = 1`; 0 for MIDI.
    pub fn source_pos(&self, c: Samples) -> f64 {
        match self.content {
            ClipContent::Audio { offset, anchor, .. } => map_pos(offset, anchor, self.rate(), c),
            ClipContent::Midi { .. } => 0.0,
        }
    }

    /// [`Clip::source_pos`] at clip time `p`: the loop wrap, then the mapping.
    pub fn source_pos_at(&self, p: Samples) -> f64 {
        self.source_pos(self.content_pos(p))
    }

    /// The content times this clip addresses (decision 5.5), first and last inclusive:
    /// `[0, length − 1]` unlooped, `[min(0, ls), le − 1]` looped.
    pub fn addressed(&self) -> (Samples, Samples) {
        match self.looping.filter(|l| l.is_valid()) {
            Some(l) => (l.start.min(0), l.end - 1),
            None => (0, self.length - 1),
        }
    }

    /// The content-time interval whose positions, computed with the declared arithmetic, lie in a
    /// source of `frames` frames (decision 5.6): every `c` in it has `x(c)` in `[0, Fa − 1]`, with
    /// `Fa = min(frames, 2^40)`. Empty for an empty source or a MIDI clip. The computed position is
    /// monotone in `c`, so the interval is one piece; each edge comes from the quotient and is then
    /// verified and corrected by at most two steps (decision 7.3's argument).
    pub fn clip_span_in_file(&self, frames: u64) -> Range {
        let empty = Range { start: 0, end: 0 };
        let ClipContent::Audio { offset, anchor, .. } = self.content else { return empty };
        let fa = frames.min(MAX_FRAMES);
        if fa == 0 {
            return empty;
        }
        let s = self.rate();
        let last = (fa - 1) as f64;
        let inside = |c: Samples| (0.0..=last).contains(&map_pos(offset, anchor, s, c));
        // The first c at or after which x is past `edge` going in the direction of travel.
        let quotient = |edge: f64| anchor as f64 + (edge - offset as f64) / s;
        let (lo_edge, hi_edge) = if s > 0.0 { (0.0, last) } else { (last, 0.0) };
        let clamp = |v: f64| v.clamp(-((1u64 << 52) as f64), (1u64 << 52) as f64) as Samples;
        let mut lo = clamp(quotient(lo_edge).ceil());
        let mut hi = clamp(quotient(hi_edge).floor());
        let mut steps = 0;
        while steps < 4 && inside(lo.saturating_sub(1)) {
            lo -= 1;
            steps += 1;
        }
        while steps < 4 && !inside(lo) && lo <= hi {
            lo += 1;
            steps += 1;
        }
        let mut steps = 0;
        while steps < 4 && inside(hi.saturating_add(1)) {
            hi += 1;
            steps += 1;
        }
        while steps < 4 && !inside(hi) && hi >= lo {
            hi -= 1;
            steps += 1;
        }
        if lo > hi || !inside(lo) {
            return empty;
        }
        debug_assert!(!inside(lo.saturating_sub(1)) && !inside(hi.saturating_add(1)), "the span's edges are exact");
        Range { start: lo, end: hi + 1 }
    }

    /// The `(offset, anchor)` of the clip's canonical form (decision 5.3), without copying the clip:
    /// at `q = 1` the offset `x(0)` and anchor 0, otherwise the stored pair. `None` for MIDI. The audio
    /// thread calls it (through `StreamMap::of`), so it allocates nothing.
    pub fn canonical_mapping(&self) -> Option<(Samples, Samples)> {
        let ClipContent::Audio { offset, anchor, .. } = self.content else { return None };
        Some(if self.rate() == 1.0 { (offset.saturating_sub(anchor), 0) } else { (offset, anchor) })
    }

    /// Store the clip in normal form (decision 5.3): at `q = 1` the offset becomes `x(0)` and the
    /// anchor 0, the same positions. A clip at any other rate is left as it is.
    pub fn normalize(&mut self) {
        let Some((o, a)) = self.canonical_mapping() else { return };
        if let ClipContent::Audio { offset, anchor, .. } = &mut self.content {
            (*offset, *anchor) = (o, a);
        }
    }

    /// The canonical form (decision 5.3): the normal form at `q = 1`, the stored fields otherwise.
    /// Two clips' mappings are compared in this form, never by their raw fields.
    #[must_use]
    pub fn canonical(&self) -> Clip {
        let mut c = self.clone();
        c.normalize();
        c
    }

    /// Why this clip's stored mapping fields are outside the numeric bounds of decision 5.5
    /// (`offset` and the loop edges within ±`MAX_POSITION`, `length` in `1..=MAX_POSITION`, the
    /// anchor within ±2^46); `None` when they are inside. A command whose result has one is refused.
    pub fn domain_problem(&self) -> Option<String> {
        let ClipContent::Audio { offset, anchor, .. } = self.content else { return None };
        let within = |v: Samples| (-MAX_POSITION..=MAX_POSITION).contains(&v);
        let why = if !within(offset) {
            "its source offset"
        } else if !(-MAX_ANCHOR..=MAX_ANCHOR).contains(&anchor) {
            "its anchor"
        } else if !(1..=MAX_POSITION).contains(&self.length) {
            "its length"
        } else if self.looping.is_some_and(|l| !within(l.start) || !within(l.end)) {
            "its loop range"
        } else {
            return None;
        };
        Some(format!("clip \"{}\": {why} would be outside what a clip can hold", self.name))
    }
}

/// The first audio clip of `after` whose stored mapping is outside the numeric bounds of decision
/// 5.5 and is not the same clip, unchanged, in `before`: what makes a command's result refused
/// (checked refusal, never a clamp). A clip that was already outside them before the command (only
/// a session built in code; a loaded one is repaired) does not refuse an unrelated edit.
pub fn new_clip_domain_problem(before: &crate::Session, after: &crate::Session) -> Option<String> {
    after.all_clips().find_map(|c| {
        let why = c.domain_problem()?;
        (!before.all_clips().any(|b| b.id == c.id && b == c)).then_some(why)
    })
}

#[cfg(test)]
#[path = "clip_map_tests.rs"]
mod tests;
