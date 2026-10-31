//! `stmlib/dsp/sample_rate_converter.h` at a ratio of 2 with the texture processor's 48-tap
//! filters (`beads/dsp/sample_rate_conversion_filters.h`, `SRC_FIR<..., 2, 48>`): the clean-tape
//! quality's converters around its half-rate granular processor.

#![forbid(unsafe_code)]

use crate::float::F;

/// The down-converter's half impulse response (`SRC_FIR<SRC_DOWN, 2, 48>`), mirrored about its
/// middle.
#[allow(clippy::excessive_precision)] // The firmware's literals, verbatim.
const DOWN: [f32; 24] = [
    -2.143728787e-04,
    -4.113243578e-04,
    3.550026182e-04,
    1.123588805e-03,
    2.013790146e-04,
    -2.135889966e-03,
    -1.946536068e-03,
    2.421957824e-03,
    5.099674271e-03,
    -4.847193956e-04,
    -8.638296788e-03,
    -5.137744633e-03,
    1.000597441e-02,
    1.464501735e-02,
    -5.419339385e-03,
    -2.582247866e-02,
    -9.106955536e-03,
    3.335521514e-02,
    3.702172341e-02,
    -2.802414194e-02,
    -8.416883646e-02,
    -1.192656678e-02,
    1.955030744e-01,
    3.837045956e-01,
];

/// The up-converter's half impulse response (`SRC_FIR<SRC_UP, 2, 48>`).
#[allow(clippy::excessive_precision)] // The firmware's literals, verbatim.
const UP: [f32; 24] = [
    -4.287457573e-04,
    -8.226487157e-04,
    7.100052364e-04,
    2.247177610e-03,
    4.027580292e-04,
    -4.271779932e-03,
    -3.893072135e-03,
    4.843915649e-03,
    1.019934854e-02,
    -9.694387912e-04,
    -1.727659358e-02,
    -1.027548927e-02,
    2.001194882e-02,
    2.929003471e-02,
    -1.083867877e-02,
    -5.164495731e-02,
    -1.821391107e-02,
    6.671043028e-02,
    7.404344683e-02,
    -5.604828388e-02,
    -1.683376729e-01,
    -2.385313356e-02,
    3.910061488e-01,
    7.674091912e-01,
];

/// Coefficient `i` of the 48-tap response stored as its first half (`Accumulator`'s `mirror`).
#[inline(always)]
fn tap(half: &[f32; 24], i: usize) -> F {
    F(if i >= 24 { half[47 - i] } else { half[i] })
}

/// `SampleRateConverter<SRC_DOWN, 2, 48>`: two input samples per output sample, through the
/// circular-history variant (blocks shorter than 384 samples, which every block here is).
#[derive(Clone, Copy, Debug)]
pub struct Downsampler {
    x: [F; 96],
    ptr: usize,
}

impl Default for Downsampler {
    fn default() -> Self {
        Self { x: [F::ZERO; 96], ptr: 47 }
    }
}

impl Downsampler {
    /// `Init`.
    pub fn init(&mut self) {
        *self = Self::default();
    }

    /// `Process(in, out, in.len())`: `out` takes `in.len() / 2` samples; `in.len()` must be even
    /// (the firmware returns without writing otherwise; the processor never passes an odd block).
    pub fn process(&mut self, input: &[F], out: &mut [F]) {
        for (pair, y) in input.as_chunks::<2>().0.iter().zip(out.iter_mut()) {
            for &s in pair {
                self.x[self.ptr] = s;
                self.x[self.ptr + 48] = s;
                if self.ptr == 0 {
                    self.ptr = 47;
                } else {
                    self.ptr -= 1;
                }
            }
            // `Accumulator<48, 1, 1, 48>` over `&x_ptr_[1]`: a right fold ending in `+ 0.0f`.
            let base = self.ptr + 1;
            let mut acc = F::ZERO;
            for i in (0..48).rev() {
                acc = self.x[base + i] * tap(&DOWN, i) + acc;
            }
            *y = acc;
        }
    }
}

/// `SampleRateConverter<SRC_UP, 2, 48>`: two output samples per input sample from a 24-sample
/// history, the newest first.
#[derive(Clone, Copy, Debug, Default)]
pub struct Upsampler {
    x: [F; 24],
}

impl Upsampler {
    /// `Init`.
    pub fn init(&mut self) {
        *self = Self::default();
    }

    /// `Process(in, out, in.len())`: `out` takes `2 * in.len()` samples.
    pub fn process(&mut self, input: &[F], out: &mut [F]) {
        for (&s, pair) in input.iter().zip(out.as_chunks_mut::<2>().0.iter_mut()) {
            self.x.copy_within(0..23, 1);
            self.x[0] = s;
            // `PolyphaseStage<2, 48>`: phase 0 reads taps 0, 2, 4, ..., phase 1 taps 1, 3, 5, ...
            for (phase, y) in pair.iter_mut().enumerate() {
                let mut acc = F::ZERO;
                for i in (0..24).rev() {
                    acc = self.x[i] * tap(&UP, phase + 2 * i) + acc;
                }
                *y = acc;
            }
        }
    }
}
