//! The Sampler's pure slicing logic (ADR 0041 "Slice sources and Slice to MIDI", with its
//! amendments): beat-grid boundaries, the edits of one marker set and the zero-crossing snap.
//! Nothing here reads files, touches the engine or knows about the UI; commands call these and
//! report the results.
//!
//! - **Grid:** [`grid_boundaries`] places the boundary `start + round(k * step)` for each k,
//!   from the exact division of the step, never by adding rounded steps.
//! - **Markers:** a set's first marker is the fixed loop start S; the loop end E is a boundary,
//!   not a marker. Every edit checks before it changes anything, so a refused edit leaves the
//!   set exactly as it was.
//! - **Snap:** [`snap_to_zero_crossing`] searches ±5 ms of source frames inside the legal
//!   interval the caller passes; the caller rounds a pointer position with [`round_request`]
//!   and clamps a move with [`Markers::legal_move`] first.

use crate::sampler_patch::{Division, MAX_LABEL_BYTES, MAX_SLICES, Marker, Markers};

/// The text a refused 65th slice shows in the status line.
pub const FULL_TEXT: &str = "Cannot add slice: 64 slices per loop";

impl Division {
    /// Divisions per whole note (4, 8, 16 or 32).
    #[must_use]
    pub fn per_whole(self) -> u32 {
        match self {
            Division::Quarter => 4,
            Division::Eighth => 8,
            Division::Sixteenth => 16,
            Division::ThirtySecond => 32,
        }
    }
}

/// A beat grid: every boundary from S to E, both included, and the tempo it used.
#[derive(Debug, Clone, PartialEq)]
pub struct GridResult {
    /// Strictly increasing source frames: the slice starts, then the loop end.
    pub boundaries: Vec<u64>,
    /// The grid's BPM: the passed one (From tempo), or the one a bar count derives.
    pub bpm: f64,
}

/// Why a grid cannot be made.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GridError {
    #[error("the loop is empty")]
    EmptyRegion,
    #[error("the source rate is zero")]
    ZeroRate,
    #[error("the grid tempo must be a finite number above 0 BPM")]
    BadTempo,
    #[error("bars must be from 1 to 16")]
    BadBars,
    #[error("the meter's numerator and denominator must be above 0")]
    BadMeter,
    #[error("the grid makes more than {MAX_SLICES} slices: choose a coarser division")]
    OverCapacity,
}

/// The grid over `[start, end)` at source rate `rate`, one boundary per `division`.
///
/// `bars: None` (From tempo) uses `bpm`. `bars: Some(n)`, 1 to 16, makes n bars of `meter`
/// (numerator u, denominator v) span the loop: `B = n * u * 4/v` quarter notes and the BPM is
/// `60 * B * rate / (end - start)`; `bpm` is then unused. Each boundary is
/// `start + round(k * step)`, ties upward, with the step taken from its exact ratio; duplicates
/// (a step under one frame) are dropped, a partial last division is kept and `end` is appended.
/// More than [`MAX_SLICES`] slices is refused, never truncated.
pub fn grid_boundaries(
    start: u64,
    end: u64,
    rate: u32,
    bpm: f64,
    division: Division,
    meter: (u32, u32),
    bars: Option<u32>,
) -> Result<GridResult, GridError> {
    if start >= end {
        return Err(GridError::EmptyRegion);
    }
    if rate == 0 {
        return Err(GridError::ZeroRate);
    }
    let len = end - start;
    let d = u128::from(division.per_whole());
    // The step in frames as a ratio, offset(k) = round(k * num / den).
    let (step, bpm) = match bars {
        Some(n) => {
            if !(1..=16).contains(&n) {
                return Err(GridError::BadBars);
            }
            let (u, v) = meter;
            if u == 0 || v == 0 {
                return Err(GridError::BadMeter);
            }
            // step = 4 quarters / d at the derived tempo = len * v / (n * u * d), exactly.
            let quarters = f64::from(n) * f64::from(u) * 4.0 / f64::from(v);
            let derived = 60.0 * quarters * f64::from(rate) / len as f64;
            (Step::Exact { num: u128::from(len) * u128::from(v), den: u128::from(n) * u128::from(u) * d }, derived)
        }
        None => {
            if !(bpm.is_finite() && bpm > 0.0) {
                return Err(GridError::BadTempo);
            }
            // step = rate * 60 / bpm * 4 / d = rate * 240 / (bpm * d).
            (Step::Tempo { num: f64::from(rate) * 240.0, den: bpm * d as f64 }, bpm)
        }
    };
    let mut boundaries = Vec::new();
    if step.under_one_frame() {
        // Consecutive offsets then differ by 0 or 1, so every frame of the loop is a start.
        if len > MAX_SLICES as u64 {
            return Err(GridError::OverCapacity);
        }
        boundaries.extend(start..end);
    } else {
        for k in 0u64.. {
            let Some(offset) = step.offset(k).filter(|&o| o < len) else { break };
            if boundaries.len() == MAX_SLICES {
                return Err(GridError::OverCapacity);
            }
            boundaries.push(start + offset);
        }
    }
    boundaries.push(end);
    Ok(GridResult { boundaries, bpm })
}

