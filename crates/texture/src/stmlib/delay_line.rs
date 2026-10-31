//! `stmlib/dsp/delay_line.h` for floats, the parts the tape wow and flutter uses.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::integral_fractional;

/// `stmlib::DelayLine<float, N>`: a circular line written backwards, read with linear
/// interpolation.
#[derive(Clone, Copy, Debug)]
pub struct DelayLine<const N: usize> {
    write_ptr: usize,
    line: [F; N],
}

impl<const N: usize> Default for DelayLine<N> {
    fn default() -> Self {
        Self { write_ptr: 0, line: [F::ZERO; N] }
    }
}

impl<const N: usize> DelayLine<N> {
    /// `Init` / `Reset`.
    pub fn init(&mut self) {
        self.line = [F::ZERO; N];
        self.write_ptr = 0;
    }

    /// `Write(sample)`.
    #[inline(always)]
    pub fn write(&mut self, sample: F) {
        self.line[self.write_ptr] = sample;
        self.write_ptr = (self.write_ptr + N - 1) % N;
    }

    /// `Read(float delay)`: linear interpolation `delay` samples back. The delay must be
    /// non-negative and below `N - 1`, as the firmware's callers keep it.
    #[inline(always)]
    #[must_use]
    pub fn read(&self, delay: F) -> F {
        let (integral, fractional) = integral_fractional(delay);
        let integral = usize::try_from(integral).unwrap_or(0);
        let a = self.line[(self.write_ptr + integral) % N];
        let b = self.line[(self.write_ptr + integral + 1) % N];
        a + (b - a) * fractional
    }

    /// `WriteRead(sample, delay)`.
    #[inline(always)]
    pub fn write_read(&mut self, sample: F, delay: F) -> F {
        self.write(sample);
        self.read(delay)
    }
}
