//! Time Stretcher (#199, ADR 0033): the pitch shifter and time stretcher of Émilie Gillet's texture
//! processor firmware as an insert effect, over `transit-granular` (its stretch playback mode, a
//! port of the MIT firmware nulled bit for bit against its C++).
//!
//! **What it does.** The input is recorded into the firmware's buffer; two overlapping windows read
//! it back at the pitch ratio, each new window placed where a sign-bit correlator finds it lines up
//! best with the one fading out (WSOLA). So Pitch transposes without changing the speed, and
//! Position moves the read point back into the recording without changing the pitch: with Freeze
//! (the recording stops) Position scrubs the frozen audio, and Trigger restarts the read at the
//! oldest audio and glides back to Position at the rate of the triggers. Window is the windows'
//! length (4096 samples at 100 % down to 128 at 0), Diffusion the firmware's diffuser, Tone its
//! low-pass (below 50 %) and high-pass (above) filters; Feedback, Reverb and Dry/Wet as the module.
//! (The name: the pitch shifting is shared with the existing Pitch Shifter built-in; what this one
//! adds is the stretch, the frozen scrub and the trigger.)
//!
//! **Rate.** The firmware runs at 32 kHz, its codec's rate, in 32-frame blocks. The device converts
//! at its edges from the session rate and back through the Texture's windowed-sinc converters
//! ([`super::resample`], gathered into blocks by [`super::codec_path`]), so the pitch and the
//! buffer's length in seconds are the module's at any session rate, and the output is bit-identical
//! whatever the host block size. The latency (the block less one frame plus both converters, a
//! whole number of session samples, the same for every quality) goes to delay compensation every
//! block ([`Plugin::latency`]).
//!
//! **Levels.** The session's floats enter as the codec's int16 frames (full scale ±1, saturating),
//! after `Input Level` (dB). The firmware has no automatic input gain (the Texture's comes from the
//! other module's firmware), so there is no Auto setting. Its output crossfade and soft clipper
//! leave a dry signal at 0.354 of its input level (the equal-power table's 0.707 times the
//! clipper's 0.5); the device makes that up at its output ([`OUTPUT_GAIN`], +9.03 dB), so a dry
//! signal comes out at its input level and the clipper's ceiling sits at +9 dBFS.
//!
//! **Quality.** The four buffer qualities (16-bit or 8-bit µ-law, stereo or mono; the 8-bit ones
//! run the windows at 16 kHz behind the firmware's 2:1 converters) restart the firmware: the output
//! fades to silence over 10 ms, the firmware takes the quality (clearing its recording, as the
//! module does), and the output fades back in. A quality that returns to the running one before the
//! bottom of the fade cancels the restart. Before the first block after a reset (where a session's
//! stored values arrive) a new quality applies at once. All memory is allocated at construction (the
//! firmware's 184 KB of buffers and its effects) and in `prepare` (the converters); nothing
//! allocates in the callback, including the restart and `reset`.

#[cfg(test)]
mod tests;

use super::codec_path::Path;
use super::resample::Kernel;
use crate::params::{Params, choice, lin, toggle};
use crate::util::{MAX_CHANNELS, flush, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, Unit};
use transit_granular::{BufferSize, Frame, Granular, MAX_BLOCK_SIZE, Parameters, PlaybackMode, Quality};

const P_PITCH: usize = 0;
const P_FINE: usize = 1;
const P_WINDOW: usize = 2;
const P_POSITION: usize = 3;
const P_DIFFUSION: usize = 4;
const P_TONE: usize = 5;
const P_FEEDBACK: usize = 6;
const P_DRY_WET: usize = 7;
const P_REVERB: usize = 8;
const P_FREEZE: usize = 9;
const P_TRIGGER: usize = 10;
const P_QUALITY: usize = 11;
const P_INPUT_LEVEL: usize = 12;

/// The firmware's codec rate, Hz.
pub const CODEC_RATE: u32 = 32_000;
/// The qualities in the firmware's order (the `quality` parameter's index is [`Quality::index`]),
/// with the module's buffer lengths.
pub const QUALITIES: [&str; 4] = ["Stereo (1 s)", "Mono (2 s)", "Lo-Fi Stereo (4 s)", "Lo-Fi Mono (7 s)"];
/// The output makeup: the inverse of the firmware's dry gain at Dry/Wet 0 for small signals,
/// `1 / (lut_xfade_out[0] · 0.5)` = 2√2.
pub const OUTPUT_GAIN: f32 = std::f32::consts::SQRT_2 * 2.0;
/// The fade around a quality restart, ms each way.
pub const RESTART_FADE_MS: f32 = 10.0;

