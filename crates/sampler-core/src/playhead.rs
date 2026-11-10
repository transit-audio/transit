//! Where a voice reads (ADR 0041 "Loop points and crossfade reference"): a zone's [`Region`] of its
//! source and the [`Playhead`] that walks it.
//!
//! - **Bounds.** Start, end and loop boundaries are native source frames, half-open:
//!   `start <= loop_start < loop_end <= end`, a loop at least two frames. Playback enters at
//!   `start` (reverse: `end - 1`) and steps by the voice's step (source frames per output frame,
//!   pitch and the source-to-session rate in one; no second resampler).
//! - **No loop.** The phase stops a voice at the region's end (reverse: below its start); the cubic
//!   taps outside the region read zero.
//! - **Forward loop.** Past `loop_end` the phase wraps to `loop_start + (p - loop_start) mod L`
//!   (reverse: below `loop_start` it wraps up the same way, `loop_start - 1` to `loop_end - 1`). The
//!   wrap is one modulo whatever the step, so the work never grows with pitch.
//! - **Ping-pong.** The phase reflects on `[loop_start, loop_end - 1]` without repeating an endpoint:
//!   integer steps visit `.., hi - 1, hi, hi - 1, .., lo, lo + 1, ..`. Multiple reflections in one
//!   step fold by a modulo of the period `2 (loop_end - 1 - loop_start)`.
//! - **Taps.** Each of the four cubic taps maps through the same rule: on the exit side (past
//!   `loop_end - 1`, or below `loop_start` in reverse) taps always wrap or reflect; on the entry side
//!   they read the original region (the intro) until the first wrap or reflection, and wrap or
//!   reflect after it.
//! - **Crossfade** (forward loops only). With X the used crossfade ([`Region::crossfade_used`]:
//!   the stored count clamped to `floor(L / 2)`, 0 unless the loop is Forward), the final X frames
//!   before the exit blend two reads of the same cubic reader, outgoing `a` at the phase p and
//!   incoming `b`, as `a + (b - a) * w` in that order ([`Playhead::crossfade_at`]). Forward:
//!   `d = p - (loop_end - X)` on `[loop_end - X, loop_end)`, `w = d / X`, incoming phase
//!   `loop_start + d`, and past `loop_end` the phase continues at `loop_start + X` with its
//!   fractional overshoot, so the period is `L - X`. Reverse is the same arithmetic in the mirrored
//!   coordinate `u = (loop_start + loop_end - 1) - p` (the mirror that maps reverse's entry
//!   `end - 1` and its wrap below `loop_start` to `loop_end - 1` onto the forward ones):
//!   `d = (loop_start + X - 1) - p` on `(loop_start - 1, loop_start + X - 1]`, incoming phase
//!   `(loop_end - 1) - d`, and at or below `loop_start - 1` the phase continues at
//!   `loop_end - 1 - X` with its overshoot. The incoming read's taps always map through the loop
//!   rule (they are loop taps, never intro taps). X = 0 takes none of these paths: the arithmetic
//!   is the plain loop's, bit for bit.

/// How a zone loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    Off,
    Forward,
    PingPong,
}

/// A zone's region of its source, in native source frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// The source's index in the [`crate::reader::SourceBank`].
    pub source: u16,
    pub start: u64,
    /// Exclusive.
    pub end: u64,
    pub loop_mode: LoopMode,
    pub loop_start: u64,
    /// Exclusive.
    pub loop_end: u64,
    /// The forward-loop crossfade X in native source frames as stored (0: none). Any value is
    /// accepted; playback uses [`Region::crossfade_used`].
    pub crossfade: u64,
}

impl Region {
    /// The whole of source `source`, `frames` long, without a loop.
    #[must_use]
    pub fn whole(source: u16, frames: u64) -> Region {
        Region { source, start: 0, end: frames, loop_mode: LoopMode::Off, loop_start: 0, loop_end: frames, crossfade: 0 }
    }

    /// The region with a loop over `loop_start..loop_end`.
    #[must_use]
    pub fn looped(self, mode: LoopMode, loop_start: u64, loop_end: u64) -> Region {
        Region { loop_mode: mode, loop_start, loop_end, ..self }
    }

    /// The region with a stored crossfade of `frames` source frames.
    #[must_use]
    pub fn crossfaded(self, frames: u64) -> Region {
        Region { crossfade: frames, ..self }
    }

