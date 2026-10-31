//! `beads/dsp/trigger_generator.cc`: the regular or random pulse train that seeds grains, the
//! clock divider a SEED input drives, and the manual taps.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::hysteresis::HysteresisQuantizer;
use crate::stmlib::random::Random;
use crate::stmlib::{GATE_FLAG_HIGH, GATE_FLAG_RISING, constrain, extract_gate_flags, one_pole, semitones_to_ratio};
use crate::{MAX_BLOCK_SIZE, SeedButton};

/// One sample of the trigger stream (`TriggerGenerator::Output`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TriggerOutput {
    /// A grain starts on this sample.
    pub trigger: bool,
    /// The gate output.
    pub gate: bool,
    /// Where between this sample and the next the grain starts, 0 to 1.
    pub fractional_sample: F,
    /// 1 for a regular train, 0 for a random one (the grains' pan spread grows as it falls).
    pub regularity: F,
    /// The train's phase.
    pub phase: F,
}

const MAX_GRAIN_FREQUENCY: f32 = 130.812_78;
const DIVIDER_RESET_INTERVAL: f32 = 4.0;
const DIVISION_RATIOS: [i32; 8] = [1, 2, 3, 4, 6, 8, 12, 16];

/// `LogOneMinusX(x)`: a fourth-order series for `ln(1 - x)`.
#[inline(always)]
fn log_one_minus_x(x: F) -> F {
    -x * (1.0 + x * (0.5 + x * (0.3333 + x * 0.25)))
}

/// `TriggerGenerator`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TriggerGenerator {
    phase: F,
    frequency: F,
    target_frequency: F,
    frequency_lp_coefficient: F,
    ratio: F,
    one_hertz: F,
    division: i32,
    audio_rate: bool,
    divider_quantizer: HysteresisQuantizer,
    max_pulse_frequency: F,
    audio_rate_interval: i32,
    accept_pulse: bool,
    pulse_count: i32,
    elapsed_since_last_pulse: i32,
    divider_reset_interval: i32,
    default_reset_interval: i32,
    previous_flags: u8,
}

impl TriggerGenerator {
    /// `Init(sample_rate)`.
    pub fn init(&mut self, sample_rate: F) {
        self.one_hertz = 1.0 / sample_rate;
        self.max_pulse_frequency = MAX_GRAIN_FREQUENCY / sample_rate;
        self.default_reset_interval = (sample_rate * DIVIDER_RESET_INTERVAL).trunc_i32();
        self.divider_reset_interval = self.default_reset_interval;
        self.audio_rate_interval = (sample_rate * (1.0_f32 / 48.0)).trunc_i32();
        self.audio_rate = false;
        self.divider_quantizer.init(8, F(0.075), false);
        self.accept_pulse = true;
        self.pulse_count = 0;
        self.elapsed_since_last_pulse = (sample_rate / 2.0).trunc_i32();
        self.phase = F::ZERO;
        self.frequency = 1.0 / F::from_i32(self.elapsed_since_last_pulse);
        self.target_frequency = self.frequency;
        self.frequency_lp_coefficient = F(1.0);
        self.division = 1;
        self.ratio = F(1.0);
        self.previous_flags = 0;
    }

    fn render_pulse_train(&mut self, rng: &mut Random, gate_flags: &[u8], frequency: F, randomize: bool, out: &mut [TriggerOutput]) {
        if !randomize {
            self.ratio = F(1.0);
        }
        for (&flags, o) in gate_flags.iter().zip(out.iter_mut()) {
            let f = constrain(frequency * self.ratio, F(0.000_001), F(0.25));
            let gate = flags & GATE_FLAG_HIGH != 0;
            let trigger = flags & GATE_FLAG_RISING != 0;
            self.phase += if gate { f } else { F::ZERO };
            let mut t = F::ZERO;
            let mut reset = false;
            if trigger {
                self.phase = F::ZERO;
                reset = true;
            } else if self.phase >= 1.0 {
                self.phase -= 1.0;
                t = self.phase / f;
                reset = true;
            }
            if reset && randomize {
                self.ratio = -1.0 / log_one_minus_x(0.1 + 0.9 * rng.get_float());
            }
            *o = TriggerOutput {
                fractional_sample: t,
                trigger: reset && gate,
                gate: self.phase < 0.5 && gate,
                regularity: if randomize { F::ZERO } else { F(1.0) },
                phase: self.phase,
            };
        }
    }

