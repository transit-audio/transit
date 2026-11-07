//! Dynamics: Compressor/Limiter and Expander/Gate.
//!
//! All detectors are stereo-linked (the loudest channel drives a shared gain) so the image does
//! not wander. Gain computers work in the log domain with smoothed gain reduction.

use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, lin, log, param_plumbing, toggle};
use crate::util::{DelayLine, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr, time_coef};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain, gain_to_db};

/// Static soft-knee gain computer: returns the gain reduction amount in dB (≥ 0).
#[inline]
pub(crate) fn compress_gr(level_db: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    let over = level_db - threshold;
    if knee > 1e-3 && 2.0 * over.abs() <= knee {
        let t = over + knee / 2.0;
        slope * t * t / (2.0 * knee)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

#[inline]
fn ballistic(state: f32, target: f32, att: f32, rel: f32) -> f32 {
    let c = if target > state { att } else { rel };
    target + (state - target) * c
}

// ---------------------------------------------------------------------------------------------

static COMP_PARAMS: [ParamInfo; 11] = [
    lin("threshold", "Threshold", -60.0, 0.0, -20.0, Unit::Db),
    log("ratio", "Ratio", 1.0, 100.0, 4.0, Unit::Ratio),
    log("attack", "Attack", 0.01, 300.0, 10.0, Unit::Ms),
    log("release", "Release", 5.0, 3000.0, 100.0, Unit::Ms),
    lin("knee", "Knee", 0.0, 24.0, 6.0, Unit::Db),
    lin("makeup", "Makeup", 0.0, 40.0, 0.0, Unit::Db),
    toggle("sc_hpf_on", "SC HPF", false),
    log("sc_hpf_freq", "SC HPF Freq", 20.0, 500.0, 100.0, Unit::Hz),
    toggle("limiter", "Limit", false),
    lin("lookahead", "Lookahead", 0.0, 10.0, 0.0, Unit::Ms),
    lin("mix", "Mix", 0.0, 100.0, 100.0, Unit::Percent),
];

pub static COMP_INFO: PluginInfo = PluginInfo {
    id: "compressor",
    name: "Compressor/Limiter",
    short_name: "Comp",
    category: Category::Dynamics,
    params: &COMP_PARAMS,
    is_instrument: false,
    audiosuite: true,
    pages: &[],
};

/// Feed-forward compressor / limiter with optional lookahead and sidechain high-pass.
pub struct Compressor {
    p: Params,
    sr: f32,
    sc: Vec<Biquad>,
    delay: Vec<DelayLine>,
    gr: f32,
    att: f32,
    rel: f32,
    makeup: Smoother,
    mix: Smoother,
    lookahead: usize,
    gr_meter: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        let mut c = Self {
            p: Params::new(&COMP_PARAMS),
            sr: 48_000.0,
            sc: Vec::new(),
            delay: Vec::new(),
            gr: 0.0,
            att: 0.0,
            rel: 0.0,
            makeup: Smoother::new(1.0),
            mix: Smoother::new(1.0),
            lookahead: 0,
            gr_meter: 0.0,
        };
        c.update();
        c
    }

    fn limiting(&self) -> bool {
        self.p.on(8)
    }

    fn update(&mut self) {
        let sr = self.sr;
        self.att = time_coef(self.p.v(2), sr);
        self.rel = time_coef(self.p.v(3), sr);
        self.makeup.set(db_to_gain(self.p.v(5)));
        self.mix.set(self.p.v(10) / 100.0);
        let f = self.p.v(7);
        for b in &mut self.sc {
            b.set(FilterType::HighPass, f, std::f32::consts::FRAC_1_SQRT_2, 0.0, sr);
        }
        let max = self.delay.first().map(|d| d.max_delay() as usize).unwrap_or(0);
        self.lookahead = (ms_to_samples(self.p.v(9), sr).round() as usize).min(max);
    }

    /// One block, detecting on `key` (aligned with `io`, same channel count) when given, else on
    /// the input itself.
    fn run(&mut self, io: &mut [Vec<f32>], key: Option<&[Vec<f32>]>, frames: usize) {
        let ch = self.sc.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let hpf = self.p.on(6);
        let la = self.lookahead;
        let mut max_gr = 0.0f32;
        for n in 0..frames {
            let mut level = 0.0f32;
            for c in 0..ch {
                let x = key.unwrap_or(&*io).get(c).and_then(|b| b.get(n)).copied().unwrap_or(0.0);
                let s = match self.sc.get_mut(c) {
                    Some(f) if hpf => f.process_sample(x),
                    _ => x,
                };
                if s.is_finite() {
                    level = level.max(s.abs());
                }
            }
            let gr = self.gain_for(level);
            max_gr = max_gr.max(gr);
            let g = db_to_gain(-gr) * self.makeup.next();
            let m = self.mix.next();
            for c in 0..ch {
                let (Some(buf), Some(d)) = (io.get_mut(c), self.delay.get_mut(c)) else { continue };
                let Some(x) = buf.get_mut(n) else { continue };
                d.push(*x);
                let dry = if la == 0 { *x } else { d.tap(la + 1) };
                *x = dry * (1.0 - m) + dry * g * m;
            }
        }
        self.gr_meter = max_gr;
    }

    /// Processes one frame of gain computation from a sidechain level (linear).
    #[inline]
    fn gain_for(&mut self, level: f32) -> f32 {
        let ratio = if self.limiting() { 1000.0 } else { self.p.v(1) };
        let target = compress_gr(gain_to_db(level), self.p.v(0), ratio, self.p.v(4));
        self.gr = ballistic(self.gr, target, self.att, self.rel);
        if !self.gr.is_finite() {
            self.gr = 0.0;
        }
        self.gr
    }
}

impl Plugin for Compressor {
    param_plumbing!(COMP_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        let ch = sane_channels(channels);
        self.sc = vec![Biquad::default(); ch];
        let max = ms_to_samples(10.0, self.sr).ceil() as usize + 2;
        self.delay = (0..ch)
            .map(|_| {
                let mut d = DelayLine::default();
                d.allocate(max);
                d
            })
            .collect();
        self.makeup.set_time(20.0, self.sr);
        self.mix.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.sc.iter_mut().for_each(Biquad::reset);
        self.delay.iter_mut().for_each(DelayLine::clear);
        self.gr = 0.0;
        self.gr_meter = 0.0;
        self.makeup.snap();
        self.mix.snap();
    }

    fn has_key_input(&self) -> bool {
        crate::builtin_takes_key(self.info().id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(io, None, frames);
    }

    fn process_with_key(&mut self, io: &mut [Vec<f32>], key: &[Vec<f32>], frames: usize) {
        self.run(io, Some(key), frames);
    }

    fn latency(&self) -> usize {
        self.lookahead
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }

    /// The detector envelope, the sidechain filters' memory, the lookahead lines (contents and
    /// write positions) and the makeup and mix smoothers.
    #[cfg(feature = "testing")]
    fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        h.write_u32(self.gr.to_bits());
        h.write_u32(self.gr_meter.to_bits());
        h.write_usize(self.lookahead);
        self.sc.iter().for_each(|b| b.fingerprint(h));
        self.delay.iter().for_each(|d| d.fingerprint(h));
        self.makeup.fingerprint(h);
        self.mix.fingerprint(h);
    }
}

// ---------------------------------------------------------------------------------------------

static GATE_PARAMS: [ParamInfo; 6] = [
    lin("threshold", "Threshold", -80.0, 0.0, -40.0, Unit::Db),
    log("ratio", "Ratio", 1.0, 100.0, 100.0, Unit::Ratio),
    lin("range", "Range", 0.0, 80.0, 80.0, Unit::Db),
    log("attack", "Attack", 0.01, 100.0, 0.5, Unit::Ms),
    lin("hold", "Hold", 0.0, 2000.0, 50.0, Unit::Ms),
    log("release", "Release", 1.0, 4000.0, 100.0, Unit::Ms),
];

pub static GATE_INFO: PluginInfo = PluginInfo {
    id: "expander_gate",
    name: "Expander/Gate",
    short_name: "Gate",
    category: Category::Dynamics,
    params: &GATE_PARAMS,
    is_instrument: false,
    audiosuite: true,
    pages: &[],
};

/// Downward expander / gate with hold.
pub struct ExpanderGate {
    p: Params,
    sr: f32,
    ch: usize,
    env: f32,
    env_rel: f32,
    gr: f32,
    att: f32,
    rel: f32,
    hold: usize,
    hold_left: usize,
    gr_meter: f32,
}

impl Default for ExpanderGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpanderGate {
    pub fn new() -> Self {
        let mut g = Self {
            p: Params::new(&GATE_PARAMS),
            sr: 48_000.0,
            ch: 0,
            env: 0.0,
            env_rel: 0.0,
            gr: 0.0,
            att: 0.0,
            rel: 0.0,
            hold: 0,
            hold_left: 0,
            gr_meter: 0.0,
        };
        g.update();
        g
    }

    fn update(&mut self) {
        self.att = time_coef(self.p.v(3), self.sr);
        self.rel = time_coef(self.p.v(5), self.sr);
        self.hold = ms_to_samples(self.p.v(4), self.sr) as usize;
        self.env_rel = time_coef(5.0, self.sr);
    }

    /// One block, detecting on `key` (aligned with `io`, same channel count) when given, else on
    /// the input itself.
    fn run(&mut self, io: &mut [Vec<f32>], key: Option<&[Vec<f32>]>, frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let mut max_gr = 0.0f32;
        for n in 0..frames {
            let level = key.unwrap_or(&*io).iter().take(ch).filter_map(|b| b.get(n)).fold(0.0f32, |m, v| m.max(v.abs()));
            let gr = self.tick(level);
            max_gr = max_gr.max(gr);
            let g = db_to_gain(-gr);
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x *= g;
                }
            }
        }
        self.gr_meter = max_gr;
    }

    /// Returns the gain reduction (dB ≥ 0) for one frame given the linked peak level.
    #[inline]
    pub(crate) fn tick(&mut self, level: f32) -> f32 {
        let level = if level.is_finite() { level } else { 0.0 };
        self.env = level.max(self.env * self.env_rel);
        let ldb = gain_to_db(self.env);
        let thr = self.p.v(0);
        let mut target = if ldb < thr { ((thr - ldb) * (self.p.v(1) - 1.0)).min(self.p.v(2)) } else { 0.0 };
        if target <= 0.0 {
            self.hold_left = self.hold;
        } else if target > self.gr && self.hold_left > 0 {
            self.hold_left -= 1;
            target = self.gr;
        }
        // Opening (less reduction) uses attack, closing uses release.
        let c = if target < self.gr { self.att } else { self.rel };
        self.gr = target + (self.gr - target) * c;
        if !self.gr.is_finite() {
            self.gr = 0.0;
        }
        self.gr
    }
}

impl Plugin for ExpanderGate {
    param_plumbing!(GATE_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.gr = 0.0;
        self.hold_left = 0;
        self.gr_meter = 0.0;
    }

    fn has_key_input(&self) -> bool {
        crate::builtin_takes_key(self.info().id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(io, None, frames);
    }

    fn process_with_key(&mut self, io: &mut [Vec<f32>], key: &[Vec<f32>], frames: usize) {
        self.run(io, Some(key), frames);
    }

    fn gain_reduction_db(&self) -> f32 {
        self.gr_meter
    }
}
