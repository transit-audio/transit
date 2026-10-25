//! The real-time render of a session with External Instruments (ADR 0027, ADR 0020 slice 3): the
//! audio thread's side.
//!
//! A hardware synth sounds only while it plays, so a render that includes one plays the session
//! through the live Player at speed 1 and keeps what the plan rendered. While armed, the audio
//! thread copies each block of render frames `[start, start + len + reach)` into a ring allocated
//! with the Player: Main's channels as the plan rendered them (before the device fold and clamp),
//! then the device input channels of every External Instrument's return, as the plan read them.
//! A writer thread off the audio thread drains the ring and places each file's frames (see
//! [`crate::player::RealtimeRender`]): Main's frame for session sample `s` is the one rendered at
//! `s + A` (A the plan's delay compensation, snapshotted at the first captured block), a return's
//! is the one read at `s + C` (C the device's latency, the same figure the plan compensates).
//!
//! The audio thread only reads parameters set before arming, stores samples as `f32` bits, and
//! moves counters: no allocation, no lock, no log. A full ring is never waited on: the block is
//! dropped, counted, and the render fails, so a file with a hole in it is never written. So does a
//! block that does not follow the previous one (a locate, a loop wrap), a stop before the end, and
//! a change of A while capturing.

use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};

mod writer;
pub(crate) use writer::spawn;
pub use writer::{FileSink, RealtimeError, RealtimeSpec, RenderSink, Rendered, StemSpec};

/// Channels a capture may span: Main's plus every return's.
pub const MAX_CHANNELS: usize = 32;
/// The ring holds this many channel-seconds at the session rate (4 s of a stereo mix with one
/// stereo return, 2 s of 8 channels).
pub const RING_CHANNEL_SECONDS: usize = 16;

const IDLE: u8 = 0;
const ARMED: u8 = 1;
const CAPTURING: u8 = 2;
const CAPTURED: u8 = 3;
const FAILED: u8 = 4;

/// Where a real-time render is: mix frames written of the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderProgress {
    pub done: u64,
    pub of: u64,
}

/// Where a capture is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapState {
    Idle,
    /// Armed, waiting for the transport to reach the start (the pre-roll).
    Armed,
    Capturing,
    /// Every frame the files need is in the ring.
    Captured,
    Failed(Failure),
}

/// Why the audio thread stopped a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The ring was full: the writer fell behind and a block was dropped.
    Overrun = 1,
    /// A block did not start where the previous one ended (a locate or a loop wrap).
    Jumped = 2,
    /// The transport stopped before the end.
    Stopped = 3,
    /// The plan's delay compensation changed while capturing (a recompiled plan).
    LatencyChanged = 4,
    /// The first block that played was already past the start.
    MissedStart = 5,
}

impl Failure {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Overrun,
            2 => Self::Jumped,
            3 => Self::Stopped,
            4 => Self::LatencyChanged,
            5 => Self::MissedStart,
            _ => return None,
        })
    }
}

/// The capture shared by control, the audio thread and the writer thread.
pub struct RenderTap {
    state: AtomicU8,
    failure: AtomicU8,
    /// The first session sample of the range, which is the first render frame captured.
    start: AtomicI64,
    len: AtomicU64,
    /// The largest return latency C: the capture runs this far past the range (or A, if larger).
    reach: AtomicU64,
    /// The render frame after the last one captured, set at the first captured block.
    end: AtomicI64,
    /// A, the plan's delay compensation at the first captured block.
    latency: AtomicU64,
    main: AtomicU16,
    ret: [AtomicU16; MAX_CHANNELS],
    rets: AtomicU16,
    /// The render frame the next block must start at.
    next: AtomicI64,
    ring: Box<[AtomicU32]>,
    /// Frames pushed and frames taken, since arming.
    write: AtomicU64,
    read: AtomicU64,
    overruns: AtomicU64,
}

