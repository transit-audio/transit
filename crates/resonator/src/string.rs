//! The string (`rings/dsp/string.{h,cc}`): a Karplus-Strong comb with Hermite-interpolated delay,
//! FIR and IIR damping and, for the inharmonic models, dispersion (an allpass stiffness section, a
//! curved bridge and delay noise).

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::LUT_SVF_SHIFT;
use crate::stmlib::delay_line::DelayLine;
use crate::stmlib::filter::{DcBlocker, FilterMode, FrequencyApproximation, Svf};
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::random::Random;
use crate::stmlib::{constrain, crossfade, interpolate, semitones_to_ratio};

/// `kDelayLineSize`.
pub const DELAY_LINE_SIZE: usize = 2048;

/// `rings::DampingFilter`: a three-tap FIR whose damping and brightness ramp over a block.
#[derive(Clone, Copy, Debug, Default)]
struct DampingFilter {
    x: F,
    x_: F,
    brightness: F,
    brightness_increment: F,
    damping: F,
    damping_increment: F,
}

impl DampingFilter {
    fn init(&mut self) {
        *self = Self::default();
    }

    /// `Configure(damping, brightness, size)` for a non-empty block: ramps to the new values.
    fn configure(&mut self, damping: F, brightness: F, size: usize) {
        let step = 1.0 / F::from_usize(size);
        self.damping_increment = (damping - self.damping) * step;
        self.brightness_increment = (brightness - self.brightness) * step;
    }

    #[inline(always)]
    fn process(&mut self, x: F) -> F {
        let h0 = (1.0 + self.brightness) * 0.5;
        let h1 = (1.0 - self.brightness) * 0.25;
        let y = self.damping * (h0 * self.x + h1 * (x + self.x_));
        self.x_ = self.x;
        self.x = x;
        self.brightness += self.brightness_increment;
        self.damping += self.damping_increment;
        y
    }
}

/// `rings::String`.
#[derive(Clone, Debug, Default)]
pub struct KarplusString {
    frequency: F,
    dispersion: F,
    brightness: F,
    damping: F,
    position: F,
    delay: F,
    clamped_position: F,
    previous_dispersion: F,
    previous_damping_compensation: F,
    enable_dispersion: bool,
    dispersion_noise: F,
    /// The low-pitch resampler's phase. `Init` never writes it: it starts at zero (the module's
    /// static storage) and survives re-initialization, as on the module.
    src_phase: F,
    out_sample: [F; 2],
    aux_sample: [F; 2],
    curved_bridge: F,
    string: DelayLine<DELAY_LINE_SIZE>,
    stretch: DelayLine<{ DELAY_LINE_SIZE / 2 }>,
    fir_damping_filter: DampingFilter,
    iir_damping_filter: Svf,
    dc_blocker: DcBlocker,
}

impl KarplusString {
    /// `Init(enable_dispersion)`.
    pub fn init(&mut self, enable_dispersion: bool) {
        self.enable_dispersion = enable_dispersion;
        self.string.init();
        self.stretch.init();
        self.fir_damping_filter.init();
        self.iir_damping_filter.init();
        self.frequency = F(220.0) / crate::SAMPLE_RATE;
        self.dispersion = F(0.25);
        self.brightness = F(0.5);
        self.damping = F(0.3);
        self.position = F(0.8);
        self.delay = 1.0 / self.frequency;
        self.clamped_position = F::ZERO;
        self.previous_dispersion = F::ZERO;
        self.dispersion_noise = F::ZERO;
        self.curved_bridge = F::ZERO;
        self.previous_damping_compensation = F::ZERO;
        self.out_sample = [F::ZERO; 2];
        self.aux_sample = [F::ZERO; 2];
        self.dc_blocker.init(1.0 - F(20.0) / crate::SAMPLE_RATE);
    }

    /// `set_frequency(frequency, coefficient)`: glides toward `frequency` by `coefficient`.
    pub fn set_frequency_glide(&mut self, frequency: F, coefficient: F) {
        self.frequency += coefficient * (frequency - self.frequency);
    }

