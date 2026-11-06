//! The legacy renderer: one sample across the keyboard, 32 voices, the exponential ADSR, the
//! 4-point cubic `hermite` and the 19 parameters (`Insert::sample` instances).

use super::Sampler;
use crate::params::{Params, choice, lin, log, toggle};
use crate::plugins::instrument_queue::{Adsr, AdsrRates, EventKind, EventQueue, Stage};
use crate::util::Smoother;
use crate::{ParamInfo, Unit, db_to_gain};

const PLAY_MODES: &[&str] = &["Gate", "One Shot"];
const LOOP_MODES: &[&str] = &["Off", "Forward", "Ping-Pong"];

const ROOT: usize = 0;
const TRANSPOSE: usize = 1;
const FINE: usize = 2;
const KEY_TRACK: usize = 3;
const KEY_LOW: usize = 4;
const KEY_HIGH: usize = 5;
const VEL_LOW: usize = 6;
const VEL_HIGH: usize = 7;
const VEL_SENS: usize = 8;
const ATTACK: usize = 9;
const DECAY: usize = 10;
const SUSTAIN: usize = 11;
pub(super) const RELEASE: usize = 12;
const PLAY_MODE: usize = 13;
const START: usize = 14;
const LOOP_MODE: usize = 15;
const LOOP_START: usize = 16;
const LOOP_END: usize = 17;
const LEVEL: usize = 18;

pub(super) static SAMPLER_PARAMS: [ParamInfo; 19] = [
    lin("root_note", "Root Note", 0.0, 127.0, 60.0, Unit::None),
    lin("transpose", "Transpose", -48.0, 48.0, 0.0, Unit::Semitones),
    lin("fine_tune", "Fine Tune", -100.0, 100.0, 0.0, Unit::Cents),
    toggle("key_track", "Key Tracking", true),
    lin("key_low", "Lowest Key", 0.0, 127.0, 0.0, Unit::None),
    lin("key_high", "Highest Key", 0.0, 127.0, 127.0, Unit::None),
    lin("vel_low", "Lowest Velocity", 1.0, 127.0, 1.0, Unit::None),
    lin("vel_high", "Highest Velocity", 1.0, 127.0, 127.0, Unit::None),
    lin("velocity", "Velocity Sens", 0.0, 100.0, 70.0, Unit::Percent),
    log("attack", "Attack", 0.1, 5000.0, 1.0, Unit::Ms),
    log("decay", "Decay", 1.0, 10000.0, 300.0, Unit::Ms),
    lin("sustain", "Sustain", 0.0, 100.0, 100.0, Unit::Percent),
    log("release", "Release", 1.0, 10000.0, 150.0, Unit::Ms),
    choice("play_mode", "Play Mode", PLAY_MODES, 0),
    lin("start", "Sample Start", 0.0, 100.0, 0.0, Unit::Percent),
    choice("loop_mode", "Loop", LOOP_MODES, 0),
    lin("loop_start", "Loop Start", 0.0, 100.0, 0.0, Unit::Percent),
    lin("loop_end", "Loop End", 0.0, 100.0, 100.0, Unit::Percent),
    lin("level", "Level", -48.0, 12.0, 0.0, Unit::Db),
];

