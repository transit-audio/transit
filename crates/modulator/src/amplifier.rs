//! `SaturatingAmplifier` (`warps/dsp/modulator.h`): an input's noise gate, its contribution to the
//! aux output, and its drive into soft saturation.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::parameter_interpolator::ParameterInterpolator;
use crate::stmlib::soft_clip;

/// One input amplifier. Its level follower and gains start at zero, as the module's global
/// object does (`Init` writes only the drive).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Amplifier {
    level: F,
    drive: F,
    post_gain: F,
    pre_gain: F,
}

impl Amplifier {
    /// `Init`.
    pub(crate) fn init(&mut self) {
        self.drive = F::ZERO;
    }

    /// `Process(drive, limit, in, out, out_raw, stride, size)` with the input already in floats
    /// (`in / 32768`): gates `input` into `out`, adds the gated input times the ramped drive to
    /// `raw`, then drives `out` into the saturator, `limit` blending from linear (0) to saturated
    /// (1).
    pub(crate) fn process(&mut self, drive: F, limit: F, input: &[F], out: &mut [F], raw: &mut [F]) {
        let size = out.len().min(input.len()).min(raw.len());
        let mut drive_modulation = ParameterInterpolator::new(self.drive, drive, size);
        let mut level = self.level;
        let gate = 1.0_f32 / 0.0001_f32;
        for ((x, o), r) in input.iter().zip(out.iter_mut()).zip(raw.iter_mut()).take(size) {
            let mut s = *x;
            let error = s * s - level;
            level += error * F(if error > 0.0 { 0.1 } else { 0.0001 });
            s *= if level <= 0.0001 { gate * level } else { F::ONE };
            *o = s;
            *r += s * drive_modulation.next();
        }
        self.level = level;
        self.drive = drive_modulation.value();

        let drive_2 = drive * drive;
        let pre_gain_a = drive * 0.5;
        let pre_gain_b = drive_2 * drive_2 * drive * 24.0;
        let pre_gain = pre_gain_a + (pre_gain_b - pre_gain_a) * drive_2;
        let drive_squished = drive * (2.0 - drive);
        let post_gain = 1.0 / soft_clip(0.33 + drive_squished * (pre_gain - 0.33));
        let mut pre_gain_modulation = ParameterInterpolator::new(self.pre_gain, pre_gain, size);
        let mut post_gain_modulation = ParameterInterpolator::new(self.post_gain, post_gain, size);
        for o in out.iter_mut().take(size) {
            let pre = pre_gain_modulation.next() * *o;
            let post = soft_clip(pre) * post_gain_modulation.next();
            *o = pre + (post - pre) * limit;
        }
        self.pre_gain = pre_gain_modulation.value();
        self.post_gain = post_gain_modulation.value();
    }
}
