//! Texture (#241, ADR 0033): the granular texture processor as an insert effect, over
//! `transit-texture` (a port of Émilie Gillet's texture synthesizer firmware, MIT, nulled bit for bit
//! against its C++).
//!
//! **Rates.** Each quality runs the firmware at its own codec rate, as the module does: 24 kHz for
//! dirty tape, 32 kHz for vintage digital, 48 kHz for clean tape and digital. The device converts at
//! its edges from the session rate and back ([`resample`], the offline resampler's windowed-sinc
//! family), so a grain's pitch and the buffer's length in seconds are the module's at any session
//! rate. A 48 kHz session with the digital or clean tape quality converts nothing: the firmware
//! hears the session's samples and its output is the session's output, bit for bit.
//!
//! **Blocks.** The firmware runs in its own blocks of `rate / 4000` frames (12, 8 or 6), gathered
//! across host blocks ([`path`]): the output for the same input and the same parameter values is
//! bit-identical whatever the host block size. The latency (the block plus the converters, a whole
//! number of session samples) goes to delay compensation ([`Plugin::latency`]).
//!
//! **Input stage.** The module's automatic input gain is on by default (`Input Gain: Auto`): the
//! firmware's processor expects the level it sets. `Manual` sets the gain from `Input Level`
//! instead (0 dB passes already-leveled material untouched, apart from the module's clipping).
//!
//! **Reconfiguration.** Quality, mono input and buffer length restart the firmware (its
//! `Configure`, or for the buffer an in-place rebuild of the whole chain). The output fades to
//! silence over 10 ms, the chain restarts, and the output fades back in (before the first block
//! after a reset, where a session's stored values arrive, it restarts at once). The buffer memory is
//! allocated once, at construction, for the longest buffer (8 s), so the rebuild allocates nothing:
//! parameters reach a built-in only on the audio thread, and a rebuild there at a frame the
//! parameter values fix keeps a render deterministic (offline and realtime alike).

mod path;
mod resample;
#[cfg(test)]
mod tests;

use crate::params::{Params, choice, lin, toggle};
use crate::util::{MAX_CHANNELS, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use path::Path;
use resample::Kernel;
use transit_texture::stmlib::extract_gate_flags;
use transit_texture::{BufferSize, Controls, InputStage, MAX_BLOCK_SIZE, Media, Parameter, SeedButton, SoftClipping, Texture};

const P_TIME: usize = 0;
const P_TIME_RANDOM: usize = 1;
const P_SIZE: usize = 2;
const P_SIZE_RANDOM: usize = 3;
const P_SHAPE: usize = 4;
const P_SHAPE_RANDOM: usize = 5;
const P_PITCH: usize = 6;
const P_PITCH_RANDOM: usize = 7;
const P_DENSITY: usize = 8;
const P_FEEDBACK: usize = 9;
const P_DRY_WET: usize = 10;
const P_REVERB: usize = 11;
const P_FREEZE: usize = 12;
const P_SEED: usize = 13;
const P_SEED_LATCH: usize = 14;
const P_TRIGGER: usize = 15;
const P_TRIGGER_INPUT: usize = 16;
const P_QUALITY: usize = 17;
const P_MONO_INPUT: usize = 18;
const P_MONO_GRAINS: usize = 19;
const P_INPUT_GAIN: usize = 20;
const P_INPUT_LEVEL: usize = 21;
const P_BUFFER: usize = 22;

/// The qualities in the firmware's order (the `quality` parameter's index is [`Media::index`]).
pub const QUALITIES: [&str; 4] = ["Dirty Tape", "Clean Tape", "Vintage Digital", "Digital"];
/// The buffer lengths the panel offers, seconds (4 s is the module's).
pub const BUFFERS: [&str; 5] = ["4 s", "5 s", "6 s", "7 s", "8 s"];
/// The fade around a restart (quality, mono input, buffer length), ms each way.
pub const RESTART_FADE_MS: f32 = 10.0;

static TEXTURE_PARAMS: [ParamInfo; 23] = [
    lin("time", "Time", 0.0, 100.0, 25.0, Unit::Percent),
    lin("time_random", "Time Random", -100.0, 100.0, 0.0, Unit::Percent),
    lin("size", "Size", 0.0, 100.0, 50.0, Unit::Percent),
    lin("size_random", "Size Random", -100.0, 100.0, 0.0, Unit::Percent),
    lin("shape", "Shape", 0.0, 100.0, 50.0, Unit::Percent),
    lin("shape_random", "Shape Random", -100.0, 100.0, 0.0, Unit::Percent),
    lin("pitch", "Pitch", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("pitch_random", "Pitch Random", -100.0, 100.0, 0.0, Unit::Percent),
    lin("density", "Density", 0.0, 100.0, 75.0, Unit::Percent),
    lin("feedback", "Feedback", 0.0, 100.0, 0.0, Unit::Percent),
    lin("dry_wet", "Dry/Wet", 0.0, 100.0, 50.0, Unit::Percent),
    lin("reverb", "Reverb", 0.0, 100.0, 0.0, Unit::Percent),
    toggle("freeze", "Freeze", false),
    toggle("seed", "Seed", false),
    toggle("seed_latch", "Seed Latch", true),
    toggle("trigger", "Trigger", false),
    toggle("trigger_input", "Trigger Input", false),
    choice("quality", "Quality", &QUALITIES, 3),
    toggle("mono_input", "Mono Input", false),
    toggle("mono_grains", "Mono Grains", false),
    choice("input_gain", "Input Gain", &["Auto", "Manual"], 0),
    lin("input_level", "Input Level", 0.0, 32.0, 0.0, Unit::Db),
    choice("buffer", "Buffer", &BUFFERS, 0),
];

pub static TEXTURE_INFO: PluginInfo = PluginInfo {
    id: "texture",
    name: "Texture",
    short_name: "Texture",
    category: Category::Delay,
    params: &TEXTURE_PARAMS,
    is_instrument: false,
    // An insert that learns its input level and keeps a buffer: not an offline process.
    audiosuite: false,
};

/// What restarts the firmware when it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Setup {
    media: Media,
    mono: bool,
    buffer: BufferSize,
}

impl Setup {
    const DEFAULT: Setup = Setup { media: Media::Digital, mono: false, buffer: BufferSize::FIRMWARE };
}

/// The restart fade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fade {
    Steady,
    /// Samples left before the restart.
    Out(u32),
    /// Samples left of the fade-in.
    In(u32),
}

