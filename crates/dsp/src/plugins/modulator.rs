//! Cross Modulator (#200, ADR 0033): the meta-modulator as an insert effect with a sidechain
//! second input, over `transit-modulator` (a port of Émilie Gillet's meta-modulator firmware, MIT,
//! nulled bit for bit against its C++).
//!
//! **Inputs.** The firmware combines a carrier (its input 1) with a modulator (its input 2). The
//! track is the modulator and the sidechain key is the carrier: pick the key in the insert's key
//! picker (the compressor's route, ADR 0015 section 4). With no key set, the firmware's internal
//! oscillator is the carrier, as on the module with nothing patched into input 1 and the oscillator
//! on (`Osc Shape`, `Osc Frequency`). `Carrier: Oscillator` keeps the oscillator with a key set, the key then
//! modulating its phase (sine) or frequency (triangle, saw), as input 1 does on the module. `Swap
//! Inputs` makes the key the modulator and the track the carrier (or the oscillator's modulation).
//! The vocoder's convention is the same: the modulator (a voice) shapes the carrier's spectrum
//! (a synth).
//!
//! **Channels.** The firmware is mono: a stereo track runs two of them, left with the key's left and
//! right with its right (the right one's noise from its own random generator), a mono track one.
//!
//! **Rate.** The firmware runs at 96 kHz in 60-frame blocks, as the module's codec does; the device
//! converts at its edges from the session rate and back ([`path`], the Texture's converters), so
//! the oscillator's pitch, the vocoder's bands and the algorithms' oversampling are the module's at
//! any session rate. A 96 kHz session converts nothing: the firmware hears the session's samples
//! and its output is the session's output, bit for bit. The latency (the block plus the converters,
//! a whole number of session samples; [`path::latency_at`]) goes to delay compensation, and the dry
//! signal of `Dry/Wet` is delayed to match.
//!
//! **The codec's float scale.** The firmware runs between its codec's conversions
//! ([`Modulator::process_float`]): a full-scale session sample is a full-scale int16, nothing is
//! quantized or clipped at the edges.

mod path;
#[cfg(test)]
mod tests;