    /// The crossfade playback uses: `min(crossfade, floor(L / 2))` with L the loop's length on a
    /// Forward loop, 0 for any other loop mode.
    #[must_use]
    pub fn crossfade_used(&self) -> u64 {
        if self.loop_mode != LoopMode::Forward {
            return 0;
        }
        self.crossfade.min(self.loop_end.saturating_sub(self.loop_start) / 2)
    }

    /// Whether the bounds hold: a non-empty region and, when it loops, a loop of at least two frames
    /// inside it. A region past the source's end is refused when a voice starts (the source's
    /// length is known then).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.start < self.end
            && (self.loop_mode == LoopMode::Off
                || (self.start <= self.loop_start
                    && self.loop_start < self.loop_end
                    && self.loop_end <= self.end
                    && self.loop_end - self.loop_start >= 2))
    }
}

/// A voice's read position and its loop state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playhead {
    /// The phase in native source frames (above -1 while playing: only a crossfaded reverse loop
    /// starting at frame 0 reaches below 0, mirroring the forward loop's last frame).
    pub pos: f64,
    /// Playback runs from the end down.
    pub reverse: bool,
    /// The phase moves up (ping-pong flips it; otherwise it is `!reverse`).
    pub up: bool,
    /// The voice loops (a Gate voice on a looped region).
    pub looping: bool,
    /// The first wrap or reflection has happened (the intro is over).
    pub wrapped: bool,
    /// The phase left the region: the voice ends.
    pub done: bool,
    /// The crossfade in use, X (0: none; nonzero only while looping a Forward loop).
    pub crossfade: u64,
}

impl Playhead {
    /// The entry of `region`: `start`, or `end - 1` in reverse. `looping` is honored only when the
    /// region has a loop.
    #[must_use]
    pub fn start(region: &Region, reverse: bool, looping: bool) -> Playhead {
        let pos = if reverse { region.end.saturating_sub(1) } else { region.start } as f64;
        let looping = looping && region.loop_mode != LoopMode::Off;
        let crossfade = if looping { region.crossfade_used() } else { 0 };
        Playhead { pos, reverse, up: !reverse, looping, wrapped: false, done: false, crossfade }
    }

    /// The blend at the phase while it is in the final X frames of a crossfaded forward loop: the
    /// incoming phase and the incoming weight `w` (`None`: no blend here).
    #[must_use]
    pub fn crossfade_at(&self, r: &Region) -> Option<(f64, f32)> {
        if self.crossfade == 0 || !self.looping {
            return None;
        }
        let x = self.crossfade as f64;
        let (ls, le) = (r.loop_start as f64, r.loop_end as f64);
        // Reverse reads the forward equations in the mirrored coordinate u = (ls + le - 1) - p.
        let (d, inside) = if self.reverse {
            let d = (ls + x - 1.0) - self.pos;
            (d, d >= 0.0 && self.pos > ls - 1.0)
        } else {
            let d = self.pos - (le - x);
            (d, d >= 0.0 && self.pos < le)
        };
        if !inside {
            return None;
        }
        let incoming = if self.reverse { (le - 1.0) - d } else { ls + d };
        Some((incoming, (d / x) as f32))
    }

    /// The four cubic taps of the incoming read at phase `incoming` ([`Playhead::crossfade_at`]):
    /// loop taps, mapped through the loop rule on both sides.
    #[must_use]
    pub fn incoming_taps(&self, r: &Region, incoming: f64) -> ([Option<u64>; 4], f32) {
        Playhead { pos: incoming, wrapped: true, ..*self }.taps(r)
    }

    /// The source frame tap `j` reads (`None`: zero, outside the region or the loop's reach).
    #[must_use]
    pub fn tap(&self, r: &Region, j: i64) -> Option<u64> {
        let (start, end) = (r.start as i64, r.end as i64);
        let inside = |j: i64| (start <= j && j < end).then_some(j as u64);
        if !self.looping {
            return inside(j);
        }
        let (lo, hi) = (r.loop_start as i64, r.loop_end as i64 - 1);
        let (exit, entry) = if self.reverse { (j < lo, j > hi) } else { (j > hi, j < lo) };
        if !exit && !(entry && self.wrapped) {
            return inside(j);
        }
        Some(match r.loop_mode {
            LoopMode::PingPong => reflect(j, lo, hi),
            _ => wrap(j, lo, hi + 1),
        } as u64)
    }

