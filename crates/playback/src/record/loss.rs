//! What the input path put in place of input it did not have, and how the take says so (#184).
//!
//! Every stretch of a take that is silence transit (or its backend) substituted for input is
//! counted here, on the audio thread with one relaxed atomic per event (no lock, no allocation),
//! and reported at Stop with where in the take it began. Separately, the take writer measures the
//! longest stretch of exact digital silence between signal in each take ([`ZeroRun`]): that names
//! an input that went silent on its own (a device that delivered zeros while still calling back),
//! which nothing upstream can tell from real silence.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// No input frame recorded yet.
const NONE: u64 = u64::MAX;

/// Lock-free counters of substituted input for the current take. The audio thread adds; the
/// control thread resets them at arm (no producer admitted) and reads them at Stop.
pub(super) struct Losses {
    /// Input frame ordinal of the first chunk the full ring refused (its samples were owed as
    /// silence), or [`NONE`].
    overflow_at: AtomicU64,
    /// Frames the backend delivered as zeros in place of device input it lost.
    substituted: AtomicUsize,
    /// Input frame ordinal of the first such block, or [`NONE`].
    substituted_at: AtomicU64,
    /// Non-finite input samples (NaN, infinities) recorded as zero.
    nonfinite: AtomicUsize,
    /// Input frame ordinal of the take file's first frame, published by the writer once it wrote
    /// one, or [`NONE`]: turns the ordinals above into take positions.
    origin: AtomicU64,
}

impl Losses {
    pub(super) fn new() -> Losses {
        Losses {
            overflow_at: AtomicU64::new(NONE),
            substituted: AtomicUsize::new(0),
            substituted_at: AtomicU64::new(NONE),
            nonfinite: AtomicUsize::new(0),
            origin: AtomicU64::new(NONE),
        }
    }

    /// Control thread, at arm, while no producer is admitted.
    pub(super) fn reset(&self) {
        self.overflow_at.store(NONE, Ordering::Relaxed);
        self.substituted.store(0, Ordering::Relaxed);
        self.substituted_at.store(NONE, Ordering::Relaxed);
        self.nonfinite.store(0, Ordering::Relaxed);
        self.origin.store(NONE, Ordering::Relaxed);
    }

    /// Audio thread: the full ring refused input of the chunk starting at input frame `frame`.
    pub(super) fn overflow(&self, frame: u64) {
        let _ = self.overflow_at.compare_exchange(NONE, frame, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Audio thread: `frames` of zeros the backend substituted, starting at input frame `frame`.
    pub(super) fn substituted(&self, frame: u64, frames: usize) {
        self.substituted.fetch_add(frames, Ordering::Relaxed);
        let _ = self.substituted_at.compare_exchange(NONE, frame, Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Audio thread: one input sample, or zero (counted) for NaN and infinities, which would
    /// poison the take and the mix.
    pub(super) fn finite(&self, x: f32) -> f32 {
        if x.is_finite() {
            x
        } else {
            self.nonfinite.fetch_add(1, Ordering::Relaxed);
            0.0
        }
    }

    /// Writer: the take file's first frame is input frame `frame` (kept from the first call).
    pub(super) fn set_origin(&self, frame: u64) {
        let _ = self.origin.compare_exchange(NONE, frame, Ordering::Relaxed, Ordering::Relaxed);
    }

    pub(super) fn substituted_frames(&self) -> usize {
        self.substituted.load(Ordering::Relaxed)
    }

    /// Where input frame `at` lies in the take, as " from S s into the take" (" starting before the
    /// take began" when it precedes the take's first frame), or nothing when the take had no frame
    /// yet or `at` is unknown.
    fn position(&self, at: &AtomicU64, rate: u32) -> String {
        let (at, origin) = (at.load(Ordering::Relaxed), self.origin.load(Ordering::Relaxed));
        if at == NONE || origin == NONE {
            return String::new();
        }
        // Input lost before the take's first frame (a count-in or pre-roll the take does not keep).
        if at < origin {
            return " starting before the take began".into();
        }
        format!(" from {:.2} s into the take", (at - origin) as f64 / f64::from(rate.max(1)))
    }

    /// What the take lost, for the Stop message and the log: `overflow` frames refused by the full
    /// ring, then what the backend substituted, then invalid samples. None when nothing was lost.
    pub(super) fn report(&self, overflow: usize, rate: u32) -> Option<String> {
        let ms = |frames: usize| frames.saturating_mul(1000) / rate.max(1) as usize;
        let mut parts = Vec::new();
        if overflow > 0 {
            parts.push(format!(
                "{} ms of input lost{}: the take writer fell behind (a busy disk or a starved thread) and the take holds silence there",
                ms(overflow),
                self.position(&self.overflow_at, rate)
            ));
        }
        let substituted = self.substituted_frames();
        if substituted > 0 {
            parts.push(format!(
                "{} ms of input lost by the audio device{}, recorded as silence",
                ms(substituted),
                self.position(&self.substituted_at, rate)
            ));
        }
        let nonfinite = self.nonfinite.load(Ordering::Relaxed);
        if nonfinite > 0 {
            parts.push(format!("{nonfinite} invalid input samples (not a number or infinite) recorded as silence"));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

/// Both reports when both exist: a write failure never hides lost input, nor the reverse.
pub(super) fn join(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}

/// The longest stretch of a take in which every one of its channels is exactly zero, counted
/// only between signal (a take that starts or ends silent is not a stretch of lost input).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct ZeroRun {
    /// A frame with signal has been seen.
    signal: bool,
    /// The current run of zero frames: its first take frame and its length.
    start: u64,
    len: u64,
    /// The longest closed run: (first take frame, length).
    longest: Option<(u64, u64)>,
}

impl ZeroRun {
    /// Writer: the take frame `at` is all zeros (`zero`) or not.
    pub(super) fn frame(&mut self, at: u64, zero: bool) {
        if zero {
            if self.len == 0 {
                self.start = at;
            }
            self.len += 1;
            return;
        }
        if self.signal && self.len > 0 && self.longest.is_none_or(|(_, len)| self.len > len) {
            self.longest = Some((self.start, self.len));
        }
        (self.signal, self.len) = (true, 0);
    }

    #[cfg(test)]
    pub(super) fn longest(&self) -> Option<(u64, u64)> {
        self.longest
    }

    /// The log note for a take whose longest run reaches 100 ms: a muted digital input is
    /// legitimate, so it is a note, not an error; on an analog input it is input that went missing.
    pub(super) fn note(&self, file: &str, rate: u32) -> Option<String> {
        let rate = rate.max(1);
        let (start, len) = self.longest.filter(|(_, len)| *len >= u64::from(rate / 10))?;
        let s = |frames: u64| frames as f64 / f64::from(rate);
        Some(format!(
            "{file} holds {:.3} s of exact digital silence between signal, from {:.3} s into the take (frames {start} to {})",
            s(len),
            s(start),
            start + len
        ))
    }
}
