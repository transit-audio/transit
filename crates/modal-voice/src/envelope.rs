//! The gate envelope (`elements/dsp/multistage_envelope.{h,cc}`): up to six segments with linear,
//! exponential or quartic shapes read from tables, a sustain point held while the gate is high, run
//! once per block. The voice uses it as an ADSR (`set_adsr`).

#![forbid(unsafe_code)]

use transit_resonator::float::F;

use crate::dsp::index;
use crate::resources::{LUT_ENV_EXPO, LUT_ENV_INCREMENTS, LUT_ENV_LINEAR, LUT_ENV_QUARTIC};

/// `ENVELOPE_FLAG_RISING_EDGE`.
pub(crate) const RISING_EDGE: u8 = 1;
/// `ENVELOPE_FLAG_FALLING_EDGE`.
pub(crate) const FALLING_EDGE: u8 = 2;
/// `ENVELOPE_FLAG_GATE`.
pub(crate) const GATE: u8 = 4;

/// `kMaxNumSegments`.
const MAX_SEGMENTS: usize = 6;

/// `EnvelopeShape`, in the firmware's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Shape {
    /// `ENV_SHAPE_LINEAR` (zero: the shape of a slot `set_adsr` never writes).
    #[default]
    Linear,
    Exponential,
    Quartic,
}

impl Shape {
    fn table(self) -> &'static [f32] {
        match self {
            Self::Linear => &LUT_ENV_LINEAR,
            Self::Exponential => &LUT_ENV_EXPO,
            Self::Quartic => &LUT_ENV_QUARTIC,
        }
    }
}

/// `Interpolate8(table, index)`: linear interpolation at `index * 256`.
#[inline(always)]
fn interpolate8(table: &[f32], index_: F) -> F {
    let scaled = index_ * 256.0;
    let integral = scaled.trunc_usize();
    let fractional = scaled - F::from_usize(integral);
    let i = integral.min(table.len() - 2);
    let a = F(table[i]);
    let b = F(table[i + 1]);
    a + (b - a) * fractional
}

/// `elements::MultistageEnvelope`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Envelope {
    /// The segments' levels. `set_adsr` writes four; the fifth, read once the envelope is done, is
    /// never written and stays zero (the module's static storage).
    level: [F; MAX_SEGMENTS],
    time: [F; MAX_SEGMENTS],
    shape: [Shape; MAX_SEGMENTS],
    segment: i16,
    start_value: F,
    value: F,
    phase: F,
    num_segments: u16,
    sustain_point: u16,
    loop_start: u16,
    loop_end: u16,
    hard_reset: bool,
}

impl Envelope {
    /// `Init()`.
    pub(crate) fn init(&mut self) {
        self.set_adsr(F::ZERO, F(0.25), F(0.25), F(0.5));
        #[allow(clippy::cast_possible_wrap)] // At most six segments.
        let done = self.num_segments as i16;
        self.segment = done;
        self.phase = F::ZERO;
        self.start_value = F::ZERO;
        self.value = F::ZERO;
        self.hard_reset = false;
    }

    /// `set_adsr(attack, decay, sustain, release)`.
    pub(crate) fn set_adsr(&mut self, attack: F, decay: F, sustain: F, release: F) {
        self.num_segments = 3;
        self.sustain_point = 2;
        self.level[0] = F::ZERO;
        self.level[1] = F(1.0);
        self.level[2] = sustain;
        self.level[3] = F::ZERO;
        self.time[0] = attack;
        self.time[1] = decay;
        self.time[2] = release;
        self.shape[0] = Shape::Quartic;
        self.shape[1] = Shape::Exponential;
        self.shape[2] = Shape::Exponential;
        self.loop_start = 0;
        self.loop_end = 0;
    }

    fn slot(&self, offset: i16) -> usize {
        index(i32::from(self.segment) + i32::from(offset), MAX_SEGMENTS)
    }

    /// `Process(flags)`: one step, returning the envelope's value.
    pub(crate) fn process(&mut self, flags: u8) -> F {
        let segment_count = i16::try_from(self.num_segments).unwrap_or(i16::MAX);
        if flags & RISING_EDGE != 0 {
            self.start_value = if self.segment == segment_count || self.hard_reset { self.level[0] } else { self.value };
            self.segment = 0;
            self.phase = F::ZERO;
        } else if flags & FALLING_EDGE != 0 && self.sustain_point != 0 {
            self.start_value = self.value;
            self.segment = i16::try_from(self.sustain_point).unwrap_or(0);
            self.phase = F::ZERO;
        } else if self.phase >= 1.0 {
            self.start_value = self.level[self.slot(1)];
            self.segment += 1;
            self.phase = F::ZERO;
            if self.segment == i16::try_from(self.loop_end).unwrap_or(0) {
                self.segment = i16::try_from(self.loop_start).unwrap_or(0);
            }
        }

        let done = self.segment == segment_count;
        let sustained = self.sustain_point != 0 && i32::from(self.segment) == i32::from(self.sustain_point) && flags & GATE != 0;

        let mut phase_increment = F::ZERO;
        if !sustained && !done {
            phase_increment = interpolate8(&LUT_ENV_INCREMENTS, self.time[self.slot(0)]);
        }
        let t = interpolate8(self.shape[self.slot(0)].table(), self.phase);
        self.phase += phase_increment;
        self.value = self.start_value + (self.level[self.slot(1)] - self.start_value) * t;
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gated_envelope_rises_sustains_and_releases() {
        let mut e = Envelope::default();
        e.init();
        e.set_adsr(F(0.2), F(0.3), F(0.5), F(0.3));
        assert_eq!(e.process(RISING_EDGE | GATE), F::ZERO);
        let mut peak = F::ZERO;
        for _ in 0..2000 {
            peak = peak.max(e.process(GATE));
        }
        assert!(peak.0 > 0.99, "peak {peak:?}");
        let sustain = e.process(GATE);
        assert!((sustain.0 - 0.5).abs() < 1e-3, "sustain {sustain:?}");
        e.process(FALLING_EDGE);
        for _ in 0..4000 {
            e.process(0);
        }
        assert!(e.process(0).0.abs() < 1e-3);
    }
}
