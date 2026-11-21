//! The live input aligned from the output converter's coordinates (#709; ADR 0015, Amendment
//! 2026-11-16 on the converter term, rules 1 to 4 and 9).
//!
//! On a paired device (one callback delivers the capture block and then asks for the render block,
//! the ring built with `capture_precedes_render` true) the newest captured input frame and the
//! callback's last output frame `n` are the same device instant. A plan block's frame `j`, whose
//! callback's last output frame sits at render position `x` of that block, reads the input at device
//! position `n + (j − x − T)/q`, `q = S/D` and `T` the converter term
//! ([`transit_model::converter_term`]). Every round trip through the two converters then gains
//! exactly `T` plan frames, at every phase, buffer and attachment, because the position is computed
//! from the coordinates for every block instead of continuing from where the last read stopped.
//!
//! Positions are exact integers in thousandths of a session frame (the output converter's
//! denominator is the device rate times 1000). The ring keeps the input from each read's first tap
//! onward. A read whose taps are not all queued (history a clear, a loss or an empty ring did not
//! keep; input not captured yet) reads counted silence while the coordinates advance: it never holds
//! or shifts the alignment to wait for samples (rule 9).
//!
//! The pairing of capture and output frame counts is the anchor: set at the first aligned read that
//! finds input, checked at every read, and invalidated by a capture loss (a push that dropped frames,
//! or a cycle whose capture did not arrive). It is re-established from the next callback's pair. A
//! read without a valid anchor, or while the output converter's alignment is not valid (rule 10), is
//! the ring's ordinary read and reports [`INVALID`]. No allocation, lock or wait: integer arithmetic
//! and the band-limited reader over the queued frames, bounded by the block.

use super::{InputRing, MonitorReader, Queued, finite};
use crate::band_limited;
use std::sync::atomic::Ordering;

/// [`InputRing::alignment`]: no read with coordinates yet, an unpaired ring, or no input captured.
pub(super) const NONE: u8 = 0;
/// The last read was aligned from the output converter's coordinates with the term in force.
pub(super) const ALIGNED: u8 = 1;
/// The last read had coordinates but its alignment was not valid (rule 10, or a capture loss not
/// re-anchored yet): the ordinary read.
pub(super) const INVALID: u8 = 2;

/// The output converter's coordinates for one plan block (rule 3), from the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coordinates {
    /// The output frame, counted from the output stream's start, of the device callback's last
    /// output frame (`n`).
    pub output: u64,
    /// That output frame's render position from the block's frame 0 (`x`), in units of
    /// `1 / den` render frames.
    pub units: i64,
    /// Units per render frame: the output converter's denominator, the device rate times 1000.
    pub den: i64,
    /// The converter term `T` in plan frames.
    pub term: u32,
    /// The output converter's alignment is valid: speed 1 since a converter restart at speed 1
    /// (rule 10).
    pub valid: bool,
}

/// Why a read with coordinates was not aligned.
enum Unaligned {
    /// No term can apply (another device's coordinates, no input captured yet): [`NONE`].
    NotApplicable,
    /// Rule 10 or a capture loss: [`INVALID`].
    Invalid,
}

impl InputRing {
    /// How the last read was made, for the latency lookup and the panels: [`transit_model::Alignment`].
    pub fn alignment(&self) -> transit_model::Alignment {
        match self.alignment.load(Ordering::Relaxed) {
            ALIGNED => transit_model::Alignment::Valid,
            INVALID => transit_model::Alignment::Invalid,
            _ => transit_model::Alignment::None,
        }
    }

    /// Whether capture and render share one callback ([`super::ALIGNED_BACKLOG_BLOCKS_PAIRED`]):
    /// the only rings the term applies to (rule 1).
    pub fn paired(&self) -> bool {
        self.aligned_backlog_blocks == super::ALIGNED_BACKLOG_BLOCKS_PAIRED
    }

    /// [`Self::pop_into`] with the output converter's coordinates: on a paired ring with a valid
    /// alignment and anchor, the block is read at the positions rule 3 prescribes and the result is
    /// `true` (the term is in force for this block); otherwise it is the ordinary read and `false`.
    pub fn pop_aligned(&self, out: &mut [Vec<f32>], frames: usize, session_rate: u32, at: Coordinates) -> bool {
        if !self.paired() {
            self.pop_into(out, frames, session_rate);
            return false;
        }
        let Some((mut reader, frames)) = self.begin_read(out, frames) else { return false };
        match self.anchor(&mut reader, session_rate, at) {
            Ok(newest) => {
                self.read_at(&mut reader, out, frames, session_rate, at, newest);
                self.alignment.store(ALIGNED, Ordering::Relaxed);
                true
            }
            Err(why) => {
                let state = if matches!(why, Unaligned::Invalid) { INVALID } else { NONE };
                self.alignment.store(state, Ordering::Relaxed);
                self.read_queued(&mut reader, out, frames, session_rate);
                false
            }
        }
    }