/// The firmware's soft clipping for a quality (the module picks it by quality).
fn clipping(media: Media) -> SoftClipping {
    match media {
        Media::DirtyTape => SoftClipping::On,
        Media::CleanTape => SoftClipping::Dynamic,
        Media::VintageDigital | Media::Digital => SoftClipping::Off,
    }
}

/// The input stage's manual gain setting (1 to 255, a gain of `1 + 39·(s/255)⁴`) nearest `db`.
pub fn manual_gain_setting(db: f32) -> u8 {
    let g = 10f32.powf(db.clamp(0.0, 32.0) / 20.0);
    let s = 255.0 * ((g - 1.0) / 39.0).max(0.0).powf(0.25);
    s.round().clamp(1.0, 255.0) as u8
}

/// The firmware state one block reads besides the audio.
struct Block<'a> {
    texture: &'a mut Texture,
    stage: &'a mut InputStage,
    controls: &'a Controls,
    freeze: bool,
    trigger: Option<bool>,
    gate_flags: &'a mut u8,
    gain: u8,
    mono: bool,
}

impl Block<'_> {
    /// The input stage then the effect chain over one codec block.
    fn render(&mut self, input: &[[f32; 2]], output: &mut [[f32; 2]]) {
        let n = input.len().min(output.len()).min(MAX_BLOCK_SIZE);
        let mut raw = [[0i32; 2]; MAX_BLOCK_SIZE];
        for (r, x) in raw.iter_mut().zip(&input[..n]) {
            // The codec's 32-bit frames: full scale is ±1 (a saturating conversion, NaN is 0).
            *r = x.map(|s| (s * 2_147_483_648.0) as i32);
        }
        let mut leveled = [[0.0f32; 2]; MAX_BLOCK_SIZE];
        self.stage.process(true, self.gain, [false, self.mono], &raw[..n], &mut leveled[..n]);
        let freeze = [self.freeze; MAX_BLOCK_SIZE];
        let mut flags = [0u8; MAX_BLOCK_SIZE];
        let seed = self.trigger.map(|gate| {
            for f in &mut flags[..n] {
                *self.gate_flags = extract_gate_flags(*self.gate_flags, gate);
                *f = *self.gate_flags;
            }
            &flags[..n]
        });
        self.texture.process(self.controls, &freeze[..n], seed, &leveled[..n], &mut output[..n]);
    }
}