    fn render_divider(&mut self, rng: &mut Random, gate_flags: &[u8], density: F, out: &mut [TriggerOutput]) {
        let division = if density < 0.48 { DIVISION_RATIOS[usize::try_from(self.divider_quantizer.process(density * 2.15)).unwrap_or(0)] } else { 1 };
        let probability_sqrt = if density < 0.48 { F(1.01) } else { ((density - 0.52) * 2.15).max(F::ZERO) };
        let probability = probability_sqrt * probability_sqrt;
        let regularity = if probability >= 1.0 { F(1.0) } else { F::ZERO };
        for (&flags, o) in gate_flags.iter().zip(out.iter_mut()) {
            one_pole(&mut self.frequency, self.target_frequency, self.frequency_lp_coefficient);
            self.phase += self.frequency;
            let gate = flags & GATE_FLAG_HIGH != 0;
            let trigger = flags & GATE_FLAG_RISING != 0;
            if trigger {
                if self.elapsed_since_last_pulse >= self.divider_reset_interval {
                    self.pulse_count = 0;
                    self.phase = F::ZERO;
                }
                self.divider_reset_interval = self.default_reset_interval.max(4 * self.elapsed_since_last_pulse);
                let threshold = self.audio_rate_interval + if self.audio_rate { 8 } else { 0 };
                self.audio_rate = self.elapsed_since_last_pulse < threshold && probability >= 1.0;
                self.accept_pulse = rng.get_float() < probability && self.pulse_count % division == 0;
                if self.accept_pulse {
                    let period = F::from_i32(self.elapsed_since_last_pulse);
                    let frequency = 1.0 / period;
                    self.target_frequency = if probability >= 1.0 { frequency / F::from_i32(division) } else { frequency * probability };
                    if self.audio_rate {
                        let mut no_glide = division != self.division;
                        self.division = division;
                        let up_tolerance = (1.02 + 2.0 * frequency) * self.frequency;
                        let down_tolerance = (0.98 - 2.0 * frequency) * self.frequency;
                        no_glide |= self.target_frequency > up_tolerance || self.target_frequency < down_tolerance;
                        self.frequency_lp_coefficient = if no_glide { F(1.0) } else { period * self.one_hertz };
                    } else {
                        self.phase = F::ZERO;
                        self.frequency_lp_coefficient = F(1.0);
                    }
                } else if probability < 1.0 {
                    self.target_frequency *= 1.0 - probability;
                }
                self.pulse_count = (self.pulse_count + 1) % 48;
                self.elapsed_since_last_pulse = 0;
            }
            self.elapsed_since_last_pulse += 1;
            if self.audio_rate {
                let mut t = F::ZERO;
                let reset = self.phase >= 1.0;
                if reset {
                    self.phase -= 1.0;
                    t = self.phase / self.frequency;
                }
                o.fractional_sample = t;
                o.trigger = reset;
                o.gate = self.phase < 0.5;
            } else {
                if self.phase >= 1.0 {
                    self.phase = F(1.0);
                }
                o.fractional_sample = F::ZERO;
                o.trigger = self.accept_pulse && trigger;
                o.gate = self.accept_pulse && gate;
            }
            o.regularity = regularity;
            o.phase = self.phase;
        }
    }

    /// `Render(seed_button_state, external_input, density, linear_fm, exponential_fm, out, size)`
    /// over `out.len()` samples (at most [`MAX_BLOCK_SIZE`]); `external` holds one gate-flags byte
    /// per sample, or is `None` when nothing is patched into SEED.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        rng: &mut Random,
        seed_button: SeedButton,
        external: Option<&[u8]>,
        density: F,
        linear_fm: F,
        exponential_fm: F,
        out: &mut [TriggerOutput],
    ) {
        if let (Some(external), SeedButton::Locked) = (external, seed_button) {
            let density = constrain(density + (linear_fm + exponential_fm), F::ZERO, F(1.0));
            self.render_divider(rng, external, density, out);
            return;
        }
        let density = constrain(density + linear_fm, F::ZERO, F(1.0));
        let fm = semitones_to_ratio(exponential_fm * 60.0);
        let mut merged = [0_u8; MAX_BLOCK_SIZE];
        let merged = &mut merged[..out.len()];
        for (i, m) in merged.iter_mut().enumerate() {
            let mut gate = seed_button != SeedButton::Off;
            if let Some(external) = external {
                gate |= external[i] & GATE_FLAG_HIGH != 0;
            }
            self.previous_flags = extract_gate_flags(self.previous_flags, gate);
            *m = self.previous_flags;
        }
        if density < 0.48 {
            let frequency = self.max_pulse_frequency * semitones_to_ratio(-density * 192.0) * fm;
            self.render_pulse_train(rng, merged, frequency, false, out);
        } else if density > 0.52 {
            let frequency = 0.5 * self.max_pulse_frequency * semitones_to_ratio(-(1.0 - density) * 192.0) * fm;
            self.render_pulse_train(rng, merged, frequency, true, out);
        } else if seed_button == SeedButton::Locked {
            out.fill(TriggerOutput { phase: self.phase, ..TriggerOutput::default() });
        } else {
            for (&flags, o) in merged.iter().zip(out.iter_mut()) {
                self.phase += self.frequency;
                let tap = flags & GATE_FLAG_RISING != 0;
                if tap {
                    self.phase = F::ZERO;
                    self.frequency = 1.0 / F::from_i32(self.elapsed_since_last_pulse);
                    self.elapsed_since_last_pulse = 0;
                }
                self.elapsed_since_last_pulse += 1;
                if self.phase >= 1.0 {
                    self.phase = F(1.0);
                }
                *o = TriggerOutput {
                    fractional_sample: F::ZERO,
                    trigger: tap,
                    gate: flags & GATE_FLAG_HIGH != 0,
                    regularity: 1.0 - (density - 0.48) * 25.0,
                    phase: self.phase,
                };
            }
        }
    }
}
