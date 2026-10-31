//! `beads/dsp/synth.cc` with `wavetable_voice.h`, `wavetable_grain.h` and
//! `wavetable_granular_player.h`: what the module plays when nothing is patched into its inputs.
//! A wavetable oscillator (TIME scans the waves, PITCH tunes it, FEEDBACK picks one of 12 banks)
//! and grains of the same wavetables, then the limiters and the reverb, at 48 kHz.

#![forbid(unsafe_code)]

use crate::delay_line::Frame;
use crate::float::F;
use crate::fx::{Limiter, Reverb};
use crate::grain::{GrainCore, Scheduler, Voice};
use crate::randomized::{Parameters, RandomWalkModulation};
use crate::stmlib::hysteresis::HysteresisQuantizer;
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::{constrain, integral_fractional, one_pole, semitones_to_ratio};
use crate::trigger::{TriggerGenerator, TriggerOutput};
use crate::wavetables::{BANKS, TABLE_SIZE, WAVE_STRIDE, WAVES, WAVES_PER_BANK};
use crate::{Controls, MAX_BLOCK_SIZE};

/// The synth's sample rate, Hz.
pub const SYNTH_SAMPLE_RATE: u32 = 48_000;

/// The oscillator's root note, Hz (`kWavetableRootNote`).
const ROOT_NOTE: f32 = 130.81;

/// The synth's resolution (`Synth::Resolution`): the quality setting picks how coarsely the
/// oscillator and the grains are quantized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SynthResolution {
    /// 4 bits (the dirty tape setting).
    Bits4,
    /// A 16-bit oscillator and 6-bit grains (clean tape).
    Bits16Grains6,
    /// 7 bits (vintage digital).
    Bits7,
    /// 16 bits (digital).
    #[default]
    Bits16,
}

impl SynthResolution {
    /// Every resolution, in the firmware's order.
    pub const ALL: [Self; 4] = [Self::Bits4, Self::Bits16Grains6, Self::Bits7, Self::Bits16];

    /// The firmware's index.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Bits4 => 0,
            Self::Bits16Grains6 => 1,
            Self::Bits7 => 2,
            Self::Bits16 => 3,
        }
    }
}

/// `InterpolateWaveHermite(table, index_integral, index_fractional)` at `start` in the table.
#[inline(always)]
fn hermite(start: usize, index_integral: i32, f: F) -> F {
    let i = start + usize::try_from(index_integral).unwrap_or(0);
    let at = |k: usize| F::from_i32(i32::from(WAVES[(i + k).min(WAVES.len() - 1)]));
    let (xm1, x0, x1, x2) = (at(0), at(1), at(2), at(3));
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b_neg = w + a;
    (((a * f) - b_neg) * f + c) * f + x0
}

/// Two adjacent waves at `phase`, crossfaded by `fraction`.
#[inline(always)]
fn read_waves(start: usize, phase: F, fraction: F) -> F {
    let (pi, pf) = integral_fractional(phase * F::from_usize(TABLE_SIZE));
    let x0 = hermite(start, pi, pf);
    let x1 = hermite(start + WAVE_STRIDE, pi, pf);
    x0 + (x1 - x0) * fraction
}

fn wave_start(bank: usize, wave: usize) -> usize {
    (bank.min(BANKS - 1) * WAVES_PER_BANK + wave) * WAVE_STRIDE
}

/// `Differentiator`: the derivative of the integrated waves, through two one-poles.
#[derive(Clone, Copy, Debug, Default)]
struct Differentiator {
    previous: F,
    lp: F,
    lp_2: F,
    cutoff: F,
    scale: F,
}

impl Differentiator {
    fn init(&mut self, value: F, scale: F, cutoff: F) {
        *self = Self { previous: value, lp: F::ZERO, lp_2: F::ZERO, cutoff, scale };
    }

    fn process(&mut self, s: F) -> F {
        one_pole(&mut self.lp, (s - self.previous) * self.scale, self.cutoff);
        one_pole(&mut self.lp_2, self.lp, self.cutoff);
        self.previous = s;
        self.lp_2
    }
}

/// `WavetableGrain`.
#[derive(Clone, Copy, Debug, Default)]
pub struct WavetableGrain {
    core: GrainCore,
    phase: F,
    frequency: F,
    envelope_sample: F,
    scale_up: F,
    scale_down: F,
    gain_l: F,
    gain_r: F,
    wave_fractional: F,
    wave_start: usize,
    differentiator: Differentiator,
}

