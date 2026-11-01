//! The modal resonator (`elements/dsp/resonator.{h,cc}`): up to 64 band-pass modes (the voice runs
//! 52) tuned by the geometry's stiffness, their sum shaped by the position as a pickup comb, a
//! side channel read at an LFO-swept position, and eight banded waveguides for the bow.

#![forbid(unsafe_code)]

use transit_resonator::float::F;
use transit_resonator::stmlib::cosine_oscillator::CosineOscillator;
use transit_resonator::stmlib::delay_line::DelayLine;
use transit_resonator::stmlib::filter::FrequencyApproximation;
use transit_resonator::stmlib::interpolate;

use crate::dsp::{Mode, Svf};
use crate::resources::{LUT_4_DECADES, LUT_STIFFNESS};

/// `kMaxModes`.
const MAX_MODES: usize = 64;
/// `kMaxBowedModes`.
const MAX_BOWED_MODES: usize = 8;
/// `kMaxDelayLineSize`.
const MAX_DELAY_LINE_SIZE: usize = 1024;

/// `elements::Resonator`.
#[derive(Clone, Debug)]
pub(crate) struct ModalResonator {
    frequency: F,
    geometry: F,
    brightness: F,
    position: F,
    previous_position: F,
    damping: F,
    modulation_frequency: F,
    modulation_offset: F,
    /// The side channel's LFO phase. `Init` never writes it: it starts at zero (the module's static
    /// storage) and survives re-initializations.
    lfo_phase: F,
    bow_signal: F,
    resolution: usize,
    f: [Svf; MAX_MODES],
    f_bow: [Svf; MAX_BOWED_MODES],
    d_bow: [DelayLine<MAX_DELAY_LINE_SIZE>; MAX_BOWED_MODES],
    /// Picks which half of the upper modes is refreshed each block. `Init` never writes it either.
    clock_divider: usize,
}

impl Default for ModalResonator {
    fn default() -> Self {
        Self {
            frequency: F::ZERO,
            geometry: F::ZERO,
            brightness: F::ZERO,
            position: F::ZERO,
            previous_position: F::ZERO,
            damping: F::ZERO,
            modulation_frequency: F::ZERO,
            modulation_offset: F::ZERO,
            lfo_phase: F::ZERO,
            bow_signal: F::ZERO,
            resolution: 0,
            f: [Svf::default(); MAX_MODES],
            f_bow: [Svf::default(); MAX_BOWED_MODES],
            d_bow: std::array::from_fn(|_| DelayLine::default()),
            clock_divider: 0,
        }
    }
}

/// `BowTable(x, velocity)`. The firmware adds the double literal 0.75 to `fabs(bow)`, so that sum
/// is computed in double and rounded to float once.
#[inline(always)]
fn bow_table(x: F, velocity: F) -> F {
    let x = 0.13 * velocity - x;
    let mut bow = x;
    bow *= 6.0;
    #[allow(clippy::cast_possible_truncation)]
    let sum = (f64::from(bow.abs().0) + 0.75) as f32;
    bow = F(transit_resonator::float::ftz(sum));
    bow *= bow;
    bow *= bow;
    bow = 0.25 / bow;
    if bow < 0.0025 {
        bow = F(0.0025);
    }
    if bow > 0.245 {
        bow = F(0.245);
    }
    x * bow
}

impl ModalResonator {
    /// This resonator as [`Default`] builds it, moving its delay lines out without allocating:
    /// `self` is left only fit to be dropped or replaced.
    #[must_use]
    pub(crate) fn take_zeroed(&mut self) -> Self {
        Self {
            frequency: F::ZERO,
            geometry: F::ZERO,
            brightness: F::ZERO,
            position: F::ZERO,
            previous_position: F::ZERO,
            damping: F::ZERO,
            modulation_frequency: F::ZERO,
            modulation_offset: F::ZERO,
            lfo_phase: F::ZERO,
            bow_signal: F::ZERO,
            resolution: 0,
            f: [Svf::default(); MAX_MODES],
            f_bow: [Svf::default(); MAX_BOWED_MODES],
            d_bow: std::array::from_fn(|i| self.d_bow[i].take_zeroed()),
            clock_divider: 0,
        }
    }

    /// `Init()`.
    pub(crate) fn init(&mut self) {
        for f in &mut self.f {
            f.init();
        }
        for (f, d) in self.f_bow.iter_mut().zip(self.d_bow.iter_mut()) {
            f.init();
            d.init();
        }
        self.frequency = F(220.0) / crate::SAMPLE_RATE;
        self.geometry = F(0.25);
        self.brightness = F(0.5);
        self.damping = F(0.3);
        self.position = F(0.999);
        self.previous_position = F::ZERO;
        self.resolution = MAX_MODES;
        self.bow_signal = F::ZERO;
    }

    pub(crate) fn set_frequency(&mut self, frequency: F) {
        self.frequency = frequency;
    }

    pub(crate) fn set_geometry(&mut self, geometry: F) {
        self.geometry = geometry;
    }

    pub(crate) fn set_brightness(&mut self, brightness: F) {
        self.brightness = brightness;
    }

    pub(crate) fn set_damping(&mut self, damping: F) {
        self.damping = damping;
    }

    pub(crate) fn set_position(&mut self, position: F) {
        self.position = position;
    }