pub(super) const VOICES: usize = 32;
/// Fastest playback step in sample frames per output frame (keeps hostile pitch bounded).
const MAX_STEP: f64 = 256.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum LoopMode {
    Off,
    Forward,
    PingPong,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Voice {
    note: u8,
    /// MIDI channel and host voice id (-1: none).
    ch: u8,
    id: i32,
    /// Key still down (gate mode only; one-shot voices are never held).
    held: bool,
    gain: f32,
    /// Read position in sample frames.
    pos: f64,
    step: f64,
    forward: bool,
    looping: bool,
    env: Adsr,
}

impl Voice {
    const fn new() -> Self {
        Self { note: 0, ch: 0, id: -1, held: false, gain: 0.0, pos: 0.0, step: 1.0, forward: true, looping: false, env: Adsr::new() }
    }

    pub(super) fn active(&self) -> bool {
        self.env.stage != Stage::Idle
    }

    pub(super) fn stop(&mut self) {
        *self = Voice::new();
    }
}

/// Values derived from the parameters (refreshed on every change).
#[derive(Debug, Clone, Copy)]
pub(super) struct Derived {
    root: f64,
    /// Transpose plus fine tune, in semitones.
    offset: f64,
    key_track: bool,
    keys: (u8, u8),
    vels: (u8, u8),
    sens: f32,
    pub(super) one_shot: bool,
    start: f64,
    loop_mode: LoopMode,
    /// Loop start and end as fractions of the sample (start <= end).
    loop_span: (f64, f64),
}

/// A MIDI key or velocity parameter as a whole number in `lo..=127`.
fn whole(v: f32, lo: u8) -> u8 {
    if v.is_finite() { v.round().clamp(f32::from(lo), 127.0) as u8 } else { lo }
}

fn ordered(a: u8, b: u8) -> (u8, u8) {
    (a.min(b), a.max(b))
}

/// 4-point cubic (Catmull-Rom) interpolation of `x` at fractional frame `pos` (>= 0). Reads
/// outside the slice are silence.
#[inline]
fn hermite(x: &[f32], pos: f64) -> f32 {
    let i = pos as usize;
    let t = (pos - i as f64) as f32;
    let at = |k: usize| x.get(k).copied().unwrap_or(0.0);
    let x0 = at(i);
    if t == 0.0 {
        return if x0.is_finite() { x0 } else { 0.0 };
    }
    let xm1 = at(i.saturating_sub(1));
    let x1 = at(i.saturating_add(1));
    let x2 = at(i.saturating_add(2));
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    let y = ((c3 * t + c2) * t + c1) * t + x0;
    // A float file can hold NaN or infinities; they play as silence.
    if y.is_finite() { y } else { 0.0 }
}

impl Sampler {
    pub fn new() -> Self {
        let mut s = Self {
            p: Params::new(&SAMPLER_PARAMS),
            d: Derived {
                root: 60.0,
                offset: 0.0,
                key_track: true,
                keys: (0, 127),
                vels: (1, 127),
                sens: 0.7,
                one_shot: false,
                start: 0.0,
                loop_mode: LoopMode::Off,
                loop_span: (0.0, 1.0),
            },
            sr: 48_000.0,
            ch: 0,
            sample: None,
            len: 0,
            sample_sr: 48_000.0,
            voices: [Voice::new(); VOICES],
            events: EventQueue::new(),
            rates: AdsrRates::default(),
            level: Smoother::new(1.0),
            core: None,
        };
        s.update();
        s
    }

    pub(super) fn update(&mut self) {
        let v = |i| self.p.v(i);
        let pct = |i| f64::from(v(i) / 100.0).clamp(0.0, 1.0);
        let (ls, le) = (pct(LOOP_START), pct(LOOP_END));
        self.d = Derived {
            root: f64::from(whole(v(ROOT), 0)),
            offset: f64::from(v(TRANSPOSE)) + f64::from(v(FINE)) / 100.0,
            key_track: self.p.on(KEY_TRACK),
            keys: ordered(whole(v(KEY_LOW), 0), whole(v(KEY_HIGH), 0)),
            vels: ordered(whole(v(VEL_LOW), 1), whole(v(VEL_HIGH), 1)),
            sens: (v(VEL_SENS) / 100.0).clamp(0.0, 1.0),
            one_shot: self.p.choice(PLAY_MODE) == 1,
            start: pct(START),
            loop_mode: match self.p.choice(LOOP_MODE) {
                1 => LoopMode::Forward,
                2 => LoopMode::PingPong,
                _ => LoopMode::Off,
            },
            loop_span: (ls.min(le), ls.max(le)),
        };
        self.rates = AdsrRates::new(v(ATTACK), v(DECAY), v(SUSTAIN) / 100.0, v(RELEASE), self.sr);
        self.level.set(db_to_gain(v(LEVEL)));
    }

    /// The loop region in sample frames, when it is long enough to loop.
    fn loop_frames(&self) -> Option<(f64, f64)> {
        if self.d.loop_mode == LoopMode::Off {
            return None;
        }
        let len = self.len as f64;
        let (a, b) = (self.d.loop_span.0 * len, self.d.loop_span.1 * len);
        (b - a >= 1.0).then_some((a, b))
    }

    fn start_note(&mut self, ch: u8, note: u8, vel: u8, id: i32) {
        if vel == 0 {
            self.stop_note(ch, note, id);
            return;
        }
        let d = self.d;
        let (note, vel) = (note.min(127), vel.min(127));
        if self.len == 0 || note < d.keys.0 || note > d.keys.1 || vel < d.vels.0 || vel > d.vels.1 {
            return;
        }
        let semis = if d.key_track { f64::from(note) - d.root } else { 0.0 } + d.offset;
        let step = f64::from(self.sample_sr) / f64::from(self.sr) * 2f64.powf(semis / 12.0);
        if !step.is_finite() || step <= 0.0 {
            return;
        }
        if !d.one_shot && id < 0 {
            // Retriggering a held key releases the earlier voice instead of stacking it. A note
            // with a voice id is its own voice: the host releases the earlier one by its id.
            self.stop_note(ch, note, -1);
        }
        let idx = self
            .voices
            .iter()
            .position(|v| !v.active())
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| (a.held, a.env.level).partial_cmp(&(b.held, b.env.level)).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
            })
            .unwrap_or(0);
        let start = d.start * self.len.saturating_sub(1) as f64;
        let looping = self.loop_frames().is_some_and(|(_, end)| start < end);
        let gain = (1.0 - d.sens) + d.sens * f32::from(vel) / 127.0;
        if let Some(v) = self.voices.get_mut(idx) {
            *v = Voice { note, ch, id, held: !d.one_shot, gain, pos: start, step: step.min(MAX_STEP), forward: true, looping, env: Adsr::new() };
            v.env.gate_on();
        }
    }

    /// Releases the voice with host id `id`, or without an id the held voices playing `note` on
    /// channel `ch`.
    fn stop_note(&mut self, ch: u8, note: u8, id: i32) {
        for v in self.voices.iter_mut().filter(|v| v.held && v.ch == ch && if id >= 0 { v.id == id } else { v.note == note }) {
            v.held = false;
            v.env.gate_off();
        }
    }

    pub(super) fn apply(&mut self, kind: EventKind) {
        match kind {
            EventKind::On(c, n, v, id) => self.start_note(c, n, v, id),
            EventKind::Off(c, n, id) => self.stop_note(c, n, id),
            EventKind::AllOff => {
                for v in &mut self.voices {
                    v.held = false;
                    v.env.gate_off();
                }
            }
        }
    }

    /// Adds every voice's output for frame `n` into `io` and advances the voices.
    #[inline]
    pub(super) fn render_frame(&mut self, io: &mut [Vec<f32>], ch: usize, n: usize, level: f32) {
        let Some(sample) = self.sample.as_deref() else { return };
        let nch = sample.num_channels();
        let len = self.len as f64;
        let mode = self.d.loop_mode;
        let region = self.loop_frames();
        for v in &mut self.voices {
            if !v.active() {
                continue;
            }
            let a = v.env.next(&self.rates) * v.gain * level;
            for (c, b) in io.iter_mut().take(ch).enumerate() {
                let src = if nch == 1 { 0 } else { c };
                if let (Some(x), Some(data)) = (b.get_mut(n), sample.channel(src)) {
                    *x += hermite(data, v.pos) * a;
                }
            }
            let r = if v.looping { region } else { None };
            advance(v, len, r, mode);
        }
    }
}

/// Moves a voice's read position one output frame on, looping or ending it.
#[inline]
fn advance(v: &mut Voice, len: f64, region: Option<(f64, f64)>, mode: LoopMode) {
    v.pos = if v.forward { v.pos + v.step } else { v.pos - v.step };
    match region {
        Some((ls, le)) if mode == LoopMode::PingPong => {
            if v.forward && v.pos >= le {
                v.pos = (le - (v.pos - le)).max(ls);
                v.forward = false;
            } else if !v.forward && v.pos <= ls {
                v.pos = (ls + (ls - v.pos)).min(le);
                v.forward = true;
            }
        }
        Some((ls, le)) => {
            if v.pos >= le {
                v.pos = ls + (v.pos - ls) % (le - ls);
            }
        }
        None => {
            if v.pos >= len || !v.pos.is_finite() {
                v.stop();
            }
        }
    }
    if !v.pos.is_finite() || v.pos < 0.0 {
        v.stop();
    }
}
