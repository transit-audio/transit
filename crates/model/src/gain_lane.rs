//! The mixer's gain-lane evaluator (ADR 0058, decision 2): the value of a mixer gain lane or a clip
//! gain envelope at a timeline (or clip) sample, in `f64`, from breakpoints sorted stably by time.
//!
//! - **Breakpoints sharing a time** form a group. At its own sample a group gives its **first**
//!   point's value (its *incoming* value); the segment after it starts from its **last** point's
//!   value (its *outgoing* value). Before the first group the lane is the first group's first value,
//!   after the last group the last group's last value.
//! - Between consecutive group times `a < n < b`, with `u` the outgoing value at `a` and `w` the
//!   incoming value at `b`: `u + (w - u) * ((n - a) as f64 / (b - a) as f64)`, in that order. A step
//!   lane (Mute, SendMute) holds `u`. Positions are integers and the division is in `f64`, so a lane
//!   hours long has no positional rounding, and a group's own sample returns the stored value itself.
//! - **Bounded traversal.** [`GainCursor`] finds its place with one binary search and then walks
//!   forward, crossing each group in one step through a group index ([`build_group_index`]: for each
//!   point, the index of the last point sharing its time), so the work per sample is at most one group
//!   crossing however many points share a time. The index changes cost, never values:
//!   [`gain_value_at`] needs none and returns the same value.
//!
//! **Precondition:** breakpoints sorted by time ([`is_sorted_by_time`]). Lanes are (`Session::sanitize`
//! sorts them stably); clip envelopes are kept as written, so the plan compiler builds no index for an
//! unsorted one ([`fill_group_index`] refuses it) and a debug build asserts the order where the
//! evaluator interpolates.
//!
//! Device-parameter lanes keep [`crate::AutomationLane::value_at`]; nothing in the mixer calls this
//! module before ADR 0058 slice G1 (lanes) and G3 (clip envelopes). Allocation-free except
//! [`build_group_index`], which runs off the audio thread (the plan compiler).

use crate::AutomationPoint;
use transit_time::Samples;

/// A breakpoint the gain evaluator reads: an automation lane's point, or a clip gain envelope's
/// `(offset from the clip start, dB)` pair.
pub trait GainPoint {
    /// The breakpoint's position.
    fn at(&self) -> Samples;
    /// Its stored value (widened exactly to `f64` by the evaluator).
    fn value(&self) -> f32;
}

impl GainPoint for AutomationPoint {
    #[inline]
    fn at(&self) -> Samples {
        self.at
    }
    #[inline]
    fn value(&self) -> f32 {
        self.value
    }
}

impl GainPoint for (Samples, f32) {
    #[inline]
    fn at(&self) -> Samples {
        self.0
    }
    #[inline]
    fn value(&self) -> f32 {
        self.1
    }
}

/// The bytes a group index for `points` breakpoints takes (one `u32` per point), or `None` when a
/// `u32` cannot index that many points (the plan refuses such a lane) or the count overflows.
#[must_use]
pub fn group_index_bytes(points: usize) -> Option<usize> {
    u32::try_from(points.saturating_sub(1)).ok()?;
    points.checked_mul(size_of::<u32>())
}

/// True when `points` are sorted by time (equal times allowed): the precondition of every function in
/// this module. Lanes are sorted by `Session::sanitize`; clip envelopes are kept as written, so a file
/// can hold an unsorted one.
#[must_use]
pub fn is_sorted_by_time<P: GainPoint>(points: &[P]) -> bool {
    points.iter().zip(points.iter().skip(1)).all(|(p, q)| p.at() <= q.at())
}

/// The group index of `points` (sorted by time): for each point, the index of the last point that
/// shares its time. `None` when a `u32` cannot index the points or they are not sorted by time (no
/// meaningless index is ever built). Allocates: off the audio thread only.
#[must_use]
pub fn build_group_index<P: GainPoint>(points: &[P]) -> Option<Box<[u32]>> {
    group_index_bytes(points.len())?;
    let mut out = vec![0u32; points.len()];
    fill_group_index(points, &mut out).then_some(out.into_boxed_slice())
}

/// [`build_group_index`] into storage the caller allocated (the plan compiler's one table): writes
/// `points.len()` entries to the front of `out`. False, writing nothing, when `out` is too short, a
/// `u32` cannot index the points, or they are not sorted by time. Allocates nothing.
pub fn fill_group_index<P: GainPoint>(points: &[P], out: &mut [u32]) -> bool {
    if group_index_bytes(points.len()).is_none() || !is_sorted_by_time(points) {
        return false;
    }
    let Some(out) = out.get_mut(..points.len()) else { return false };
    let mut last = 0u32;
    for (i, slot) in out.iter_mut().enumerate().rev() {
        let starts_group_end = match (points.get(i), points.get(i + 1)) {
            (Some(p), Some(q)) => p.at() != q.at(),
            _ => true,
        };
        if starts_group_end {
            // In range: the point count was checked against a u32 above.
            last = u32::try_from(i).unwrap_or(u32::MAX);
        }
        *slot = last;
    }
    true
}