    /// The four cubic taps around the phase (frames `i - 1 ..= i + 2`, mapped) and the fraction.
    #[must_use]
    pub fn taps(&self, r: &Region) -> ([Option<u64>; 4], f32) {
        let (i, t) = if self.pos < 0.0 {
            let i = crate::math::floor(self.pos);
            (i as i64, (self.pos - i) as f32)
        } else {
            let (i, t) = crate::reader::split(self.pos);
            (i as i64, t)
        };
        ([self.tap(r, i - 1), self.tap(r, i), self.tap(r, i + 1), self.tap(r, i + 2)], t)
    }

    /// Moves the phase one output frame on by `step` (> 0) source frames.
    pub fn advance(&mut self, r: &Region, step: f64) {
        if self.done {
            return;
        }
        let (start, end) = (r.start as f64, r.end as f64);
        if !self.looping {
            self.pos = if self.reverse { self.pos - step } else { self.pos + step };
            self.done = !self.pos.is_finite() || if self.reverse { self.pos < start } else { self.pos >= end };
            return;
        }
        let (lo, hi) = (r.loop_start as f64, r.loop_end as f64 - 1.0);
        match r.loop_mode {
            LoopMode::PingPong => {
                // Reflection period 2M on [lo, hi].
                let m = hi - lo;
                if self.up {
                    self.pos += step;
                    if self.pos > hi {
                        let q = (self.pos - hi) % (2.0 * m);
                        (self.pos, self.up) = if q <= m { (hi - q, false) } else { (lo + (q - m), true) };
                        self.wrapped = true;
                    }
                } else {
                    self.pos -= step;
                    if self.pos < lo {
                        let q = (lo - self.pos) % (2.0 * m);
                        (self.pos, self.up) = if q <= m { (lo + q, true) } else { (hi - (q - m), false) };
                        self.wrapped = true;
                    }
                }
            }
            _ => {
                // After a wrap the phase stays in [lo + X, hi + 1) (reverse, mirrored:
                // (lo - 1, hi - X]), the period L - X; with X = 0 every term is the plain loop's.
                let x = self.crossfade as f64;
                let len = hi + 1.0 - lo - x;
                if self.reverse && self.crossfade > 0 {
                    self.pos -= step;
                    if self.pos <= lo - 1.0 {
                        self.pos = (hi - x) - (lo - 1.0 - self.pos) % len;
                        self.wrapped = true;
                    }
                } else if self.reverse {
                    self.pos -= step;
                    if self.pos < lo {
                        let q = (lo - self.pos) % len;
                        self.pos = if q == 0.0 { lo } else { hi + 1.0 - x - q };
                        self.wrapped = true;
                    }
                } else {
                    self.pos += step;
                    if self.pos >= hi + 1.0 {
                        let lo = lo + x;
                        self.pos = lo + (self.pos - lo) % len;
                        self.wrapped = true;
                    }
                }
            }
        }
        self.done = !self.pos.is_finite() || self.pos <= -1.0;
    }

    /// Output frames left before the phase leaves a non-looping region at `step` (this one
    /// included); `u64::MAX` while looping.
    #[must_use]
    pub fn frames_left(&self, r: &Region, step: f64) -> u64 {
        if self.looping {
            return u64::MAX;
        }
        let room = if self.reverse { self.pos - r.start as f64 } else { r.end as f64 - self.pos };
        if self.reverse && room == 0.0 {
            return 1;
        }
        if room.is_nan() || room <= 0.0 || step.is_nan() || step <= 0.0 {
            return 0;
        }
        let n = room / step;
        // Forward: frames while pos < end; reverse: while pos >= start.
        let n = if self.reverse { crate::math::floor(n) + 1.0 } else { -crate::math::floor(-n) };
        if n >= u64::MAX as f64 { u64::MAX } else { n as u64 }
    }
}

/// `j` wrapped into `[lo, end)`.
fn wrap(j: i64, lo: i64, end: i64) -> i64 {
    lo + (j - lo).rem_euclid((end - lo).max(1))
}

/// `j` reflected on `[lo, hi]` without repeating an endpoint (period `2 (hi - lo)`).
fn reflect(j: i64, lo: i64, hi: i64) -> i64 {
    let m = (hi - lo).max(1);
    let q = (j - lo).rem_euclid(2 * m);
    lo + if q <= m { q } else { 2 * m - q }
}
