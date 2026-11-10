//! One voice: a zone's region played at a step, its amp envelope, its shaper and matrix (through the
//! shaper's external amp-gain seam, so the amp envelope applies once), the declick, level and pan,
//! and the choke fade after everything.
//!
//! Per output frame: the amp envelope's gain; the cubic read of each source channel; the shaper's
//! filter (when the zone's shaper is on; its envelopes run either way); `y * amp * velocity_gain`
//! (the order of the shaper's own amp multiply); the held-voice switch crossfade when the shaper was
//! switched (the shaper's 12-frame rule); the declick ramp; level and pan; the choke fade; then the
//! phase moves on. Modulation (the matrix, the filter's coefficients, the LFOs, the step's pitch,
//! level and pan) is worked out on the voice's own 12-frame grid, which starts at its note and
//! persists across host blocks, so the result does not depend on where blocks end. While its zone's
//! level and pan glide ([`crate::SamplerCore::set_zone_glide`]) the gains follow the glide every
//! frame, with the matrix's Level and Pan sums of the last grid point.

use transit_instrument_core::matrix::{NDST, NSRC, PITCH_SEMITONES, RATE_OCTAVES, dst, src};
use transit_instrument_core::shaper::{Fade, FilterMods, Shaper};

use crate::envelope::{Envelope, Ramp};
use crate::patch::{Derived, PlayMode, Zone};
use crate::playhead::{Playhead, Region};
use crate::reader::{RegionReader, cubic};
use crate::{CHOKE_FRAMES, MAX_STEP, MOD_BLOCK, math};

/// Who holds a voice: the note that started it, or a direct pad press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    Note { channel: u8, key: u8, id: i32 },
    Press { pad: u8 },
}

/// Channel controllers and rack macros: matrix sources every voice reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Controls {
    pub wheel: f32,
    pub bend: f32,
    pub aftertouch: f32,
    pub poly: [f32; 128],
    pub sustain: bool,
    pub macros: [f32; 8],
}

impl Controls {
    pub(crate) const fn new() -> Controls {
        Controls { wheel: 0.0, bend: 0.0, aftertouch: 0.0, poly: [0.0; 128], sustain: false, macros: [0.0; 8] }
    }
}

/// The transport at a frame, for the voice's modulation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Transport {
    pub tempo: f64,
    pub quarters: Option<f64>,
    /// The absolute output frame (a zone's level and pan glide is keyed to it).
    pub frame: u64,
}

/// What a voice starts with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Start {
    pub zone: u16,
    pub pad: Option<u8>,
    pub owner: Owner,
    pub key: u8,
    pub velocity: u8,
    pub serial: u64,
    /// Source frames per output frame before pitch: source rate / session rate.
    pub ratio: f64,
    pub channels: u8,
    /// Free-running LFO phases at the note (cycles).
    pub free: [f64; 2],
}

#[derive(Debug, Clone)]
pub(crate) struct Voice {
    pub active: bool,
    pub zone: u16,
    pub pad: Option<u8>,
    pub owner: Owner,
    pub key: u8,
    /// 0..1.
    velocity: f32,
    release_velocity: f32,
    /// The start order (render and steal order).
    pub serial: u64,
    /// The release order (0: not released): the oldest release is stolen first.
    pub released: u64,
    /// The key (or press) is down.
    pub held: bool,
    /// Released while the pedal was down.
    pub sustained: bool,
    pub one_shot: bool,
    head: Playhead,
    region: Region,
    ratio: f64,
    /// Semitones from the key, root, transpose and cents (before the matrix).
    semis: f64,
    base_step: f64,
    step: f64,
    env: Envelope,
    vel_gain: f32,
    declick: u32,
    /// Frames rendered since the note.
    age: u64,
    gains: (f32, f32),
    /// The matrix's Level and Pan sums at the last grid point (a gliding zone's gains are worked
    /// out per frame from them).
    level_mod: f32,
    pan_mod: f32,
    pub fade: Option<Ramp>,
    /// Frames left on the modulation grid (0: modulate before the next frame).
    grid: u32,
    shaped: bool,
    switch: Fade,
    switch_k: u32,
    channels: u8,
    /// A tap was missing in this block.
    pub missed: bool,
    shaper: [Shaper; 2],
}

