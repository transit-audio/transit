//! The generators locked to a host's transport: the transport-driven random sequencer the MIDI
//! effect plays (#202).
//!
//! **Clock.** The firmware follows a patched clock at a division the rate knob picks (`p/q` of the
//! clock from 1/4 to 4, by the range a quarter or four times that). Here the clock is the beat (a
//! quarter note) and [`RATES`] lists those same divisions, slowest first, by note value. Ticks fall
//! on the grid of the division counted from the song's start (beat 0), so a rate change lands on
//! the new grid and a tempo change keeps it: every time is kept in beats and turned into a sample
//! offset only in the block that plays it. Jitter bends each tick by the firmware's own multiplier,
//! pulled back toward the grid as the firmware pulls its jittery clock toward the straight one;
//! with no jitter every tick is on the grid exactly.
//!
//! **Decisions.** Every random decision is the firmware's code: at a tick the t generator draws its
//! vector, takes the jitter's multiplier and configures its two gate ramps by the model; the X and
//! Y channels draw their voltages through their deja vu sequences, shaped by spread and bias and
//! quantized by steps. What the firmware samples frame by frame, the gates' highs and lows, is
//! computed here from the ramps' slopes, in beats: a Bernoulli ramp's gate ends where its phase
//! passes the pulse width, a divider's gates start and end where its ramp crosses each multiple of
//! `q/p` (and the width past it).
//!
//! **Notes.** Each voice is a gate with its voltage: gate 1 with X1, the clock (t2, high for half a
//! tick) with X2, gate 2 with X3. A gate's rise starts a note at its channel's voltage, a fall ends
//! it, and a pulse that starts while the gate is still high (the firmware's gate then stays high)
//! ends the note and starts the next at the same offset. A voltage becomes a key at a semitone per
//! twelfth of a volt above the base note: the quantized voltage, or below the steps' center (a
//! channel the bump or tilt mode turns that way) the free voltage rounded to the nearest semitone;
//! keys fold into 0 to 127 by octaves. The velocity is the Y channel's voltage, 0 to 5 V across 1
//! to 127, drawn at every tick. A note on a key another voice holds ends that voice's note first.
//!
//! **Determinism.** The state advances only at ticks, in order, so a seed and a transport path
//! give the same events at any host block size. A play start, a locate or a loop restarts the
//! generators at the grid point at or after the position (the firmware's reset, as its clock
//! restarting does); with the seed latched they also restart from the seed, so the same path plays
//! the same notes.

#![forbid(unsafe_code)]

use crate::deja_vu_dead_band;
use crate::float::F;
use crate::output_channel::OutputChannel;
use crate::random::{RandomSequence, RandomStream};
use crate::scales::{CHROMATIC, PRESETS};
use crate::t_generator::{Model, RandomVector, TGenerator};
use crate::xy_generator::{ControlMode, NUM_CHANNELS, VoltageRange};

/// The rates, slowest first: a name by note value and the ticks per beat `(p, q)`. They are the
/// firmware's divisions of a patched clock, the beat as the clock.
pub static RATES: [(&str, (i32, i32)); 21] = [
    ("4/1", (1, 16)),
    ("3/1", (1, 12)),
    ("2/1", (1, 8)),
    ("1/1.", (1, 6)),
    ("1/1", (1, 4)),
    ("1/2.", (1, 3)),
    ("1/1T", (3, 8)),
    ("1/2", (1, 2)),
    ("1/4.", (2, 3)),
    ("1/2T", (3, 4)),
    ("1/4", (1, 1)),
    ("1/8.", (4, 3)),
    ("1/4T", (3, 2)),
    ("1/8", (2, 1)),
    ("1/16.", (8, 3)),
    ("1/8T", (3, 1)),
    ("1/16", (4, 1)),
    ("1/16T", (6, 1)),
    ("1/32", (8, 1)),
    ("1/32T", (12, 1)),
    ("1/64", (16, 1)),
];

