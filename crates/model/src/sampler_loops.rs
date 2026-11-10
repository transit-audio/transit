//! A Sampler zone's loop as playback uses it (ADR 0041 "Loop points and crossfade reference" and
//! amendment 2026-11-08, items 1 and 2 and "Validator changes"): the region a slice-bound zone
//! derives from its slice, the loop mode and crossfade playback uses, and the crossfade's
//! millisecond conversion at the zone source's rate.
//!
//! - **The slice is the authority.** A slice-bound zone plays its bound slice in its own set: the
//!   slice's start marker to the next boundary (the last slice to the loop end E), and when it
//!   loops, over that whole region ([`derived_region`]). Stored `start`, `end`, `loop_start` and
//!   `loop_end` that disagree are accepted on load and on store and not played; the commands that
//!   bind, loop or move markers write the derived values through.
//! - **Checked when set, clamped when used.** [`loop_used`] is Off for a One-shot zone, for a loop
//!   mode Off and for an effective loop shorter than two frames (a slice shortened after its loop
//!   was set); [`crossfade_used`] is `min(crossfade, floor(L / 2))` on a Forward loop in use and 0
//!   otherwise (Ping-pong included). The edits refuse the over-limit values (their texts are the
//!   commands'); the stored values stay as they are.
//! - **Milliseconds.** The UI edits the crossfade in milliseconds with one decimal, counted in
//!   tenths here so the conversion is exact: `X = floor(m * r / 1000 + 1/2)` with r the zone
//!   source's native rate ([`crossfade_frames`]), shown as `X * 1000 / r` to one decimal
//!   ([`crossfade_tenths`], [`crossfade_ms_text`]).

use crate::sampler_patch::{LoopMode, PatchV1, PlayMode, SliceSet, ZoneEntry};

/// A zone's region and loop as playback reads them, in native frames of source `source` (an index
/// into [`PatchV1::sources`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedRegion {
    pub source: u16,
    pub start: u64,
    /// Exclusive.
    pub end: u64,
    pub loop_start: u64,
    /// Exclusive.
    pub loop_end: u64,
}

impl DerivedRegion {
    /// The loop's length L.
    #[must_use]
    pub fn loop_len(&self) -> u64 {
        self.loop_end.saturating_sub(self.loop_start)
    }
}

/// The region slice-bound zone `zone` plays, from its bound slice in `patch`: the loop's source,
/// the slice's start marker to the next boundary (the last slice to the loop end), and a loop over
/// that whole region. `None` when the zone has no slice binding or the binding names no marker.
#[must_use]
pub fn derived_region(zone: &ZoneEntry, patch: &PatchV1) -> Option<DerivedRegion> {
    let b = zone.slice.as_ref()?;
    let l = patch.slicing.as_ref()?;
    let set = match b.set {
        SliceSet::Auto => l.auto.as_ref(),
        SliceSet::Manual => l.manual.as_ref(),
    }?;
    let i = set.markers.iter().position(|m| m.id == b.id)?;
    let start = set.markers.get(i)?.frame;
    let end = set.markers.get(i + 1).map_or(l.end, |m| m.frame);
    Some(DerivedRegion { source: l.source, start, end, loop_start: start, loop_end: end })
}

/// Writes `d` into `zone`'s stored source, region and loop fields: the write-through of a
/// slice-bound zone (amendment 2026-11-08, item 1), so the stored zone describes what plays.
/// Whether any field changed.
pub fn write_region(zone: &mut ZoneEntry, d: DerivedRegion) -> bool {
    let before = (zone.source, zone.start, zone.end, zone.loop_start, zone.loop_end);
    (zone.source, zone.start, zone.end, zone.loop_start, zone.loop_end) = (d.source, d.start, d.end, d.loop_start, d.loop_end);
    before != (d.source, d.start, d.end, d.loop_start, d.loop_end)
}

/// The region `zone` plays: [`derived_region`] for a slice-bound zone, its stored fields otherwise.
#[must_use]
pub fn effective_region(zone: &ZoneEntry, patch: &PatchV1) -> DerivedRegion {
    derived_region(zone, patch).unwrap_or(DerivedRegion {
        source: zone.source,
        start: zone.start,
        end: zone.end,
        loop_start: zone.loop_start,
        loop_end: zone.loop_end,
    })
}

/// The loop mode `zone` plays: Off for a One-shot zone, for a stored mode Off and for an effective
/// loop shorter than two frames; the stored mode otherwise.
#[must_use]
pub fn loop_used(zone: &ZoneEntry, patch: &PatchV1) -> LoopMode {
    if zone.mode == PlayMode::OneShot || zone.loop_mode == LoopMode::Off || effective_region(zone, patch).loop_len() < 2 {
        return LoopMode::Off;
    }
    zone.loop_mode
}

/// The largest crossfade the edit accepts for `zone`'s effective loop, `floor(L / 2)`.
#[must_use]
pub fn crossfade_max(zone: &ZoneEntry, patch: &PatchV1) -> u64 {
    effective_region(zone, patch).loop_len() / 2
}

/// The crossfade `zone` plays: `min(crossfade, floor(L / 2))` when [`loop_used`] is Forward, 0
/// otherwise.
#[must_use]
pub fn crossfade_used(zone: &ZoneEntry, patch: &PatchV1) -> u64 {
    if loop_used(zone, patch) != LoopMode::Forward {
        return 0;
    }
    zone.crossfade.min(crossfade_max(zone, patch))
}

/// Source frames for an entry of `tenths` tenths of a millisecond at the source rate `rate` Hz:
/// `floor(m * rate / 1000 + 1/2)` with `m = tenths / 10`, ties upward, computed exactly.
#[must_use]
pub fn crossfade_frames(tenths: u64, rate: u32) -> u64 {
    let x = (u128::from(tenths) * u128::from(rate) + 5000) / 10_000;
    u64::try_from(x).unwrap_or(u64::MAX)
}

/// The milliseconds `frames` source frames last at `rate` Hz, in tenths: `frames * 1000 / rate`
/// rounded to one decimal, ties upward (0 at rate 0).
#[must_use]
pub fn crossfade_tenths(frames: u64, rate: u32) -> u64 {
    if rate == 0 {
        return 0;
    }
    let r = u128::from(rate);
    let x = (u128::from(frames) * 20_000 + r) / (2 * r);
    u64::try_from(x).unwrap_or(u64::MAX)
}

/// The field's text for `frames` at `rate` Hz: milliseconds with one decimal (`4.7`).
#[must_use]
pub fn crossfade_ms_text(frames: u64, rate: u32) -> String {
    let t = crossfade_tenths(frames, rate);
    format!("{}.{}", t / 10, t % 10)
}

/// A typed millisecond value in tenths, rounded to one decimal with ties upward (`None`: negative
/// or not finite).
#[must_use]
pub fn tenths_of_ms(ms: f64) -> Option<u64> {
    if !ms.is_finite() || ms < 0.0 {
        return None;
    }
    let t = (ms * 10.0 + 0.5).floor();
    (t < u64::MAX as f64).then_some(t as u64)
}

impl PatchV1 {
    /// [`loop_used`] of zone `id`.
    #[must_use]
    pub fn loop_used(&self, id: u16) -> Option<LoopMode> {
        self.zone(id).map(|z| loop_used(z, self))
    }

    /// [`crossfade_used`] of zone `id`.
    #[must_use]
    pub fn crossfade_used(&self, id: u16) -> Option<u64> {
        self.zone(id).map(|z| crossfade_used(z, self))
    }
}

#[cfg(test)]
#[path = "sampler_loops_tests.rs"]
mod tests;