    /// `set_dispersion`.
    pub fn set_dispersion(&mut self, dispersion: F) {
        self.dispersion = dispersion;
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

    /// `Process(in, out, aux, size)`: adds the string's output to `out` and its pickup to `aux`.
    pub fn process(&mut self, input: &[F], out: &mut [F], aux: &mut [F], random: &mut Random) {
        if self.enable_dispersion {
            self.process_internal::<true>(input, out, aux, random);
        } else {
            self.process_internal::<false>(input, out, aux, random);
        }
    }

    fn process_internal<const DISPERSION: bool>(&mut self, input: &[F], out: &mut [F], aux: &mut [F], random: &mut Random) {
        let size = input.len();
        let delay = constrain(1.0 / self.frequency, F(4.0), F::from_usize(DELAY_LINE_SIZE) - 4.0);

        // If there is not enough delay time in the delay line, the string plays at the lowest
        // possible note and is upsampled on the fly with a linear interpolator (f0 < 11.7 Hz).
        let mut src_ratio = delay * self.frequency;
        if src_ratio >= 0.9999 {
            // Above 11.7 Hz the interpolator does not get in the way.
            self.src_phase = F(1.0);
            src_ratio = F(1.0);
        }

        let clamped_position = 0.5 - 0.98 * (self.position - 0.5).abs();

        // Linearly interpolate all comb-related CV parameters for each sample.
        let mut delay_modulation = ParameterInterpolator::new(self.delay, delay, size);
        let mut position_modulation = ParameterInterpolator::new(self.clamped_position, clamped_position, size);
        let mut dispersion_modulation = ParameterInterpolator::new(self.previous_dispersion, self.dispersion, size);

        // For damping and absorption, the interpolation is done in the filter.
        let lf_damping = self.damping * (2.0 - self.damping);
        let rt60 = 0.07 * semitones_to_ratio(lf_damping * 96.0) * crate::SAMPLE_RATE;
        let rt60_base_2_12 = (-120.0 * delay / src_ratio / rt60).max(F(-127.0));
        let mut damping_coefficient = semitones_to_ratio(rt60_base_2_12);
        let mut brightness = self.brightness * self.brightness;
        let noise_filter = semitones_to_ratio((self.brightness - 1.0) * 48.0);
        let mut damping_cutoff = (24.0 + self.damping * self.damping * 48.0 + self.brightness * self.brightness * 24.0).min(F(84.0));
        let mut damping_f = (self.frequency * semitones_to_ratio(damping_cutoff)).min(F(0.499));

        // Crossfade to infinite decay.
        if self.damping >= 0.95 {
            let to_infinite = 20.0 * (self.damping - 0.95);
            damping_coefficient += to_infinite * (1.0 - damping_coefficient);
            brightness += to_infinite * (1.0 - brightness);
            damping_f += to_infinite * (0.4999 - damping_f);
            damping_cutoff += to_infinite * (128.0 - damping_cutoff);
        }

        self.fir_damping_filter.configure(damping_coefficient, brightness, size);
        self.iir_damping_filter.set_f_q(FrequencyApproximation::Accurate, damping_f, F(0.5));
        let mut damping_compensation_modulation =
            ParameterInterpolator::new(self.previous_damping_compensation, 1.0 - interpolate(&LUT_SVF_SHIFT, damping_cutoff, 1.0), size);

        for ((x, o), a) in input.iter().zip(out.iter_mut()).zip(aux.iter_mut()) {
            self.src_phase += src_ratio;
            if self.src_phase > 1.0 {
                self.src_phase -= 1.0;

                let mut delay = delay_modulation.next();
                let comb_delay = delay * position_modulation.next();
                delay *= damping_compensation_modulation.next(); // IIR delay.
                delay -= 1.0; // FIR delay.

                let mut s;
                if DISPERSION {
                    let mut noise = 2.0 * random.get_float() - 1.0;
                    noise *= 1.0 / (0.2 + noise_filter);
                    self.dispersion_noise += noise_filter * (noise - self.dispersion_noise);

                    let dispersion = dispersion_modulation.next();
                    let stretch_point = if dispersion <= 0.0 { F::ZERO } else { dispersion * (2.0 - dispersion) * 0.475 };
                    let mut noise_amount = if dispersion > 0.75 { 4.0 * (dispersion - 0.75) } else { F::ZERO };
                    let mut bridge_curving = if dispersion < 0.0 { -dispersion } else { F::ZERO };

                    noise_amount = noise_amount * noise_amount * 0.025;
                    let ac_blocking_amount = bridge_curving;

                    bridge_curving = bridge_curving * bridge_curving * 0.01;
                    let ap_gain = -0.618 * dispersion / (0.15 + dispersion.abs());

                    let mut delay_fm = F(1.0);
                    delay_fm += self.dispersion_noise * noise_amount;
                    delay_fm -= self.curved_bridge * bridge_curving;
                    delay *= delay_fm;

                    let ap_delay = delay * stretch_point;
                    let main_delay = delay - ap_delay;
                    if ap_delay >= 4.0 && main_delay >= 4.0 {
                        s = self.string.read_hermite(main_delay);
                        s = self.stretch.allpass(s, ap_delay.trunc_usize(), ap_gain);
                    } else {
                        s = self.string.read_hermite(delay);
                    }
                    let mut s_ac = [s];
                    self.dc_blocker.process(&mut s_ac);
                    s += ac_blocking_amount * (s_ac[0] - s);

                    let value = s.abs() - 0.025;
                    let sign = if s > 0.0 { F(1.0) } else { F(-1.5) };
                    self.curved_bridge = (value.abs() + value) * sign;
                } else {
                    s = self.string.read_hermite(delay);
                }

                s += *x; // When f0 < 11.7 Hz, causes ugly bitcrushing on the input!
                s = self.fir_damping_filter.process(s);
                s = self.iir_damping_filter.process(s, FilterMode::LowPass);
                self.string.write(s);

                self.out_sample[1] = self.out_sample[0];
                self.aux_sample[1] = self.aux_sample[0];

                self.out_sample[0] = s;
                self.aux_sample[0] = self.string.read(comb_delay);
            }
            *o += crossfade(self.out_sample[1], self.out_sample[0], self.src_phase);
            *a += crossfade(self.aux_sample[1], self.aux_sample[0], self.src_phase);
        }

        self.delay = delay_modulation.value();
        self.clamped_position = position_modulation.value();
        self.previous_dispersion = dispersion_modulation.value();
        self.previous_damping_compensation = damping_compensation_modulation.value();
    }
}