/// Left and right balance gains for `pan` (-1..1), unity at center.
fn balance(pan: f32) -> (f32, f32) {
    let p = if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 };
    if p > 0.0 {
        (1.0 - p, 1.0)
    } else if p < 0.0 {
        (1.0, 1.0 + p)
    } else {
        (1.0, 1.0)
    }
}

/// The cubic read of channel `ch` through `taps` at fraction `frac` (a tap outside the region reads
/// zero; a tap the source does not have now reads zero and sets `missed`).
#[inline]
fn read(source: Option<&dyn RegionReader>, ch: usize, taps: [Option<u64>; 4], frac: f32, missed: &mut bool) -> f32 {
    let mut at = [0.0f32; 4];
    for (a, tap) in at.iter_mut().zip(taps) {
        if let Some(f) = tap {
            match source.and_then(|s| s.sample(ch, f)) {
                Some(v) => *a = v,
                None => *missed = true,
            }
        }
    }
    cubic(at[0], at[1], at[2], at[3], frac)
}

impl Voice {
    /// An idle voice whose shapers draw from `seed`.
    pub(crate) fn new(seed: u32) -> Voice {
        Voice {
            active: false,
            zone: 0,
            pad: None,
            owner: Owner::Press { pad: 0 },
            key: 0,
            velocity: 0.0,
            release_velocity: 0.0,
            serial: 0,
            released: 0,
            held: false,
            sustained: false,
            one_shot: false,
            head: Playhead { pos: 0.0, reverse: false, up: true, looping: false, wrapped: false, done: true, crossfade: 0 },
            region: Region::whole(0, 0),
            ratio: 1.0,
            semis: 0.0,
            base_step: 1.0,
            step: 1.0,
            env: Envelope::new(),
            vel_gain: 1.0,
            declick: 0,
            age: 0,
            gains: (1.0, 1.0),
            level_mod: 0.0,
            pan_mod: 0.0,
            fade: None,
            grid: 0,
            shaped: false,
            switch: Fade::None,
            switch_k: 0,
            channels: 1,
            missed: false,
            shaper: [Shaper::new(seed), Shaper::new(seed ^ 0x5A5A_5A5A)],
        }
    }

    /// Back to the reset state (idle, random state reseeded).
    pub(crate) fn reset(&mut self) {
        self.cut();
        for s in &mut self.shaper {
            s.reset();
        }
    }

    /// Ends the voice at once.
    pub(crate) fn cut(&mut self) {
        self.active = false;
        self.held = false;
        self.sustained = false;
        self.fade = None;
        self.pad = None;
        self.env = Envelope::new();
        for s in &mut self.shaper {
            s.rest();
        }
    }

