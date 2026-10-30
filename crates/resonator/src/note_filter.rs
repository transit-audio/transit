//! The pitch smoother (`rings/dsp/note_filter.h`): follows a sharp change or a strum at once, and
//! otherwise median-filters and lags the V/OCT input; also keeps a delayed stable note for the
//! voice a strum leaves behind.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::delay_line::DelayLine;

/// The median filter's order (`N`).
const ORDER: usize = 4;

/// `rings::NoteFilter`.
#[derive(Clone, Debug, Default)]
pub struct NoteFilter {
    previous_values: [F; ORDER],
    note: F,
    stable_note: F,
    delayed_stable_note: DelayLine<16>,
    coefficient: F,
    stable_coefficient: F,
    fast_coefficient: F,
    slow_coefficient: F,
    lag_coefficient: F,
}

impl NoteFilter {
    /// `Init(sample_rate, time_constant_fast_edge, time_constant_steady_part, edge_recovery_time,
    /// edge_avoidance_delay)`; the rate is the block rate.
    pub fn init(&mut self, sample_rate: F, fast_edge: F, steady_part: F, edge_recovery: F, edge_avoidance_delay: F) {
        self.fast_coefficient = 1.0 / (fast_edge * sample_rate);
        self.slow_coefficient = 1.0 / (steady_part * sample_rate);
        self.lag_coefficient = 1.0 / (edge_recovery * sample_rate);
        self.delayed_stable_note.init();
        self.delayed_stable_note.set_delay(15.min((edge_avoidance_delay * sample_rate).trunc_usize()));
        self.note = F(69.0);
        self.stable_note = F(69.0);
        self.coefficient = self.fast_coefficient;
        self.stable_coefficient = self.slow_coefficient;
        self.previous_values = [self.note; ORDER];
    }

    /// `Process(note, strum)`.
    pub fn process(&mut self, note: F, strum: bool) -> F {
        // If there is a sharp change, follow it instantly.
        if (note - self.note).abs() > 0.4 || strum {
            self.stable_note = note;
            self.note = note;
            self.coefficient = self.fast_coefficient;
            self.stable_coefficient = self.slow_coefficient;
            self.previous_values = [note; ORDER];
        } else {
            // Median filtering of the raw value.
            self.previous_values.rotate_left(1);
            self.previous_values[ORDER - 1] = note;
            let mut sorted = self.previous_values;
            insertion_sort(&mut sorted);
            let median = 0.5 * (sorted[(ORDER - 1) / 2] + sorted[ORDER / 2]);

            // Adaptive lag processor.
            self.note += self.coefficient * (median - self.note);
            self.stable_note += self.stable_coefficient * (self.note - self.stable_note);
            self.coefficient += self.lag_coefficient * (self.slow_coefficient - self.coefficient);
            self.stable_coefficient += self.lag_coefficient * (self.lag_coefficient - self.stable_coefficient);
            self.delayed_stable_note.write(self.stable_note);
        }
        self.note
    }

    /// `note()`.
    #[must_use]
    pub fn note(&self) -> F {
        self.note
    }

    /// `stable_note()`: the stable note a few blocks ago.
    #[must_use]
    pub fn stable_note(&self) -> F {
        self.delayed_stable_note.read_fixed()
    }
}

/// `std::sort` on four floats: the standard libraries sort so few elements by insertion with `<`,
/// which keeps equal elements (a `+0.0` and a `-0.0`) in their order, as this does.
fn insertion_sort(values: &mut [F; ORDER]) {
    for i in 1..ORDER {
        let mut j = i;
        while j > 0 && values[j] < values[j - 1] {
            values.swap(j, j - 1);
            j -= 1;
        }
    }
}