/// A grid step in frames.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `num / den` in integers (a bar count).
    Exact { num: u128, den: u128 },
    /// `num / den` with a tempo that is not an integer ratio: one division per boundary.
    Tempo { num: f64, den: f64 },
}

impl Step {
    fn under_one_frame(self) -> bool {
        match self {
            Step::Exact { num, den } => num < den,
            Step::Tempo { num, den } => num / den < 1.0,
        }
    }

    /// `round(k * step)`, ties upward (`None` past the frame range).
    fn offset(self, k: u64) -> Option<u64> {
        match self {
            Step::Exact { num, den } => {
                let n = u128::from(k).checked_mul(num)?;
                let q = n / den;
                let r = n % den;
                let q = if r * 2 >= den { q + 1 } else { q };
                u64::try_from(q).ok()
            }
            Step::Tempo { num, den } => {
                let x = k as f64 * num / den;
                let r = round_half_up(x);
                (r < u64::MAX as f64).then_some(r as u64)
            }
        }
    }
}

/// `x` rounded to the nearest integer, ties upward (`x` finite and not negative).
fn round_half_up(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// A pointer position in fractional source frames rounded to the nearest frame, ties upward.
/// A negative or non-finite position is frame 0; one past the range saturates.
#[must_use]
pub fn round_request(x: f64) -> u64 {
    if !(x.is_finite() && x > 0.0) {
        return 0;
    }
    let r = round_half_up(x);
    if r >= u64::MAX as f64 { u64::MAX } else { r as u64 }
}

/// Why a marker edit is refused. A refused edit changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MarkerError {
    /// A move or remove of S, the first marker.
    #[error("The loop start cannot move.")]
    Fixed,
    /// An add at or past E, or before S.
    #[error("Cannot add slice: outside the loop")]
    OutsideLoop,
    /// An add on an existing marker (S included): it is never merged or moved.
    #[error("Cannot add slice: a marker is already there")]
    OnMarker,
    /// The 65th slice (an add, or a copy of more than 64 starts).
    #[error("{FULL_TEXT}")]
    Full,
    /// No marker in the set has this id.
    #[error("No slice {0} in this set")]
    NoMarker(u32),
    /// A label over [`MAX_LABEL_BYTES`].
    #[error("The label is too long")]
    LabelTooLong,
    /// The set does not start at S, or its markers are not strictly increasing inside the loop.
    #[error("The marker set does not match its loop")]
    Malformed,
    /// Every id has been used.
    #[error("No slice ids are left in this set")]
    IdsExhausted,
}

/// Where a move was stopped by its legal interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClampedAt {
    /// Next to marker `index` (1-based, S is marker 1), with id `id`.
    Marker { index: usize, id: u32 },
    /// Next to the loop end E.
    End,
}

impl std::fmt::Display for ClampedAt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClampedAt::Marker { index, .. } => write!(f, "clamped at marker {index}"),
            ClampedAt::End => f.write_str("clamped at the loop end"),
        }
    }
}

/// A move's frame after its clamp, and the neighbor that stopped it (`None`: not clamped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Moved {
    pub frame: u64,
    pub clamped: Option<ClampedAt>,
}

/// A removed marker: its id (its slice's binding is freed and reported) and the id of the
/// preceding marker, whose slice now runs to the next boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Removed {
    pub id: u32,
    pub merged_into: u32,
}

impl Markers {
    /// A set holding only S at `start`, with a new id.
    #[must_use]
    pub fn start_only(start: u64) -> Markers {
        Markers { markers: vec![Marker { id: 1, frame: start, ..Marker::default() }], next_id: 2, ..Markers::default() }
    }

    /// The set's boundaries over `[start, end)`: the starts, then `end` (an empty set is S only).
    #[must_use]
    pub fn boundaries(&self, start: u64, end: u64) -> Vec<u64> {
        let mut b: Vec<u64> = self.markers.iter().map(|m| m.frame).collect();
        if b.is_empty() {
            b.push(start);
        }
        b.push(end);
        b
    }