impl RenderTap {
    /// A tap whose ring holds [`RING_CHANNEL_SECONDS`] at `rate`.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self::with_capacity(rate as usize * RING_CHANNEL_SECONDS)
    }

    /// A tap whose ring holds `samples` samples (all channels together).
    #[must_use]
    pub fn with_capacity(samples: usize) -> Self {
        Self {
            state: AtomicU8::new(IDLE),
            failure: AtomicU8::new(0),
            start: AtomicI64::new(0),
            len: AtomicU64::new(0),
            reach: AtomicU64::new(0),
            end: AtomicI64::new(0),
            latency: AtomicU64::new(0),
            main: AtomicU16::new(0),
            ret: std::array::from_fn(|_| AtomicU16::new(0)),
            rets: AtomicU16::new(0),
            next: AtomicI64::new(0),
            ring: (0..samples.max(1)).map(|_| AtomicU32::new(0)).collect(),
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
        }
    }

    /// Arm a capture of `len` session samples from `start`: `main` of Main's channels, then the
    /// device input channels `ret`, for `reach` frames past the range (the largest return
    /// latency). False when a capture is armed or running, the channels do not fit
    /// [`MAX_CHANNELS`] or the ring, or the range is empty. Control side.
    pub fn arm(&self, start: i64, len: u64, reach: u64, main: u16, ret: &[u16]) -> bool {
        let stride = usize::from(main) + ret.len();
        if len == 0 || main == 0 || stride > MAX_CHANNELS || stride > self.ring.len() || self.state.load(Ordering::Acquire) != IDLE {
            return false;
        }
        self.start.store(start, Ordering::Relaxed);
        self.len.store(len, Ordering::Relaxed);
        self.reach.store(reach, Ordering::Relaxed);
        self.end.store(i64::MAX, Ordering::Relaxed);
        self.latency.store(0, Ordering::Relaxed);
        self.main.store(main, Ordering::Relaxed);
        for (slot, &c) in self.ret.iter().zip(ret) {
            slot.store(c, Ordering::Relaxed);
        }
        self.rets.store(ret.len() as u16, Ordering::Relaxed);
        self.next.store(start, Ordering::Relaxed);
        self.write.store(0, Ordering::Relaxed);
        self.read.store(0, Ordering::Relaxed);
        self.overruns.store(0, Ordering::Relaxed);
        self.failure.store(0, Ordering::Relaxed);
        self.state.compare_exchange(IDLE, ARMED, Ordering::Release, Ordering::Relaxed).is_ok()
    }

    /// Stop capturing (Cancel, a device loss, a result taken): the audio thread does nothing more.
    pub fn cancel(&self) {
        self.state.store(IDLE, Ordering::Release);
    }

    #[must_use]
    pub fn state(&self) -> TapState {
        match self.state.load(Ordering::Acquire) {
            ARMED => TapState::Armed,
            CAPTURING => TapState::Capturing,
            CAPTURED => TapState::Captured,
            FAILED => TapState::Failed(Failure::from_code(self.failure.load(Ordering::Relaxed)).unwrap_or(Failure::Overrun)),
            _ => TapState::Idle,
        }
    }

    /// A capture is armed or under way.
    #[must_use]
    pub fn busy(&self) -> bool {
        matches!(self.state.load(Ordering::Acquire), ARMED | CAPTURING)
    }

    /// A, as snapshotted at the first captured block (valid once a frame was pushed).
    #[must_use]
    pub fn latency(&self) -> u64 {
        self.latency.load(Ordering::Relaxed)
    }

    /// Blocks dropped because the ring was full.
    #[must_use]
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// Samples per captured frame.
    #[must_use]
    pub fn stride(&self) -> usize {
        usize::from(self.main.load(Ordering::Relaxed)) + usize::from(self.rets.load(Ordering::Relaxed))
    }

    fn fail(&self, from: u8, why: Failure) {
        self.failure.store(why as u8, Ordering::Relaxed);
        let _ = self.state.compare_exchange(from, FAILED, Ordering::Release, Ordering::Relaxed);
    }

    /// Audio thread, after the plan rendered a block of `frames` render frames at `position`:
    /// `main` is the plan's output (Main's channels first), `input` the live input it read,
    /// `latency` the plan's delay compensation A. Copies the frames the capture needs.
    pub fn block(&self, playing: bool, position: i64, frames: usize, latency: usize, main: &[Vec<f32>], input: &[Vec<f32>]) {
        let state = self.state.load(Ordering::Acquire);
        if state != ARMED && state != CAPTURING {
            return;
        }
        if !playing {
            if state == CAPTURING {
                self.fail(state, Failure::Stopped);
            }
            return;
        }
        let start = self.start.load(Ordering::Relaxed);
        let block_end = position.saturating_add(frames as i64);
        let latency = latency as u64;
        if state == ARMED {
            if block_end <= start {
                // The pre-roll: chase, the first notes and the delay lines settle.
                return;
            }
            if position > start {
                self.fail(state, Failure::MissedStart);
                return;
            }
            let reach = self.reach.load(Ordering::Relaxed).max(latency);
            let len = self.len.load(Ordering::Relaxed);
            self.latency.store(latency, Ordering::Relaxed);
            self.end.store(start.saturating_add(i64::try_from(len.saturating_add(reach)).unwrap_or(i64::MAX)), Ordering::Relaxed);
            if self.state.compare_exchange(ARMED, CAPTURING, Ordering::AcqRel, Ordering::Relaxed).is_err() {
                return;
            }
        } else if position != self.next.load(Ordering::Relaxed) {
            self.fail(state, Failure::Jumped);
            return;
        } else if latency != self.latency.load(Ordering::Relaxed) {
            self.fail(state, Failure::LatencyChanged);
            return;
        }
        let end = self.end.load(Ordering::Relaxed);
        let from = self.next.load(Ordering::Relaxed).max(position);
        let to = block_end.min(end);
        let count = u64::try_from(to.saturating_sub(from)).unwrap_or(0);
        let mains = usize::from(self.main.load(Ordering::Relaxed));
        let rets = usize::from(self.rets.load(Ordering::Relaxed));
        let stride = mains + rets;
        let cap = (self.ring.len() / stride.max(1)) as u64;
        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);
        if w.saturating_sub(r).saturating_add(count) > cap {
            self.overruns.fetch_add(1, Ordering::Relaxed);
            self.fail(CAPTURING, Failure::Overrun);
            return;
        }
        let skip = usize::try_from(from - position).unwrap_or(0);
        for i in 0..count as usize {
            let src = skip + i;
            let base = ((w + i as u64) % cap) as usize * stride;
            for c in 0..mains {
                let v = main.get(c).and_then(|ch| ch.get(src)).copied().unwrap_or(0.0);
                self.ring[base + c].store(v.to_bits(), Ordering::Relaxed);
            }
            for k in 0..rets {
                let ch = usize::from(self.ret[k].load(Ordering::Relaxed));
                let v = input.get(ch).and_then(|x| x.get(src)).copied().unwrap_or(0.0);
                self.ring[base + mains + k].store(v.to_bits(), Ordering::Relaxed);
            }
        }
        self.write.store(w + count, Ordering::Release);
        self.next.store(block_end, Ordering::Relaxed);
        if block_end >= end {
            let _ = self.state.compare_exchange(CAPTURING, CAPTURED, Ordering::Release, Ordering::Relaxed);
        }
    }

    /// Writer thread: move up to `max` captured frames, interleaved (`stride` samples each), into
    /// `out` (cleared first). Returns the frames moved.
    pub fn drain(&self, out: &mut Vec<f32>, max: usize) -> usize {
        out.clear();
        let stride = self.stride();
        if stride == 0 {
            return 0;
        }
        let cap = (self.ring.len() / stride) as u64;
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Relaxed);
        let n = w.saturating_sub(r).min(max as u64);
        for i in 0..n {
            let base = ((r + i) % cap) as usize * stride;
            out.extend(self.ring[base..base + stride].iter().map(|s| f32::from_bits(s.load(Ordering::Relaxed))));
        }
        self.read.store(r + n, Ordering::Release);
        n as usize
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn block(v: f32, frames: usize) -> Vec<Vec<f32>> {
        vec![vec![v; frames]; 2]
    }

    #[test]
    fn captures_from_the_start_through_the_reach_and_says_captured() {
        let tap = RenderTap::with_capacity(4096);
        assert!(tap.arm(100, 200, 30, 2, &[1]));
        assert!(!tap.arm(0, 10, 0, 2, &[]), "one at a time");
        let input = vec![vec![0.0; 64], vec![0.5; 64]];
        // Pre-roll: nothing.
        tap.block(true, 0, 64, 10, &block(1.0, 64), &input);
        assert_eq!(tap.state(), TapState::Armed);
        let mut out = Vec::new();
        let mut got = 0;
        let mut pos = 64;
        while tap.state() == TapState::Armed || tap.state() == TapState::Capturing {
            tap.block(true, pos, 64, 10, &block(1.0, 64), &input);
            got += tap.drain(&mut out, 1 << 20);
            pos += 64;
        }
        assert_eq!(tap.state(), TapState::Captured);
        assert_eq!(got, 230, "the range plus the largest of A and C");
        assert_eq!(tap.latency(), 10);
        assert_eq!(&out[out.len() - 3..], &[1.0, 1.0, 0.5], "Main's channels, then the return's");
    }

    #[test]
    fn a_full_ring_drops_the_block_counts_it_and_fails() {
        let tap = RenderTap::with_capacity(3 * 100);
        assert!(tap.arm(0, 1000, 0, 2, &[0]));
        let input = block(0.0, 64);
        tap.block(true, 0, 64, 0, &block(1.0, 64), &input);
        tap.block(true, 64, 64, 0, &block(1.0, 64), &input);
        assert_eq!(tap.state(), TapState::Failed(Failure::Overrun));
        assert_eq!(tap.overruns(), 1);
    }

    #[test]
    fn a_jump_a_stop_a_late_start_or_a_new_latency_fails() {
        let input = block(0.0, 64);
        let run = |f: &dyn Fn(&RenderTap)| {
            let tap = RenderTap::with_capacity(1 << 16);
            assert!(tap.arm(0, 1000, 0, 2, &[]));
            f(&tap);
            tap.state()
        };
        let main = block(1.0, 64);
        assert_eq!(run(&|t| t.block(true, 64, 64, 0, &main, &input)), TapState::Failed(Failure::MissedStart));
        assert_eq!(
            run(&|t| {
                t.block(true, 0, 64, 0, &main, &input);
                t.block(true, 640, 64, 0, &main, &input);
            }),
            TapState::Failed(Failure::Jumped)
        );
        assert_eq!(
            run(&|t| {
                t.block(true, 0, 64, 0, &main, &input);
                t.block(false, 64, 64, 0, &main, &input);
            }),
            TapState::Failed(Failure::Stopped)
        );
        assert_eq!(
            run(&|t| {
                t.block(true, 0, 64, 0, &main, &input);
                t.block(true, 64, 64, 5, &main, &input);
            }),
            TapState::Failed(Failure::LatencyChanged)
        );
        // Stopped before the start (the pre-roll) is not a failure; cancel goes idle.
        assert_eq!(
            run(&|t| {
                t.block(false, -64, 64, 0, &main, &input);
                t.cancel();
                t.block(true, 0, 64, 0, &main, &input);
            }),
            TapState::Idle
        );
    }
}