    /// Starts the voice on zone `z` (an earlier note on this voice is cut at this frame). Returns
    /// false (the voice stays idle) if the step is not playable.
    pub(crate) fn start(&mut self, st: &Start, z: &Zone, d: &Derived) -> bool {
        let p = &z.playback;
        let semis = if p.key_tracking { f64::from(st.key) - f64::from(p.root) } else { 0.0 } + f64::from(p.transpose) + f64::from(p.cents) / 100.0;
        let step = st.ratio * math::pow(2.0, semis / 12.0);
        if !step.is_finite() || step <= 0.0 {
            return false;
        }
        self.cut();
        let one_shot = p.mode == PlayMode::OneShot;
        let velocity = f32::from(st.velocity.min(127));
        self.active = true;
        self.zone = st.zone;
        self.pad = st.pad;
        self.owner = st.owner;
        self.key = st.key;
        self.velocity = velocity / 127.0;
        self.release_velocity = 0.0;
        self.serial = st.serial;
        self.released = 0;
        self.held = true;
        self.one_shot = one_shot;
        self.region = z.region;
        self.head = Playhead::start(&z.region, p.reverse, !one_shot);
        self.ratio = st.ratio;
        self.semis = semis;
        self.base_step = step.min(MAX_STEP);
        self.step = self.base_step;
        self.env.start(&d.env);
        self.vel_gain = (1.0 - p.velocity_sensitivity) + p.velocity_sensitivity * velocity / 127.0;
        let rendered = self.head.frames_left(&z.region, self.step);
        self.declick = if self.head.looping { d.declick } else { d.declick.min(u32::try_from(rendered / 2).unwrap_or(u32::MAX)) };
        self.age = 0;
        self.gains = (d.level, d.level);
        self.level_mod = 0.0;
        self.pan_mod = 0.0;
        self.grid = 0;
        self.shaped = z.shaper.on;
        self.switch = Fade::None;
        self.switch_k = 0;
        self.channels = st.channels;
        for s in &mut self.shaper {
            s.note(&z.shaper, self.velocity, st.free);
        }
        true
    }

    /// The key (or press) let go: a Gate voice releases, or holds on while the pedal is down; a
    /// one-shot ignores it.
    pub(crate) fn key_up(&mut self, d: &Derived, pedal: bool, velocity: u8, order: u64) {
        if !self.active || !self.held {
            return;
        }
        self.held = false;
        self.release_velocity = f32::from(velocity.min(127)) / 127.0;
        if self.one_shot {
            return;
        }
        if pedal {
            self.sustained = true;
        } else {
            self.release(d, order);
        }
    }

    /// Releases a Gate voice (one-shots ignore it).
    pub(crate) fn release(&mut self, d: &Derived, order: u64) {
        if !self.active || self.one_shot {
            return;
        }
        self.held = false;
        self.sustained = false;
        if self.released == 0 {
            self.released = order;
        }
        self.env.release(&d.env);
        for s in &mut self.shaper {
            s.release();
        }
    }

    /// Starts the 32-frame fade that ends the voice (a running fade keeps going). Returns whether a
    /// new fade started.
    pub(crate) fn choke(&mut self, order: u64) -> bool {
        if !self.active || self.fade.is_some() {
            return false;
        }
        self.fade = Some(Ramp::new(CHOKE_FRAMES));
        if self.released == 0 {
            self.released = order;
        }
        true
    }

    /// The source index the voice reads.
    pub(crate) fn source(&self) -> u16 {
        self.region.source
    }

    /// The voice is ending (releasing or fading): stolen before sounding voices.
    pub(crate) fn is_ending(&self) -> bool {
        self.fade.is_some() || self.env.is_releasing()
    }

    /// The matrix's source values at a grid point.
    fn sources(&self, z: &Zone, c: &Controls) -> [f32; NSRC] {
        let mut v = [0.0f32; NSRC];
        let mut set = |i: usize, x: f32| {
            if let Some(slot) = v.get_mut(i) {
                *slot = x;
            }
        };
        let [s0, _] = &self.shaper;
        if z.matrix.reads(src::LFO1) {
            set(src::LFO1, s0.lfo_value(&z.shaper, 0));
        }
        if z.matrix.reads(src::LFO2) {
            set(src::LFO2, s0.lfo_value(&z.shaper, 1));
        }
        set(src::AMP_ENV, self.env.level());
        set(src::FILTER_ENV, s0.filter_env.level);
        set(src::VELOCITY, self.velocity);
        set(src::KEY, ((f32::from(self.key) - 60.0) / 60.0).clamp(-1.0, 1.0));
        set(src::RELEASE_VELOCITY, self.release_velocity);
        set(src::AFTERTOUCH, c.aftertouch);
        set(src::POLY_AFTERTOUCH, c.poly.get(usize::from(self.key)).copied().unwrap_or(0.0));
        set(src::MOD_WHEEL, c.wheel);
        set(src::PITCH_BEND, c.bend);
        set(src::RANDOM, s0.random);
        for (i, x) in c.macros.iter().enumerate() {
            set(src::MACRO1 + i, *x);
        }
        v
    }

