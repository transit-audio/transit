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
        Self::with_capacity(buffer_size, buffer_size, media, mono)
    }

    /// [`Texture::new`] on memory sized for `capacity` (the larger of the two), so that
    /// [`Texture::rebuild`] can later switch to any buffer up to it without allocating. The extra
    /// memory changes nothing: every line addresses its own length only.
    #[must_use]
    pub fn with_capacity(capacity: BufferSize, buffer_size: BufferSize, media: Media, mono: bool) -> Self {
        let capacity = if capacity.frames() > buffer_size.frames() { capacity } else { buffer_size };
        let items = Self::items_for(capacity);
        let mut texture = Self {
            rng: Random::default(),
            buffer_size,
            memory: DelayMemory::new(items),
            processors: Self::processors_for(buffer_size),
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
        texture.memory.set_span(Self::items_for(buffer_size));
        texture.init(media, mono);
        texture
    }

    /// Rebuilds the chain in place on `buffer_size`, at `media` and `mono`: afterwards it is in
    /// the state [`Texture::new`] would build (the recording, the grains, the reverb's memory and
    /// the random generator all start over), without allocating. `false`, and nothing changes,
    /// when `buffer_size` needs more memory than the chain holds ([`Texture::with_capacity`]).
    pub fn rebuild(&mut self, buffer_size: BufferSize, media: Media, mono: bool) -> bool {
        let processors = Self::processors_for(buffer_size);
        let span = Self::items_for(buffer_size);
        if span > self.memory.capacity() {
            return false;
        }
        self.rng = Random::default();
        self.buffer_size = buffer_size;
        self.processors = processors;
        self.trigger_generator = TriggerGenerator::default();
        self.delay_controller = DelayController::default();
        self.media = Media::Digital;
        self.mono = false;
        self.delay_mode_transition = F::ZERO;
        self.dry = F::ZERO;
        self.wet = F::ZERO;
        self.wow_and_flutter = WowAndFlutter::default();
        self.reverb.clear();
        self.pre_reverb_limiter = Limiter::default();
        self.output_limiter = Limiter::default();
        // `init` resets every line of every quality, each clearing the memory's span; only the last
        // clear is visible, so it alone fills the span (the rebuild runs on the audio thread). The
        // span is what the new length's layouts use, so a 4 s buffer fills half of an 8 s memory.
        self.memory.set_span(span);
        self.memory.defer_clears();
        self.init(media, mono);
        self.memory.apply_deferred_clear();
        true
    }

    /// The stored samples the largest of `buffer_size`'s layouts uses (what [`Texture::new`]
    /// allocates for it).
    fn items_for(buffer_size: BufferSize) -> usize {
        Self::processors_for(buffer_size).iter().map(GranularProcessor::items_needed).max().unwrap_or(0)
    }

    /// The four qualities' processors on `buffer_size`, in [`Media`] order.
    fn processors_for(buffer_size: BufferSize) -> [GranularProcessor; 4] {
        [
            GranularProcessor::new(Resolution::Bits8, buffer_size.max_delay(Media::DirtyTape), 1),
            GranularProcessor::new(Resolution::Bits12, buffer_size.max_delay(Media::CleanTape), 2),
            GranularProcessor::new(Resolution::Bits12, buffer_size.max_delay(Media::VintageDigital), 1),
            GranularProcessor::new(Resolution::Bits16, buffer_size.max_delay(Media::Digital), 1),
        ]
    }

    /// `Processor::Init`, then `Configure`: every line's reset fills the memory's span (and the
    /// freeze tail) with its quality's silence, so what the memory held before does not matter.
    fn init(&mut self, media: Media, mono: bool) {
        // `Processor::Init`: digital, vintage digital, clean tape, dirty tape.
        for (m, rate) in [(Media::Digital, 48_000.0), (Media::VintageDigital, 32_000.0), (Media::CleanTape, 24_000.0), (Media::DirtyTape, 24_000.0)] {
            self.processors[m.index()].init(F(rate), &mut self.memory);
        }
        self.reverb.init(F(48_000.0));
        self.wow_and_flutter.init(F(24_000.0));
        self.wow_and_flutter.set_lp(F(0.95));
        self.configure(Media::Digital, false);
        self.delay_mode_transition = F(-DELAY_MODE_TRANSITION_TIME);
        self.configure(media, mono);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic test input: two detuned saws with a slow swell.
    fn input(frames: usize, offset: usize) -> Vec<[f32; 2]> {
        (offset..offset + frames)
            .map(|i| {
                let t = i as f32;
                let swell = 0.5 + 0.4 * (t * 0.000_05).sin();
                [swell * ((t * 0.011) % 2.0 - 1.0), swell * ((t * 0.0073) % 2.0 - 1.0)]
            })
            .collect()
    }

    /// Renders `frames` in 12-frame blocks with controls that move every block; the freeze holds
    /// for a stretch and the SEED button latches.
    fn render(t: &mut Texture, frames: usize, offset: usize) -> Vec<[f32; 2]> {
        let x = input(frames, offset);
        let mut out = vec![[0.0; 2]; frames];
        for (b, (i, o)) in x.chunks(12).zip(out.chunks_mut(12)).enumerate() {
            let k = (b + offset / 12) as f32;
            let controls = Controls {
                time: Parameter { base: 0.3 + 0.2 * (k * 0.01).sin(), attenuverter: 0.4, cv: 0.0, has_cv: false },
                size: Parameter::knob(if (b / 2000) % 2 == 0 { 0.6 } else { 1.0 }),
                shape: Parameter::knob(0.4),
                pitch: Parameter { base: 7.0, attenuverter: -0.3, cv: 0.0, has_cv: false },
                density: 0.8,
                feedback: 0.4,
                dry_wet: 0.7,
                reverb: 0.3,
                seed_button: SeedButton::Locked,
                ..Controls::default()
            };
            let freeze = vec![(b / 700) % 3 == 2; i.len()];
            t.process(&controls, &freeze, None, i, o);
        }
        out
    }

    /// At 4, 6 and 8 s, from a running chain on another length (a longer one first, so stale
    /// samples sit past the new span; at 8 s a shorter one, so the span grows): the rebuilt chain
    /// renders what a new one does.
    #[test]
    fn a_rebuild_leaves_the_chain_as_new_would_build_it() {
        for (seconds, from) in [(4.0, BufferSize::MAX), (6.0, BufferSize::MAX), (8.0, BufferSize::MIN)] {
            let size = BufferSize::from_seconds(seconds).expect("a length");
            for media in Media::ALL {
                for mono in [false, true] {
                    let label = format!("{seconds} s {media:?} mono {mono}");
                    let mut fresh = Texture::new(size, media, mono);
                    let mut rebuilt = Texture::with_capacity(BufferSize::MAX, from, Media::DirtyTape, !mono);
                    rebuilt.seed(99);
                    let _ = render(&mut rebuilt, 48_000, 0);
                    assert!(rebuilt.rebuild(size, media, mono));
                    assert_eq!((rebuilt.buffer_size(), rebuilt.media(), rebuilt.mono()), (size, media, mono));
                    let a = render(&mut fresh, 96_000, 0);
                    let b = render(&mut rebuilt, 96_000, 0);
                    assert!(a.iter().zip(&b).all(|(x, y)| x[0].to_bits() == y[0].to_bits() && x[1].to_bits() == y[1].to_bits()), "{label}");
                    assert_eq!(fresh.rng_state(), rebuilt.rng_state(), "{label}");
                    assert!(a.iter().any(|f| f[0] != 0.0), "{label}: silent render");
                }
            }
        }
    }

    /// A rebuild fills only the span its length's layouts use: at 4 s about half an 8 s memory.
    #[test]
    fn a_rebuild_fills_only_the_span_of_its_length() {
        let mut t = Texture::with_capacity(BufferSize::MAX, BufferSize::MAX, Media::Digital, false);
        let full = t.memory.capacity();
        assert_eq!(t.memory.span(), full);
        assert!(t.rebuild(BufferSize::MIN, Media::Digital, false));
        assert_eq!(t.memory.span(), Texture::items_for(BufferSize::MIN));
        assert!(t.memory.span() < full * 51 / 100 && t.memory.span() > full * 49 / 100, "{} of {full}", t.memory.span());
        assert!(t.rebuild(BufferSize::MAX, Media::CleanTape, true));
        assert_eq!(t.memory.span(), full);
    }

    #[test]
    fn extra_capacity_changes_nothing_and_a_rebuild_past_it_is_refused() {
        let mut exact = Texture::new(BufferSize::MIN, Media::CleanTape, false);
        let mut roomy = Texture::with_capacity(BufferSize::MAX, BufferSize::MIN, Media::CleanTape, false);
        let a = render(&mut exact, 48_000, 0);
        let b = render(&mut roomy, 48_000, 0);
        assert!(a.iter().zip(&b).all(|(x, y)| x[0].to_bits() == y[0].to_bits() && x[1].to_bits() == y[1].to_bits()));
        assert!(!exact.rebuild(BufferSize::MAX, Media::Digital, false), "4 s of memory cannot hold 8 s");
        assert_eq!((exact.buffer_size(), exact.media()), (BufferSize::MIN, Media::CleanTape), "a refused rebuild changes nothing");
        let c = render(&mut exact, 12_000, 48_000);
        let d = render(&mut roomy, 12_000, 48_000);
        assert!(c.iter().zip(&d).all(|(x, y)| x[0].to_bits() == y[0].to_bits() && x[1].to_bits() == y[1].to_bits()));
    }
}