    /// `set_resolution(resolution)`: at most [`MAX_MODES`].
    pub(crate) fn set_resolution(&mut self, resolution: usize) {
        self.resolution = resolution.min(MAX_MODES);
    }

    pub(crate) fn set_modulation_frequency(&mut self, modulation_frequency: F) {
        self.modulation_frequency = modulation_frequency;
    }

    pub(crate) fn set_modulation_offset(&mut self, modulation_offset: F) {
        self.modulation_offset = modulation_offset;
    }

    /// `ComputeFilters()`: retunes the modes and returns how many sit below 0.49 of the rate.
    fn compute_filters(&mut self) -> usize {
        self.clock_divider = self.clock_divider.wrapping_add(1);
        let mut stiffness = interpolate(&LUT_STIFFNESS, self.geometry, 256.0);
        let mut harmonic = self.frequency;
        let mut stretch_factor = F(1.0);
        let mut q = 500.0 * interpolate(&LUT_4_DECADES, self.damping * 0.8, 256.0);
        let mut brightness_attenuation = 1.0 - self.geometry;
        // Reduces the range of brightness when geometry is very low, to prevent clipping.
        brightness_attenuation *= brightness_attenuation;
        brightness_attenuation *= brightness_attenuation;
        brightness_attenuation *= brightness_attenuation;
        let brightness = self.brightness * (1.0 - 0.2 * brightness_attenuation);
        let mut q_loss = brightness * (2.0 - brightness) * 0.85 + 0.15;
        let q_loss_damping_rate = self.geometry * (2.0 - self.geometry) * 0.1;
        let mut num_modes = 0;
        for i in 0..MAX_MODES.min(self.resolution) {
            // The first 25 modes are updated every block; the higher ones every other block.
            let update = i <= 24 || ((i & 1) == (self.clock_divider & 1));
            let mut partial_frequency = harmonic * stretch_factor;
            if partial_frequency >= 0.49 {
                partial_frequency = F(0.49);
            } else {
                num_modes = i + 1;
            }
            if update {
                self.f[i].set_f_q(FrequencyApproximation::Fast, partial_frequency, 1.0 + partial_frequency * q);
                if i < MAX_BOWED_MODES {
                    let mut period = (1.0 / partial_frequency).trunc_usize();
                    while period >= MAX_DELAY_LINE_SIZE {
                        period >>= 1;
                    }
                    self.d_bow[i].set_delay(period);
                    let g = self.f[i].g();
                    self.f_bow[i].set_g_q(g, 1.0 + partial_frequency * 1500.0);
                }
            }
            stretch_factor += stiffness;
            if stiffness < 0.0 {
                // Make sure that the partials do not fold back into negative frequencies.
                stiffness *= 0.93;
            } else {
                // This helps adding a few extra partials in the highest frequencies.
                stiffness *= 0.98;
            }
            // This prevents the highest partials from decaying too fast.
            q_loss += q_loss_damping_rate * (1.0 - q_loss);
            harmonic += self.frequency;
            q *= q_loss;
        }
        num_modes
    }

    /// `Process(bow_strength, in, center, sides, size)`.
    pub(crate) fn process(&mut self, bow_strength: &[F], input: &[F], center: &mut [F], sides: &mut [F]) {
        let num_modes = self.compute_filters();
        let num_banded_wg = MAX_BOWED_MODES.min(num_modes);
        // Linearly interpolate the position: it is extremely sensitive to zipper noise.
        let position_increment = (self.position - self.previous_position) / F::from_usize(input.len());
        for (((x, bow), c), s) in input.iter().zip(bow_strength).zip(center.iter_mut()).zip(sides.iter_mut()) {
            // A 0.5 Hz LFO moves the position the side channel is read at.
            self.lfo_phase += self.modulation_frequency;
            if self.lfo_phase >= 1.0 {
                self.lfo_phase -= 1.0;
            }
            self.previous_position += position_increment;
            let lfo = if self.lfo_phase > 0.5 { 1.0 - self.lfo_phase } else { self.lfo_phase };
            let mut amplitudes = CosineOscillator::default();
            let mut aux_amplitudes = CosineOscillator::default();
            amplitudes.init_approximate(self.previous_position);
            aux_amplitudes.init_approximate(self.modulation_offset + lfo);

            // The normal modes; the pickup comb is applied in the frequency domain, as each mode's
            // amplitude in the sum.
            let mut input = *x * 0.125;
            let mut sum_center = F::ZERO;
            let mut sum_side = F::ZERO;
            amplitudes.start();
            aux_amplitudes.start();
            for f in &mut self.f[..num_modes] {
                let v = f.process(input, Mode::BandPass);
                sum_center += v * amplitudes.next();
                sum_side += v * aux_amplitudes.next();
            }
            *s = sum_side - sum_center;

            // The bowed modes.
            let mut bow_signal = F::ZERO;
            input += self.bow_signal;
            amplitudes.start();
            for (f, d) in self.f_bow[..num_banded_wg].iter_mut().zip(self.d_bow.iter_mut()) {
                let mut v = 0.99 * d.read_fixed();
                bow_signal += v;
                v = f.process(input + v, Mode::BandPassNormalized);
                d.write(v);
                sum_center += v * amplitudes.next() * 8.0;
            }
            self.bow_signal = bow_table(bow_signal, *bow);
            *c = sum_center;
        }
    }
}
