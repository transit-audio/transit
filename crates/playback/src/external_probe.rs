//! The External Instrument's Measure (ADR 0020 decision 3, slice 2): the latency C from a note-on
//! to its sound on the return, in the plan's own frames.
//!
//! Like the hardware insert probe ([`crate::hardware_probe`]) it counts plan frames since it
//! armed: the live input block the plan reads while rendering frame `t` is recorded at `t`. The
//! note goes out through the MIDI output thread, never from here: the audio thread pushes each
//! note-on onto the playback ring at the start of a block, stamped with that block's presentation
//! time P(R0) exactly as #61 stamps a clip's note on an External Instrument track, and remembers
//! R0. A note's latency is R1 - R0, R1 the onset [`crate::onset`] finds on the recording; C is the
//! median over the notes. That is decision 2's definition of C, so the delay compensation that
//! uses it lines the return up with every software path.
//!
//! The run: [`FLOOR_MS`] of return as the reference floor, one unmeasured warm-up note held
//! [`WARMUP_NOTE_MS`], then [`NOTES`] notes [`GAP_MS`] apart, each held [`HOLD_MS`], and a tail
//! of [`WINDOW_MS`] plus the detector's peak window for the last one: about 13 s. Every exit sends
//! the note-off and All Notes Off (CC 123) on the channel through the output thread: the run's end
//! and a Cancel from here (the audio thread pushes them), a device loss from control
//! ([`crate::Player`] asks the output thread to release the track).
//!
//! The audio thread only reads parameters set before arming, stores samples as `f32` bits into a
//! buffer allocated with the Player, and bumps counters: no allocation, no lock, no log. The
//! analysis runs on the control thread once the run is done.

use crate::onset;
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};

/// Channels a return may span.
pub const MAX_RETURN_CHANNELS: usize = 16;
/// The reference noise floor taken before any note, and the window before each note.
pub const FLOOR_MS: u64 = 100;
/// How long the unmeasured warm-up note is held.
pub const WARMUP_NOTE_MS: u64 = 200;
/// Measured notes.
pub const NOTES: usize = 8;
/// From one note-on to the next (the Model D's release needs the gap).
pub const GAP_MS: u64 = 1500;
/// How long each measured note is held.
pub const HOLD_MS: u64 = 100;
/// An onset must come within this time of its note-on.
pub const WINDOW_MS: u64 = 1000;
/// The note and velocity Measure plays.
pub const NOTE: u8 = 60;
pub const VELOCITY: u8 = 100;
/// A note with an attack sample at or above this level (-0.5 dBFS) clipped.
pub const CLIP_LEVEL: f32 = 0.944;
/// A spread (standard deviation) above this many ms is shown and kept only on Keep.
pub const MAX_SPREAD_MS: f64 = 1.0;

/// The fewest notes with an onset for a run to count: half the notes, and never fewer than 3.
#[must_use]
pub fn min_found() -> usize {
    NOTES.div_ceil(2).max(3)
}

const IDLE: u8 = 0;
const ARMED: u8 = 1;
const DONE: u8 = 2;
/// Control asked to stop; the audio thread releases the note and goes idle.
const CANCEL: u8 = 3;
const NONE: u64 = u64::MAX;
/// The most B may move during a run before the run is refused: the ring counts pending
/// interpolation history, which can differ by a frame or two between blocks.
pub const BACKLOG_SLACK: u64 = 2;
/// Events of a run: an on and an off per note, the warm-up included.
const EVENTS: usize = 2 * (NOTES + 1);

fn frames_of(rate: u32, ms: u64) -> u64 {
    u64::from(rate) * ms / 1000
}

/// The plan frame (since arming) a note goes on: note 0 is the warm-up.
fn on_at(rate: u32, note: usize) -> u64 {
    frames_of(rate, FLOOR_MS + GAP_MS * note as u64)
}

/// The plan frame event `i` falls due: `2n` is note `n`'s on, `2n + 1` its off.
fn event_at(rate: u32, i: usize) -> u64 {
    let note = i / 2;
    let hold = if note == 0 { WARMUP_NOTE_MS } else { HOLD_MS };
    on_at(rate, note) + if i % 2 == 1 { frames_of(rate, hold) } else { 0 }
}

