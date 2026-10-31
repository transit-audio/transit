//! Utilities: Gain, Trim, Invert, DC Offset Removal, Signal Generator, Dither.

use crate::osc::{Osc, Waveform};
use crate::params::{Params, choice, lin, log, param_plumbing, toggle};
use crate::util::{DcBlocker, Rng, Smoother, frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit, db_to_gain};

/// Gain parameter floor that means "-inf".
const OFF_DB: f32 = -96.0;

fn fader_gain(db: f32) -> f32 {
    if db <= OFF_DB { 0.0 } else { db_to_gain(db) }
}

/// Applies a per-sample smoothed gain to all channels.
fn apply_gain(io: &mut [Vec<f32>], frames: usize, ch: usize, g: &mut Smoother) {
    let ch = ch.min(io.len());
    let frames = frames_in(io, frames, ch);
    if !g.settling() {
        let v = g.value();
        if v == 1.0 {
            return;
        }
        io.iter_mut().take(ch).for_each(|b| b.iter_mut().take(frames).for_each(|x| *x *= v));
        return;
    }
    for n in 0..frames {
        let v = g.next();
        for b in io.iter_mut().take(ch) {
            if let Some(x) = b.get_mut(n) {
                *x *= v;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------

static GAIN_PARAMS: [ParamInfo; 1] = [lin("gain", "Gain", OFF_DB, 24.0, 0.0, Unit::Db)];

pub static GAIN_INFO: PluginInfo = PluginInfo {
    id: "gain",
    name: "Gain",
    short_name: "Gain",
    category: Category::Other,
    params: &GAIN_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Smoothed gain change (-inf .. +24 dB).
pub struct Gain {
    p: Params,
    ch: usize,
    g: Smoother,
}

impl Default for Gain {
    fn default() -> Self {
        Self::new()
    }
}

impl Gain {
    pub fn new() -> Self {
        Self { p: Params::new(&GAIN_PARAMS), ch: 0, g: Smoother::new(1.0) }
    }

    fn update(&mut self) {
        self.g.set(fader_gain(self.p.v(0)));
    }
}

impl Plugin for Gain {
    param_plumbing!(GAIN_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.ch = sane_channels(channels);
        self.g.set_time(20.0, sane_sr(sample_rate));
        self.update();
        self.g.snap();
    }

    fn reset(&mut self) {
        self.g.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        apply_gain(io, frames, self.ch, &mut self.g);
    }

    /// The gain smoother.
    #[cfg(feature = "testing")]
    fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        self.g.fingerprint(h);
    }
}

// ---------------------------------------------------------------------------------------------

static TRIM_PARAMS: [ParamInfo; 2] = [lin("gain", "Trim", OFF_DB, 12.0, 0.0, Unit::Db), toggle("invert", "Phase Invert", false)];

pub static TRIM_INFO: PluginInfo = PluginInfo {
    id: "trim",
    name: "Trim",
    short_name: "Trim",
    category: Category::Other,
    params: &TRIM_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Trim (-inf .. +12 dB) with a click-free polarity switch.
pub struct Trim {
    p: Params,
    ch: usize,
    g: Smoother,
}

impl Default for Trim {
    fn default() -> Self {
        Self::new()
    }
}

impl Trim {
    pub fn new() -> Self {
        Self { p: Params::new(&TRIM_PARAMS), ch: 0, g: Smoother::new(1.0) }
    }

    fn update(&mut self) {
        let sign = if self.p.on(1) { -1.0 } else { 1.0 };
        self.g.set(sign * fader_gain(self.p.v(0)));
    }
}

impl Plugin for Trim {
    param_plumbing!(TRIM_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.ch = sane_channels(channels);
        self.g.set_time(10.0, sane_sr(sample_rate));
        self.update();
        self.g.snap();
    }

    fn reset(&mut self) {
        self.g.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        apply_gain(io, frames, self.ch, &mut self.g);
    }
}

// ---------------------------------------------------------------------------------------------

pub static INVERT_INFO: PluginInfo =
    PluginInfo { id: "invert", name: "Invert", short_name: "Invert", category: Category::Other, params: &[], is_instrument: false, audiosuite: true };

/// Polarity inversion.
#[derive(Default)]
pub struct Invert {
    ch: usize,
}

impl Plugin for Invert {
    fn info(&self) -> &'static PluginInfo {
        &INVERT_INFO
    }

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, channels: usize) {
        self.ch = sane_channels(channels);
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }

    fn param(&self, _id: &str) -> Option<f32> {
        None
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        io.iter_mut().take(ch).for_each(|b| b.iter_mut().take(frames).for_each(|x| *x = -*x));
    }
}

// ---------------------------------------------------------------------------------------------

static DC_PARAMS: [ParamInfo; 1] = [log("cutoff", "Cutoff", 1.0, 40.0, 5.0, Unit::Hz)];

pub static DC_INFO: PluginInfo = PluginInfo {
    id: "dc_offset_removal",
    name: "DC Offset Removal",
    short_name: "DC Rmv",
    category: Category::Other,
    params: &DC_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// One-pole DC-blocking high-pass.
pub struct DcRemoval {
    p: Params,
    sr: f32,
    f: Vec<DcBlocker>,
}

impl Default for DcRemoval {
    fn default() -> Self {
        Self::new()
    }
}

impl DcRemoval {
    pub fn new() -> Self {
        Self { p: Params::new(&DC_PARAMS), sr: 48_000.0, f: Vec::new() }
    }

    fn update(&mut self) {
        let (fc, sr) = (self.p.v(0), self.sr);
        self.f.iter_mut().for_each(|f| f.set_cutoff(fc, sr));
    }
}

impl Plugin for DcRemoval {
    param_plumbing!(DC_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.f = vec![DcBlocker::new(self.p.v(0), self.sr); sane_channels(channels)];
    }

    fn reset(&mut self) {
        self.f.iter_mut().for_each(DcBlocker::reset);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.f.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        for (b, f) in io.iter_mut().zip(self.f.iter_mut()) {
            for x in b.iter_mut().take(frames) {
                *x = f.process(if x.is_finite() { *x } else { 0.0 });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------

const WAVE_NAMES: &[&str] = &["Sine", "Square", "Saw", "Triangle", "White Noise", "Pink Noise"];

static GEN_PARAMS: [ParamInfo; 3] = [
    choice("waveform", "Waveform", WAVE_NAMES, 0),
    log("freq", "Frequency", 20.0, 20000.0, 1000.0, Unit::Hz),
    lin("level", "Level", OFF_DB, 0.0, -20.0, Unit::Db),
];

pub static GEN_INFO: PluginInfo = PluginInfo {
    id: "signal_generator",
    name: "Signal Generator",
    short_name: "SigGen",
    category: Category::Other,
    params: &GEN_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// Test-tone and noise generator; replaces its input.
pub struct SignalGenerator {
    p: Params,
    sr: f32,
    ch: usize,
    osc: Osc,
    level: Smoother,
    freq: Smoother,
}

impl Default for SignalGenerator {
    fn default() -> Self {
        Self::new()
    }
}

impl SignalGenerator {
    pub fn new() -> Self {
        let mut g = Self {
            p: Params::new(&GEN_PARAMS),
            sr: 48_000.0,
            ch: 0,
            osc: Osc::new(0xC0FFEE),
            level: Smoother::new(0.1),
            freq: Smoother::new(1000.0),
        };
        g.update();
        g
    }

    fn update(&mut self) {
        self.level.set(fader_gain(self.p.v(2)));
        self.freq.set(self.p.v(1));
    }
}

impl Plugin for SignalGenerator {
    param_plumbing!(GEN_INFO);

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels);
        self.level.set_time(20.0, self.sr);
        self.freq.set_time(20.0, self.sr);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.osc.reset();
        self.level.snap();
        self.freq.snap();
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len());
        let frames = frames_in(io, frames, ch);
        let wave = Waveform::from_index(self.p.choice(0));
        for n in 0..frames {
            let g = self.level.next();
            let f = self.freq.next();
            let y = self.osc.next(wave, f / self.sr) * g;
            for b in io.iter_mut().take(ch) {
                if let Some(x) = b.get_mut(n) {
                    *x = y;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------

static DITHER_PARAMS: [ParamInfo; 2] = [choice("bits", "Bit Depth", &["16", "20", "24"], 0), toggle("noise_shaping", "Noise Shaping", false)];

pub static DITHER_INFO: PluginInfo = PluginInfo {
    id: "dither",
    name: "Dither",
    short_name: "Dither",
    category: Category::Dither,
    params: &DITHER_PARAMS,
    is_instrument: false,
    audiosuite: true,
};

/// TPDF dither and requantization with optional first-order error-feedback noise shaping.
/// Digital silence passes through as silence.
pub struct Dither {
    p: Params,
    rng: Rng,
    err: Vec<f32>,
}

impl Default for Dither {
    fn default() -> Self {
        Self::new()
    }
}

impl Dither {
    pub fn new() -> Self {
        Self { p: Params::new(&DITHER_PARAMS), rng: Rng::new(0xD17E), err: Vec::new() }
    }

    fn update(&mut self) {}
}

impl Plugin for Dither {
    param_plumbing!(DITHER_INFO);

    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, channels: usize) {
        self.err = vec![0.0; sane_channels(channels)];
    }

    fn reset(&mut self) {
        self.err.iter_mut().for_each(|e| *e = 0.0);
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.err.len().min(io.len());
        let frames = frames_in(io, frames, ch);
        let bits = [16.0f32, 20.0, 24.0].get(self.p.choice(0)).copied().unwrap_or(16.0);
        let lsb = 2f32.powf(1.0 - bits);
        let shape = self.p.on(1);
        for (b, e) in io.iter_mut().zip(self.err.iter_mut()) {
            for x in b.iter_mut().take(frames) {
                if *x == 0.0 || !x.is_finite() {
                    *x = 0.0;
                    *e = 0.0;
                    continue;
                }
                let v = if shape { *x - *e } else { *x };
                let d = (self.rng.bipolar() + self.rng.bipolar()) * 0.5 * lsb;
                let q = ((v + d) / lsb).round() * lsb;
                *e = q - v;
                *x = q.clamp(-1.0, 1.0 - lsb);
            }
        }
    }
}