    /// One grid point: the switch, the anchor, the matrix, the step, the gains, the filter and the LFOs.
    fn modulate(&mut self, z: &Zone, d: &Derived, c: &Controls, t: Transport) {
        if self.switch == Fade::Out {
            self.shaper.iter_mut().for_each(Shaper::rest_filter);
        }
        self.switch = Fade::None;
        if z.shaper.on != self.shaped {
            // The held-voice rule: a sounding voice follows the switch, crossfading this grid block.
            self.shaped = z.shaper.on;
            self.switch = if z.shaper.on { Fade::In } else { Fade::Out };
            if z.shaper.on {
                self.shaper.iter_mut().for_each(Shaper::rest_filter);
            }
        }
        self.switch_k = 0;
        let s = &z.shaper;
        if let Some(q) = t.quarters {
            self.shaper[0].anchor(s, q);
        }
        let sums = if z.matrix.is_empty() { [0.0; NDST] } else { z.matrix.evaluate(&self.sources(z, c)) };
        let sum = |i: usize| sums.get(i).copied().unwrap_or(0.0);
        let pitch = sum(dst::PITCH);
        let step = if pitch == 0.0 {
            self.base_step
        } else {
            let semis = (self.semis + f64::from(PITCH_SEMITONES * pitch)).clamp(-96.0, 96.0);
            (self.ratio * math::pow(2.0, semis / 12.0)).min(MAX_STEP)
        };
        if step.is_finite() && step > 0.0 {
            self.step = step;
        }
        self.level_mod = sum(dst::LEVEL);
        self.pan_mod = sum(dst::PAN);
        self.gains = self.gains_for(d.level, z.playback.pan);
        let [s0, s1] = &mut self.shaper;
        if self.shaped || self.switch != Fade::None {
            let m = FilterMods { cutoff: sum(dst::CUTOFF), resonance: sum(dst::RESONANCE), env_amount: sum(dst::ENV_AMOUNT) };
            let note = f32::from(self.key);
            s0.design(s, note, m);
            s1.design(s, note, m);
        }
        s0.lfo_amount_mod = [sum(dst::LFO1_AMOUNT), sum(dst::LFO2_AMOUNT)];
        let rate = |i: usize| RATE_OCTAVES * sum(i).clamp(-1.0, 1.0);
        s0.advance_lfos(s, t.tempo, [rate(dst::LFO1_RATE), rate(dst::LFO2_RATE)], t.quarters.is_some());
    }

    /// Left and right gains for the linear level `level` and pan `pan`, with the matrix's Level and
    /// Pan sums of the last grid point.
    fn gains_for(&self, level: f32, pan: f32) -> (f32, f32) {
        let lvl = self.level_mod;
        let level = if lvl == 0.0 { level } else { level * (1.0 + lvl).clamp(0.0, 2.0) };
        let (l, r) = balance(pan + self.pan_mod);
        (level * l, level * r)
    }