/// Plan frames a run records.
fn total_for(rate: u32) -> u64 {
    on_at(rate, NOTES) + frames_of(rate, WINDOW_MS + u64::from(onset::PEAK_MS) + FLOOR_MS)
}

/// A MIDI message the audio thread hands to the output thread for the measured track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emit {
    pub track: u64,
    pub bytes: [u8; 3],
}

/// Where a run is, for the card's "Measuring… 3/8".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// Measured notes sent so far (the warm-up is not counted).
    pub note: u32,
    pub of: u32,
    /// Peak level of the return over the last recorded 50 ms, linear.
    pub level: f32,
}

/// What a run found.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    /// C: the median of the notes' latencies, in plan frames, rounded.
    pub frames: u32,
    /// The standard deviation of the latencies, in frames: the spread the card shows.
    pub spread: f64,
    /// Notes with an onset, of [`NOTES`].
    pub found: u32,
    pub sent: u32,
    /// Each found note's latency, in frames, in the order played.
    pub latencies: Vec<u64>,
    /// The live input ring's backlog B the run held, in frames (`None` when the plan reported
    /// none). C moves frame for frame with it (ADR 0020 decision 2).
    pub backlog: Option<u32>,
}

impl Measured {
    /// The spread in ms at `rate`.
    #[must_use]
    pub fn spread_ms(&self, rate: u32) -> f64 {
        self.spread * 1000.0 / f64::from(rate.max(1))
    }

    /// Whether the spread is high enough to need Keep (above [`MAX_SPREAD_MS`]).
    #[must_use]
    pub fn needs_keep(&self, rate: u32) -> bool {
        self.spread_ms(rate) > MAX_SPREAD_MS
    }
}

/// Why a run was refused. Nothing is stored for any of these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasureError {
    /// Fewer than [`min_found`] notes came back within [`WINDOW_MS`].
    NoSound { found: u32, sent: u32 },
    /// The return was not quiet before the notes (a drone, a held note, an arpeggiator): its
    /// floor or a pre-note window above -20 dBFS, or more than half the notes 6 dB over the floor.
    NotQuiet { peak: f32 },
    /// A note's attack reached [`CLIP_LEVEL`].
    Clipped { peak: f32 },
    /// The audio device stopped or went away during the run.
    DeviceLost,
    /// The input ring's backlog B changed during the run (a different input alignment): C would
    /// be off by the change. Measure again.
    InputAlignmentChanged { first: u32, last: u32 },
}

impl std::fmt::Display for MeasureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSound { found, sent } => write!(f, "a sound came back on {found} of {sent} notes"),
            Self::NotQuiet { peak } => write!(f, "the return is not quiet before the notes ({})", onset::dbfs(*peak)),
            Self::Clipped { peak } => write!(f, "the return clipped ({})", onset::dbfs(*peak)),
            Self::DeviceLost => write!(f, "the audio device went away while measuring"),
            Self::InputAlignmentChanged { .. } => write!(f, "input alignment changed during the measurement: measure again"),
        }
    }
}

/// The probe shared by the control thread and the audio thread. One per Player.
pub struct ExtProbe {
    rate: u32,
    /// The sum of the return channels, as `f32` bits, by plan frame since arming.
    capture: Box<[AtomicU32]>,
    /// Plan frames recorded since arming: the first frame of the next block.
    frame: AtomicU64,
    state: AtomicU8,
    /// Events pushed so far (audio thread's cursor; control reads it for progress).
    next: AtomicU32,
    /// R0 of each note (0: the warm-up), [`NONE`] before it goes out.
    sent: [AtomicU64; NOTES + 1],
    track: AtomicU64,
    channel: AtomicU8,
    note: AtomicU8,
    velocity: AtomicU8,
    ret_n: AtomicU8,
    ret: [AtomicU16; MAX_RETURN_CHANNELS],
    /// The input ring's backlog B at the first block of the run, and its extremes since
    /// ([`NONE`] / 0 before any was reported). Audio thread writes; control reads at the end.
    backlog_first: AtomicU64,
    backlog_min: AtomicU64,
    backlog_max: AtomicU64,
}