impl Voice for WavetableGrain {
    fn init(&mut self, sample_rate: F) {
        self.phase = F::ZERO;
        self.frequency = F::ZERO;
        self.envelope_sample = F::ZERO;
        self.scale_up = F(256.0);
        self.scale_down = F(1.0 / 256.0);
        self.wave_fractional = F::ZERO;
        self.gain_l = F::ZERO;
        self.gain_r = F::ZERO;
        self.core.envelope.init(sample_rate);
        self.core.stealing.reset_and_pause();
        self.differentiator.init(F::ZERO, F::ZERO, F::ZERO);
        self.core.remaining_samples = 0;
    }

    fn core(&self) -> &GrainCore {
        &self.core
    }

    fn core_mut(&mut self) -> &mut GrainCore {
        &mut self.core
    }
}

impl WavetableGrain {
    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, bank: usize, bit_shift: u32, wave: F, frequency: F, duration: F, shape: F, gain: F, pan: F, fractional_sample: F) {
        let (wi, wf) = integral_fractional(wave * F::from_usize(WAVES_PER_BANK - 1));
        self.wave_fractional = wf;
        self.wave_start = wave_start(bank, usize::try_from(wi).unwrap_or(0));
        let frequency = frequency.min(F(0.25));
        self.core.remaining_samples = duration.trunc_i32();
        self.core.envelope.start(duration, shape, fractional_sample);
        self.gain_l = gain * (1.0 - pan) * (1.0 + pan);
        self.gain_r = gain * pan * (2.0 - pan);
        self.core.stealing.reset_and_pause();
        self.scale_up = F::from_i32(1 << bit_shift);
        self.scale_down = 1.0 / self.scale_up;
        self.phase = fractional_sample * frequency;
        self.frequency = frequency;
        let first = read_waves(self.wave_start, self.phase, self.wave_fractional);
        self.differentiator.init(first, 1.0 / (frequency * 131_072.0), (F::from_usize(TABLE_SIZE) * frequency).min(F(1.0)));
    }

    #[inline(always)]
    fn render(&mut self, out: &mut Frame, freeze: bool) {
        if self.core.done() {
            return;
        }
        self.phase += self.frequency;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        let sample = self.differentiator.process(read_waves(self.wave_start, self.phase, self.wave_fractional));
        let sample = F::from_i32((sample * self.scale_up).trunc_i32()) * self.scale_down;
        if !freeze {
            self.envelope_sample = self.core.envelope.render() * self.core.stealing.render();
            self.core.remaining_samples -= 1;
            if self.core.done() {
                self.core.remaining_samples = 0;
            }
        }
        let sample = sample * self.envelope_sample;
        out[0] += sample * self.gain_l;
        out[1] += sample * self.gain_r;
    }
}

/// `WavetableVoice`: the oscillator, a wavetable grain of infinite length.
#[derive(Clone, Copy, Debug, Default)]
struct WavetableVoice {
    frequency: F,
    phase: F,
    root_note: F,
    lp: F,
    lp_2: F,
    previous: F,
    wave_lp: F,
    bank: usize,
    next_bank: usize,
    xfade_counter: i32,
    xfade_duration: i32,
    position_random_walk: RandomWalkModulation,
}

impl WavetableVoice {
    fn init(&mut self, sample_rate: F) {
        self.phase = F::ZERO;
        self.root_note = ROOT_NOTE / sample_rate;
        self.position_random_walk.init(1.01 / sample_rate);
        self.lp = F::ZERO;
        self.lp_2 = F::ZERO;
        self.previous = F::ZERO;
        self.frequency = self.root_note;
        self.wave_lp = F::ZERO;
        self.bank = 0;
        self.next_bank = 0;
        self.xfade_counter = 0;
        // A 25 ms crossfade when the bank changes.
        self.xfade_duration = (sample_rate * 0.025).trunc_i32();
    }