/// The Texture insert. See the module documentation.
pub struct TextureFx {
    p: Params,
    sr: f32,
    ch: usize,
    kernel: Kernel,
    /// One path per codec rate: 24, 32 and 48 kHz.
    paths: Vec<Path>,
    active_path: usize,
    texture: Texture,
    stage: InputStage,
    controls: Controls,
    /// The SEED input's last gate flags (`Trigger Input`).
    gate_flags: u8,
    /// The setup the chain runs and the one the parameters ask for.
    setup: Setup,
    want: Setup,
    fade: Fade,
    fade_len: u32,
    /// Nothing processed since the last reset: a new setup applies at once, with no fade (a
    /// session's stored values reach the device on its first block).
    idle: bool,
}

impl Default for TextureFx {
    fn default() -> Self {
        Self::new()
    }
}

/// The path index of a quality's codec rate.
fn path_of(media: Media) -> usize {
    match media.sample_rate() {
        24_000 => 0,
        32_000 => 1,
        _ => 2,
    }
}

impl TextureFx {
    pub fn new() -> Self {
        let setup = Setup::DEFAULT;
        let mut t = TextureFx {
            p: Params::new(&TEXTURE_PARAMS),
            sr: 48_000.0,
            ch: 2,
            kernel: Kernel::new(),
            paths: Vec::new(),
            active_path: path_of(setup.media),
            texture: Texture::with_capacity(BufferSize::MAX, setup.buffer, setup.media, setup.mono),
            stage: InputStage::new(setup.media.sample_rate() as f32, clipping(setup.media)),
            controls: Controls::default(),
            gate_flags: 0,
            setup,
            want: setup,
            fade: Fade::Steady,
            fade_len: 480,
            idle: true,
        };
        t.make_paths();
        t.update();
        t
    }

    fn make_paths(&mut self) {
        let host = self.sr.round() as u32;
        self.paths = [Media::DirtyTape, Media::VintageDigital, Media::Digital].map(|m| Path::new(&self.kernel, host, m)).into();
    }

    /// The setup the parameter values ask for.
    fn wanted(&self) -> Setup {
        let media = Media::ALL.get(self.p.choice(P_QUALITY)).copied().unwrap_or_default();
        let seconds = 4.0 + self.p.choice(P_BUFFER).min(BUFFERS.len() - 1) as f32;
        Setup { media, mono: self.p.on(P_MONO_INPUT), buffer: BufferSize::from_seconds(seconds).unwrap_or(BufferSize::FIRMWARE) }
    }

    /// The firmware's controls from the parameter values.
    fn update(&mut self) {
        let p = &self.p;
        let unit = |i: usize| p.v(i) / 100.0;
        let knob = |base: f32, random: usize| Parameter { base, attenuverter: unit(random), cv: 0.0, has_cv: false };
        self.controls = Controls {
            time: knob(unit(P_TIME), P_TIME_RANDOM),
            size: knob(unit(P_SIZE), P_SIZE_RANDOM),
            shape: knob(unit(P_SHAPE), P_SHAPE_RANDOM),
            pitch: knob(p.v(P_PITCH), P_PITCH_RANDOM),
            density: unit(P_DENSITY),
            density_cv: 0.0,
            feedback: unit(P_FEEDBACK),
            dry_wet: unit(P_DRY_WET),
            reverb: unit(P_REVERB),
            seed_button: if p.on(P_SEED_LATCH) {
                SeedButton::Locked
            } else if p.on(P_SEED) {
                SeedButton::On
            } else {
                SeedButton::Off
            },
            monophonic: p.on(P_MONO_GRAINS),
            output_gate: false,
        };
        self.want = self.wanted();
    }

    /// The restart at the bottom of the fade: the chain takes the wanted setup.
    fn restart(&mut self) {
        let want = self.want;
        let old = self.setup;
        if want.buffer != old.buffer {
            self.texture.rebuild(want.buffer, want.media, want.mono);
        } else {
            self.texture.configure(want.media, want.mono);
        }
        if want.media != old.media {
            self.stage.init(want.media.sample_rate() as f32, clipping(want.media), false);
        }
        self.active_path = path_of(want.media);
        if let Some(path) = self.paths.get_mut(self.active_path) {
            path.reset();
        }
        self.gate_flags = 0;
        self.setup = want;
    }

