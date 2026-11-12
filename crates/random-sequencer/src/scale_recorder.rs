//! Learns a scale from notes: `marbles/scale_recorder.h`. Each accepted note's pitch class (its
//! voltage within an octave) joins the nearest recorded degree within a third of a semitone, or
//! starts a new one; the scale's weights come from how often each degree came.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::quantizer::{MAX_DEGREES, Scale};
use crate::stmlib::one_pole;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Degree {
    average_voltage: F,
    total_voltage: F,
    count: F,
}

/// `ScaleRecorder`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScaleRecorder {
    num_degrees: usize,
    current_voltage: F,
    total_count: F,
    degrees: [Degree; MAX_DEGREES],
}

impl ScaleRecorder {
    /// `Init()` / `Clear()`.
    pub fn clear(&mut self) {
        self.num_degrees = 0;
        self.current_voltage = F::ZERO;
        self.total_count = F::ZERO;
    }

    /// `NewNote(v)`.
    pub fn new_note(&mut self, v: F) {
        self.current_voltage = v;
    }

    /// `UpdateVoltage(v)`.
    pub fn update_voltage(&mut self, v: F) {
        one_pole(&mut self.current_voltage, v, F(0.01));
    }

    /// `AcceptNote()`.
    pub fn accept_note(&mut self) {
        let base_interval = 1.0_f32;
        let mut v = self.current_voltage;
        while v < 0.0 {
            v += base_interval;
        }
        let octave = F::from_i32((v / base_interval).trunc_i32()) * base_interval;
        v -= octave;
        let mut nearest = None;
        for (i, degree) in self.degrees[..self.num_degrees].iter().enumerate() {
            let av = degree.average_voltage;
            let tolerance = 1.0_f32 / 36.0;
            if (v - av).abs() < tolerance {
                nearest = Some(i);
                break;
            }
            if ((v - base_interval) - av).abs() < tolerance {
                v -= base_interval;
                nearest = Some(i);
                break;
            }
        }
        if nearest.is_none() && self.num_degrees != MAX_DEGREES {
            let i = self.num_degrees;
            self.degrees[i] = Degree::default();
            self.num_degrees += 1;
            nearest = Some(i);
        }
        if let Some(i) = nearest {
            let d = &mut self.degrees[i];
            d.total_voltage += v;
            d.count += 1.0;
            d.average_voltage = d.total_voltage / d.count;
            self.total_count += 1.0;
        }
    }

    /// `ExtractScale(scale)`: the degrees sorted by voltage, weighted 1 to 255 by count; `None`
    /// below two degrees.
    pub fn extract_scale(&mut self) -> Option<Scale> {
        if self.num_degrees < 2 {
            return None;
        }
        let n = self.num_degrees;
        self.degrees[..n].sort_by(|a, b| a.average_voltage.0.total_cmp(&b.average_voltage.0));
        let mut max_count = F::ZERO;
        for d in &self.degrees[..n] {
            max_count = d.count.max(max_count);
        }
        let mut scale = Scale { base_interval: F::ONE, num_degrees: n as i32, ..Scale::default() };
        for (slot, d) in scale.degrees.iter_mut().zip(&self.degrees[..n]) {
            let mut weight = (255.0 * d.count / max_count).trunc_i32().clamp(0, 255) as u8;
            if weight == 0 {
                weight += 1;
            }
            *slot = (d.average_voltage, weight);
        }
        Some(scale)
    }
}
