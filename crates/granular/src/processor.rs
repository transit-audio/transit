//! The granular processor (`clouds/dsp/granular_processor.{h,cc}`) in its granular playback mode:
//! the input (with feedback) recorded into the buffers, the grains, the diffuser, the reverb and
//! the dry/wet crossfade, at 32 kHz, at four buffer qualities.

#![forbid(unsafe_code)]

use crate::audio_buffer::{AudioBuffer, Resolution};
use crate::float::F;
use crate::fx::{Diffuser, Reverb};
use crate::resources::{LUT_XFADE_IN, LUT_XFADE_OUT};
use crate::sample_player::{GrainParameters, GranularSamplePlayer};
use crate::sample_rate_converter::SampleRateConverter;
use crate::stmlib::filter::{FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::{constrain, interpolate, one_pole, soft_convert, soft_limit};
use crate::{MAX_BLOCK_SIZE, SAMPLE_RATE};

/// One stereo frame of the codec, left then right, as the firmware's `ShortFrame`.
pub type Frame = [i16; 2];

/// `kDownsamplingFactor`: the low-fidelity qualities run the grains at half the rate.
const DOWNSAMPLING_FACTOR: usize = 2;

/// The buffer quality (`set_quality`), in the firmware's order: bit 0 selects mono, bit 1 the 8-bit
/// mu-law buffers with the grains at 16 kHz.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Quality {
    /// 16-bit stereo at 32 kHz (quality 0, the module's default).
    #[default]
    Stereo,
    /// 16-bit mono at 32 kHz (quality 1): the input is summed, and the buffer is about twice as
    /// long.
    Mono,
    /// 8-bit mu-law stereo with the grains at 16 kHz (quality 2): four times the stereo length.
    StereoLowFidelity,
    /// 8-bit mu-law mono with the grains at 16 kHz (quality 3).
    MonoLowFidelity,
}

impl Quality {
    /// Every quality, in index order.
    pub const ALL: [Self; 4] = [Self::Stereo, Self::Mono, Self::StereoLowFidelity, Self::MonoLowFidelity];

    /// The firmware's quality index.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|q| *q == self).unwrap_or(0)
    }

    /// The quality at `index`, if any.
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// Mono: one buffer, the input summed.
    #[must_use]
    pub fn mono(self) -> bool {
        self.index() & 1 != 0
    }

    /// Low fidelity: 8-bit mu-law buffers and the grains at half the rate.
    #[must_use]
    pub fn low_fidelity(self) -> bool {
        self.index() & 2 != 0
    }
}

/// The memory the processor records into, as the firmware's two blocks (`Init(large_buffer,
/// large_buffer_size, small_buffer, small_buffer_size)`): in stereo each channel records into
/// `small` bytes (the rest of `large` is the FX workspace); in mono the one channel records into all
/// of `large` (and `small` is the workspace). A 16-bit sample takes two bytes, a mu-law sample one.
///
/// The port keeps the firmware's arithmetic, so it keeps two of its limits, checked by
/// [`BufferSize::new`]: the FX workspace must fit beside the recording in both layouts (the port
/// gives its effects their own memory, but holds the same rule so every size it accepts can be
/// nulled against the C++), and the longest recording (8-bit mono, `large` samples) must stay
/// under 131072 samples, because a grain's read position is 16.16 fixed point in 32 bits: a
/// transposed grain may read a quarter of the buffer ahead, and its phase would overflow past that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BufferSize {
    large: usize,
    small: usize,
}

impl BufferSize {
    /// The module's blocks: 118784 bytes (`block_mem`) and 65408 (`block_ccm`). 16-bit stereo
    /// records 1.02 s, mono 1.86 s, 8-bit stereo 4.09 s and 8-bit mono 7.42 s.
    pub const FIRMWARE: Self = Self { large: 118_784, small: 65_408 };

