//! The variable-resolution quantizer and its scales: `marbles/random/quantizer.{h,cc}`.
//!
//! A scale lists up to 16 degrees within an interval (1 V, an octave, for the presets), each with a
//! weight. The quantizer keeps seven sets of degrees, by weight thresholds (0, 16, 32, 64, 128,
//! 192, 255): as the steps control grows it snaps to ever fewer, heavier degrees.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::{HysteresisQuantizer, index, integral_fractional};

/// `kMaxDegrees`.
pub const MAX_DEGREES: usize = 16;
const NUM_THRESHOLDS: usize = 7;

/// `Scale`: the interval it repeats at and its degrees (voltage within one interval, weight).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub base_interval: F,
    pub num_degrees: i32,
    pub degrees: [(F, u8); MAX_DEGREES],
}

impl Default for Scale {
    /// `Scale::Init()`: one degree at 0 V, weight 0, repeating every volt.
    fn default() -> Self {
        let mut degrees = [(F::ZERO, 0_u8); MAX_DEGREES];
        degrees[0] = (F::ZERO, 0);
        Self { base_interval: F::ONE, num_degrees: 1, degrees }
    }
}

impl Scale {
    /// A scale from its interval and its degrees (at most 16 are kept).
    #[must_use]
    pub fn new(base_interval: f32, degrees: &[(f32, u8)]) -> Self {
        let mut d = [(F::ZERO, 0_u8); MAX_DEGREES];
        for (slot, (v, w)) in d.iter_mut().zip(degrees) {
            *slot = (F(*v), *w);
        }
        Self { base_interval: F(base_interval), num_degrees: i32::try_from(degrees.len().min(MAX_DEGREES)).unwrap_or(0), degrees: d }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Level {
    /// The active degrees.
    bitmask: u16,
    /// The first and last active degree (0xff: none).
    first: u8,
    last: u8,
}

/// `Quantizer`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Quantizer {
    voltage: [F; MAX_DEGREES],
    level: [Level; NUM_THRESHOLDS],
    feedback: [F; NUM_THRESHOLDS],
    base_interval: F,
    base_interval_reciprocal: F,
    num_degrees: i32,
    level_quantizer: HysteresisQuantizer,
}

impl Quantizer {
    /// `Init(scale)`: a scale with no degree, more than 16 or a zero interval is ignored (the
    /// previous one stays).
    pub fn init(&mut self, scale: &Scale) {
        let n = scale.num_degrees;
        if n == 0 || n > MAX_DEGREES as i32 || scale.base_interval == 0.0 {
            return;
        }
        let n = usize::try_from(n).unwrap_or(0);
        self.num_degrees = n as i32;
        self.base_interval = scale.base_interval;
        self.base_interval_reciprocal = 1.0 / scale.base_interval;
        let mut second_largest_threshold = 0_u8;
        for (i, (v, w)) in scale.degrees.iter().take(n).enumerate() {
            self.voltage[i] = *v;
            if *w != 255 && *w >= second_largest_threshold {
                second_largest_threshold = *w;
            }
        }
        let mut thresholds: [u8; NUM_THRESHOLDS] = [0, 16, 32, 64, 128, 192, 255];
        if second_largest_threshold > 192 {
            // Be more selective: only the degrees of rank 1 and 2 at the last but one level.
            thresholds[NUM_THRESHOLDS - 2] = second_largest_threshold;
        }
        for (t, threshold) in thresholds.iter().enumerate() {
            let mut bitmask = 0_u16;
            let mut first = 0xff_u8;
            let mut last = 0_u8;
            for (i, (_, w)) in scale.degrees.iter().take(n).enumerate() {
                if *w >= *threshold {
                    bitmask |= 1 << i;
                    if first == 0xff {
                        first = i as u8;
                    }
                    last = i as u8;
                }
            }
            self.level[t] = Level { bitmask, first, last };
        }
        self.level_quantizer.init();
        self.feedback = [F::ZERO; NUM_THRESHOLDS];
    }

    /// `Process(value, amount, hysteresis)`: `amount` 0 to 1 picks the level (0: no quantization).
    pub fn process(&mut self, value: F, amount: F, hysteresis: bool) -> F {
        let level = self.level_quantizer.process(amount, NUM_THRESHOLDS as i32 + 1);
        let mut quantized_voltage = value;
        if level > 0 {
            let level = index(i64::from(level - 1), NUM_THRESHOLDS);
            let raw_value = value;
            let mut value = value;
            if hysteresis {
                value += self.feedback[level];
            }
            let note = value * self.base_interval_reciprocal;
            let (mut note_integral, mut note_fractional) = integral_fractional(note);
            if value < 0.0 {
                note_integral -= 1;
                note_fractional += 1.0;
            }
            note_fractional *= self.base_interval;
            // The tightest bounds among the active degrees (some entries are masked, so no binary
            // search).
            let l = self.level[level];
            let at = |i: u8| self.voltage[index(i64::from(i), MAX_DEGREES)];
            let mut a = at(l.last) - self.base_interval;
            let mut b = at(l.first) + self.base_interval;
            let mut bitmask = l.bitmask;
            for i in 0..usize::try_from(self.num_degrees).unwrap_or(0) {
                if bitmask & 1 != 0 {
                    let v = self.voltage[i];
                    if note_fractional > v {
                        a = v;
                    } else {
                        b = v;
                        break;
                    }
                }
                bitmask >>= 1;
            }
            quantized_voltage = if note_fractional < (a + b) * 0.5 { a } else { b };
            quantized_voltage += F::from_i32(note_integral) * self.base_interval;
            self.feedback[level] = (quantized_voltage - raw_value) * 0.25;
        }
        quantized_voltage
    }
}