/// A forward cursor over one lane's breakpoints (ADR 0058, decision 2: bounded traversal). `next` is
/// the first point at or after the cursor's position, which is always the first point of its group;
/// the point before it is the last point of the previous group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GainCursor {
    next: usize,
}

impl GainCursor {
    /// A cursor positioned at `at`: one binary search.
    #[must_use]
    pub fn seek<P: GainPoint>(points: &[P], at: Samples) -> Self {
        GainCursor { next: points.partition_point(|p| p.at() < at) }
    }

    /// Moves the cursor forward to `at` (never backward: an earlier `at` leaves it where it is) and
    /// returns the steps it took. With the lane's group index each group is one step, so moving one
    /// sample takes at most one; without it a group is walked point by point. A malformed index (too
    /// short, or pointing backward) falls back to single steps.
    pub fn advance<P: GainPoint>(&mut self, points: &[P], index: Option<&[u32]>, at: Samples) -> usize {
        let mut steps = 0;
        while let Some(p) = points.get(self.next)
            && p.at() < at
        {
            let last = index.and_then(|ix| ix.get(self.next)).and_then(|&l| usize::try_from(l).ok()).unwrap_or(self.next);
            self.next = last.max(self.next).saturating_add(1);
            steps += 1;
        }
        steps
    }

    /// The lane's value at `at`, for a cursor positioned there ([`Self::seek`] or [`Self::advance`]
    /// to `at`); `None` for an empty lane. `step` holds the outgoing value between groups (Mute,
    /// SendMute). Precondition: `points` sorted by time ([`is_sorted_by_time`]); a debug build checks
    /// that the two points it interpolates between bracket `at`.
    #[must_use]
    pub fn value<P: GainPoint>(&self, points: &[P], at: Samples, step: bool) -> Option<f64> {
        let next = points.get(self.next);
        if let Some(w) = next
            && w.at() == at
        {
            return Some(f64::from(w.value()));
        }
        let left = self.next.checked_sub(1).and_then(|i| points.get(i));
        match (left, next) {
            (None, None) => None,
            (None, Some(w)) => Some(f64::from(w.value())),
            (Some(u), None) => Some(f64::from(u.value())),
            (Some(u), Some(w)) => {
                let (a, b) = (u.at(), w.at());
                debug_assert!(a < at && at < b, "gain lane points not sorted by time, or the cursor is not at {at}");
                let u = f64::from(u.value());
                if step {
                    return Some(u);
                }
                let w = f64::from(w.value());
                let elapsed = (i128::from(at) - i128::from(a)) as f64;
                let span = (i128::from(b) - i128::from(a)) as f64;
                Some(u + (w - u) * (elapsed / span))
            }
        }
    }
}

/// The value of a gain lane or clip envelope at `at` (ADR 0058, decision 2), `None` when it has no
/// points. One binary search; no group index needed (the same value [`GainCursor`] gives).
#[must_use]
pub fn gain_value_at<P: GainPoint>(points: &[P], at: Samples, step: bool) -> Option<f64> {
    GainCursor::seek(points, at).value(points, at, step)
}

/// The incoming and outgoing values `(first, last)` of the group at exactly `at`, or `None` when no
/// breakpoint sits there; equal for a single point. A consumer that interpolates a quantity derived
/// from the lane between group times (ADR 0058, decision 10's pan knots) uses the outgoing value at an
/// interval's left end and the incoming value at its right end. With the lane's group index the last
/// member is one lookup; without it, one more binary search.
#[must_use]
pub fn group_values_at<P: GainPoint>(points: &[P], at: Samples, index: Option<&[u32]>) -> Option<(f64, f64)> {
    let first = points.partition_point(|p| p.at() < at);
    let head = points.get(first).filter(|p| p.at() == at)?;
    let last = match index.and_then(|ix| ix.get(first)).and_then(|&l| usize::try_from(l).ok()) {
        Some(l) => l,
        None => points.partition_point(|p| p.at() <= at).saturating_sub(1),
    };
    let tail = points.get(last).filter(|p| p.at() == at)?;
    Some((f64::from(head.value()), f64::from(tail.value())))
}

#[cfg(test)]
#[path = "gain_lane_tests.rs"]
mod tests;