    /// The FX workspace the firmware carves out of the memory that does not record: the diffuser's
    /// 2048 floats, the reverb's 16384 words and the pitch shifter's 4096 words (over the
    /// correlator's blocks), bytes.
    pub const WORKSPACE: usize = 2048 * 4 + 16384 * 2 + 4096 * 2;

    /// The firmware's stereo workspace, `large - small` on the module, bytes; [`Self::from_seconds`]
    /// keeps it.
    const FIRMWARE_STEREO_WORKSPACE: usize = 118_784 - 65_408;

    /// The largest `large`: 8-bit mono records `large - 8` samples, which must stay below 131072.
    pub const MAX_LARGE: usize = 131_072;

    /// The shortest 16-bit stereo recording [`Self::from_seconds`] gives, seconds (the workspace
    /// must fit in `small` for mono).
    pub const MIN_SECONDS: f32 = 0.768;

    /// The longest 16-bit stereo recording [`Self::from_seconds`] gives, seconds (`large` at
    /// [`Self::MAX_LARGE`]).
    pub const MAX_SECONDS: f32 = 1.214;

    /// The two blocks, when they fit the firmware's rules: both a multiple of 4 bytes, `small` at
    /// least [`Self::WORKSPACE`] (mono workspace) and more than 16392 16-bit samples (the longest
    /// grain), `large` at least `small` plus the workspace and at most [`Self::MAX_LARGE`].
    #[must_use]
    pub fn new(large: usize, small: usize) -> Option<Self> {
        let fits = small.is_multiple_of(4)
            && large.is_multiple_of(4)
            && small >= Self::WORKSPACE
            && small / 2 > 16_384 + 8
            && large >= small + Self::WORKSPACE
            && large <= Self::MAX_LARGE;
        fits.then_some(Self { large, small })
    }

    /// The blocks for a 16-bit stereo recording of `seconds` at 32 kHz (clamped to
    /// [`Self::MIN_SECONDS`] to [`Self::MAX_SECONDS`]; a NaN gives the firmware's length): `small`
    /// holds the samples plus the 8-sample interpolation tail, rounded up to 4 bytes, and `large`
    /// adds the firmware's stereo workspace. 1.02175 s gives [`Self::FIRMWARE`]. The other
    /// qualities scale with it as on the module (mono about 1.8 times, 8-bit stereo 4 times).
    #[must_use]
    pub fn from_seconds(seconds: f32) -> Self {
        let seconds = if seconds.is_nan() { 32_696.0 / SAMPLE_RATE } else { seconds.clamp(Self::MIN_SECONDS, Self::MAX_SECONDS) };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let samples = (f64::from(seconds) * f64::from(SAMPLE_RATE)).round() as usize;
        let small = ((samples + 8) * 2).div_ceil(4) * 4;
        let large = (small + Self::FIRMWARE_STEREO_WORKSPACE).min(Self::MAX_LARGE);
        Self { large, small }
    }

    /// The large block, bytes.
    #[must_use]
    pub fn large(self) -> usize {
        self.large
    }

    /// The small block, bytes.
    #[must_use]
    pub fn small(self) -> usize {
        self.small
    }

    /// The recording length at `quality`, samples at the quality's rate (32 kHz, or 16 kHz at low
    /// fidelity), less the interpolation tail.
    #[must_use]
    pub fn samples(self, quality: Quality) -> usize {
        let bytes = if quality.mono() { self.large } else { self.small };
        (if quality.low_fidelity() { bytes } else { bytes / 2 }) - 8
    }

    /// The recording length at `quality`, seconds.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn seconds(self, quality: Quality) -> f32 {
        let rate = if quality.low_fidelity() { SAMPLE_RATE / 2.0 } else { SAMPLE_RATE };
        self.samples(quality) as f32 / rate
    }
}