impl std::fmt::Debug for ExtProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtProbe").field("rate", &self.rate).field("state", &self.state.load(Ordering::Relaxed)).finish_non_exhaustive()
    }
}

impl ExtProbe {
    /// A probe for a plan at `rate`. Allocates the recording (about 13.3 s of mono).
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(8_000);
        let len = usize::try_from(total_for(rate)).unwrap_or(0);
        Self {
            rate,
            capture: (0..len).map(|_| AtomicU32::new(0)).collect(),
            frame: AtomicU64::new(0),
            state: AtomicU8::new(IDLE),
            next: AtomicU32::new(0),
            sent: std::array::from_fn(|_| AtomicU64::new(NONE)),
            track: AtomicU64::new(0),
            channel: AtomicU8::new(0),
            note: AtomicU8::new(NOTE),
            velocity: AtomicU8::new(VELOCITY),
            ret_n: AtomicU8::new(0),
            ret: std::array::from_fn(|_| AtomicU16::new(0)),
            backlog_first: AtomicU64::new(NONE),
            backlog_min: AtomicU64::new(NONE),
            backlog_max: AtomicU64::new(0),
        }
    }

    /// The plan rate the probe counts in.
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// How long a run takes.
    #[must_use]
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(total_for(self.rate) as f64 / f64::from(self.rate))
    }

    /// Whether a run is going (or stopping after a Cancel).
    #[must_use]
    pub fn armed(&self) -> bool {
        matches!(self.state.load(Ordering::Acquire), ARMED | CANCEL)
    }

    /// Start a run for `track` on 0-based MIDI `channel`, playing `note` at `velocity`, recording
    /// the 0-based device input channels `ret`. False when one is running or `ret` is empty or
    /// too wide.
    pub fn arm(&self, track: u64, channel: u8, note: u8, velocity: u8, ret: &[u16]) -> bool {
        if self.armed() || ret.is_empty() || ret.len() > MAX_RETURN_CHANNELS || channel > 15 || note > 127 || !(1..=127).contains(&velocity) {
            return false;
        }
        for (slot, ch) in self.ret.iter().zip(ret) {
            slot.store(*ch, Ordering::Relaxed);
        }
        self.ret_n.store(ret.len() as u8, Ordering::Relaxed);
        self.track.store(track, Ordering::Relaxed);
        self.channel.store(channel, Ordering::Relaxed);
        self.note.store(note, Ordering::Relaxed);
        self.velocity.store(velocity, Ordering::Relaxed);
        for s in &self.sent {
            s.store(NONE, Ordering::Relaxed);
        }
        self.next.store(0, Ordering::Relaxed);
        self.frame.store(0, Ordering::Relaxed);
        self.backlog_first.store(NONE, Ordering::Relaxed);
        self.backlog_min.store(NONE, Ordering::Relaxed);
        self.backlog_max.store(0, Ordering::Relaxed);
        self.state.store(ARMED, Ordering::Release);
        true
    }

    /// Stop a run (Cancel, the window closing, a device reopen). The audio thread sends the
    /// note-off and All Notes Off at its next block; nothing is measured. Returns whether one was
    /// running.
    pub fn cancel(&self) -> bool {
        self.state.compare_exchange(ARMED, CANCEL, Ordering::AcqRel, Ordering::Acquire).is_ok()
    }

    /// Forget a run whose audio thread is gone (control, after a device loss): the output thread
    /// releases the note instead.
    pub fn abandon(&self) {
        self.state.store(IDLE, Ordering::Release);
    }

    /// The track a run plays on.
    #[must_use]
    pub fn track(&self) -> u64 {
        self.track.load(Ordering::Relaxed)
    }

    /// Plan frames recorded so far.
    #[must_use]
    pub fn frames_recorded(&self) -> u64 {
        self.frame.load(Ordering::Relaxed)
    }

    /// Where the run is; reads atomics only.
    #[must_use]
    pub fn progress(&self) -> Progress {
        let of = NOTES as u32;
        let note = self.sent.iter().skip(1).filter(|s| s.load(Ordering::Relaxed) != NONE).count() as u32;
        let at = self.frame.load(Ordering::Relaxed);
        let from = at.saturating_sub(frames_of(self.rate, 50));
        let level = (from..at)
            .filter_map(|f| usize::try_from(f).ok().and_then(|f| self.capture.get(f)))
            .map(|s| f32::from_bits(s.load(Ordering::Relaxed)).abs())
            .fold(0.0f32, f32::max);
        match self.state.load(Ordering::Acquire) {
            DONE => Progress { note: of, of, level },
            ARMED | CANCEL => Progress { note, of, level },
            _ => Progress { note: 0, of, level: 0.0 },
        }
    }

    /// The result, once the run is done (and only once).
    #[must_use]
    pub fn take_result(&self) -> Option<Result<Measured, MeasureError>> {
        if self.state.compare_exchange(DONE, IDLE, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return None;
        }
        let recorded: Vec<f32> = self.capture.iter().map(|s| f32::from_bits(s.load(Ordering::Relaxed))).collect();
        let sent: Vec<u64> = self.sent.iter().map(|s| s.load(Ordering::Relaxed)).collect();
        let (first, min, max) =
            (self.backlog_first.load(Ordering::Relaxed), self.backlog_min.load(Ordering::Relaxed), self.backlog_max.load(Ordering::Relaxed));
        if min != NONE && max - min > BACKLOG_SLACK {
            // B moved (the input was re-attached or its ring was cleared): the first sample and
            // the farthest from it say how.
            let last = if max - first >= first - min { max } else { min };
            return Some(Err(MeasureError::InputAlignmentChanged { first: first as u32, last: last as u32 }));
        }
        let backlog = (min != NONE).then_some(max as u32);
        Some(analyze(&recorded, &sent, self.rate).map(|m| Measured { backlog, ..m }))
    }

    /// Audio thread: the live input ring's backlog B after the plan popped this block (`None`
    /// without a ring). Loads and stores atomics only. Call before [`Self::block`].
    pub fn note_backlog(&self, frames: Option<usize>) {
        let Some(b) = frames.map(|b| b as u64) else { return };
        if self.state.load(Ordering::Acquire) != ARMED {
            return;
        }
        let _ = self.backlog_first.compare_exchange(NONE, b, Ordering::Relaxed, Ordering::Relaxed);
        self.backlog_min.fetch_min(b, Ordering::Relaxed);
        self.backlog_max.fetch_max(b, Ordering::Relaxed);
    }

    fn message(&self, status: u8, data: u8, value: u8) -> Emit {
        let channel = self.channel.load(Ordering::Relaxed) & 0x0F;
        Emit { track: self.track.load(Ordering::Relaxed), bytes: [status | channel, data & 0x7F, value & 0x7F] }
    }

    /// The note-off and All Notes Off every exit sends.
    fn release(&self, emit: &mut impl FnMut(Emit)) {
        emit(self.message(0x80, self.note.load(Ordering::Relaxed), 0));
        emit(self.message(0xB0, 123, 0));
    }

    /// Audio thread: the plan rendered a block of `frames` with this live `input` (planar device
    /// channels). Records the return, and hands `emit` the messages due at the block's first frame
    /// (its caller stamps them with that frame's presentation time): a note-on due anywhere up to
    /// the block start goes out at the block start, which is its R0. Allocation-free.
    pub fn block(&self, input: &[Vec<f32>], frames: usize, mut emit: impl FnMut(Emit)) {
        match self.state.load(Ordering::Acquire) {
            ARMED => {}
            CANCEL => {
                self.release(&mut emit);
                self.state.store(IDLE, Ordering::Release);
                return;
            }
            _ => return,
        }
        let start = self.frame.load(Ordering::Relaxed);
        let mut next = self.next.load(Ordering::Relaxed) as usize;
        while next < EVENTS && event_at(self.rate, next) <= start {
            let note = self.note.load(Ordering::Relaxed);
            if next.is_multiple_of(2) {
                if let Some(s) = self.sent.get(next / 2) {
                    s.store(start, Ordering::Relaxed);
                }
                emit(self.message(0x90, note, self.velocity.load(Ordering::Relaxed)));
            } else {
                emit(self.message(0x80, note, 0));
            }
            next += 1;
        }
        self.next.store(next as u32, Ordering::Relaxed);
        let n = usize::from(self.ret_n.load(Ordering::Relaxed)).min(MAX_RETURN_CHANNELS);
        for j in 0..frames {
            let Some(slot) = usize::try_from(start).ok().and_then(|s| self.capture.get(s + j)) else { break };
            let mut sum = 0.0f32;
            for ch in self.ret.iter().take(n) {
                if let Some(x) = input.get(usize::from(ch.load(Ordering::Relaxed))).and_then(|c| c.get(j)) {
                    sum += *x;
                }
            }
            slot.store(sum.to_bits(), Ordering::Relaxed);
        }
        let end = start + frames as u64;
        self.frame.store(end, Ordering::Relaxed);
        if next >= EVENTS && end >= self.capture.len() as u64 {
            // The last note-off went out long ago; All Notes Off closes the run.
            emit(self.message(0xB0, 123, 0));
            self.state.store(DONE, Ordering::Release);
        }
    }
}

