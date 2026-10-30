//! `stmlib/dsp/delay_line.h`: a delay line of `N` samples (a power of two in every use here),
//! written backwards so that a read at `delay` looks `delay` samples into the past.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::stmlib::integral_fractional;

/// `stmlib::DelayLine<float, N>`. The buffer is allocated once, at construction.
#[derive(Clone, Debug)]
pub struct DelayLine<const N: usize> {
    line: Box<[F]>,
    write_ptr: usize,
    delay: usize,
}

impl<const N: usize> Default for DelayLine<N> {
    fn default() -> Self {
        Self { line: vec![F::ZERO; N].into_boxed_slice(), write_ptr: 0, delay: 0 }
    }
}

impl<const N: usize> DelayLine<N> {
    /// `(write_ptr + offset) % N` for an offset the C++ computes as a signed sum converted to
    /// `size_t`: for a power-of-two `N` the conversion's wraparound is the Euclidean remainder.
    #[inline(always)]
    fn at(&self, offset: i64) -> usize {
        #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let i = (self.write_ptr as i64 + offset).rem_euclid(N as i64) as usize;
        i
    }

    /// `Init` / `Reset`: cleared, delay 1, write pointer 0.
    pub fn init(&mut self) {
        self.line.fill(F::ZERO);
        self.delay = 1;
        self.write_ptr = 0;
    }

    /// `set_delay(delay)` (for [`read_fixed`](Self::read_fixed)).
    pub fn set_delay(&mut self, delay: usize) {
        self.delay = delay;
    }

    /// `Write(sample)`.
    #[inline(always)]
    pub fn write(&mut self, sample: F) {
        self.line[self.write_ptr] = sample;
        self.write_ptr = (self.write_ptr + N - 1) % N;
    }

    /// `Allpass(sample, delay, coefficient)`; the C++ takes `delay` as `size_t`, so a float delay is
    /// truncated by the caller.
    #[inline(always)]
    pub fn allpass(&mut self, sample: F, delay: usize, coefficient: F) -> F {
        let read = self.line[(self.write_ptr + delay) % N];
        let write = sample + coefficient * read;
        self.write(write);
        -write * coefficient + read
    }

    /// `Read()`: at the delay set with [`set_delay`](Self::set_delay).
    #[inline(always)]
    #[must_use]
    pub fn read_fixed(&self) -> F {
        self.line[(self.write_ptr + self.delay) % N]
    }

    /// `Read(float delay)`: linear interpolation.
    #[inline(always)]
    #[must_use]
    pub fn read(&self, delay: F) -> F {
        let (integral, fractional) = integral_fractional(delay);
        let a = self.line[self.at(i64::from(integral))];
        let b = self.line[self.at(i64::from(integral) + 1)];
        a + (b - a) * fractional
    }

    /// `ReadHermite(float delay)`: four-point Hermite interpolation.
    #[inline(always)]
    #[must_use]
    pub fn read_hermite(&self, delay: F) -> F {
        let (integral, fractional) = integral_fractional(delay);
        let t = i64::from(integral);
        let xm1 = self.line[self.at(t - 1)];
        let x0 = self.line[self.at(t)];
        let x1 = self.line[self.at(t + 1)];
        let x2 = self.line[self.at(t + 2)];
        let c = (x1 - xm1) * 0.5;
        let v = x0 - x1;
        let w = c + v;
        let a = w + v + (x2 - x0) * 0.5;
        let b_neg = w + a;
        let f = fractional;
        (((a * f) - b_neg) * f + c) * f + x0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_at_delay_d_returns_the_sample_written_d_writes_ago() {
        let mut d = DelayLine::<16>::default();
        d.init();
        for i in 0..10_u8 {
            d.write(F(f32::from(i)));
        }
        // The last write was 9: delay 1 reads it, delay 2 the one before.
        assert_eq!(d.read(F(1.0)), F(9.0));
        assert_eq!(d.read(F(3.0)), F(7.0));
        assert_eq!(d.read(F(2.5)), F(7.5));
        assert_eq!(d.read_hermite(F(4.0)), F(6.0));
        d.set_delay(2);
        assert_eq!(d.read_fixed(), F(8.0));
    }
}
