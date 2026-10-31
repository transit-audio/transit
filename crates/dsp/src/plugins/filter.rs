//! Filter (#242, ADR 0038): a multimode state-variable filter insert whose cutoff an ADSR
//! triggered by the track's notes, an LFO, an envelope follower and key tracking move, with an
//! optional amp envelope from the same notes.
//!
//! **Paraphonic.** One filter and one pair of envelopes after the instrument's voice sum: a note-on
//! retriggers the envelopes for every held note (in Legato mode only when nothing was held), the
//! gate stays open while any note is held, and the envelopes release when the last one lets go.
//! The notes come from the mix (`Plugin::reads_track_notes`): what the track's instrument accepted,
//! after the MIDI effect slot, at the same sample offsets.
//!
//! **Timing.** Everything runs per sample on the plug-in's own sample clock, which a host block
//! boundary does not interrupt: a note starts its envelopes at its own frame, and the output for
//! the same input and the same events at the same absolute frames is bit-identical whatever the
//! host block size. The coefficients are worked out every [`CONTROL`] samples of that clock (and
//! at every note-on, so a pluck's first cycle already sees its envelope): `tan` per sample would
//! cost far more than the zipper it avoids at 3 kHz control rate.
//!
//! **Cutoff.** `cutoff · 2^(6·(env·env_amount·velocity + follower·follow_amount + lfo·lfo_amount)`
//! `+ key_track·(key − 60)/12)`: each source's amount is bipolar, 100% moves the cutoff six octaves
//! at full swing, and key tracking at 100% follows the keyboard one octave per octave from middle C.
//! Clamped to 16 Hz .. 0.49·fs.

mod mods;
pub mod svf;
#[cfg(test)]
mod tests;

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_SUSTAIN, Message, PluginEvent};
use crate::params::{Params, choice, lin, log, toggle};
use crate::plugins::instrument_queue::{Adsr, AdsrRates};
use crate::plugins::lfo::{DIVISIONS, Lfo, Shape, division_quarters};
use crate::util::{MAX_CHANNELS, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Transport, Unit, db_to_gain};
use mods::{Follower, Gate, Notes};
use svf::{Coefs, K_BUTTERWORTH, MIN_CUTOFF, Mode, Section, max_cutoff, q_of, soft_clip};

/// Samples between coefficient updates (aligned to the plug-in's sample clock, not to host blocks).
pub const CONTROL: u64 = 16;
/// Octaves a modulation source moves the cutoff at 100% amount and full swing.
pub const MOD_OCTAVES: f32 = 6.0;
/// Events one block may queue (more than the mix's note view holds, 512).
const MAX_QUEUE: usize = 1024;

const P_MODE: usize = 0;
const P_SLOPE: usize = 1;
const P_CUTOFF: usize = 2;
const P_RESONANCE: usize = 3;
const P_DRIVE: usize = 4;
const P_ENV_AMOUNT: usize = 5;
const P_ENV_A: usize = 6;
const P_ENV_D: usize = 7;
const P_ENV_S: usize = 8;
const P_ENV_R: usize = 9;
const P_ENV_VEL: usize = 10;
const P_RETRIGGER: usize = 11;
const P_LFO_AMOUNT: usize = 12;
const P_LFO_SHAPE: usize = 13;
const P_LFO_SYNC: usize = 14;
const P_LFO_RATE: usize = 15;
const P_LFO_DIV: usize = 16;
const P_LFO_RETRIG: usize = 17;
const P_FOLLOW_AMOUNT: usize = 18;
const P_FOLLOW_A: usize = 19;
const P_FOLLOW_R: usize = 20;
const P_KEY: usize = 21;
const P_AMP_ON: usize = 22;
const P_AMP_A: usize = 23;
const P_AMP_D: usize = 24;
const P_AMP_S: usize = 25;
const P_AMP_R: usize = 26;
const P_AMP_VEL: usize = 27;
const P_OUTPUT: usize = 28;