    fn render(&mut self, rng: &mut Random, bank: usize, bit_shift: u32, freeze: &[bool], p: &Parameters, out: &mut [Frame]) {
        let scale_up = F::from_i32(1 << bit_shift);
        let scale_down = 1.0 / scale_up;
        let gain = scale_down * 2.0 / F::from_i32(self.xfade_duration);
        if bank != self.bank && self.xfade_counter == 0 {
            self.next_bank = bank;
            self.xfade_counter = self.xfade_duration;
        }
        let target = (semitones_to_ratio(F(p.pitch.base)) * self.root_note).min(F(0.25));
        let mut fm = ParameterInterpolator::new(self.frequency, target, out.len());
        for (o, &frozen) in out.iter_mut().zip(freeze) {
            let position = p.time.value_with(rng, Some(&mut self.position_random_walk), if frozen { 0 } else { 1 }, F(1.0), true);
            one_pole(&mut self.wave_lp, position, F(0.025));
            let wave = self.wave_lp * F::from_usize(WAVES_PER_BANK - 1);
            let frequency = fm.next();
            self.phase += frequency;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            if self.xfade_counter != 0 {
                self.xfade_counter -= 1;
                if self.xfade_counter == self.xfade_duration / 2 {
                    self.bank = self.next_bank;
                }
            }
            let (wi, wf) = integral_fractional(wave);
            let sample = read_waves(wave_start(self.bank, usize::try_from(wi).unwrap_or(0)), self.phase, wf);
            let scale = 1.0 / (frequency * 131_072.0);
            let cutoff = (F::from_usize(TABLE_SIZE) * frequency).min(F(1.0));
            one_pole(&mut self.lp, (sample - self.previous) * scale, cutoff);
            one_pole(&mut self.lp_2, self.lp, cutoff);
            self.previous = sample;
            let xfade_gain = F::from_i32(self.xfade_counter - self.xfade_duration / 2).abs();
            let out_sample = F::from_i32((self.lp_2 * scale_up).trunc_i32()) * xfade_gain * gain;
            let out_sample = constrain(out_sample, F(-1.0), F(1.0));
            o[0] += out_sample;
            o[1] += out_sample;
        }
        self.frequency = fm.value();
    }
}

/// The synth (`beads::Synth`).
#[derive(Clone, Debug)]
pub struct Synth {
    rng: Random,
    trigger_generator: TriggerGenerator,
    dry: F,
    wet: F,
    reverb: Reverb,
    pre_reverb_limiter: Limiter,
    output_limiter: Limiter,
    grains: Scheduler<WavetableGrain>,
    voice: WavetableVoice,
    bank_quantizer: HysteresisQuantizer,
}

impl Default for Synth {
    fn default() -> Self {
        Self::new()
    }
}