    /// The fade's gain for the next sample, restarting at its bottom.
    #[inline]
    fn fade_gain(&mut self) -> f32 {
        let len = self.fade_len.max(1) as f32;
        match self.fade {
            Fade::Steady => {
                if self.want == self.setup {
                    return 1.0;
                }
                if self.idle {
                    self.reset();
                    return 1.0;
                }
                self.fade = Fade::Out(self.fade_len);
                1.0
            }
            Fade::Out(0) => {
                self.restart();
                self.fade = Fade::In(self.fade_len);
                0.0
            }
            Fade::Out(k) => {
                self.fade = Fade::Out(k - 1);
                (k - 1) as f32 / len
            }
            Fade::In(0) => {
                self.fade = Fade::Steady;
                1.0
            }
            Fade::In(k) => {
                self.fade = Fade::In(k - 1);
                1.0 - (k - 1) as f32 / len
            }
        }
    }

    /// Output samples any path read before the firmware rendered them (a test seam; always 0).
    #[cfg(test)]
    pub(crate) fn starved(&self) -> u64 {
        self.paths.iter().map(Path::starved).sum()
    }

    /// The codec rate the firmware runs at now, Hz.
    pub fn codec_rate(&self) -> u32 {
        self.paths.get(self.active_path).map_or(48_000, Path::rate)
    }
}

impl Plugin for TextureFx {
    fn info(&self) -> &'static PluginInfo {
        &TEXTURE_INFO
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
        self.ch = sane_channels(channels).min(MAX_CHANNELS);
        self.fade_len = ms_to_samples(RESTART_FADE_MS, self.sr).max(1.0) as u32;
        self.make_paths();
        self.reset();
    }

    /// Back to a freshly built chain at the wanted setup: the recording, the grains, the reverb,
    /// the learned input level and the random generator all start over (allocation-free).
    fn reset(&mut self) {
        let want = self.want;
        self.texture.rebuild(want.buffer, want.media, want.mono);
        self.stage = InputStage::new(want.media.sample_rate() as f32, clipping(want.media));
        self.paths.iter_mut().for_each(Path::reset);
        self.active_path = path_of(want.media);
        self.gate_flags = 0;
        self.setup = want;
        self.fade = Fade::Steady;
        self.idle = true;
    }

    fn latency(&self) -> usize {
        self.paths.get(self.active_path).map_or(0, Path::latency)
    }

    fn tail_samples(&self) -> usize {
        // The buffer can replay for as long as it holds, then the reverb rings out; a frozen buffer
        // plays for as long as it stays frozen, which no tail covers.
        let seconds = self.setup.buffer.seconds() + 10.0;
        (seconds * self.sr) as usize + self.latency()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len()).min(2);
        let frames = frames_in(io, frames, ch);
        if ch == 0 {
            return;
        }
        // Parameter values hold for the whole host block (the mix sets them before `process`).
        let freeze = self.p.on(P_FREEZE);
        let trigger = self.p.on(P_TRIGGER_INPUT).then(|| self.p.on(P_TRIGGER));
        let gain_setting = if self.p.choice(P_INPUT_GAIN) == 0 { 0 } else { manual_gain_setting(self.p.v(P_INPUT_LEVEL)) };
        let Some((left, rest)) = io.split_first_mut() else { return };
        let mut right = rest.first_mut().filter(|_| ch == 2);
        for (n, l) in left.iter_mut().take(frames).enumerate() {
            let gain = self.fade_gain();
            self.idle = false;
            let r = right.as_mut().and_then(|r| r.get_mut(n));
            let x = [*l, r.as_deref().copied().unwrap_or(*l)].map(|s| if s.is_finite() { s } else { 0.0 });
            let mut block = Block {
                texture: &mut self.texture,
                stage: &mut self.stage,
                controls: &self.controls,
                freeze,
                trigger,
                gate_flags: &mut self.gate_flags,
                gain: gain_setting,
                mono: self.setup.mono,
            };
            let Some(path) = self.paths.get_mut(self.active_path) else { return };
            let y = path.tick(&self.kernel, x, &mut |i, o| block.render(i, o));
            match r {
                Some(r) => {
                    *l = y[0] * gain;
                    *r = y[1] * gain;
                }
                None => *l = 0.5 * (y[0] + y[1]) * gain,
            }
        }
    }
}