/// What the module's CV scaler hands the processor each block (`Parameters` in
/// `clouds/dsp/parameters.h`; the granular sub-structure is derived from them every block).
///
/// The ranges are the module's: 0 to 1, pitch -48 to 48 semitones. [`Granular::process`] clamps
/// values outside them (a NaN to the bottom), which leaves values inside untouched.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parameters {
    /// Where in the recording the grains start, 0 (the newest audio) to 1 (the oldest).
    pub position: f32,
    /// Grain size, 0 to 1 (1024 to 16384 samples).
    pub size: f32,
    /// Grain transposition, semitones.
    pub pitch: f32,
    /// Grain density: below 0.5 grains are seeded at a regular rate, above it at random; the
    /// distance from 0.5 sets the overlap (none between 0.47 and 0.53).
    pub density: f32,
    /// Grain window shape (square, triangle, raised cosine), then above 0.75 the diffuser amount.
    pub texture: f32,
    /// Dry/wet crossfade (equal power).
    pub dry_wet: f32,
    /// Random panning of the grains.
    pub stereo_spread: f32,
    /// Feedback of the processed signal into the recording.
    pub feedback: f32,
    /// Reverb amount.
    pub reverb: f32,
    /// Freeze: the recording stops; it resumes with a 256-sample crossfade.
    pub freeze: bool,
    /// A trigger in this block: one grain is seeded at once.
    pub trigger: bool,
    /// The trigger input's gate (read by the other playback modes only).
    pub gate: bool,
}

/// `x` within `lo..=hi`, a NaN at `lo`.
fn sanitize(x: f32, lo: f32, hi: f32) -> F {
    F(if x.is_nan() { lo } else { x.clamp(lo, hi) })
}

/// The parameters as the processor reads them, clamped to the module's ranges.
#[derive(Clone, Copy, Debug)]
struct Inputs {
    position: F,
    size: F,
    pitch: F,
    density: F,
    texture: F,
    dry_wet: F,
    stereo_spread: F,
    feedback: F,
    reverb: F,
    freeze: bool,
    trigger: bool,
}

impl Inputs {
    fn from(p: &Parameters) -> Self {
        Self {
            position: sanitize(p.position, 0.0, 1.0),
            size: sanitize(p.size, 0.0, 1.0),
            pitch: sanitize(p.pitch, -48.0, 48.0),
            density: sanitize(p.density, 0.0, 1.0),
            texture: sanitize(p.texture, 0.0, 1.0),
            dry_wet: sanitize(p.dry_wet, 0.0, 1.0),
            stereo_spread: sanitize(p.stereo_spread, 0.0, 1.0),
            feedback: sanitize(p.feedback, 0.0, 1.0),
            reverb: sanitize(p.reverb, 0.0, 1.0),
            freeze: p.freeze,
            trigger: p.trigger,
        }
    }
}

/// The granular texture processor: the firmware's `GranularProcessor` in its granular playback
/// mode, driven as the firmware's desktop test harness drives it (`Prepare` after construction and
/// after every block). All memory is allocated by [`Granular::new`]; nothing allocates afterwards.
#[derive(Clone, Debug)]
pub struct Granular {
    buffer_size: BufferSize,
    stereo: bool,
    low_fidelity: bool,
    silence: bool,
    bypass: bool,
    reset_buffers: bool,
    prepared: bool,
    freeze_lp: F,
    dry_wet: F,
    random: Random,
    buffers: [AudioBuffer; 2],
    player: GranularSamplePlayer,
    diffuser: Diffuser,
    reverb: Reverb,
    fb_filter: [Svf; 2],
    input: [[F; 2]; MAX_BLOCK_SIZE],
    input_downsampled: [[F; 2]; MAX_BLOCK_SIZE / DOWNSAMPLING_FACTOR],
    output_downsampled: [[F; 2]; MAX_BLOCK_SIZE / DOWNSAMPLING_FACTOR],
    output: [[F; 2]; MAX_BLOCK_SIZE],
    feedback: [[F; 2]; MAX_BLOCK_SIZE],
    src_down: SampleRateConverter,
    src_up: SampleRateConverter,
}

