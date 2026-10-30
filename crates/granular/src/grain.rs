//! One grain (`clouds/dsp/grain.h`): a windowed, resampled read of the recording buffer, started at
//! any sample of a block and rendered at one of three qualities.

#![forbid(unsafe_code)]

use crate::audio_buffer::{AudioBuffer, Interpolation};
use crate::float::F;
use crate::resources::LUT_WINDOW;
use crate::stmlib::interpolate;

/// `GrainQuality`, which picks the read interpolation and the envelope shaping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum GrainQuality {
    /// `GRAIN_QUALITY_LOW`: zero-order hold, a plain triangle window. The zero-initialized state of
    /// a grain that never started; the player schedules only the other two.
    #[default]
    Low,
    /// `GRAIN_QUALITY_MEDIUM`: linear reads.
    Medium,
    /// `GRAIN_QUALITY_HIGH`: Hermite reads and the raised-cosine window.
    High,
}

impl GrainQuality {
    fn interpolation(self) -> Interpolation {
        match self {
            Self::Low => Interpolation::Zoh,
            Self::Medium => Interpolation::Linear,
            Self::High => Interpolation::Hermite,
        }
    }
}

/// `Grain`. Every field starts at zero, as the module's grains in static storage: `Init` writes
/// only `active_` and `envelope_phase_`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Grain {
    first_sample: i32,
    phase: i32,
    phase_increment: i32,
    pre_delay: i32,
    envelope_smoothness: F,
    envelope_slope: F,
    envelope_phase: F,
    envelope_phase_increment: F,
    gain_l: F,
    gain_r: F,
    active: bool,
    recommended_quality: GrainQuality,
}

/// What [`Grain::start`] takes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GrainStart {
    pub(crate) pre_delay: i32,
    pub(crate) buffer_size: i32,
    pub(crate) start: i32,
    pub(crate) width: i32,
    pub(crate) phase_increment: i32,
    pub(crate) window_shape: F,
    pub(crate) gain_l: F,
    pub(crate) gain_r: F,
    pub(crate) quality: GrainQuality,
}

impl Grain {
    /// `Init()`.
    pub(crate) fn init(&mut self) {
        self.active = false;
        self.envelope_phase = F(2.0);
    }

    /// `active()`.
    pub(crate) fn active(&self) -> bool {
        self.active
    }

    /// `Start(...)`.
    pub(crate) fn start(&mut self, s: &GrainStart) {
        self.pre_delay = s.pre_delay;
        // `(start + buffer_size) % buffer_size`: C++ `%` truncates toward zero, as Rust's.
        self.first_sample = (s.start + s.buffer_size) % s.buffer_size;
        self.phase_increment = s.phase_increment;
        self.phase = 0;
        self.envelope_phase = F::ZERO;
        self.envelope_phase_increment = F(2.0) / F::from_i32(s.width);
        if s.window_shape >= 0.5 {
            self.envelope_smoothness = (s.window_shape - 0.5) * 2.0;
            self.envelope_slope = F::ZERO;
        } else {
            self.envelope_smoothness = F::ZERO;
            self.envelope_slope = F(0.5) / (s.window_shape + 0.01);
        }
        self.active = true;
        self.gain_l = s.gain_l;
        self.gain_r = s.gain_r;
        self.recommended_quality = s.quality;
    }

    /// `RenderEnvelope<use_lut_for_envelope, quality>(destination, size)`: fills `destination` with
    /// the window, ending with -1 where the grain ends (the entries after it are left as they
    /// were, and never read).
    fn render_envelope(&mut self, destination: &mut [F], use_lut: bool, quality: GrainQuality) {
        let increment = self.envelope_phase_increment;
        let smoothness = self.envelope_smoothness;
        let slope = self.envelope_slope;
        let mut phase = self.envelope_phase;
        for d in destination.iter_mut() {
            let mut gain = phase;
            gain = if gain >= 1.0 { 2.0 - gain } else { gain };
            if use_lut {
                if quality == GrainQuality::High {
                    let window = interpolate(&LUT_WINDOW, gain, 4096.0);
                    gain += smoothness * (window - gain);
                }
            } else if quality >= GrainQuality::Medium {
                gain *= slope;
                if gain >= 1.0 {
                    gain = F(1.0);
                }
            }
            phase += increment;
            if phase >= 2.0 {
                *d = F(-1.0);
                break;
            }
            *d = gain;
        }
        self.envelope_phase = phase;
    }

    /// `OverlapAdd<num_channels, quality>(buffer, destination, envelope, size)`: adds the grain into
    /// `destination` (interleaved stereo), with `envelope` as scratch. `buffers[1]` is read only in
    /// stereo.
    pub(crate) fn overlap_add(&mut self, buffers: &[AudioBuffer; 2], stereo: bool, destination: &mut [[F; 2]], envelope: &mut [F]) {
        if !self.active {
            return;
        }
        // Rendering is done on whole blocks; the pre-delay starts a grain at any sample of one.
        let mut skip = 0;
        while self.pre_delay > 0 && skip < destination.len() {
            skip += 1;
            self.pre_delay -= 1;
        }
        let destination = &mut destination[skip..];
        let size = destination.len();
        let quality = self.recommended_quality;
        let use_lut = self.envelope_smoothness != 0.0;
        self.render_envelope(&mut envelope[..size], use_lut, quality);

        let interpolation = quality.interpolation();
        let (first_sample, phase_increment) = (self.first_sample, self.phase_increment);
        let (gain_l, gain_r) = (self.gain_l, self.gain_r);
        let mut phase = self.phase;
        for (d, &gain) in destination.iter_mut().zip(envelope.iter()) {
            let sample_index = first_sample + (phase >> 16);
            if gain == -1.0 {
                self.active = false;
                break;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let fractional = (phase & 65535) as u16;
            let l = buffers[0].read(interpolation, sample_index, fractional) * gain;
            if stereo {
                let r = buffers[1].read(interpolation, sample_index, fractional) * gain;
                d[0] += l * gain_l + r * (1.0 - gain_r);
                d[1] += r * gain_r + l * (1.0 - gain_l);
            } else {
                d[0] += l * gain_l;
                d[1] += l * gain_r;
            }
            phase = phase.wrapping_add(phase_increment);
        }
        self.phase = phase;
    }
}