    /// Check (or set) the same-cycle anchor; the newest captured frame's index from the queue's
    /// front when the read can be aligned.
    fn anchor(&self, reader: &mut MonitorReader, session_rate: u32, at: Coordinates) -> Result<i128, Unaligned> {
        let losses = self.losses.load(Ordering::Acquire);
        if losses != reader.losses_seen {
            reader.losses_seen = losses;
            reader.anchor = None;
            reader.held = Some(at.output);
        }
        if self.rate == 0 || session_rate == 0 || at.den != 1000 * i64::from(self.rate) {
            return Err(Unaligned::NotApplicable);
        }
        // Equal rates pass samples through only at a whole-frame phase; a fraction left by another
        // speed claims no term (rule 10's stated scope).
        if !at.valid || (self.rate == session_rate && at.units.rem_euclid(at.den) != 0) {
            return Err(Unaligned::Invalid);
        }
        let accepted = self.accepted_frames.load(Ordering::Acquire);
        if accepted == 0 {
            return Err(Unaligned::NotApplicable);
        }
        // A plan adopting the ring, or a clear, re-anchors at once (K22's moment, in coordinates).
        if self.align_pending.swap(false, Ordering::AcqRel) {
            reader.anchor = None;
            reader.held = None;
        }
        let pair = i128::from(accepted) - 1 - i128::from(at.output);
        match reader.anchor {
            Some(anchor) if anchor == pair => {}
            // The counts moved apart without a counted loss (a cycle whose capture did not arrive).
            Some(_) => {
                reader.anchor = None;
                reader.held = Some(at.output);
                return Err(Unaligned::Invalid);
            }
            None if reader.held == Some(at.output) => return Err(Unaligned::Invalid),
            None => {
                reader.anchor = Some(pair);
                reader.held = None;
            }
        }
        Ok(i128::from(accepted) - 1 - i128::from(reader.removed_frames))
    }

    /// Read `frames` frames at rule 3's positions, `newest` the newest captured frame's index from
    /// the queue's front: drop what lies before the first tap, read each frame whose taps are all
    /// queued (counted silence otherwise), and keep from the next read's first tap.
    fn read_at(&self, reader: &mut MonitorReader, out: &mut [Vec<f32>], frames: usize, session_rate: u32, at: Coordinates, newest: i128) {
        let (num, den) = (1000 * i64::from(self.rate), 1000 * i64::from(session_rate));
        let q = num as f64 / den as f64;
        // Frame j's position in units of `1 / den` input frames from the queue's front is
        // newest·den + (j − T)·num − x·den·(D/S), x = units / at.den render frames; with
        // at.den = num = 1000·D (one device), x·den·(D/S) = units exactly.
        let start = newest * i128::from(den) - i128::from(at.term) * i128::from(num) - i128::from(at.units);
        let Ok(mut units) = i64::try_from(start) else {
            self.counters.underrun.fetch_add(frames as u64, Ordering::Relaxed);
            return;
        };
        let mut queued = reader.queued_frames();
        // The ring keeps the input from the first tap onward: what the last read already committed
        // in steady state, the backlog after an ordinary read or an attachment otherwise.
        let first = band_limited::span(band_limited::position(units, den), q).start;
        let drop = usize::try_from(first).unwrap_or(0).min(queued);
        if drop > 0 {
            let played = reader.history_frames().min(drop);
            reader.commit(drop);
            self.counters.discarded.fetch_add((drop - played) as u64, Ordering::Relaxed);
            units -= drop as i64 * den;
            queued -= drop;
        }
        let width = out.len().min(reader.ch);
        let mut silent = 0u64;
        {
            let MonitorReader { queue, ch, frame, table, .. } = reader;
            if let Ok(chunk) = queue.read_chunk(queued * *ch) {
                let (first, second) = chunk.as_slices();
                let source = Queued { first, second, ch: *ch, frames: queued };
                for f in 0..frames {
                    let x = band_limited::position(units + f as i64 * num, den);
                    let span = band_limited::span(x, q);
                    if span.start < 0 || span.end > queued as i64 {
                        silent += 1;
                        continue;
                    }
                    band_limited::frame(table, &source, x, q, &mut frame[..width]);
                    for (channel, &value) in out.iter_mut().zip(&frame[..width]) {
                        if let Some(sample) = channel.get_mut(f) {
                            *sample = finite(value);
                        }
                    }
                }
            } else {
                silent = frames as u64;
            }
        }
        // The next read's first tap: everything before it is never read again.
        let next = units + frames as i64 * num;
        let done = usize::try_from(band_limited::span(band_limited::position(next, den), q).start).unwrap_or(0).min(queued);
        reader.commit(done);
        // The ordinary read, should the next one be one, continues from exactly here.
        reader.units = next - done as i64 * den;
        reader.den = den;
        self.retired_frames.store(reader.retired_input(), Ordering::Release);
        self.counters.underrun.fetch_add(silent, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests;