impl Granular {
    /// A processor recording into `buffer_size` at `quality`, in the granular mode, as the
    /// harness leaves it before its first block: `Init`, the quality, then `Prepare`. The random
    /// state starts at the firmware's power-on value ([`Random::DEFAULT_SEED`]).
    #[must_use]
    pub fn new(buffer_size: BufferSize, quality: Quality) -> Self {
        let mut fb_filter = [Svf::default(); 2];
        for f in &mut fb_filter {
            f.init();
        }
        let mut processor = Self {
            buffer_size,
            stereo: true,
            low_fidelity: false,
            silence: false,
            bypass: false,
            reset_buffers: true,
            prepared: false,
            freeze_lp: F::ZERO,
            dry_wet: F::ZERO,
            random: Random::default(),
            buffers: [AudioBuffer::new(buffer_size.large.max(buffer_size.small)), AudioBuffer::new(buffer_size.small)],
            player: GranularSamplePlayer::default(),
            diffuser: Diffuser::default(),
            reverb: Reverb::default(),
            fb_filter,
            input: [[F::ZERO; 2]; MAX_BLOCK_SIZE],
            input_downsampled: [[F::ZERO; 2]; MAX_BLOCK_SIZE / DOWNSAMPLING_FACTOR],
            output_downsampled: [[F::ZERO; 2]; MAX_BLOCK_SIZE / DOWNSAMPLING_FACTOR],
            output: [[F::ZERO; 2]; MAX_BLOCK_SIZE],
            feedback: [[F::ZERO; 2]; MAX_BLOCK_SIZE],
            src_down: SampleRateConverter::new(false),
            src_up: SampleRateConverter::new(true),
        };
        processor.set_quality(quality);
        processor.prepare();
        processor
    }

    /// Writes the random state (`Random::Seed`): the grains' random seeding and panning draw from
    /// it.
    pub fn seed(&mut self, seed: u32) {
        self.random = Random::new(seed);
    }

    /// The random state, for the null tests.
    #[must_use]
    pub fn rng_state(&self) -> u32 {
        self.random.state()
    }

    /// The memory sizes this processor was built with.
    #[must_use]
    pub fn buffer_size(&self) -> BufferSize {
        self.buffer_size
    }

    /// Selects the quality, as the panel's button does (`set_quality`). A change clears the
    /// recording: the next block is silent while the buffers are re-initialized, as on the module.
    pub fn set_quality(&mut self, quality: Quality) {
        let stereo = !quality.mono();
        self.reset_buffers = self.reset_buffers || self.stereo != stereo;
        self.stereo = stereo;
        let low_fidelity = quality.low_fidelity();
        self.reset_buffers = self.reset_buffers || self.low_fidelity != low_fidelity;
        self.low_fidelity = low_fidelity;
    }

    /// The current quality.
    #[must_use]
    pub fn quality(&self) -> Quality {
        Quality::from_index(usize::from(!self.stereo) | (usize::from(self.low_fidelity) << 1)).unwrap_or_default()
    }

