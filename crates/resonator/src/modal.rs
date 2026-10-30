//! The modal resonator (`rings/dsp/resonator.{h,cc}`): up to 64 band-pass modes whose spacing,
//! decay and pickup position follow structure, damping, brightness and position.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::{LUT_4_DECADES, LUT_STIFFNESS};
use crate::stmlib::cosine_oscillator::CosineOscillator;
use crate::stmlib::filter::{FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::interpolate;
use crate::stmlib::parameter_interpolator::ParameterInterpolator;

/// `kMaxModes`.
pub const MAX_MODES: usize = 64;

/// `rings::Resonator`.
#[derive(Clone, Debug)]
pub struct ModalResonator {
    frequency: F,
    structure: F,
    brightness: F,
    position: F,
    previous_position: F,
    damping: F,
    resolution: usize,
    f: [Svf; MAX_MODES],
}

impl Default for ModalResonator {
    fn default() -> Self {
        Self {
            frequency: F::ZERO,
            structure: F::ZERO,
            brightness: F::ZERO,
            position: F::ZERO,
            previous_position: F::ZERO,
            damping: F::ZERO,
            resolution: 0,
            f: [Svf::default(); MAX_MODES],
        }
    }
}

impl ModalResonator {
    /// `Init`.
    pub fn init(&mut self) {
        for f in &mut self.f {
            f.init();
        }
        self.frequency = F(220.0) / crate::SAMPLE_RATE;
        self.structure = F(0.25);
        self.brightness = F(0.5);
        self.damping = F(0.3);
        self.position = F(0.999);
        self.previous_position = F::ZERO;
        self.set_resolution(MAX_MODES);
    }

    /// `set_frequency` (normalized to the sample rate).
    pub fn set_frequency(&mut self, frequency: F) {
        self.frequency = frequency;
    }

    /// `set_structure`.
    pub fn set_structure(&mut self, structure: F) {
        self.structure = structure;
    }

    /// `set_brightness`.
    pub fn set_brightness(&mut self, brightness: F) {
        self.brightness = brightness;
    }

    /// `set_damping`.
    pub fn set_damping(&mut self, damping: F) {
        self.damping = damping;
    }

    /// `set_position`.
    pub fn set_position(&mut self, position: F) {
        self.position = position;
    }

    /// `set_resolution`: rounded down to an even number of modes, at most [`MAX_MODES`].
    pub fn set_resolution(&mut self, resolution: usize) {
        let resolution = resolution - (resolution & 1);
        self.resolution = resolution.min(MAX_MODES);
    }

    /// `ComputeFilters`: tunes every mode and returns how many lie below 0.49 of the sample rate.
    fn compute_filters(&mut self) -> usize {
        let mut stiffness = interpolate(&LUT_STIFFNESS, self.structure, 256.0);
        let mut harmonic = self.frequency;
        let mut stretch_factor = F(1.0);
        let mut q = 500.0 * interpolate(&LUT_4_DECADES, self.damping, 256.0);
        let mut brightness_attenuation = 1.0 - self.structure;
        // Reduces the range of brightness when structure is very low, to prevent clipping.
        brightness_attenuation *= brightness_attenuation;
        brightness_attenuation *= brightness_attenuation;
        brightness_attenuation *= brightness_attenuation;
        let brightness = self.brightness * (1.0 - 0.2 * brightness_attenuation);
        let mut q_loss = brightness * (2.0 - brightness) * 0.85 + 0.15;
        let q_loss_damping_rate = self.structure * (2.0 - self.structure) * 0.1;
        let mut num_modes = 0;
        for i in 0..MAX_MODES.min(self.resolution) {
            let mut partial_frequency = harmonic * stretch_factor;
            if partial_frequency >= 0.49 {
                partial_frequency = F(0.49);
            } else {
                num_modes = i + 1;
            }
            self.f[i].set_f_q(FrequencyApproximation::Fast, partial_frequency, 1.0 + partial_frequency * q);
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

    /// `Process(in, out, aux, size)`: the odd modes into `out`, the even ones into `aux`
    /// (overwritten). Modes are processed in pairs, so an odd mode count also runs the next mode.
    pub fn process(&mut self, input: &[F], out: &mut [F], aux: &mut [F]) {
        let num_modes = self.compute_filters();
        let size = input.len();
        let mut position = ParameterInterpolator::new(self.previous_position, self.position, size);
        for ((x, o), a) in input.iter().zip(out.iter_mut()).zip(aux.iter_mut()) {
            let mut amplitudes = CosineOscillator::default();
            amplitudes.init_approximate(position.next());
            let x = *x * 0.125;
            let mut odd = F::ZERO;
            let mut even = F::ZERO;
            amplitudes.start();
            let mut i = 0;
            while i < num_modes {
                odd += amplitudes.next() * self.f[i].process(x, FilterMode::BandPass);
                even += amplitudes.next() * self.f[i + 1].process(x, FilterMode::BandPass);
                i += 2;
            }
            *o = odd;
            *a = even;
        }
        self.previous_position = position.value();
    }
}