    /// Renders one frame and moves on: the voice's left and right output (0 when idle).
    #[inline]
    pub(crate) fn frame(&mut self, z: &Zone, d: &Derived, source: Option<&dyn RegionReader>, c: &Controls, t: Transport) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        if self.grid == 0 {
            self.modulate(z, d, c, t);
            self.grid = MOD_BLOCK as u32;
        }
        self.grid -= 1;
        let amp = self.env.next(&d.env);
        let (taps, frac) = self.head.taps(&self.region);
        // A crossfaded loop's final X frames: the incoming read and its weight.
        let blend = self.head.crossfade_at(&self.region).map(|(incoming, w)| (self.head.incoming_taps(&self.region, incoming), w));
        let mut x = [0.0f32; 2];
        let channels = usize::from(self.channels).clamp(1, 2);
        for (ch, xc) in x.iter_mut().enumerate().take(channels) {
            let a = read(source, ch, taps, frac, &mut self.missed);
            *xc = match blend {
                Some(((taps, frac), w)) => {
                    let b = read(source, ch, taps, frac, &mut self.missed);
                    a + (b - a) * w
                }
                None => a,
            };
        }
        let filtered = self.shaped || self.switch != Fade::None;
        let w = (self.switch_k + 1) as f32 / MOD_BLOCK as f32;
        self.switch_k += 1;
        let mut y = [0.0f32; 2];
        for ((yc, xc), sh) in y.iter_mut().zip(x).zip(&mut self.shaper).take(channels) {
            let v = sh.filter_frame(&z.shaper, filtered, xc);
            let dry = xc * amp * self.vel_gain;
            *yc = if filtered {
                let out = v * amp * self.vel_gain;
                match self.switch {
                    Fade::None => out,
                    Fade::In => dry + (out - dry) * w,
                    Fade::Out => out + (dry - out) * w,
                }
            } else {
                dry
            };
        }
        if self.declick > 0 {
            let k = self.declick;
            let mut g = if self.age < u64::from(k) { (self.age + 1) as f32 / k as f32 } else { 1.0 };
            let left = self.head.frames_left(&self.region, self.step);
            if left <= u64::from(k) {
                g = g.min(left.saturating_sub(1) as f32 / k as f32);
            }
            y.iter_mut().for_each(|v| *v *= g);
        }
        let [yl, yr] = y;
        let yr = if channels == 2 { yr } else { yl };
        let gains = if d.glides_at(t.frame) {
            let (level, pan) = d.level_pan_at(z.playback.pan, t.frame);
            self.gains_for(level, pan)
        } else {
            self.gains
        };
        let (mut l, mut r) = (yl * gains.0, yr * gains.1);
        let mut faded = false;
        if let Some(f) = &mut self.fade {
            let g = f.advance();
            l *= g;
            r *= g;
            faded = f.is_done();
        }
        self.head.advance(&self.region, self.step);
        self.age += 1;
        if faded || self.head.done || self.env.is_idle() {
            self.cut();
        }
        (l, r)
    }

    /// The voice's state bits (the testing fingerprint, issue #210).
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn fingerprint(&self, h: &mut dyn core::hash::Hasher) {
        h.write_u8(u8::from(self.active));
        if !self.active {
            return;
        }
        h.write_u16(self.zone);
        h.write_u64(self.serial);
        h.write_u64(self.released);
        h.write_u64(self.head.pos.to_bits());
        h.write_u8(u8::from(self.head.up) | u8::from(self.head.wrapped) << 1 | u8::from(self.held) << 2 | u8::from(self.sustained) << 3);
        h.write_u64(self.step.to_bits());
        let (stage, pos, level, from) = self.env.bits();
        h.write_u8(stage);
        h.write_u32(pos);
        h.write_u32(level);
        h.write_u32(from);
        if let Some(f) = &self.fade {
            let (a, b, c) = f.bits();
            h.write_u32(a);
            h.write_u32(b);
            h.write_u32(c);
        }
        h.write_u32(self.grid);
        h.write_u32(self.gains.0.to_bits());
        h.write_u32(self.gains.1.to_bits());
        h.write_u64(self.age);
        for s in &self.shaper {
            for l in &s.lfo {
                let (p, st, rng) = l.bits();
                h.write_u64(p);
                h.write_u32(st);
                h.write_u32(rng);
            }
            h.write_u32(s.filter_env.level.to_bits());
            for b in s.filter_bits().iter().flatten() {
                h.write_u32(*b);
            }
            h.write_u32(s.random.to_bits());
        }
    }
}