    /// Bypass: the input is copied to the output, and nothing is recorded.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass = bypass;
    }

    /// Silence: the output is zero and nothing is recorded.
    pub fn set_silence(&mut self, silence: bool) {
        self.silence = silence;
    }

    /// Processes one block (`Process`, then `Prepare`, as the harness): `input.len()` frames, 1 to
    /// [`MAX_BLOCK_SIZE`] (the module always renders 32), even at a low-fidelity quality, into
    /// `output` of the same length. Checked with `debug_assert!`; in a release build a longer
    /// slice is processed as 32-frame blocks with the trigger on the first only, mismatched
    /// lengths process the shortest, and an odd low-fidelity block leaves its last output frame
    /// silent.
    pub fn process(&mut self, parameters: &Parameters, input: &[Frame], output: &mut [Frame]) {
        debug_assert!(input.len() == output.len(), "input and output lengths differ");
        debug_assert!((1..=MAX_BLOCK_SIZE).contains(&input.len()), "block of {} frames", input.len());
        debug_assert!(!self.low_fidelity || input.len().is_multiple_of(2), "odd block at a low-fidelity quality");
        let mut inputs = Inputs::from(parameters);
        let n = input.len().min(output.len());
        for (i, o) in input[..n].chunks(MAX_BLOCK_SIZE).zip(output[..n].chunks_mut(MAX_BLOCK_SIZE)) {
            self.process_block(&inputs, i, o);
            self.prepare();
            inputs.trigger = false;
        }
    }

    /// `GranularProcessor::Process(input, output, size)`.
    fn process_block(&mut self, p: &Inputs, input: &[Frame], output: &mut [Frame]) {
        if self.bypass {
            output.copy_from_slice(input);
            return;
        }
        if self.silence || self.reset_buffers || !self.prepared {
            output.fill([0, 0]);
            return;
        }
        let size = input.len();

        // Convert input buffers to float, and mix down for mono processing.
        for (x, frame) in self.input.iter_mut().zip(input) {
            *x = [F(f32::from(frame[0])) / 32768.0, F(f32::from(frame[1])) / 32768.0];
        }
        if !self.stereo {
            for x in &mut self.input[..size] {
                x[0] = (x[0] + x[1]) * 0.5;
                x[1] = x[0];
            }
        }

        // Apply feedback, with high-pass filtering to prevent build-ups at very low frequencies
        // (large DC swings).
        one_pole(&mut self.freeze_lp, if p.freeze { F(1.0) } else { F::ZERO }, F(0.0005));
        let feedback = p.feedback;
        let sample_rate = if self.low_fidelity { SAMPLE_RATE / 2.0 } else { SAMPLE_RATE };
        let cutoff = (20.0 + 100.0 * feedback * feedback) / sample_rate;
        self.fb_filter[0].set_f_q(FrequencyApproximation::Fast, cutoff, F(1.0));
        let left = self.fb_filter[0];
        self.fb_filter[1].set(&left);
        self.fb_filter[0].process_channel(&mut self.feedback[..size], 0, FilterMode::HighPass);
        self.fb_filter[1].process_channel(&mut self.feedback[..size], 1, FilterMode::HighPass);
        let fb_gain = feedback * (1.0 - self.freeze_lp);
        for (x, fb) in self.input[..size].iter_mut().zip(&self.feedback) {
            for c in 0..2 {
                x[c] += fb_gain * (soft_limit(fb_gain * 1.4 * fb[c] + x[c]) - x[c]);
            }
        }

        if self.low_fidelity {
            let downsampled = size / DOWNSAMPLING_FACTOR;
            self.src_down.process(&self.input[..downsampled * DOWNSAMPLING_FACTOR], &mut self.input_downsampled[..downsampled]);
            self.process_granular(p, true, downsampled);
            self.src_up.process(&self.output_downsampled[..downsampled], &mut self.output[..downsampled * DOWNSAMPLING_FACTOR]);
            if !size.is_multiple_of(DOWNSAMPLING_FACTOR) {
                self.output[size - 1] = [F::ZERO; 2];
            }
        } else {
            self.process_granular(p, false, size);
        }

        // Diffusion post-processing (the granular mode's TEXTURE above 0.75).
        let texture = p.texture;
        let diffusion = if texture > 0.75 { (texture - 0.75) * 4.0 } else { F::ZERO };
        self.diffuser.set_amount(diffusion);
        self.diffuser.process(&mut self.output[..size]);

        // This is what is fed back. Reverb is not fed back.
        self.feedback[..size].copy_from_slice(&self.output[..size]);

        // Apply reverb.
        let mut reverb_amount = p.reverb * 0.95;
        reverb_amount += feedback * (2.0 - feedback) * self.freeze_lp;
        let reverb_amount = constrain(reverb_amount, F::ZERO, F(1.0));
        self.reverb.set_amount(reverb_amount * 0.54);
        self.reverb.set_diffusion(F(0.7));
        self.reverb.set_time(0.35 + 0.63 * reverb_amount);
        self.reverb.set_input_gain(F(0.2));
        self.reverb.set_lp(0.6 + 0.37 * feedback);
        self.reverb.process(&mut self.output[..size]);

        const POST_GAIN: f32 = 1.2;
        let mut dry_wet_mod = ParameterInterpolator::new(self.dry_wet, p.dry_wet, size);
        for ((out, frame), wet) in output.iter_mut().zip(input).zip(&self.output) {
            let dry_wet = dry_wet_mod.next();
            let fade_in = interpolate(&LUT_XFADE_IN, dry_wet, 16.0);
            let fade_out = interpolate(&LUT_XFADE_OUT, dry_wet, 16.0);
            let mut l = F(f32::from(frame[0])) / 32768.0 * fade_out;
            let mut r = F(f32::from(frame[1])) / 32768.0 * fade_out;
            l += wet[0] * POST_GAIN * fade_in;
            r += wet[1] * POST_GAIN * fade_in;
            *out = [soft_convert(l), soft_convert(r)];
        }
        self.dry_wet = dry_wet_mod.value();
    }

    /// `ProcessGranular(input, output, size)` in the granular mode: record, then play the grains.
    /// Reads `input_downsampled` and writes `output_downsampled` at low fidelity, `input` and
    /// `output` otherwise.
    fn process_granular(&mut self, p: &Inputs, downsampled: bool, size: usize) {
        let (input, output) = if downsampled {
            (&self.input_downsampled[..size], &mut self.output_downsampled[..size])
        } else {
            (&self.input[..size], &mut self.output[..size])
        };
        let channels = if self.stereo { 2 } else { 1 };
        for (channel, buffer) in self.buffers.iter_mut().enumerate().take(channels) {
            buffer.write_fade(input, channel, !p.freeze);
        }

        // In the granular mode, DENSITY is a meta parameter, and TEXTURE too.
        let density = p.density;
        let overlap = if density >= 0.53 {
            (density - 0.53) * 2.12
        } else if density <= 0.47 {
            (0.47 - density) * 2.12
        } else {
            F::ZERO
        };
        let window_shape = if p.texture < 0.75 { p.texture * 1.333 } else { F(1.0) };
        let grain = GrainParameters {
            position: p.position,
            size: p.size,
            pitch: p.pitch,
            stereo_spread: p.stereo_spread,
            trigger: p.trigger,
            overlap,
            window_shape,
            use_deterministic_seed: density < 0.5,
        };
        self.player.play(&self.buffers, &grain, &mut self.random, output);
    }

    /// `Prepare()` for the granular mode: after a quality change (or at construction) the effects'
    /// memory and the buffers are re-initialized and the grain player is set to the quality's grain
    /// count. (The firmware also initializes the correlator and the pitch shifter of the other
    /// modes here; nothing of theirs reaches the granular mode's output.)
    fn prepare(&mut self) {
        if !self.reset_buffers && self.prepared {
            return;
        }
        let bytes = if self.stereo { [self.buffer_size.small; 2] } else { [self.buffer_size.large, 0] };
        self.diffuser.init();
        self.reverb.init();
        let resolution = if self.low_fidelity { Resolution::MuLaw8 } else { Resolution::Linear16 };
        let channels = if self.stereo { 2 } else { 1 };
        for (buffer, bytes) in self.buffers.iter_mut().zip(bytes).take(channels) {
            buffer.init(resolution, bytes);
        }
        let num_grains = ((if self.stereo { 32 } else { 40 }) * (if self.low_fidelity { 23 } else { 16 })) >> 4;
        self.player.init(self.stereo, num_grains);
        self.reset_buffers = false;
        self.prepared = true;
    }
}