/// The voices that play.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Voices {
    /// Gate 1 with X1 and gate 2 with X3 (a coin toss between them at the center bias).
    #[default]
    BothGates,
    /// Gate 1 with X1.
    Gate1,
    /// Gate 2 with X3.
    Gate2,
    /// The clock with X2: a note every tick.
    Clock,
    /// All three.
    All,
}

impl Voices {
    /// Whether voice `v` (0 gate 1, 1 the clock, 2 gate 2) plays.
    #[must_use]
    pub fn plays(self, v: usize) -> bool {
        matches!((self, v), (Self::BothGates, 0 | 2) | (Self::Gate1, 0) | (Self::Gate2, 2) | (Self::Clock, 1) | (Self::All, _))
    }
}

/// What the sequencer plays by (the MIDI effect's parameters, mapped).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Index into [`RATES`].
    pub rate: usize,
    /// The t generator's model.
    pub model: Model,
    /// The t bias, 0 to 1.
    pub t_bias: f32,
    /// 0 to 1.
    pub jitter: f32,
    /// The gate length's mean, 0 to 1 (the firmware's pulse width mean).
    pub gate_length: f32,
    /// The gate length's randomness, 0 to 1.
    pub gate_random: f32,
    /// The voices that play.
    pub voices: Voices,
    /// The X spread, 0 to 1.
    pub spread: f32,
    /// The X bias, 0 to 1.
    pub bias: f32,
    /// Quantization, 0 (every semitone of the scale's chromatic level) to 1 (the root only): the
    /// firmware's steps from its center to its end.
    pub steps: f32,
    /// How the three X channels share spread, bias and steps.
    pub x_mode: ControlMode,
    /// The X voltage range: 2, 5 or 10 octaves.
    pub range: VoltageRange,
    /// 0 to 5 the firmware's scales ([`PRESETS`]), 6 chromatic.
    pub scale: usize,
    /// The key of 0 V, MIDI.
    pub base_note: i32,
    /// The deja vu knob, 0 to 1 (the firmware's dead band around the center locks the loop).
    pub deja_vu: f32,
    /// The loop's length, 1 to 16.
    pub length: i32,
    /// The loop locked (the firmware's locked deja vu state), whatever the knob.
    pub freeze: bool,
    /// Deja vu on the rhythm (the t section).
    pub deja_vu_rhythm: bool,
    /// Deja vu on the pitches (the X section).
    pub deja_vu_pitch: bool,
    /// The Y channel's bias: the velocity's center, 0 to 1.
    pub velocity: f32,
    /// The Y channel's spread: how far velocities stray, 0 to 1.
    pub dynamics: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            rate: 16,
            model: Model::CoinToss,
            t_bias: 0.5,
            jitter: 0.0,
            gate_length: 0.5,
            gate_random: 0.0,
            voices: Voices::BothGates,
            spread: 0.5,
            bias: 0.5,
            steps: 0.5,
            x_mode: ControlMode::Identical,
            range: VoltageRange::Narrow,
            scale: 0,
            base_note: 48,
            deja_vu: 0.0,
            length: 8,
            freeze: false,
            deja_vu_rhythm: true,
            deja_vu_pitch: true,
            velocity: 0.6,
            dynamics: 0.3,
        }
    }
}

/// A note event of the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// Sample offset in the block.
    pub offset: u32,
    /// What it does.
    pub kind: EventKind,
}

/// What an [`Event`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A note starts.
    NoteOn {
        /// MIDI key.
        key: u8,
        /// 1 to 127.
        velocity: u8,
    },
    /// A note ends.
    NoteOff {
        /// MIDI key.
        key: u8,
    },
}

/// The transport of one block, as the host gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    /// The transport plays.
    pub playing: bool,
    /// The block's first sample on the timeline.
    pub samples: i64,
    /// The block's start in beats (quarter notes).
    pub quarters: f64,
    /// Quarter notes per minute, held through the block.
    pub tempo: f64,
    /// Hz.
    pub sample_rate: f64,
    /// The block's length.
    pub frames: u32,
}