    /// The legal interval `[lo, hi]` for an add near `frame`: one frame inside each neighbor.
    /// The caller snaps inside it; the snapped frame is then added with [`Markers::add`].
    pub fn legal_add(&self, start: u64, end: u64, frame: u64) -> Result<(u64, u64), MarkerError> {
        self.check(start, end)?;
        if frame <= start {
            return Err(if frame == start { MarkerError::OnMarker } else { MarkerError::OutsideLoop });
        }
        if frame >= end {
            return Err(MarkerError::OutsideLoop);
        }
        let at = self.markers.partition_point(|m| m.frame < frame);
        if self.markers.get(at).is_some_and(|m| m.frame == frame) {
            return Err(MarkerError::OnMarker);
        }
        let lo = at.checked_sub(1).and_then(|i| self.markers.get(i)).map_or(start, |m| m.frame) + 1;
        let hi = self.markers.get(at).map_or(end, |m| m.frame) - 1;
        Ok((lo, hi))
    }

    /// Adds a marker at `frame` inside `(start, end)` and returns its new id. Refuses S, E and
    /// anything outside, an existing marker's frame and the 65th slice.
    pub fn add(&mut self, start: u64, end: u64, frame: u64) -> Result<u32, MarkerError> {
        self.legal_add(start, end, frame)?;
        if self.markers.len().max(1) >= MAX_SLICES {
            return Err(MarkerError::Full);
        }
        let mut next = self.clone();
        next.materialize_start(start)?;
        let id = next.allocate()?;
        let at = next.markers.partition_point(|m| m.frame < frame);
        next.markers.insert(at, Marker { id, frame, ..Marker::default() });
        *self = next;
        Ok(id)
    }

    /// The legal interval `[lo, hi]` for moving marker `id`: one frame inside its neighbors (the
    /// loop end E for the last). S refuses as [`MarkerError::Fixed`].
    pub fn legal_move(&self, start: u64, end: u64, id: u32) -> Result<(u64, u64), MarkerError> {
        self.check(start, end)?;
        let i = self.index_of(id)?;
        if i == 0 {
            return Err(MarkerError::Fixed);
        }
        let lo = self.markers.get(i - 1).map_or(start, |m| m.frame) + 1;
        let hi = self.markers.get(i + 1).map_or(end, |m| m.frame) - 1;
        Ok((lo, hi))
    }

    /// Moves marker `id` toward `frame`, clamped to its legal interval, and says whether and
    /// where it was clamped. The id and the order are kept; S refuses.
    pub fn move_to(&mut self, start: u64, end: u64, id: u32, frame: u64) -> Result<Moved, MarkerError> {
        let (lo, hi) = self.legal_move(start, end, id)?;
        let i = self.index_of(id)?;
        let (frame, clamped) = if frame < lo {
            (lo, self.markers.get(i - 1).map(|m| ClampedAt::Marker { index: i, id: m.id }))
        } else if frame > hi {
            let next = self.markers.get(i + 1).map(|m| ClampedAt::Marker { index: i + 2, id: m.id });
            (hi, Some(next.unwrap_or(ClampedAt::End)))
        } else {
            (frame, None)
        };
        if let Some(m) = self.markers.get_mut(i) {
            m.frame = frame;
        }
        Ok(Moved { frame, clamped })
    }

    /// Removes marker `id`: its slice merges into the preceding one. S refuses.
    pub fn remove(&mut self, start: u64, end: u64, id: u32) -> Result<Removed, MarkerError> {
        self.check(start, end)?;
        let i = self.index_of(id)?;
        if i == 0 {
            return Err(MarkerError::Fixed);
        }
        let merged_into = self.markers.get(i - 1).map_or(0, |m| m.id);
        self.markers.remove(i);
        Ok(Removed { id, merged_into })
    }

    /// Sets or clears marker `id`'s label (S's included).
    pub fn relabel(&mut self, id: u32, label: Option<String>) -> Result<(), MarkerError> {
        let i = self.index_of(id)?;
        if label.as_ref().is_some_and(|l| l.len() > MAX_LABEL_BYTES) {
            return Err(MarkerError::LabelTooLong);
        }
        if let Some(m) = self.markers.get_mut(i) {
            m.label = label;
        }
        Ok(())
    }

    /// Clear: leaves only S (keeping its id and label), one slice over the whole loop, and
    /// returns the removed ids in order.
    pub fn clear_to_start(&mut self, start: u64, end: u64) -> Result<Vec<u32>, MarkerError> {
        self.check(start, end)?;
        let mut next = self.clone();
        next.materialize_start(start)?;
        let removed = next.markers.drain(1..).map(|m| m.id).collect();
        *self = next;
        Ok(removed)
    }