use super::resample::Kernel;
use crate::params::{Params, choice, lin, log, toggle};
use crate::util::{MAX_CHANNELS, Smoother, frames_in, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use path::{Frame, Path};
use transit_modulator::{BLOCK_SIZE, CarrierShape, Modulator, Parameters};

const P_ALGORITHM: usize = 0;
const P_TIMBRE: usize = 1;
const P_CARRIER_LEVEL: usize = 2;
const P_MODULATOR_LEVEL: usize = 3;
const P_CARRIER: usize = 4;
const P_SHAPE: usize = 5;
const P_FREQUENCY: usize = 6;
const P_SWAP: usize = 7;
const P_DRY_WET: usize = 8;
const P_OUTPUT: usize = 9;

/// The algorithms at the whole positions of `Algorithm` (0 to 8); between two the device morphs.
pub const ALGORITHMS: [&str; 9] =
    ["Crossfade", "Cross-fold", "Diode Ring", "Digital Ring", "XOR", "Comparator", "Vocoder", "Vocoder (slow)", "Vocoder (freeze)"];
/// What carries: the key when one is set (else the oscillator), or always the oscillator.
pub const CARRIERS: [&str; 2] = ["Auto", "Oscillator"];
/// The oscillator's shapes (for the vocoder: sawtooth, pulse and noise).
pub const SHAPES: [&str; 3] = ["Sine", "Triangle", "Saw"];
/// `Osc Frequency`'s range: C2 to C7, the module's knob (MIDI notes 36 to 96).
pub const FREQUENCY_RANGE: (f32, f32) = (65.406_39, 2_093.004_5);
/// The random generators' seeds: the firmware's power-on state for the left (or only) channel,
/// another for the right, so the two noise carriers are not one signal.
pub const SEEDS: [u32; 2] = [0x21, 0x0200_0021];
/// The tail reported after the input stops: the vocoder's slowest release falls 60 dB in about 11
/// seconds.
const TAIL_SECONDS: f32 = 12.0;
/// The `Dry/Wet` and `Output` smoothing, ms.
const SMOOTH_MS: f32 = 20.0;

static MODULATOR_PARAMS: [ParamInfo; 10] = [
    lin("algorithm", "Algorithm", 0.0, 8.0, 2.0, Unit::None),
    lin("timbre", "Timbre", 0.0, 100.0, 50.0, Unit::Percent),
    lin("carrier_level", "Carrier Level", 0.0, 100.0, 75.0, Unit::Percent),
    lin("modulator_level", "Modulator Level", 0.0, 100.0, 75.0, Unit::Percent),
    choice("carrier", "Carrier", &CARRIERS, 0),
    choice("shape", "Osc Shape", &SHAPES, 0),
    log("frequency", "Osc Frequency", FREQUENCY_RANGE.0, FREQUENCY_RANGE.1, 220.0, Unit::Hz),
    toggle("swap", "Swap Inputs", false),
    lin("dry_wet", "Dry/Wet", 0.0, 100.0, 100.0, Unit::Percent),
    lin("output", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

pub static MODULATOR_INFO: PluginInfo = PluginInfo {
    id: "cross_modulator",
    name: "Cross Modulator",
    short_name: "XMod",
    category: Category::Modulation,
    params: &MODULATOR_PARAMS,
    is_instrument: false,
    // It plays a second signal from the sidechain: not an offline process.
    audiosuite: false,
    pages: &[],
};

/// The internal oscillator's MIDI note for a frequency, Hz (A4 = 440 Hz is note 69).
pub fn note_of(frequency: f32) -> f32 {
    let f = frequency.clamp(FREQUENCY_RANGE.0, FREQUENCY_RANGE.1);
    (69.0 + 12.0 * (f / 440.0).log2()).clamp(0.0, 127.0)
}

/// The name of the algorithm nearest an `Algorithm` value.
pub fn algorithm_name(value: f32) -> &'static str {
    let i = if value.is_finite() { value.round().clamp(0.0, 8.0) as usize } else { 0 };
    ALGORITHMS.get(i).copied().unwrap_or(ALGORITHMS[0])
}

/// The Cross Modulator insert. See the module documentation.
pub struct CrossModulator {
    p: Params,
    sr: f32,
    ch: usize,
    kernel: Kernel,
    path: Path,
    mods: [Modulator; 2],
    /// The track as it went in, delayed by the latency for `Dry/Wet`.
    dry: Vec<[f32; 2]>,
    dry_at: usize,
    dry_wet: Smoother,
    gain: Smoother,
}

impl Default for CrossModulator {
    fn default() -> Self {
        Self::new()
    }
}

impl CrossModulator {
    pub fn new() -> Self {
        let kernel = Kernel::new();
        let path = Path::new(&kernel, 48_000);
        let mut m = CrossModulator {
            p: Params::new(&MODULATOR_PARAMS),
            sr: 48_000.0,
            ch: 2,
            kernel,
            path,
            mods: SEEDS.map(Modulator::with_seed),
            dry: Vec::new(),
            dry_at: 0,
            dry_wet: Smoother::new(1.0),
            gain: Smoother::new(1.0),
        };
        m.size_dry();
        m.update();
        m.dry_wet.snap();
        m.gain.snap();
        m
    }

    fn size_dry(&mut self) {
        self.dry = vec![[0.0; 2]; self.path.latency() + 1];
        self.dry_at = 0;
    }

    fn update(&mut self) {
        self.dry_wet.set(self.p.v(P_DRY_WET) / 100.0);
        let db = self.p.v(P_OUTPUT);
        self.gain.set(if db == 0.0 { 1.0 } else { 10f32.powf(db / 20.0) });
    }

    /// The firmware's parameters for a block, keyed or not.
    pub fn firmware_parameters(&self, keyed: bool) -> Parameters {
        let p = &self.p;
        let level = |i: usize| {
            let x = (p.v(i) / 100.0).clamp(0.0, 1.0);
            x * x
        };
        let oscillator = !keyed || p.choice(P_CARRIER) == 1;
        let carrier = if oscillator {
            CarrierShape::ALL.get(1 + p.choice(P_SHAPE).min(2)).copied().unwrap_or(CarrierShape::Sine)
        } else {
            CarrierShape::External
        };
        Parameters {
            drive: [level(P_CARRIER_LEVEL), level(P_MODULATOR_LEVEL)],
            algorithm: (p.v(P_ALGORITHM) / 8.0).clamp(0.0, 1.0),
            timbre: (p.v(P_TIMBRE) / 100.0).clamp(0.0, 1.0),
            note: note_of(p.v(P_FREQUENCY)),
            carrier,
        }
    }

    /// Output samples the path read before the firmware rendered them (a test seam; always 0).
    #[cfg(test)]
    pub(crate) fn starved(&self) -> u64 {
        self.path.starved()
    }

    fn run(&mut self, io: &mut [Vec<f32>], key: Option<&[Vec<f32>]>, frames: usize) {
        let ch = self.ch.min(io.len()).min(2);
        let frames = frames_in(io, frames, ch);
        if ch == 0 {
            return;
        }
        // Parameter values and the key's presence hold for the whole host block (the mix sets them
        // before `process`).
        let parameters = self.firmware_parameters(key.is_some());
        let swap = key.is_some() && self.p.on(P_SWAP);
        let sane = |x: f32| if x.is_finite() { x } else { 0.0 };
        let Some((left, rest)) = io.split_first_mut() else { return };
        let mut right = rest.first_mut().filter(|_| ch == 2);
        let mods = &mut self.mods;
        let mut block = |f: &[Frame], out: &mut [[f32; 2]]| {
            for (c, m) in mods.iter_mut().enumerate().take(ch) {
                let (mut in_1, mut in_2) = ([0.0f32; BLOCK_SIZE], [0.0f32; BLOCK_SIZE]);
                for ((a, b), x) in in_1.iter_mut().zip(in_2.iter_mut()).zip(f) {
                    let (track, key) = (x[c], x[2 + c]);
                    (*a, *b) = if swap { (track, key) } else { (key, track) };
                }
                let (mut main, mut aux) = ([0.0f32; BLOCK_SIZE], [0.0f32; BLOCK_SIZE]);
                m.process_float(&parameters, &in_1, &in_2, &mut main, &mut aux);
                for (o, y) in out.iter_mut().zip(main) {
                    o[c] = y;
                }
            }
        };
        for (n, l) in left.iter_mut().take(frames).enumerate() {
            let r = right.as_mut().and_then(|r| r.get_mut(n));
            let x = [sane(*l), r.as_deref().copied().map_or(0.0, sane)];
            let k =
                key.map_or([0.0; 2], |k| [0, 1].map(|c| k.get(c.min(k.len().saturating_sub(1))).and_then(|v| v.get(n)).copied().map_or(0.0, sane)));
            let wet = self.path.tick(&self.kernel, x, k, &mut block);
            // The line holds latency + 1 frames: the slot after the one written was written
            // `latency` frames ago.
            let len = self.dry.len().max(1);
            if let Some(d) = self.dry.get_mut(self.dry_at) {
                *d = x;
            }
            self.dry_at = (self.dry_at + 1) % len;
            let dry = self.dry.get(self.dry_at).copied().unwrap_or(x);
            let mix = self.dry_wet.next();
            let gain = self.gain.next();
            let y = [0, 1].map(|c| {
                let v = if mix >= 1.0 {
                    wet[c]
                } else if mix <= 0.0 {
                    dry[c]
                } else {
                    dry[c] + (wet[c] - dry[c]) * mix
                };
                v * gain
            });
            *l = y[0];
            if let Some(r) = r {
                *r = y[1];
            }
        }
    }
}

impl Plugin for CrossModulator {
    fn info(&self) -> &'static PluginInfo {
        &MODULATOR_INFO
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
        self.ch = sane_channels(channels).min(MAX_CHANNELS).min(2);
        self.path = Path::new(&self.kernel, self.sr.round() as u32);
        self.size_dry();
        self.dry_wet.set_time(SMOOTH_MS, self.sr);
        self.gain.set_time(SMOOTH_MS, self.sr);
        self.reset();
    }

    /// Back to fresh firmware (the random generators at their seeds), silent converters and dry
    /// line, the smoothers at their targets (allocation-free).
    fn reset(&mut self) {
        for (m, seed) in self.mods.iter_mut().zip(SEEDS) {
            m.reset(seed);
        }
        self.path.reset();
        self.dry.iter_mut().for_each(|d| *d = [0.0; 2]);
        self.dry_at = 0;
        self.dry_wet.snap();
        self.gain.snap();
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
        self.path.latency()
    }

    fn tail_samples(&self) -> usize {
        (TAIL_SECONDS * self.sr) as usize + self.latency()
    }
}