static FILTER_PARAMS: [ParamInfo; 29] = [
    choice("mode", "Mode", &["Low pass", "High pass", "Band pass", "Notch"], 0),
    choice("slope", "Slope", &["12 dB", "24 dB"], 0),
    log("cutoff", "Cutoff", 20.0, 20_000.0, 1000.0, Unit::Hz),
    lin("resonance", "Resonance", 0.0, 100.0, 20.0, Unit::Percent),
    lin("drive", "Drive", 0.0, 24.0, 0.0, Unit::Db),
    lin("env_amount", "Env Amount", -100.0, 100.0, 0.0, Unit::Percent),
    log("env_attack", "Env Attack", 0.5, 10_000.0, 5.0, Unit::Ms),
    log("env_decay", "Env Decay", 1.0, 20_000.0, 300.0, Unit::Ms),
    lin("env_sustain", "Env Sustain", 0.0, 100.0, 0.0, Unit::Percent),
    log("env_release", "Env Release", 1.0, 20_000.0, 300.0, Unit::Ms),
    lin("env_velocity", "Env Velocity", 0.0, 100.0, 50.0, Unit::Percent),
    choice("retrigger", "Retrigger", &["Every note", "Legato"], 0),
    lin("lfo_amount", "LFO Amount", -100.0, 100.0, 0.0, Unit::Percent),
    choice("lfo_shape", "LFO Shape", &["Sine", "Triangle", "Saw", "Square", "Random"], 0),
    toggle("lfo_sync", "LFO Sync", false),
    log("lfo_rate", "LFO Rate", 0.05, 40.0, 1.0, Unit::Hz),
    choice("lfo_division", "LFO Division", &DIVISIONS, 4),
    toggle("lfo_retrigger", "LFO Retrigger", false),
    lin("follow_amount", "Follow Amount", -100.0, 100.0, 0.0, Unit::Percent),
    log("follow_attack", "Follow Attack", 0.1, 500.0, 10.0, Unit::Ms),
    log("follow_release", "Follow Release", 1.0, 5000.0, 200.0, Unit::Ms),
    lin("key_track", "Key Track", -100.0, 100.0, 0.0, Unit::Percent),
    toggle("amp_on", "Amp Envelope", false),
    log("amp_attack", "Amp Attack", 0.5, 10_000.0, 2.0, Unit::Ms),
    log("amp_decay", "Amp Decay", 1.0, 20_000.0, 300.0, Unit::Ms),
    lin("amp_sustain", "Amp Sustain", 0.0, 100.0, 100.0, Unit::Percent),
    log("amp_release", "Amp Release", 1.0, 20_000.0, 200.0, Unit::Ms),
    lin("amp_velocity", "Amp Velocity", 0.0, 100.0, 50.0, Unit::Percent),
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

pub static FILTER_INFO: PluginInfo = PluginInfo {
    id: "filter",
    name: "Filter",
    short_name: "Filter",
    category: Category::Eq,
    params: &FILTER_PARAMS,
    is_instrument: false,
    // It plays from the track's notes, which an offline process does not have.
    audiosuite: false,
};

/// One note-stream event at a sample offset.
#[derive(Debug, Clone, Copy)]
enum Kind {
    On { channel: u8, key: u8, velocity: u8, voice: i32 },
    Off { channel: u8, key: u8, voice: i32 },
    Pedal(bool),
    AllOff,
}

impl Kind {
    fn from_event(ev: &PluginEvent) -> Option<Kind> {
        let channel = ev.channel & 0x0F;
        Some(match ev.message {
            Message::NoteOn { key, velocity, .. } => Kind::On { channel, key: key.min(127), velocity: velocity.min(127), voice: ev.note_id },
            Message::NoteOff { key, .. } => Kind::Off { channel, key: key.min(127), voice: ev.note_id },
            Message::Control { cc: CC_SUSTAIN, value } => Kind::Pedal(value >= 64),
            Message::Control { cc: CC_ALL_NOTES_OFF | CC_ALL_SOUND_OFF, .. } => Kind::AllOff,
            _ => return None,
        })
    }

    fn is_release(&self) -> bool {
        matches!(self, Kind::Off { .. } | Kind::Pedal(false) | Kind::AllOff)
    }
}

#[derive(Debug, Clone, Copy)]
struct Timed {
    offset: usize,
    kind: Kind,
}

const NO_EVENT: Timed = Timed { offset: 0, kind: Kind::AllOff };

/// The Filter insert. See the module documentation.
pub struct Filter {
    p: Params,
    sr: f32,
    ch: usize,
    /// Samples processed since the last reset: the control-rate grid and nothing else.
    clock: u64,
    /// A note-on since the last coefficient update: work them out at the next sample.
    dirty: bool,
    queue: [Timed; MAX_QUEUE],
    queued: usize,
    notes: Notes,
    env: Adsr,
    env_rates: AdsrRates,
    amp: Adsr,
    amp_rates: AdsrRates,
    /// The velocity of the note that last triggered the envelopes.
    velocity: u8,
    lfo: Lfo,
    /// LFO cycles per sample, from the rate or the tempo.
    lfo_inc: f64,
    follower: Follower,
    /// Smoothed per sample: log2 of the cutoff knob, `k = 1/Q`, the drive and output gains.
    cutoff: Smoother,
    k: Smoother,
    drive: Smoother,
    output: Smoother,
    coefs: [Coefs; 2],
    /// Per channel: the first and the 24 dB slope's second section.
    state: Vec<[Section; 2]>,
    tempo: f64,
    bar_quarters: f64,
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        let mut f = Filter {
            p: Params::new(&FILTER_PARAMS),
            sr: 48_000.0,
            ch: 2,
            clock: 0,
            dirty: true,
            queue: [NO_EVENT; MAX_QUEUE],
            queued: 0,
            notes: Notes::new(),
            env: Adsr::new(),
            env_rates: AdsrRates::default(),
            amp: Adsr::new(),
            amp_rates: AdsrRates::default(),
            velocity: 127,
            lfo: Lfo::new(),
            lfo_inc: 0.0,
            follower: Follower::default(),
            cutoff: Smoother::new(1000f32.log2()),
            k: Smoother::new(1.0 / q_of(20.0)),
            drive: Smoother::new(1.0),
            output: Smoother::new(1.0),
            coefs: [Coefs::default(); 2],
            state: vec![[Section::default(); 2]; 2],
            tempo: 120.0,
            bar_quarters: 4.0,
        };
        f.update();
        f
    }

    /// Pushes the parameter values to the smoothers, envelopes and LFO rate.
    fn update(&mut self) {
        let p = &self.p;
        self.cutoff.set(p.v(P_CUTOFF).max(MIN_CUTOFF).log2());
        self.k.set(1.0 / q_of(p.v(P_RESONANCE)));
        self.drive.set(db_to_gain(p.v(P_DRIVE)));
        self.output.set(db_to_gain(p.v(P_OUTPUT)));
        let pct = |i: usize| p.v(i) / 100.0;
        self.env_rates = AdsrRates::new(p.v(P_ENV_A), p.v(P_ENV_D), pct(P_ENV_S), p.v(P_ENV_R), self.sr);
        self.amp_rates = AdsrRates::new(p.v(P_AMP_A), p.v(P_AMP_D), pct(P_AMP_S), p.v(P_AMP_R), self.sr);
        self.follower.set_times(p.v(P_FOLLOW_A), p.v(P_FOLLOW_R), self.sr);
        self.lfo_inc = if p.on(P_LFO_SYNC) {
            let quarters = division_quarters(p.choice(P_LFO_DIV), self.bar_quarters);
            self.tempo / 60.0 / quarters / f64::from(self.sr)
        } else {
            f64::from(p.v(P_LFO_RATE)) / f64::from(self.sr)
        };
    }

    fn push(&mut self, offset: usize, kind: Kind) -> bool {
        // The last entries take only releases, so a note-off is never refused while note-ons fill it.
        let room = if kind.is_release() { MAX_QUEUE } else { MAX_QUEUE - MAX_QUEUE / 4 };
        if self.queued >= room {
            return false;
        }
        if let Some(slot) = self.queue.get_mut(self.queued) {
            *slot = Timed { offset, kind };
            self.queued += 1;
        }
        true
    }

    /// The block's events in offset order (stable, allocation-free: blocks are short and nearly sorted).
    fn sort_queue(&mut self) {
        let q = self.queue.get_mut(..self.queued).unwrap_or_default();
        for i in 1..q.len() {
            let mut j = i;
            while j > 0 && q.get(j - 1).zip(q.get(j)).is_some_and(|(a, b)| a.offset > b.offset) {
                q.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    fn apply(&mut self, kind: Kind) {
        let legato = self.p.choice(P_RETRIGGER) == 1;
        let gate = match kind {
            Kind::On { channel, key, velocity, voice } => self.notes.on(channel, key, velocity, voice, legato),
            Kind::Off { channel, key, voice } => self.notes.off(channel, key, voice),
            Kind::Pedal(down) => self.notes.pedal(down),
            Kind::AllOff => self.notes.all_off(),
        };
        if matches!(kind, Kind::On { .. }) {
            // Key tracking follows every note-on, legato or not.
            self.dirty = true;
        }
        self.gate(gate);
    }

    fn gate(&mut self, gate: Gate) {
        match gate {
            Gate::Trigger(velocity) => {
                self.velocity = velocity;
                self.env.gate_on();
                self.amp.gate_on();
                if self.p.on(P_LFO_RETRIG) {
                    self.lfo.retrigger();
                }
            }
            Gate::Release => {
                self.env.gate_off();
                self.amp.gate_off();
            }
            Gate::Unchanged => {}
        }
    }

    /// A velocity's scale of an envelope: `amount` (0..1) of the way from 1 to velocity/127.
    fn vel_scale(&self, amount: f32) -> f32 {
        1.0 - amount + amount * f32::from(self.velocity) / 127.0
    }

    /// The modulated cutoff in Hz, from the sources' values now.
    fn cutoff_hz(&self, env: f32) -> f32 {
        let p = &self.p;
        let pct = |i: usize| p.v(i) / 100.0;
        let mut oct = MOD_OCTAVES * (pct(P_ENV_AMOUNT) * env * self.vel_scale(pct(P_ENV_VEL)));
        oct += MOD_OCTAVES * pct(P_FOLLOW_AMOUNT) * self.follower.value();
        let lfo_amount = pct(P_LFO_AMOUNT);
        if lfo_amount != 0.0 {
            oct += MOD_OCTAVES * lfo_amount * self.lfo.value(Shape::from_index(p.choice(P_LFO_SHAPE)));
        }
        oct += pct(P_KEY) * (f32::from(self.notes.last_key) - 60.0) / 12.0;
        (self.cutoff.value() + oct).exp2().clamp(MIN_CUTOFF, max_cutoff(self.sr))
    }

    fn design(&mut self, env: f32) {
        let g = Coefs::g_of(self.cutoff_hz(env), self.sr);
        self.coefs = [Coefs::new(g, self.k.value()), Coefs::new(g, K_BUTTERWORTH)];
    }
}

impl Plugin for Filter {
    fn info(&self) -> &'static PluginInfo {
        &FILTER_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if self.p.set(id, value).is_none() {
            return false;
        }
        self.update();
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.state = vec![[Section::default(); 2]; self.ch.clamp(1, MAX_CHANNELS)];
        for s in [&mut self.cutoff, &mut self.k, &mut self.drive, &mut self.output] {
            s.set_time(20.0, self.sr);
        }
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.clock = 0;
        self.dirty = true;
        self.queued = 0;
        self.notes.reset();
        self.env = Adsr::new();
        self.amp = Adsr::new();
        self.velocity = 127;
        self.lfo = Lfo::new();
        self.follower.level = 0.0;
        for s in [&mut self.cutoff, &mut self.k, &mut self.drive, &mut self.output] {
            s.snap();
        }
        self.state.iter_mut().for_each(|s| *s = [Section::default(); 2]);
    }

    fn set_transport(&mut self, t: &Transport) {
        let tempo = if t.tempo.is_finite() && t.tempo > 0.0 { t.tempo.clamp(1.0, 999.0) } else { 120.0 };
        let bar = if t.numerator > 0 && t.denominator > 0 { f64::from(t.numerator) * 4.0 / f64::from(t.denominator) } else { 4.0 };
        if tempo != self.tempo || bar != self.bar_quarters {
            self.tempo = tempo;
            self.bar_quarters = bar.clamp(0.25, 64.0);
            self.update();
        }
    }

    fn reads_track_notes(&self) -> bool {
        true
    }

    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        Kind::from_event(ev).is_none_or(|kind| self.push(offset as usize, kind))
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.push(offset, Kind::On { channel: 0, key: note.min(127), velocity: velocity.min(127), voice: -1 });
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.push(offset, Kind::Off { channel: 0, key: note.min(127), voice: -1 });
    }

    /// Lets go of every note now (the mix calls it for each block it skips this insert).
    fn all_notes_off(&mut self) {
        self.queued = 0;
        let g = self.notes.all_off();
        self.gate(g);
    }

    fn tail_samples(&self) -> usize {
        let release = self.p.v(P_ENV_R).max(self.p.v(P_AMP_R));
        ms_to_samples(release + 100.0, self.sr) as usize
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len()).min(self.state.len());
        let frames = frames_in(io, frames, ch);
        self.sort_queue();
        let mode = Mode::from_index(self.p.choice(P_MODE));
        let slope24 = self.p.choice(P_SLOPE) == 1;
        let amp_on = self.p.on(P_AMP_ON);
        let amp_vel = self.p.v(P_AMP_VEL) / 100.0;
        let mut next = 0;
        for n in 0..frames {
            // Events due at this frame (and, at the last frame, any past the block) first.
            while let Some(e) = self.queue.get(next).copied().filter(|_| next < self.queued) {
                if e.offset > n && n + 1 < frames {
                    break;
                }
                self.apply(e.kind);
                next += 1;
            }
            self.cutoff.next();
            self.k.next();
            let drive = self.drive.next();
            let out = self.output.next();
            let mut peak = 0.0f32;
            for c in io.iter().take(ch) {
                if let Some(x) = c.get(n) {
                    peak = peak.max(x.abs());
                }
            }
            self.follower.tick(peak);
            let env = self.env.next(&self.env_rates);
            let amp = self.amp.next(&self.amp_rates);
            self.lfo.advance(self.lfo_inc);
            if self.dirty || self.clock.is_multiple_of(CONTROL) {
                self.design(env);
                self.dirty = false;
            }
            let gain = out * if amp_on { amp * self.vel_scale(amp_vel) } else { 1.0 };
            let [c0, c1] = self.coefs;
            for (buf, st) in io.iter_mut().take(ch).zip(self.state.iter_mut()) {
                let Some(x) = buf.get_mut(n) else { continue };
                let v0 = soft_clip(*x * drive);
                let mut y = st[0].tick(&c0, mode, v0);
                if slope24 {
                    y = st[1].tick(&c1, mode, y);
                }
                *x = y * gain;
            }
            self.clock = self.clock.wrapping_add(1);
        }
        // A block of no frames still takes its events.
        while next < self.queued {
            if let Some(e) = self.queue.get(next).copied() {
                self.apply(e.kind);
            }
            next += 1;
        }
        self.queued = 0;
    }

    #[cfg(feature = "testing")]
    fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        h.write_u64(self.clock);
        h.write_u32(self.env.level.to_bits());
        h.write_u32(self.amp.level.to_bits());
        h.write_u32(self.follower.level.to_bits());
        let (phase, step, rng) = self.lfo.bits();
        h.write_u64(phase);
        h.write_u32(step);
        h.write_u32(rng);
        for st in &self.state {
            for s in st {
                let [a, b] = s.bits();
                h.write_u32(a);
                h.write_u32(b);
            }
        }
    }
}

/// The Filter's static magnitude response in dB at `freqs` for the parameter values `params`
/// (ids and values; missing ones take their defaults), at sample rate `sr`: the cutoff knob without
/// modulation. For the panel's curve.
pub fn filter_response(params: &[(&str, f32)], freqs: &[f32], sr: f32) -> Vec<f32> {
    let mut p = Params::new(&FILTER_PARAMS);
    for (id, v) in params {
        p.set(id, *v);
    }
    let mode = Mode::from_index(p.choice(P_MODE));
    let slope24 = p.choice(P_SLOPE) == 1;
    let q = f64::from(q_of(p.v(P_RESONANCE)));
    let sr = f64::from(sane_sr(sr));
    freqs
        .iter()
        .map(|f| {
            let m = svf::magnitude(mode, slope24, f64::from(p.v(P_CUTOFF)), q, sr, f64::from(*f).clamp(1.0, 0.499 * sr));
            (20.0 * m.max(1e-9).log10()) as f32
        })
        .collect()
}
