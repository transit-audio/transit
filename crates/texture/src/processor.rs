//! `beads/dsp/processor.cc`: the whole effect chain. The trigger generator (or, in the delay mode,
//! the delay controller) drives the selected quality's granular processor; then the dry/wet
//! crossfade, the tape wow and flutter (dirty tape), a limiter, the reverb and the output limiter.

#![forbid(unsafe_code)]

use crate::delay_controller::{DelayController, DelayOutput};
use crate::delay_line::{DelayMemory, Frame, Resolution};
use crate::float::F;
use crate::fx::{Limiter, Reverb, WowAndFlutter};
use crate::granular_processor::{Drive, GranularProcessor};
use crate::mu_law::{lin_to_mulaw, mulaw_to_lin};
use crate::randomized::{Parameter, Parameters};
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::{clip16, constrain, smooth_step};
use crate::trigger::{TriggerGenerator, TriggerOutput};
use crate::{BufferSize, Controls, MAX_BLOCK_SIZE, Media, SeedButton};

const DELAY_MODE_TRANSITION_TIME: f32 = 1.5;
const DELAY_MODE_FADE_TIME: f32 = 0.75;

/// `NaN` to the bottom of the range, anything else clamped to it.
fn sanitize(x: f32, lo: f32, hi: f32) -> f32 {
    if x.is_nan() { lo } else { x.clamp(lo, hi) }
}

fn sanitize_parameter(p: &Parameter, lo: f32, hi: f32, cv: f32) -> Parameter {
    Parameter { base: sanitize(p.base, lo, hi), attenuverter: sanitize(p.attenuverter, -1.0, 1.0), cv: sanitize(p.cv, -cv, cv), has_cv: p.has_cv }
}

/// The controls as the chain (and the synth) reads them, clamped to the module's ranges.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Inputs {
    pub(crate) parameters: Parameters,
    pub(crate) density: F,
    pub(crate) density_cv: F,
    pub(crate) feedback: F,
    pub(crate) dry_wet: F,
    pub(crate) reverb: F,
    pub(crate) seed_button: SeedButton,
    pub(crate) output_gate: bool,
}

/// [`Controls`] clamped to the module's ranges, a `NaN` at the bottom of its range.
pub(crate) fn sanitized(c: &Controls) -> Inputs {
    Inputs::from(c)
}

impl Inputs {
    fn from(c: &Controls) -> Self {
        Self {
            parameters: Parameters {
                time: sanitize_parameter(&c.time, 0.0, 1.0, 1.6),
                size: sanitize_parameter(&c.size, 0.0, 1.0, 1.6),
                shape: sanitize_parameter(&c.shape, 0.0, 1.0, 1.6),
                pitch: sanitize_parameter(&c.pitch, -24.0, 24.0, 60.0),
                monophonic: c.monophonic,
            },
            density: F(sanitize(c.density, 0.0, 1.0)),
            density_cv: F(sanitize(c.density_cv, -1.0, 1.0)),
            feedback: F(sanitize(c.feedback, 0.0, 1.0)),
            dry_wet: F(sanitize(c.dry_wet, 0.0, 1.0)),
            reverb: F(sanitize(c.reverb, 0.0, 1.0)),
            seed_button: c.seed_button,
            output_gate: c.output_gate,
        }
    }
}

/// The texture processor: the firmware's effect chain (`beads::Processor`).
#[derive(Clone, Debug)]
pub struct Texture {
    rng: Random,
    buffer_size: BufferSize,
    memory: DelayMemory,
    /// The four qualities' processors, in [`Media`] order; they share `memory`.
    processors: [GranularProcessor; 4],
    trigger_generator: TriggerGenerator,
    delay_controller: DelayController,
    media: Media,
    mono: bool,
    delay_mode_transition: F,
    dry: F,
    wet: F,
    wow_and_flutter: WowAndFlutter,
    reverb: Reverb,
    pre_reverb_limiter: Limiter,
    output_limiter: Limiter,
}