impl Synth {
    /// A synth as the firmware's `Init` leaves it, with its random generator at the firmware's
    /// power-on state.
    #[must_use]
    pub fn new() -> Self {
        let sr = F(48_000.0);
        let mut synth = Self {
            rng: Random::default(),
            trigger_generator: TriggerGenerator::default(),
            dry: F::ZERO,
            wet: F::ZERO,
            reverb: Reverb::default(),
            pre_reverb_limiter: Limiter::default(),
            output_limiter: Limiter::default(),
            grains: Scheduler::default(),
            voice: WavetableVoice::default(),
            bank_quantizer: HysteresisQuantizer::default(),
        };
        synth.reverb.init(sr);
        synth.trigger_generator.init(sr);
        synth.pre_reverb_limiter.init(1.0 / (sr * 4.0), F(0.9));
        synth.output_limiter.init(1.0 / (sr * 6.0), F(0.9));
        synth.grains.init(sr);
        synth.voice.init(sr);
        synth.bank_quantizer.init(i32::try_from(BANKS).unwrap_or(12), F(0.048), false);
        synth
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

    /// Renders `output.len()` stereo frames at 48 kHz ([`SYNTH_SAMPLE_RATE`]). `freeze` holds one
    /// flag per frame; `seed` one gate-flags byte per frame, or `None`. Longer buffers are rendered
    /// in blocks of [`MAX_BLOCK_SIZE`]; the lengths must match (checked in debug builds; a release
    /// build renders the shortest). Controls are clamped to the module's ranges.
    pub fn render(&mut self, controls: &Controls, resolution: SynthResolution, freeze: &[bool], seed: Option<&[u8]>, output: &mut [[f32; 2]]) {
        debug_assert!(output.len() == freeze.len() && seed.is_none_or(|s| s.len() == output.len()), "lengths differ");
        let mut n = output.len().min(freeze.len());
        if let Some(s) = seed {
            n = n.min(s.len());
        }
        let p = crate::processor::sanitized(controls);
        let mut start = 0;
        while start < n {
            let end = (start + MAX_BLOCK_SIZE).min(n);
            let mut out = [[F::ZERO; 2]; MAX_BLOCK_SIZE];
            let size = end - start;
            self.render_block(&p, resolution, &freeze[start..end], seed.map(|s| &s[start..end]), &mut out[..size]);
            for (o, f) in output[start..end].iter_mut().zip(&out) {
                *o = [f[0].0, f[1].0];
            }
            start = end;
        }
    }

    fn render_block(&mut self, c: &crate::processor::Inputs, resolution: SynthResolution, freeze: &[bool], seed: Option<&[u8]>, out: &mut [Frame]) {
        let size = out.len();
        let mut p = c.parameters;
        p.pitch.base = (F(p.pitch.base) + if p.pitch.has_cv { F(p.pitch.cv) } else { F::ZERO }).0;
        p.pitch.has_cv = false;
        let expo_fm_amount = constrain(20.0 * ((c.density - 0.5).abs() - 0.225), F::ZERO, F(1.0));
        let mut triggers = [TriggerOutput::default(); MAX_BLOCK_SIZE];
        let triggers = &mut triggers[..size];
        self.trigger_generator.render(
            &mut self.rng,
            c.seed_button,
            seed,
            c.density,
            (1.0 - expo_fm_amount) * c.density_cv,
            expo_fm_amount * c.density_cv,
            triggers,
        );
        let bank = usize::try_from(self.bank_quantizer.process(c.feedback)).unwrap_or(0);
        let continuous_shift = [4, 16, 7, 16][resolution.index()];
        let grain_shift = [4, 6, 7, 16][resolution.index()];
        out.fill([F::ZERO; 2]);
        self.voice.render(&mut self.rng, bank, continuous_shift, freeze, &p, out);
        let dry_wet = c.dry_wet;
        let dry = (1.0 - dry_wet) * (1.0 + dry_wet) * 0.5;
        let wet = dry_wet * (2.0 - dry_wet) * 1.0;
        let mut dry_modulation = ParameterInterpolator::new(self.dry, dry, size);
        let mut wet_modulation = ParameterInterpolator::new(self.wet, wet, size);
        for (i, o) in out.iter_mut().enumerate() {
            let mut w = [F::ZERO; 2];
            self.render_grains(bank, grain_shift, &triggers[i], freeze[i], &p, &mut w);
            let dry_gain = dry_modulation.next();
            let wet_gain = wet_modulation.next();
            for ch in 0..2 {
                o[ch] = dry_gain * o[ch] + w[ch] * wet_gain;
            }
        }
        self.dry = dry_modulation.value();
        self.wet = wet_modulation.value();
        self.pre_reverb_limiter.process(out);
        let reverb = c.reverb;
        let reverb_mix = reverb * reverb * 0.8;
        let reverb_time = reverb * reverb * reverb;
        self.reverb.set_mix(reverb_mix);
        self.reverb.set_time(0.3 + 0.6 * reverb_time);
        self.reverb.set_lp(F(0.6));
        self.reverb.process(out);
        self.output_limiter.process(out);
        if c.output_gate {
            for (o, t) in out.iter_mut().zip(triggers.iter()) {
                o[0] = (o[0] + o[1]) * 0.5;
                o[1] = if t.gate { F(-0.8) } else { F::ZERO };
            }
        }
    }

    /// `WavetableGranularPlayer::Render(bank, bit_shift, trigger, freeze, p, out)`.
    fn render_grains(&mut self, bank: usize, bit_shift: u32, trigger: &TriggerOutput, freeze: bool, p: &Parameters, out: &mut Frame) {
        if trigger.trigger && !freeze {
            let (amplitude, pan) = self.grains.amplitude_and_pan(&mut self.rng, trigger, p);
            // The grain's random draws in source order, as the oracle's build sequences them
            // (`patches/all/0003`).
            let time_value = p.time.value(&mut self.rng);
            let frequency = p.pitch.pitch_value(&mut self.rng) * ROOT_NOTE * self.grains.one_hertz;
            let size = p.size.value(&mut self.rng);
            let grain_duration = self.grains.grain_duration(size);
            let shape_value = p.shape.value(&mut self.rng);
            self.grains.new_grain().start(
                bank,
                bit_shift,
                time_value,
                frequency,
                grain_duration,
                shape_value,
                amplitude,
                pan,
                trigger.fractional_sample,
            );
            self.grains.sort(p.monophonic);
        }
        for grain in &mut self.grains.grains {
            grain.render(out, freeze);
        }
        self.grains.tick();
    }
}
