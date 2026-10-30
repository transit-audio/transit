//! The 2:1 sample-rate converters of the low-fidelity qualities
//! (`clouds/dsp/sample_rate_converter.h` with `src_filter_1x_2_45`): a 45-tap FIR decimator in
//! front of the grains and the matching interpolator after them.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::SRC_FILTER_1X_2_45;

/// The filter length.
const TAPS: usize = 45;

/// `SampleRateConverter<ratio, 45, src_filter_1x_2_45>` with `ratio` -2 (down) or +2 (up).
#[derive(Clone, Debug)]
pub(crate) struct SampleRateConverter {
    up: bool,
    history: [[F; 2]; TAPS * 2],
    history_ptr: usize,
}

impl SampleRateConverter {
    /// A decimator by 2 (`ratio` -2) or an interpolator by 2 (`ratio` +2), cleared (`Init`).
    pub(crate) fn new(up: bool) -> Self {
        Self { up, history: [[F::ZERO; 2]; TAPS * 2], history_ptr: TAPS - 1 }
    }

    /// `Process(in, out, input_size)`: the decimator consumes two input frames per output frame,
    /// the interpolator produces two output frames per input frame. `output` must hold
    /// `input.len() / 2` (down) or `input.len() * 2` (up) frames.
    pub(crate) fn process(&mut self, input: &[[F; 2]], output: &mut [[F; 2]]) {
        let (consumed, produced) = if self.up { (1, 2) } else { (2, 1) };
        // `scale`: 1 when decimating, the ratio when interpolating.
        let scale = if self.up { 2.0 } else { 1.0 };
        let mut ptr = self.history_ptr;
        let mut outputs = output.iter_mut();
        for chunk in input.chunks_exact(consumed) {
            for &frame in chunk {
                self.history[ptr + TAPS] = frame;
                self.history[ptr] = frame;
                ptr = if ptr == 0 { TAPS - 1 } else { ptr - 1 };
            }
            for i in 0..produced {
                let (mut y_l, mut y_r) = (F::ZERO, F::ZERO);
                let mut x = ptr + 1;
                let mut j = i;
                while j < TAPS {
                    let h = SRC_FILTER_1X_2_45[j];
                    y_l += self.history[x][0] * h;
                    y_r += self.history[x][1] * h;
                    x += 1;
                    j += produced;
                }
                if let Some(out) = outputs.next() {
                    *out = [y_l * scale, y_r * scale];
                }
            }
        }
        self.history_ptr = ptr;
    }
}