impl Texture {
    /// A chain on `buffer_size` of delay memory, at `media` and `mono`, as the firmware leaves it
    /// before its first block (`Init`, then `Configure`), with its random generator at the
    /// firmware's power-on state. Allocates its memory here; nothing allocates afterwards.
    #[must_use]
    pub fn new(buffer_size: BufferSize, media: Media, mono: bool) -> Self {
        let processors = [
            GranularProcessor::new(Resolution::Bits8, buffer_size.max_delay(Media::DirtyTape), 1),
            GranularProcessor::new(Resolution::Bits12, buffer_size.max_delay(Media::CleanTape), 2),
            GranularProcessor::new(Resolution::Bits12, buffer_size.max_delay(Media::VintageDigital), 1),
            GranularProcessor::new(Resolution::Bits16, buffer_size.max_delay(Media::Digital), 1),
        ];
        let items = processors.iter().map(GranularProcessor::items_needed).max().unwrap_or(0);
        let mut texture = Self {
            rng: Random::default(),
            buffer_size,
            memory: DelayMemory::new(items),
            processors,
            trigger_generator: TriggerGenerator::default(),
            delay_controller: DelayController::default(),
            media: Media::Digital,
            mono: false,
            delay_mode_transition: F::ZERO,
            dry: F::ZERO,
            wet: F::ZERO,
            wow_and_flutter: WowAndFlutter::default(),
            reverb: Reverb::default(),
            pre_reverb_limiter: Limiter::default(),
            output_limiter: Limiter::default(),
        };
        // `Processor::Init`: digital, vintage digital, clean tape, dirty tape.
        for (media, rate) in
            [(Media::Digital, 48_000.0), (Media::VintageDigital, 32_000.0), (Media::CleanTape, 24_000.0), (Media::DirtyTape, 24_000.0)]
        {
            texture.processors[media.index()].init(F(rate), &mut texture.memory);
        }
        texture.reverb.init(F(48_000.0));
        texture.wow_and_flutter.init(F(24_000.0));
        texture.wow_and_flutter.set_lp(F(0.95));
        texture.configure(Media::Digital, false);
        texture.delay_mode_transition = F(-DELAY_MODE_TRANSITION_TIME);
        texture.configure(media, mono);
        texture
    }

    /// Writes the random generator's state (the firmware's `Random::Seed`).
    pub fn seed(&mut self, seed: u32) {
        self.rng = Random::new(seed);
    }

    /// The random generator's state.
    #[must_use]
    pub fn rng_state(&self) -> u32 {
        self.rng.state()
    }

    /// The delay memory it was built with.
    #[must_use]
    pub fn buffer_size(&self) -> BufferSize {
        self.buffer_size
    }

    /// The selected quality.
    #[must_use]
    pub fn media(&self) -> Media {
        self.media
    }

    /// Whether it records in mono.
    #[must_use]
    pub fn mono(&self) -> bool {
        self.mono
    }

    /// `Configure(media, mono)`: as the module does when the quality or the mono setting changes.
    /// The selected quality's buffer is cleared; the triggers, the delay controller and the
    /// limiters restart at its rate; the reverb keeps its memory.
    pub fn configure(&mut self, media: Media, mono: bool) {
        self.media = media;
        self.mono = mono;
        let processor = &mut self.processors[media.index()];
        processor.reset(&mut self.memory);
        let max_delay = processor.max_delay_times_ratio();
        let sr = F::from_i32(i32::try_from(media.sample_rate()).unwrap_or(48_000));
        self.pre_reverb_limiter.init(1.0 / (sr * 4.0), F(0.9));
        self.output_limiter.init(1.0 / (sr * 6.0), F(0.9));
        self.trigger_generator.init(sr);
        self.delay_controller.init(sr, max_delay * if mono { 2 } else { 1 });
        self.reverb.reset(sr);
    }