    /// Replaces this set with a copy of `other`'s positions and labels under NEW ids from this
    /// set, and returns this set's previous ids (all removed; their bindings are freed). Refuses
    /// a source that does not fit the loop or holds more than [`MAX_SLICES`] starts.
    pub fn copy_from(&mut self, start: u64, end: u64, other: &Markers) -> Result<Vec<u32>, MarkerError> {
        self.check(start, end)?;
        other.check(start, end)?;
        if other.markers.len() > MAX_SLICES {
            return Err(MarkerError::Full);
        }
        let mut next = self.clone();
        let removed: Vec<u32> = next.markers.drain(..).map(|m| m.id).collect();
        let frames: Vec<(u64, Option<String>)> =
            if other.markers.is_empty() { vec![(start, None)] } else { other.markers.iter().map(|m| (m.frame, m.label.clone())).collect() };
        for (frame, label) in frames {
            let id = next.allocate()?;
            next.markers.push(Marker { id, frame, label, ..Marker::default() });
        }
        *self = next;
        Ok(removed)
    }

    /// Checks the set against its loop: empty (S only), or starting at S and strictly increasing
    /// inside `[start, end)`.
    fn check(&self, start: u64, end: u64) -> Result<(), MarkerError> {
        if start >= end {
            return Err(MarkerError::Malformed);
        }
        if self.markers.first().is_some_and(|m| m.frame != start) {
            return Err(MarkerError::Malformed);
        }
        let mut last: Option<u64> = None;
        for m in &self.markers {
            if m.frame >= end || last.is_some_and(|f| m.frame <= f) {
                return Err(MarkerError::Malformed);
            }
            last = Some(m.frame);
        }
        Ok(())
    }

    fn index_of(&self, id: u32) -> Result<usize, MarkerError> {
        self.markers.iter().position(|m| m.id == id).ok_or(MarkerError::NoMarker(id))
    }

    /// Stores S explicitly in an empty set (S only by default), with a new id.
    fn materialize_start(&mut self, start: u64) -> Result<(), MarkerError> {
        if self.markers.is_empty() {
            let id = self.allocate()?;
            self.markers.push(Marker { id, frame: start, ..Marker::default() });
        }
        Ok(())
    }

    /// The next id: above every id present and every id handed out before (ids start at 1).
    fn allocate(&mut self) -> Result<u32, MarkerError> {
        let above = self.markers.iter().map(|m| m.id).max().map_or(Some(1), |m| m.checked_add(1));
        let id = above.ok_or(MarkerError::IdsExhausted)?.max(self.next_id).max(1);
        self.next_id = id.checked_add(1).ok_or(MarkerError::IdsExhausted)?;
        Ok(id)
    }
}

/// Why a snapped edit cannot snap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SnapError {
    /// No source data, or not enough to cover the search: the edit is refused with a status
    /// message instead of pretending to snap.
    #[error("The source audio is not available to snap to.")]
    NoSource,
    /// The legal interval is empty or does not hold the requested frame (clamp first).
    #[error("The requested frame is outside its legal interval")]
    OutsideInterval,
}

/// The zero crossing nearest `requested` within ±5 ms (`floor(rate * 0.005)` frames) and inside
/// `[lo, hi]`, ties to the earlier frame; `requested` when there is none.
///
/// A crossing is frame n where samples n-1 and n change sign in ANY channel (zero, and NaN,
/// count as nonnegative); channels are never summed. `requested` must lie in `[lo, hi]` (a move
/// is clamped before it snaps), and every channel must cover the frames the search reads.
pub fn snap_to_zero_crossing(channels: &[&[f32]], requested: u64, rate: u32, lo: u64, hi: u64) -> Result<u64, SnapError> {
    if !(lo <= requested && requested <= hi) {
        return Err(SnapError::OutsideInterval);
    }
    let radius = u64::from(rate / 200);
    let first = requested.saturating_sub(radius).max(lo);
    let last = requested.saturating_add(radius).min(hi);
    let need = usize::try_from(last).ok().and_then(|l| l.checked_add(1)).ok_or(SnapError::NoSource)?;
    if channels.is_empty() || channels.iter().any(|c| c.len() < need) {
        return Err(SnapError::NoSource);
    }
    let crossing = |n: u64| {
        n >= 1
            && usize::try_from(n).is_ok_and(|n| {
                channels.iter().any(|c| match (c.get(n - 1), c.get(n)) {
                    (Some(a), Some(b)) => (*a < 0.0) != (*b < 0.0),
                    _ => false,
                })
            })
    };
    for d in 0..=radius {
        if let Some(n) = requested.checked_sub(d).filter(|&n| n >= first)
            && crossing(n)
        {
            return Ok(n);
        }
        if let Some(n) = requested.checked_add(d).filter(|&n| n <= last)
            && crossing(n)
        {
            return Ok(n);
        }
    }
    Ok(requested)
}

#[cfg(test)]
#[path = "slicing_tests.rs"]
mod tests;