/// Find each measured note's onset in `recorded` (the return by plan frame since arming) against
/// R0 in `sent` (index 0 the warm-up, not counted) and summarize. Pure; control side.
pub fn analyze(recorded: &[f32], sent: &[u64], rate: u32) -> Result<Measured, MeasureError> {
    let w = onset::window(rate);
    let floor_frames = usize::try_from(frames_of(rate, FLOOR_MS)).unwrap_or(usize::MAX);
    let window = usize::try_from(frames_of(rate, WINDOW_MS)).unwrap_or(usize::MAX);
    let peak_len = usize::try_from(frames_of(rate, u64::from(onset::PEAK_MS))).unwrap_or(usize::MAX);
    let reference = onset::noise_floor(recorded.get(..floor_frames.min(recorded.len())).unwrap_or(&[]), w);
    onset::threshold(reference).map_err(|loud| MeasureError::NotQuiet { peak: loud.peak })?;
    // Each measured note's R0 and the window before it; the floor is the lowest of them all.
    let notes: Vec<(usize, onset::Floor)> = sent
        .iter()
        .skip(1)
        .filter_map(|&r0| usize::try_from(r0).ok().filter(|&r| r0 != NONE && r < recorded.len()))
        .map(|r0| (r0, onset::noise_floor(recorded.get(r0.saturating_sub(floor_frames)..r0).unwrap_or(&[]), w)))
        .collect();
    let sent_n = notes.len() as u32;
    let mut floor = reference.peak;
    for (_, pre) in &notes {
        onset::not_sounding(*pre).map_err(|loud| MeasureError::NotQuiet { peak: loud.peak })?;
        floor = floor.min(pre.peak);
    }
    let noisy = notes.iter().filter(|(_, pre)| pre.peak > floor.max(reference.peak) * onset::QUIET_FACTOR).count();
    if noisy * 2 > notes.len() {
        let peak = notes.iter().map(|(_, pre)| pre.peak).fold(0.0f32, f32::max);
        return Err(MeasureError::NotQuiet { peak });
    }
    let mut latencies = Vec::with_capacity(notes.len());
    for (r0, pre) in &notes {
        let (threshold, guard) = onset::levels(floor, pre.peak).map_err(|loud| MeasureError::NotQuiet { peak: loud.peak })?;
        // One envelope window before R0 is kept, so the envelope at R0 is a full one.
        let base = r0.saturating_sub(w);
        let from = r0 - base;
        let end = r0.saturating_add(window).saturating_add(peak_len).min(recorded.len());
        let samples = recorded.get(base..end).unwrap_or(&[]);
        let scan = samples.get(..(from + window).min(samples.len())).unwrap_or(&[]);
        let Some(crossing) = onset::first_crossing(scan, from, w, threshold) else { continue };
        let found = onset::refine(samples, from, crossing, w, peak_len, guard.min(threshold));
        let attack = samples.get(from..).unwrap_or(&[]).iter().map(|x| if x.is_finite() { x.abs() } else { 1.0 }).fold(0.0f32, f32::max);
        if attack >= CLIP_LEVEL {
            return Err(MeasureError::Clipped { peak: attack });
        }
        latencies.push((found.at - from) as u64);
    }
    let found = latencies.len();
    if found < min_found() {
        return Err(MeasureError::NoSound { found: found as u32, sent: sent_n });
    }
    let summary = onset::summarize(&latencies).ok_or(MeasureError::NoSound { found: 0, sent: sent_n })?;
    Ok(Measured { frames: summary.median.round() as u32, spread: summary.std_dev, found: found as u32, sent: sent_n, latencies, backlog: None })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const BLOCK: usize = 64;

    /// A synth answering each note-on `delay` frames after its R0 (plus `jitter[k]`) with a fast
    /// attack at `level`, over a quiet floor (`hum` adds a steady tone to the whole return).
    struct Synth {
        delay: u64,
        jitter: Vec<i64>,
        level: f32,
        hum: f32,
        /// Notes after which nothing comes back.
        silent_from: usize,
    }

    struct Run {
        emitted: Vec<(u64, Emit)>,
        result: Option<Result<Measured, MeasureError>>,
    }

    fn noise(at: u64) -> f32 {
        let x = (at.wrapping_mul(2_654_435_761) >> 7) as u32;
        (x as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.000_5
    }

    /// Drive the probe block by block as the audio thread does, the synth answering each on.
    fn run(synth: &Synth, cancel_at: Option<u64>) -> Run {
        let p = ExtProbe::new(RATE);
        assert!(p.arm(7, 2, NOTE, VELOCITY, &[0]));
        let mut ons: Vec<u64> = Vec::new();
        let mut emitted = Vec::new();
        let mut t = 0u64;
        for _ in 0..20_000 {
            if cancel_at.is_some_and(|c| t >= c) {
                p.cancel();
            }
            let input = vec![
                (0..BLOCK as u64)
                    .map(|j| {
                        let at = t + j;
                        let mut x = noise(at) + synth.hum * (at as f32 * 0.05).sin();
                        // Measured notes only answer when not silenced; the warm-up always does.
                        for (k, &on) in ons.iter().enumerate() {
                            if k > 0 && k > synth.silent_from {
                                continue;
                            }
                            let d = (synth.delay as i64 + synth.jitter.get(k).copied().unwrap_or(0)) as u64;
                            let start = on + d;
                            // A fast attack: one loud sample, then a quieter tone for 100 ms.
                            if at == start {
                                x += synth.level * 1.8;
                            } else if at > start && at < start + 4_800 {
                                x += if at.is_multiple_of(2) { synth.level * 0.1 } else { -synth.level * 0.1 };
                            }
                        }
                        x
                    })
                    .collect::<Vec<f32>>(),
            ];
            let before = emitted.len();
            p.block(&input, BLOCK, |e| emitted.push((t, e)));
            for (at, e) in &emitted[before..] {
                if e.bytes[0] & 0xF0 == 0x90 {
                    ons.push(*at);
                }
            }
            t += BLOCK as u64;
            if !p.armed() {
                break;
            }
        }
        Run { emitted, result: p.take_result() }
    }

    fn quiet(delay: u64) -> Synth {
        Synth { delay, jitter: vec![0, 3, -2, 1, 0, -1, 40, 0, -3], level: 0.5, hum: 0.0, silent_from: usize::MAX }
    }

    #[test]
    fn the_median_is_the_round_trip_and_the_warm_up_is_not_counted() {
        // The warm-up answers 300 frames later than every measured note: it must not count.
        let mut synth = quiet(955);
        synth.jitter[0] = 300;
        let r = run(&synth, None);
        let m = r.result.expect("done").expect("measured");
        assert_eq!(m.frames, 955, "{m:?}");
        assert_eq!((m.found, m.sent, m.latencies.len()), (8, 8, 8));
        // One stray note (40 frames late) moves the spread, not the median.
        assert!(m.spread > 10.0 && m.spread < 20.0 && !m.needs_keep(RATE), "{m:?}");
        // Nine notes on and off, on channel 3, then All Notes Off.
        let ons = r.emitted.iter().filter(|(_, e)| e.bytes == [0x92, NOTE, VELOCITY]).count();
        let offs = r.emitted.iter().filter(|(_, e)| e.bytes == [0x82, NOTE, 0]).count();
        assert_eq!((ons, offs), (9, 9));
        assert_eq!(r.emitted.last().map(|(_, e)| e.bytes), Some([0xB2, 123, 0]));
        assert!(r.emitted.iter().all(|(_, e)| e.track == 7));
        // Each on goes out at the block start where it falls due, 1500 ms apart.
        let at: Vec<u64> = r.emitted.iter().filter(|(_, e)| e.bytes[0] == 0x92).map(|(t, _)| *t).collect();
        assert_eq!(at[0], 4800);
        assert!(at.windows(2).all(|w| w[1] - w[0] == 72_000));
    }

    #[test]
    fn a_wide_spread_is_reported_and_needs_keep() {
        let mut synth = quiet(1000);
        synth.jitter = vec![0, 0, 80, -80, 60, -60, 90, -90, 0];
        let m = run(&synth, None).result.unwrap().unwrap();
        assert!(m.needs_keep(RATE), "spread {} ms", m.spread_ms(RATE));
        assert!(m.spread_ms(RATE) > MAX_SPREAD_MS);
    }

    #[test]
    fn too_few_onsets_a_drone_and_a_clipped_attack_are_refused() {
        let mut few = quiet(955);
        few.silent_from = 3;
        assert_eq!(run(&few, None).result.unwrap().unwrap_err(), MeasureError::NoSound { found: 3, sent: 8 });
        let mut half = quiet(955);
        half.silent_from = 4;
        assert_eq!(run(&half, None).result.unwrap().unwrap().found, 4, "half the notes is enough");
        let mut drone = quiet(955);
        drone.hum = 0.3;
        assert!(matches!(run(&drone, None).result.unwrap(), Err(MeasureError::NotQuiet { .. })));
        let mut loud = quiet(955);
        loud.level = 0.55;
        assert!(matches!(run(&loud, None).result.unwrap(), Err(MeasureError::Clipped { .. })));
        // A late answer, beyond the 1 s window, is not an onset.
        assert!(matches!(run(&quiet(49_000), None).result.unwrap(), Err(MeasureError::NoSound { found: 0, .. })));
    }

    #[test]
    fn cancel_sends_the_note_off_and_all_notes_off_and_measures_nothing() {
        // Cancel while the second measured note is held.
        let at = on_at(RATE, 2) + 960;
        let r = run(&quiet(955), Some(at));
        assert!(r.result.is_none(), "nothing measured");
        let tail: Vec<[u8; 3]> = r.emitted.iter().rev().take(2).rev().map(|(_, e)| e.bytes).collect();
        assert_eq!(tail, [[0x82, NOTE, 0], [0xB2, 123, 0]]);
        let ons = r.emitted.iter().filter(|(_, e)| e.bytes[0] == 0x92).count();
        assert_eq!(ons, 3, "the warm-up and two notes");
    }

    #[test]
    fn progress_counts_the_measured_notes_and_arming_is_checked() {
        let p = ExtProbe::new(RATE);
        assert_eq!(p.progress(), Progress { note: 0, of: 8, level: 0.0 });
        assert!(!p.arm(1, 0, NOTE, VELOCITY, &[]), "no return");
        assert!(!p.arm(1, 16, NOTE, VELOCITY, &[0]), "no such channel");
        assert!(p.arm(1, 0, NOTE, VELOCITY, &[0]));
        assert!(!p.arm(1, 0, NOTE, VELOCITY, &[0]), "one run at a time");
        let silence = vec![vec![0.0f32; 480]];
        let mut seen = Vec::new();
        while p.armed() {
            p.block(&silence, 480, |_| {});
            seen.push(p.progress().note);
        }
        assert!(seen.windows(2).all(|w| w[0] <= w[1]));
        assert!(seen.contains(&3));
        assert_eq!(p.progress().note, 8);
        assert!(matches!(p.take_result(), Some(Err(MeasureError::NoSound { found: 0, sent: 8 }))));
        assert!(p.take_result().is_none(), "taken once");
        assert!((p.duration().as_secs_f64() - 13.22).abs() < 0.01, "{:?}", p.duration());
    }
}