static TIME_STRETCHER_PARAMS: [ParamInfo; 13] = [
    lin("pitch", "Pitch", -24.0, 24.0, 0.0, Unit::Semitones),
    lin("fine", "Fine", -100.0, 100.0, 0.0, Unit::Cents),
    lin("window", "Window", 0.0, 100.0, 70.0, Unit::Percent),
    lin("position", "Position", 0.0, 100.0, 0.0, Unit::Percent),
    lin("diffusion", "Diffusion", 0.0, 100.0, 0.0, Unit::Percent),
    lin("tone", "Tone", 0.0, 100.0, 50.0, Unit::Percent),
    lin("feedback", "Feedback", 0.0, 100.0, 0.0, Unit::Percent),
    lin("dry_wet", "Dry/Wet", 0.0, 100.0, 100.0, Unit::Percent),
    lin("reverb", "Reverb", 0.0, 100.0, 0.0, Unit::Percent),
    toggle("freeze", "Freeze", false),
    toggle("trigger", "Trigger", false),
    choice("quality", "Quality", &QUALITIES, 0),
    lin("input_level", "Input Level", -18.0, 18.0, 0.0, Unit::Db),
];

pub static TIME_STRETCHER_INFO: PluginInfo = PluginInfo {
    id: "time_stretcher",
    name: "Time Stretcher",
    short_name: "Stretch",
    category: Category::PitchShift,
    params: &TIME_STRETCHER_PARAMS,
    is_instrument: false,
    // An insert that keeps a buffer and a correlator's state: not an offline process.
    audiosuite: false,
};

/// The restart fade. A quality that returns to the running one during `Out` cancels the restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fade {
    Steady,
    /// Samples left before the restart.
    Out(u32),
    /// Samples left of the fade-in.
    In(u32),
}

/// One session float as the codec's int16 sample: full scale ±1, saturating, truncated.
#[inline]
fn to_codec(x: f32) -> i16 {
    (x * 32_768.0).clamp(-32_768.0, 32_767.0) as i16
}

/// The firmware state one block reads besides the audio.
struct Block<'a> {
    granular: &'a mut Granular,
    parameters: Parameters,
    gate: bool,
    last_gate: &'a mut bool,
    gain: f32,
}

impl Block<'_> {
    /// One firmware block: the floats to int16 frames, `Process` and `Prepare`, back to floats. The
    /// trigger is the rising edge of the Trigger gate, sampled once per firmware block, as the
    /// module's trigger input.
    fn render(&mut self, input: &[[f32; 2]], output: &mut [[f32; 2]]) {
        let n = input.len().min(output.len()).min(MAX_BLOCK_SIZE);
        let mut codec_in: [Frame; MAX_BLOCK_SIZE] = [[0; 2]; MAX_BLOCK_SIZE];
        for (c, x) in codec_in.iter_mut().zip(&input[..n]) {
            *c = x.map(|s| to_codec(s * self.gain));
        }
        let parameters = Parameters { trigger: self.gate && !*self.last_gate, ..self.parameters };
        *self.last_gate = self.gate;
        let mut codec_out: [Frame; MAX_BLOCK_SIZE] = [[0; 2]; MAX_BLOCK_SIZE];
        self.granular.process(&parameters, &codec_in[..n], &mut codec_out[..n]);
        for (o, c) in output.iter_mut().zip(&codec_out[..n]) {
            *o = c.map(|s| f32::from(s) / 32_768.0 * OUTPUT_GAIN);
        }
    }
}

/// The Time Stretcher insert. See the module documentation.
pub struct TimeStretcher {
    p: Params,
    sr: f32,
    ch: usize,
    kernel: Kernel,
    path: Path,
    granular: Granular,
    /// The Trigger gate as the last firmware block saw it.
    last_gate: bool,
    /// The quality the firmware runs and the one the parameters ask for.
    quality: Quality,
    want: Quality,
    fade: Fade,
    fade_len: u32,
    /// Nothing processed since the last reset: a new quality applies at once, with no fade (a
    /// session's stored values reach the device on its first block).
    idle: bool,
}

impl Default for TimeStretcher {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeStretcher {
    pub fn new() -> Self {
        let kernel = Kernel::new();
        let path = Path::new(&kernel, 48_000, CODEC_RATE, MAX_BLOCK_SIZE);
        let quality = Quality::default();
        TimeStretcher {
            p: Params::new(&TIME_STRETCHER_PARAMS),
            sr: 48_000.0,
            ch: 2,
            kernel,
            path,
            granular: Granular::new_in_mode(BufferSize::FIRMWARE, quality, PlaybackMode::Stretch),
            last_gate: false,
            quality,
            want: quality,
            fade: Fade::Steady,
            fade_len: 480,
            idle: true,
        }
    }