    /// `Trim()`: the panel's MEDIA + FREEZE gesture; the recording's length drops to zero. As in
    /// the firmware (its switch has no breaks), the selected quality's line is trimmed together
    /// with those after it in [`Media`] order; their trims are erased by the reset any change of
    /// quality performs.
    pub fn trim(&mut self) {
        for p in &mut self.processors[self.media.index()..] {
            p.trim(self.mono);
        }
    }

    /// `remaining()`: frames before an unfrozen recording overwrites what was frozen.
    #[must_use]
    pub fn remaining(&self) -> i32 {
        self.processors[self.media.index()].remaining(self.mono)
    }

    /// Processes `input.len()` stereo frames at the quality's rate ([`Media::sample_rate`]) into
    /// `output`. `freeze` holds one flag per frame; `seed` one gate-flags byte per frame (bit 0
    /// high, bit 1 rising, bit 2 falling: [`crate::stmlib::extract_gate_flags`]), or `None` when
    /// nothing is patched into SEED.
    ///
    /// The firmware renders blocks of 1 to [`MAX_BLOCK_SIZE`] frames, an even number at the clean
    /// tape quality; longer buffers are rendered in blocks of [`MAX_BLOCK_SIZE`]. The lengths must
    /// match (checked in debug builds; a release build renders the shortest and leaves the rest of
    /// `output` alone), and an odd last frame at the clean tape quality is silent. Controls are
    /// clamped to the module's ranges and a `NaN` takes the bottom of its range; input samples are
    /// clamped to plus or minus 16 (a `NaN` is silence).
    pub fn process(&mut self, controls: &Controls, freeze: &[bool], seed: Option<&[u8]>, input: &[[f32; 2]], output: &mut [[f32; 2]]) {
        debug_assert!(input.len() == output.len() && input.len() == freeze.len(), "lengths differ");
        debug_assert!(seed.is_none_or(|s| s.len() == input.len()), "seed length differs");
        let inputs = Inputs::from(controls);
        let mut n = input.len().min(output.len()).min(freeze.len());
        if let Some(s) = seed {
            n = n.min(s.len());
        }
        let ratio = self.processors[self.media.index()].ratio();
        let even = n - n % ratio;
        let mut start = 0;
        while start < even {
            let end = (start + MAX_BLOCK_SIZE).min(even);
            let mut frames = [[F::ZERO; 2]; MAX_BLOCK_SIZE];
            for (f, x) in frames.iter_mut().zip(&input[start..end]) {
                for c in 0..2 {
                    f[c] = F(if x[c].is_nan() { 0.0 } else { x[c].clamp(-16.0, 16.0) });
                }
            }
            let mut out = [[F::ZERO; 2]; MAX_BLOCK_SIZE];
            let size = end - start;
            self.process_block(&inputs, &freeze[start..end], seed.map(|s| &s[start..end]), &frames[..size], &mut out[..size]);
            for (o, f) in output[start..end].iter_mut().zip(&out) {
                *o = [f[0].0, f[1].0];
            }
            start = end;
        }
        output[even..n].fill([0.0; 2]);
    }

