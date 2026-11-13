//! Group indexes for every mixer gain lane and clip gain envelope a plan reads (ADR 0058, decision 2
//! and decision 16, slice G1a): counted in [`super::measure`] before anything is allocated, built off
//! the audio thread after the plan is admitted, and held in the plan. Nothing reads them yet: slice G1
//! reads the lane indexes, G3 the envelope indexes. The indexes change the cost of crossing a group of
//! breakpoints sharing a time, never a value.

use super::*;
use transit_model::gain_lane::{fill_group_index, group_index_bytes};
use transit_model::{AutomationLane, ClipId};

/// What a group index belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexOwner {
    /// The lane at this position in its track's `automation`.
    Lane(usize),
    /// This clip's gain envelope.
    Envelope(ClipId),
}

/// One index: its owner and its place in [`GainIndexes::data`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexEntry {
    pub(crate) track: TrackId,
    pub(crate) owner: IndexOwner,
    pub(crate) start: usize,
    pub(crate) len: usize,
}

/// Every group index of a plan, in one table (one `u32` per point, offsets within each lane).
#[derive(Debug, Default)]
pub(crate) struct GainIndexes {
    pub(crate) data: Box<[u32]>,
    pub(crate) entries: Box<[IndexEntry]>,
}

impl GainIndexes {
    fn find(&self, track: TrackId, owner: IndexOwner) -> Option<&[u32]> {
        let e = self.entries.iter().find(|e| e.track == track && e.owner == owner)?;
        self.data.get(e.start..e.start.checked_add(e.len)?)
    }

    /// The index of the lane at `lane` in `track`'s automation.
    #[cfg_attr(not(test), allow(dead_code))] // Read from slice G1 on (ADR 0058).
    pub(crate) fn lane(&self, track: TrackId, lane: usize) -> Option<&[u32]> {
        self.find(track, IndexOwner::Lane(lane))
    }

    /// The index of `clip`'s gain envelope on `track`.
    #[cfg_attr(not(test), allow(dead_code))] // Read from slice G3 on (ADR 0058).
    pub(crate) fn envelope(&self, track: TrackId, clip: ClipId) -> Option<&[u32]> {
        self.find(track, IndexOwner::Envelope(clip))
    }
}

/// The lanes of `t` that get an index: every mixer gain lane with points.
fn lanes(t: &Track) -> impl Iterator<Item = (usize, &AutomationLane)> {
    t.automation.iter().enumerate().filter(|(_, l)| l.param.is_mixer_gain() && !l.points.is_empty())
}

/// The clip envelopes of `t` that get an index: every audio clip's non-empty envelope on an audio track
/// (the clips the mixer renders).
fn envelopes(t: &Track) -> impl Iterator<Item = &Clip> {
    let audio = t.kind == TrackKind::Audio;
    t.clips().iter().filter(move |c| audio && c.is_audio() && !c.gain_env.is_empty())
}

fn too_many(what: &'static str, need: usize) -> CompileError {
    CompileError::ResourceLimit { what, need, limit: usize::try_from(u64::from(u32::MAX) + 1).unwrap_or(usize::MAX) }
}

fn overflow() -> CompileError {
    CompileError::ResourceLimit { what: "prepared_bytes", need: usize::MAX, limit: usize::MAX }
}

/// The bytes `s`'s group indexes take (the table and its entries), with checked arithmetic.
/// Allocates nothing. A lane or envelope with more points than a `u32` can index refuses the plan.
pub(super) fn bytes(s: &Session) -> Result<usize, CompileError> {
    let mut points = 0usize;
    let mut entries = 0usize;
    let mut count = |n: usize| -> Result<(), CompileError> {
        group_index_bytes(n).ok_or_else(|| too_many("gain_index_points", n))?;
        points = points.checked_add(n).ok_or_else(overflow)?;
        entries = entries.checked_add(1).ok_or_else(overflow)?;
        Ok(())
    };
    for t in &s.tracks {
        for (_, l) in lanes(t) {
            count(l.points.len())?;
        }
        for c in envelopes(t) {
            count(c.gain_env.len())?;
        }
    }
    let table = points.checked_mul(size_of::<u32>()).ok_or_else(overflow)?;
    entries.checked_mul(size_of::<IndexEntry>()).and_then(|e| e.checked_add(table)).ok_or_else(overflow)
}

/// Builds `s`'s group indexes. Call only after [`bytes`] (through `measure`) admitted them: the two
/// walk the same lanes and envelopes, so what is allocated here is what was counted.
pub(super) fn build(s: &Session) -> GainIndexes {
    let total: usize =
        s.tracks.iter().map(|t| lanes(t).map(|(_, l)| l.points.len()).sum::<usize>() + envelopes(t).map(|c| c.gain_env.len()).sum::<usize>()).sum();
    let mut data = vec![0u32; total];
    let mut entries = Vec::with_capacity(s.tracks.iter().map(|t| lanes(t).count() + envelopes(t).count()).sum());
    let mut start = 0usize;
    for t in &s.tracks {
        for (i, l) in lanes(t) {
            if data.get_mut(start..).is_some_and(|out| fill_group_index(&l.points, out)) {
                entries.push(IndexEntry { track: t.id, owner: IndexOwner::Lane(i), start, len: l.points.len() });
                start += l.points.len();
            }
        }
        for c in envelopes(t) {
            if data.get_mut(start..).is_some_and(|out| fill_group_index(&c.gain_env, out)) {
                entries.push(IndexEntry { track: t.id, owner: IndexOwner::Envelope(c.id), start, len: c.gain_env.len() });
                start += c.gain_env.len();
            }
        }
    }
    GainIndexes { data: data.into_boxed_slice(), entries: entries.into_boxed_slice() }
}

#[cfg(test)]
#[path = "gain_index_tests.rs"]
mod tests;