    /// The firmware's parameters from the parameter values (the trigger is set per block).
    fn parameters(&self) -> Parameters {
        let p = &self.p;
        let unit = |i: usize| (p.v(i) / 100.0).clamp(0.0, 1.0);
        Parameters {
            position: unit(P_POSITION),
            size: unit(P_WINDOW),
            pitch: (p.v(P_PITCH) + p.v(P_FINE) / 100.0).clamp(-48.0, 48.0),
            density: unit(P_DIFFUSION),
            texture: unit(P_TONE),
            dry_wet: unit(P_DRY_WET),
            stereo_spread: 0.0,
            feedback: unit(P_FEEDBACK),
            reverb: unit(P_REVERB),
            freeze: p.on(P_FREEZE),
            trigger: false,
            gate: false,
        }
    }

    /// The restart at the bottom of the fade: the firmware takes the wanted quality (its next block
    /// is silent while it clears the recording, as on the module) and the path starts over.
    fn restart(&mut self) {
        self.granular.set_quality(self.want);
        self.path.reset();
        self.last_gate = false;
        self.quality = self.want;
    }

    /// The fade's gain for the next sample, restarting at its bottom.
    #[inline]
    fn fade_gain(&mut self) -> f32 {
        let len = self.fade_len.max(1) as f32;
        match self.fade {
            Fade::Steady => {
                if self.want == self.quality {
                    return 1.0;
                }
                if self.idle {
                    self.reset();
                    return 1.0;
                }
                self.fade = Fade::Out(self.fade_len);
                1.0
            }
            Fade::Out(k) if self.want == self.quality => {
                // The quality came back to the running one before the restart: no restart (the
                // recording stays), the fade turns round from its present level.
                self.fade = Fade::In(self.fade_len - k.min(self.fade_len));
                k as f32 / len
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

    /// Output samples the path read before the firmware rendered them (a test seam; always 0).
    #[cfg(test)]
    pub(crate) fn starved(&self) -> u64 {
        self.path.starved()
    }
}

impl Plugin for TimeStretcher {
    fn info(&self) -> &'static PluginInfo {
        &TIME_STRETCHER_INFO
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if self.p.set(id, value).is_none() {
            return false;
        }
        self.want = Quality::from_index(self.p.choice(P_QUALITY)).unwrap_or_default();
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.p.get(id)
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, channels: usize) {
        self.sr = sane_sr(sample_rate);
        self.ch = sane_channels(channels).min(MAX_CHANNELS);
        self.fade_len = ms_to_samples(RESTART_FADE_MS, self.sr).max(1.0) as u32;
        self.path = Path::new(&self.kernel, self.sr.round() as u32, CODEC_RATE, MAX_BLOCK_SIZE);
        self.reset();
    }

    /// Back to a freshly built firmware at the wanted quality: the recording, the windows, the
    /// correlator, the filters and the effects all start over (allocation-free).
    fn reset(&mut self) {
        self.granular.set_quality(self.want);
        self.granular.reset();
        self.path.reset();
        self.last_gate = false;
        self.quality = self.want;
        self.fade = Fade::Steady;
        self.idle = true;
    }

    fn latency(&self) -> usize {
        self.path.latency()
    }

    fn tail_samples(&self) -> usize {
        // The windows can read as far back as the recording holds, then the reverb rings out; a
        // frozen recording plays for as long as it stays frozen, which no tail covers.
        let seconds = self.granular.buffer_size().seconds(self.quality) + 10.0;
        (seconds * self.sr) as usize + self.latency()
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len()).min(2);
        let frames = frames_in(io, frames, ch);
        if ch == 0 {
            return;
        }
        // Parameter values hold for the whole host block (the mix sets them before `process`).
        let parameters = self.parameters();
        let gate = self.p.on(P_TRIGGER);
        let gain = 10f32.powf(self.p.v(P_INPUT_LEVEL).clamp(-18.0, 18.0) / 20.0);
        let Some((left, rest)) = io.split_first_mut() else { return };
        let mut right = rest.first_mut().filter(|_| ch == 2);
        for (n, l) in left.iter_mut().take(frames).enumerate() {
            let fade = self.fade_gain();
            self.idle = false;
            let r = right.as_mut().and_then(|r| r.get_mut(n));
            let x = [*l, r.as_deref().copied().unwrap_or(*l)].map(|s| if s.is_finite() { s } else { 0.0 });
            let mut block = Block { granular: &mut self.granular, parameters, gate, last_gate: &mut self.last_gate, gain };
            let y = self.path.tick(&self.kernel, x, &mut |i, o| block.render(i, o));
            // Steady, the output is the path's (a copy at 32 kHz); fading, it is flushed so a fade
            // over a quiet tail hands nothing subnormal on.
            let y = if fade < 1.0 { y.map(|v| flush(v * fade)) } else { y };
            match r {
                Some(r) => {
                    *l = y[0];
                    *r = y[1];
                }
                None => *l = 0.5 * (y[0] + y[1]),
            }
        }
    }
}