const NUM_SCALE_SLOTS: usize = 7;
/// The most events one tick pushes: at most `EDGES` (72) edges, each a start (its On plus the Offs
/// of its voice and of up to two other voices on the same key: 4 pushes) or an end (1 push), so
/// 72 x 4 = 288; the three voices' starts at the tick itself add 3 x 4 = 12 (300 in all). The
/// schedule holds only the current tick's events (`process` emits what is due before each tick),
/// so 384 leaves a margin of 84 over the worst tick.
const PENDING: usize = 384;
/// Slots only an Off may take: an On is refused once the schedule is this close to full, so every
/// note the schedule has begun can still be ended (at most 3 voices, each with one Off, plus the
/// at most 2 other-voice Offs one start pushes before its On).
const OFF_RESERVE: usize = 8;
/// Edges a tick schedules per gate channel at most (a 16-times divider: 16 starts and 16 ends).
const EDGES: usize = 72;
/// Beats closer than this many samples before a sample count as on it (the transport's beat
/// positions differ by rounding from block to block).
const SAMPLE_EPSILON: f64 = 1e-6;
/// The gate channels' voices and voltage channels: gate 1 plays X1 (voice 0), gate 2 X3 (voice 2).
const GATE_VOICE: [usize; 2] = [0, 2];
/// The clock's channel among a tick's edges (the gates are 0 and 1).
const CLOCK: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pending {
    On { voice: usize, key: u8, velocity: u8 },
    Off { voice: usize, key: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Scheduled {
    beat: f64,
    what: Pending,
}

const NO_EVENT: Scheduled = Scheduled { beat: 0.0, what: Pending::Off { voice: 0, key: 0 } };

/// One gate edge inside a tick, at `tau` (0 to 1 of the tick).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Edge {
    tau: f64,
    channel: usize,
    start: bool,
}

const NO_EDGE: Edge = Edge { tau: 0.0, channel: 0, start: false };

/// The generators' state a seed restarts.
#[derive(Clone, Copy, Debug)]
struct Core {
    stream: RandomStream,
    t: TGenerator,
    sequences: [RandomSequence; NUM_CHANNELS],
    channels: [OutputChannel<NUM_SCALE_SLOTS>; NUM_CHANNELS],
}

impl Core {
    fn new(seed: u32) -> Self {
        let mut stream = RandomStream::new(seed);
        let t = TGenerator::new(&mut stream, crate::SAMPLE_RATE);
        let sequences: [RandomSequence; NUM_CHANNELS] = std::array::from_fn(|_| RandomSequence::new(&mut stream));
        let mut channels = [OutputChannel::<NUM_SCALE_SLOTS>::default(); NUM_CHANNELS];
        for c in &mut channels {
            c.init();
            for (i, s) in PRESETS.iter().enumerate() {
                c.load_scale(i, &s.to_scale());
            }
            c.load_scale(PRESETS.len(), &CHROMATIC.to_scale());
        }
        Self { stream, t, sequences, channels }
    }
}

/// The transport-locked random sequencer (see the module documentation). [`Sequencer::process`]
/// and the rest never allocate; a new seed is applied in place.
#[derive(Clone, Debug)]
pub struct Sequencer {
    seed: u32,
    latch: bool,
    core: Core,
    started: bool,
    /// Whether the last block played, and where the next one starts if nothing jumped.
    playing: bool,
    next_samples: i64,
    /// The current tick's grid index (of `k_div`), its beat, and whether it is the first since a
    /// start.
    k: i64,
    k_div: f64,
    tick_beat: f64,
    first_tick: bool,
    /// How far the jittery clock is from the grid, in ticks (the firmware's `phase_difference_`).
    phase_difference: f64,
    /// Ticks since the divider pattern started.
    pattern_tick: i32,
    /// Per voice: the key it holds once the scheduled events play, and the key it holds now.
    scheduled: [Option<u8>; 3],
    sounding: [Option<u8>; 3],
    /// The Y channel's voltage (the velocity), the last drawn.
    y: F,
    pending: [Scheduled; PENDING],
    pending_len: usize,
    dropped: u64,
}

impl Sequencer {
    /// A sequencer at `seed`; with `latch` it restarts from the seed at every start and locate.
    #[must_use]
    pub fn new(seed: u32, latch: bool) -> Self {
        Self {
            seed,
            latch,
            core: Core::new(seed),
            started: false,
            playing: false,
            next_samples: 0,
            k: 0,
            k_div: 0.25,
            tick_beat: 0.0,
            first_tick: true,
            phase_difference: 0.0,
            pattern_tick: 0,
            scheduled: [None; 3],
            sounding: [None; 3],
            y: F::ZERO,
            pending: [NO_EVENT; PENDING],
            pending_len: 0,
            dropped: 0,
        }
    }

    /// The seed and the latch. With the latch on, a new seed is heard from the next start, locate
    /// or loop; with it off the sequence runs on across passes (the module's rule), so a new seed
    /// is heard only while the sequencer has not played yet.
    pub fn set_seed(&mut self, seed: u32, latch: bool) {
        if seed != self.seed && !self.started {
            self.core = Core::new(seed);
        }
        self.seed = seed;
        self.latch = latch;
    }

    /// The seed.
    #[must_use]
    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Events a full schedule refused (never expected, see `PENDING`; counted). The MIDI effect
    /// adds them to the chain's overflow count, which reaches
    /// `MixEngine::event_status().midi_fx.overflow` (no UI reader shows it yet).
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Whether a note sounds.
    #[must_use]
    pub fn sounding(&self) -> bool {
        self.sounding.iter().any(Option::is_some)
    }

    /// Forgets every note and the schedule (the host ended the notes); the next playing block
    /// restarts the generators.
    pub fn reset(&mut self) {
        self.pending_len = 0;
        self.scheduled = [None; 3];
        self.sounding = [None; 3];
        self.playing = false;
    }

    /// Ends the sounding notes at offset 0 and clears the schedule.
    fn release_all(&mut self, emit: &mut impl FnMut(Event)) {
        for v in 0..3 {
            if let Some(key) = self.sounding[v].take() {
                emit(Event { offset: 0, kind: EventKind::NoteOff { key } });
            }
        }
        self.scheduled = [None; 3];
        self.pending_len = 0;
    }

    /// One block: the notes it plays, in order, through `emit`.
    pub fn process(&mut self, s: &Settings, block: &Block, emit: &mut impl FnMut(Event)) {
        let unusable = |x: f64| x.is_nan() || x <= 0.0;
        if !block.playing || block.frames == 0 || unusable(block.tempo) || unusable(block.sample_rate) {
            if self.playing || self.sounding() {
                self.release_all(emit);
            }
            self.playing = false;
            return;
        }
        let bps = block.tempo / 60.0 / block.sample_rate;
        let div = division(s.rate);
        if !self.playing || block.samples != self.next_samples {
            self.release_all(emit);
            self.start(s, block.quarters, div);
        }
        self.playing = true;
        self.next_samples = block.samples + i64::from(block.frames);
        let end = i64::from(block.frames);
        #[allow(clippy::cast_possible_truncation)] // Offsets within a block and a little before it.
        let offset_of = |beat: f64| -> i64 { ((beat - block.quarters) / bps - SAMPLE_EPSILON).ceil().clamp(0.0, 1e12) as i64 };
        // The ticks due in this block, each scheduling its events up to the next tick. Before a
        // tick runs, the events due before its beat go out, so the schedule holds about one tick's
        // events whatever the block size (events at the tick's own beat stay behind it, as they
        // were inserted, so the order is the one of running every tick first).
        let mut guard = 0;
        while offset_of(self.tick_beat) < end && guard < 4096 {
            self.emit_due(self.tick_beat, end, &offset_of, emit);
            self.tick(s, division(s.rate));
            guard += 1;
        }
        // Then the scheduled events due in this block, in time order.
        self.emit_due(f64::INFINITY, end, &offset_of, emit);
    }

    /// Emits the scheduled events before `before` (a beat) that fall inside the block (offset
    /// below `end`), in time order, and takes them off the schedule.
    fn emit_due(&mut self, before: f64, end: i64, offset_of: &impl Fn(f64) -> i64, emit: &mut impl FnMut(Event)) {
        let mut taken = 0;
        while taken < self.pending_len {
            let e = self.pending[taken];
            let at = offset_of(e.beat);
            if e.beat >= before || at >= end {
                break;
            }
            let offset = u32::try_from(at).unwrap_or(0);
            match e.what {
                Pending::On { voice, key, velocity } => {
                    self.sounding[voice] = Some(key);
                    emit(Event { offset, kind: EventKind::NoteOn { key, velocity } });
                }
                Pending::Off { voice, key } => {
                    if self.sounding[voice] == Some(key) {
                        self.sounding[voice] = None;
                    }
                    emit(Event { offset, kind: EventKind::NoteOff { key } });
                }
            }
            taken += 1;
        }
        self.pending.copy_within(taken..self.pending_len, 0);
        self.pending_len -= taken;
    }

    /// A start, a locate or a loop: the generators restart (from the seed when latched) on the grid
    /// point at or after `beat`, with the firmware's reset.
    fn start(&mut self, s: &Settings, beat: f64, div: f64) {
        if self.latch || !self.started {
            self.core = Core::new(self.seed);
        }
        self.started = true;
        #[allow(clippy::cast_possible_truncation)] // A grid index within any session's length.
        let k = (beat / div - 1e-9).ceil() as i64;
        self.k = k;
        self.k_div = div;
        self.tick_beat = k as f64 * div;
        self.phase_difference = 0.0;
        self.first_tick = true;
        self.pattern_tick = 0;
        self.apply(s);
        let c = &mut self.core;
        c.t.jitter_multiplier = F::ONE;
        c.t.reset(&mut c.stream);
        for seq in &mut c.sequences {
            seq.reset();
        }
    }

    /// The settings into the generators, as the firmware's main loop sets them each block.
    fn apply(&mut self, s: &Settings) {
        let knob = deja_vu_dead_band(F(s.deja_vu.clamp(0.0, 1.0)));
        let deja_vu = |on: bool| {
            if !on {
                F::ZERO
            } else if s.freeze {
                F(0.5)
            } else {
                knob
            }
        };
        let t = &mut self.core.t;
        t.set_model(s.model);
        t.set_bias(F(s.t_bias));
        t.set_jitter(F(s.jitter));
        t.set_deja_vu(deja_vu(s.deja_vu_rhythm));
        t.set_length(s.length);
        t.set_pulse_width_mean(F(s.gate_length));
        t.set_pulse_width_std(F(s.gate_random));
        let steps = 0.5 + 0.5 * F(s.steps.clamp(0.0, 1.0));
        let scale = s.scale.min(NUM_SCALE_SLOTS - 1);
        for i in 0..3 {
            let amount = s.x_mode.amount(i);
            let c = &mut self.core.channels[i];
            c.set_scale_offset(s.range.scale_offset());
            c.set_spread(0.5 + (F(s.spread) - 0.5) * amount);
            c.set_bias(0.5 + (F(s.bias) - 0.5) * amount);
            c.set_steps(0.5 + (steps - 0.5) * amount);
            c.set_scale_index(scale);
            c.set_register_mode(false);
            let seq = &mut self.core.sequences[i];
            seq.set_length(s.length);
            seq.set_deja_vu(deja_vu(s.deja_vu_pitch));
        }
        let y = &mut self.core.channels[3];
        y.set_scale_offset(VoltageRange::Positive.scale_offset());
        y.set_spread(F(s.dynamics));
        y.set_bias(F(s.velocity));
        y.set_steps(F::ZERO);
        y.set_scale_index(0);
        y.set_register_mode(false);
        let seq = &mut self.core.sequences[3];
        seq.set_length(1);
        seq.set_deja_vu(F::ZERO);
    }

    /// Schedules `what` at `beat`; false (counted) when there is no room. An Off may use every
    /// slot, an On leaves [`OFF_RESERVE`] of them for Offs.
    fn push(&mut self, beat: f64, what: Pending) -> bool {
        let limit = if matches!(what, Pending::On { .. }) { PENDING - OFF_RESERVE } else { PENDING };
        if self.pending_len >= limit {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        // After every event at or before `beat`: events at one time keep the order they came in.
        let mut at = self.pending_len;
        while at > 0 && self.pending[at - 1].beat > beat {
            at -= 1;
        }
        self.pending.copy_within(at..self.pending_len, at + 1);
        self.pending[at] = Scheduled { beat, what };
        self.pending_len += 1;
        true
    }

    /// Ends the voice's scheduled note at `beat`. The key stays busy until its Off is scheduled
    /// (it is, unless the schedule is full, which the capacity rules out).
    fn end_voice(&mut self, voice: usize, beat: f64) {
        if let Some(key) = self.scheduled[voice]
            && self.push(beat, Pending::Off { voice, key })
        {
            self.scheduled[voice] = None;
        }
    }

    fn start_voice(&mut self, voice: usize, beat: f64, key: u8) {
        if self.pending_len >= PENDING - OFF_RESERVE {
            // No room for a note: refused (counted), nothing else changes; a note the voice
            // holds still gets its Off.
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.end_voice(voice, beat);
        // Another voice on the same key ends first: the instrument cannot tell them apart.
        for other in 0..3 {
            if other != voice && self.scheduled[other] == Some(key) {
                self.end_voice(other, beat);
            }
        }
        if self.scheduled[voice].is_some() || (0..3).any(|o| o != voice && self.scheduled[o] == Some(key)) {
            return; // A note that could not be ended: no second note-on for its key.
        }
        if self.push(beat, Pending::On { voice, key, velocity: velocity_of(self.y) }) {
            self.scheduled[voice] = Some(key);
        }
    }

    /// Channel `i`'s key from its last drawn voltage.
    fn key(&self, s: &Settings, i: usize) -> u8 {
        let c = &self.core.channels[i];
        key_of(if c.steps() >= 0.5 { c.quantized() } else { c.voltage() }, s.base_note)
    }

    /// Draws channel `i`'s next voltage (a tick of its clock) and returns its key.
    fn draw(&mut self, s: &Settings, i: usize) -> u8 {
        let c = &mut self.core;
        c.channels[i].tick(&mut c.sequences[i], &mut c.stream);
        self.key(s, i)
    }

    /// Gate channel `c`'s edges inside this tick (`edges`, unsorted) and whether its gate is high
    /// at the tick (0), from its ramp as the configuration left it.
    fn gate_edges(&self, c: usize, edges: &mut [Edge; EDGES], n: &mut usize) -> bool {
        let r = &self.core.t.slave_ramp[c];
        let pw = f64::from(r.pulse_width.0);
        let mut add = |tau: f64, start: bool| {
            if *n < EDGES {
                edges[*n] = Edge { tau, channel: c, start };
                *n += 1;
            }
        };
        if r.bernoulli {
            // phase(tau) = p0 + ratio tau, its output held at 1; the gate is high below the width.
            let p0 = f64::from(r.phase.0);
            let ratio = f64::from(r.ratio.0);
            if self.core.t.restarted[c] {
                add(0.0, true);
            }
            let high = pw > 0.0 && p0.min(1.0) < pw;
            if high && ratio > 0.0 {
                let off = (pw - p0) / ratio;
                if off < 1.0 {
                    add(off.max(0.0), false);
                }
            }
            high
        } else {
            // phase(tau) = n0 + tau (held below L); gate j starts at j q / p (j q < L p) and ends
            // at (j + pw) q / p.
            let pattern = self.core.t.pattern;
            let ratio = pattern.ratios[c];
            let (p, q) = (i64::from(ratio.p.max(1)), i64::from(ratio.q.max(1)));
            let len = i64::from(pattern.length.max(1));
            let n0 = i64::from(self.pattern_tick);
            let lo = n0 as f64;
            let mut high = false;
            let first = ((n0 - 1) * p / q - 1).max(0);
            let last = (n0 + 1) * p / q + 1;
            for j in first..=last {
                if j * q >= len * p {
                    break;
                }
                let start = (j * q) as f64 / p as f64;
                let stop = (j as f64 + pw) * q as f64 / p as f64;
                if start >= lo && start < lo + 1.0 {
                    add(start - lo, true);
                    high |= start == lo;
                }
                if stop >= lo && stop < lo + 1.0 {
                    add(stop - lo, false);
                }
                high |= start < lo && stop > lo;
            }
            high
        }
    }

    /// One tick at `self.tick_beat`: the t generator's decision, the voltages, and the notes up to
    /// the next tick.
    fn tick(&mut self, s: &Settings, div: f64) {
        self.apply(s);
        let t_beat = self.tick_beat;
        if div != self.k_div {
            // A new rate: the tick's place on the new grid.
            #[allow(clippy::cast_possible_truncation)]
            let k = (t_beat / div).round() as i64;
            self.k = k;
            self.k_div = div;
            self.phase_difference = (t_beat - k as f64 * div) / div;
        }
        // The gate ramps move on by one master period since the last tick.
        if !self.first_tick {
            for r in &mut self.core.t.slave_ramp {
                if r.bernoulli {
                    r.phase += r.ratio;
                } else {
                    r.phase = (r.phase + 1.0).min(r.max_phase);
                }
            }
            self.pattern_tick = self.pattern_tick.saturating_add(1);
        }
        self.first_tick = false;
        let mut v = RandomVector::default();
        {
            let c = &mut self.core;
            c.t.sequence.next_vector(&mut c.stream, &mut v.0);
        }
        #[allow(clippy::cast_possible_truncation)] // The firmware keeps it in single precision.
        let multiplier = self.core.t.jitter_multiplier_for(&v, F(self.phase_difference as f32));
        self.core.t.configure_slave_ramps(&v);
        if self.core.t.pattern_started {
            self.pattern_tick = 0;
        }
        // The next tick: on the grid without jitter (the first grid point at least 1e-3 of a division
        // ahead, so `period` is never clamped after jittered ticks), else where the jittery clock reaches it.
        let jitter = s.jitter > 0.0;
        #[allow(clippy::cast_possible_truncation)]
        let next_k = if jitter { self.k + 1 } else { (t_beat / div + 1e-3).floor() as i64 + 1 };
        let next_beat = if jitter { t_beat + div / f64::from(multiplier.0).max(1e-3) } else { next_k as f64 * div };
        let period = (next_beat - t_beat).max(div * 1e-3);
        let at = |tau: f64| t_beat + tau * period;

        let mut edges = [NO_EDGE; EDGES];
        let mut n = 0;
        let high = [self.gate_edges(0, &mut edges, &mut n), self.gate_edges(1, &mut edges, &mut n)];
        // The clock's fall, half a tick in: every voice's edges in one time order, so a key's state
        // at each edge is the state then.
        edges[n.min(EDGES - 1)] = Edge { tau: 0.5, channel: CLOCK, start: false };
        n = (n + 1).min(EDGES);
        // In time order; at one time ends before starts, gate 1 before gate 2.
        let rank = |e: &Edge| (e.start, e.channel);
        for i in 1..n {
            let mut j = i;
            while j > 0 && (edges[j - 1].tau > edges[j].tau || (edges[j - 1].tau == edges[j].tau && rank(&edges[j - 1]) > rank(&edges[j]))) {
                edges.swap(j - 1, j);
                j -= 1;
            }
        }
        let starts_now = |c: usize| edges[..n].iter().any(|e| e.channel == c && e.start && e.tau == 0.0);

        // The voltages at the tick, in the firmware's channel order: X1 and X3 when their gate
        // starts a pulse now, X2 on the clock, Y every tick.
        let restart = [starts_now(0), starts_now(1)];
        let x1 = restart[0].then(|| self.draw(s, 0));
        let x2 = self.draw(s, 1);
        let x3 = restart[1].then(|| self.draw(s, 2));
        {
            let c = &mut self.core;
            let (voltage, _) = c.channels[3].tick(&mut c.sequences[3], &mut c.stream);
            self.y = voltage;
        }
        for (c, key) in [(0, x1), (1, x3)] {
            let voice = GATE_VOICE[c];
            if !s.voices.plays(voice) {
                self.end_voice(voice, t_beat);
                continue;
            }
            match key {
                Some(key) => self.start_voice(voice, t_beat, key),
                // A gate high without a new pulse (a voice turned on, or a model changed mid-gate)
                // plays its channel's last voltage.
                None if high[c] && self.scheduled[voice].is_none() => {
                    let key = self.key(s, 2 * c);
                    self.start_voice(voice, t_beat, key);
                }
                None if !high[c] => self.end_voice(voice, t_beat),
                None => {}
            }
        }
        if s.voices.plays(1) {
            self.start_voice(1, t_beat, x2);
        } else {
            self.end_voice(1, t_beat);
        }
        for e in edges.iter().take(n) {
            if e.start && e.tau == 0.0 {
                continue;
            }
            if e.channel == CLOCK {
                self.end_voice(1, at(e.tau));
                continue;
            }
            let voice = GATE_VOICE[e.channel];
            if e.start {
                let key = self.draw(s, 2 * e.channel);
                if s.voices.plays(voice) {
                    self.start_voice(voice, at(e.tau), key);
                }
            } else {
                self.end_voice(voice, at(e.tau));
            }
        }

        self.phase_difference = if jitter { (next_beat - next_k as f64 * div) / div } else { 0.0 };
        self.k = next_k;
        self.tick_beat = next_beat;
    }
}

/// Beats per tick of rate `rate` (an index into [`RATES`]; out of range, 1/16).
#[must_use]
pub fn division(rate: usize) -> f64 {
    let (p, q) = RATES.get(rate).map_or((4, 1), |r| r.1);
    f64::from(q) / f64::from(p)
}

/// A voltage's key: `base_note` plus a semitone per twelfth of a volt (rounded half away from
/// zero), folded into 0 to 127 by octaves.
#[must_use]
pub fn key_of(voltage: F, base_note: i32) -> u8 {
    let semis = (voltage.0 * 12.0).round();
    #[allow(clippy::cast_possible_truncation)] // Clamped.
    let semis = if semis.is_finite() { semis.clamp(-1000.0, 1000.0) as i32 } else { 0 };
    let mut n = base_note + semis;
    while n < 0 {
        n += 12;
    }
    while n > 127 {
        n -= 12;
    }
    u8::try_from(n).unwrap_or(60)
}

/// The Y voltage (0 to 5 V) as a velocity, 1 to 127.
#[must_use]
pub fn velocity_of(y: F) -> u8 {
    let v = (1.0 + 126.0 * f64::from(y.0) / 5.0).round().clamp(1.0, 127.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 1 to 127.
    let v = v as u8;
    v
}