    /// `Processor::Process(...)` over one block.
    fn process_block(&mut self, p: &Inputs, freeze: &[bool], seed: Option<&[u8]>, input: &[Frame], out: &mut [Frame]) {
        let size = input.len();
        let sample_rate = F::from_i32(i32::try_from(self.media.sample_rate()).unwrap_or(48_000));
        let delay_mode = self.delay_mode_transition > 0.0;
        let threshold = if delay_mode { F(0.98) } else { F(0.99) };
        let direction = if F(p.parameters.size.base) > threshold { F(1.0) } else { F(-1.0) };
        self.delay_mode_transition += direction * F::from_usize(size) / sample_rate;
        self.delay_mode_transition = constrain(self.delay_mode_transition, F(-DELAY_MODE_TRANSITION_TIME), F(DELAY_MODE_TRANSITION_TIME));
        let expo_fm_amount = constrain(20.0 * ((p.density - 0.5).abs() - 0.225), F::ZERO, F(1.0));
        let linear_fm = (1.0 - expo_fm_amount) * p.density_cv;
        let exponential_fm = expo_fm_amount * p.density_cv;

        let mut triggers = [TriggerOutput::default(); MAX_BLOCK_SIZE];
        let mut taps = [DelayOutput::default(); MAX_BLOCK_SIZE];
        let mut gate_out = [false; MAX_BLOCK_SIZE];
        let drive = if delay_mode {
            let taps = &mut taps[..size];
            let (time, shape) = (p.parameters.time, p.parameters.shape);
            self.delay_controller.render(&mut self.rng, p.seed_button, seed, freeze, p.density, linear_fm, exponential_fm, &time, &shape, taps);
            for (g, t) in gate_out.iter_mut().zip(taps.iter()) {
                *g = t.gate;
            }
            Drive::Taps(taps)
        } else {
            let triggers = &mut triggers[..size];
            self.trigger_generator.render(&mut self.rng, p.seed_button, seed, p.density, linear_fm, exponential_fm, triggers);
            for (g, t) in gate_out.iter_mut().zip(triggers.iter()) {
                *g = t.gate;
            }
            Drive::Grains(triggers)
        };

        let media = self.media;
        self.processors[media.index()].process(&mut self.rng, &mut self.memory, drive, freeze, &p.parameters, p.feedback, self.mono, input, out);
        let mut dry_signal = [[F::ZERO; 2]; MAX_BLOCK_SIZE];
        for (d, x) in dry_signal.iter_mut().zip(input) {
            *d = match media {
                Media::Digital | Media::CleanTape => *x,
                Media::VintageDigital => x.map(|s| F::from_i32((s * 4096.0).trunc_i32()) / 4096.0),
                Media::DirtyTape => x.map(|s| mulaw_to_lin(lin_to_mulaw(i16::try_from(clip16((s * 32768.0).trunc_i32())).unwrap_or(0)))),
            };
        }

        // Fade the dry signal out and in when switching between the granular and the delay mode.
        let wet_fade = smooth_step(F(1.0).min(self.delay_mode_transition.abs() / DELAY_MODE_FADE_TIME));
        let dry_wet = p.dry_wet;
        let dry = (1.0 - dry_wet) * (1.0 + dry_wet) * 0.75;
        let wet = dry_wet * (2.0 - dry_wet) * 1.5 * wet_fade;
        let mut dry_modulation = ParameterInterpolator::new(self.dry, dry, size);
        let mut wet_modulation = ParameterInterpolator::new(self.wet, wet, size);
        for (o, d) in out.iter_mut().zip(&dry_signal) {
            let dry_gain = dry_modulation.next();
            let wet_gain = wet_modulation.next();
            for c in 0..2 {
                o[c] = dry_gain * d[c] + wet_gain * o[c];
            }
        }
        self.dry = dry_modulation.value();
        self.wet = wet_modulation.value();

        if media == Media::DirtyTape {
            self.wow_and_flutter.process(&mut self.rng, out);
        }
        self.pre_reverb_limiter.process(out);
        let caveman = media != Media::Digital;
        let reverb = p.reverb;
        let reverb_mix = reverb * reverb * 0.9;
        let reverb_time = reverb * reverb * reverb;
        self.reverb.set_mix(reverb_mix);
        self.reverb.set_time(if caveman { 0.45 + 0.5 * reverb_time } else { 0.3 + 0.6 * reverb_time });
        self.reverb.set_lp(if caveman {
            F(0.4)
        } else if media == Media::VintageDigital {
            F(0.75)
        } else {
            F(0.6)
        });
        self.reverb.process(out);
        self.output_limiter.process(out);
        if p.output_gate {
            for (o, &g) in out.iter_mut().zip(&gate_out) {
                o[0] = (o[0] + o[1]) * 0.5;
                o[1] = if g { F(-0.8) } else { F::ZERO };
            }
        }
    }
}
