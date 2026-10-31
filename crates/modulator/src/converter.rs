//! `warps/dsp/sample_rate_converter.h`: the polyphase FIR converters around the cross-modulation
//! algorithms (6x) and inside the vocoder's filter bank (3x and 4x).
//!
//! Both directions sum their taps as the firmware's template recursion does: from the oldest tap
//! to the newest, each product added to the running sum (`x[i] * h + rest`, the innermost term
//! `x[N-1] * h + 0`). The kernels are symmetric and stored as their first half
//! ([`crate::resources`]).

#![forbid(unsafe_code)]

use crate::float::F;

/// The kernel tap `j` of a symmetric kernel of `len` taps stored as its first half.
#[inline(always)]
fn tap(half: &[f32], len: usize, j: usize) -> F {
    let k = if j >= len / 2 { len - 1 - j } else { j };
    F(half.get(k).copied().unwrap_or(0.0))
}

/// `SampleRateConverter<SRC_UP, K, K * N>`: one input sample in, `K` out, over the last `N`
/// inputs.
#[derive(Clone, Debug)]
pub(crate) struct Up<const K: usize, const N: usize> {
    /// The newest input first (`FilterState`).
    x: [F; N],
    kernel: &'static [f32],
}

impl<const K: usize, const N: usize> Up<K, N> {
    pub(crate) fn new(kernel: &'static [f32]) -> Self {
        Self { x: [F::ZERO; N], kernel }
    }

    /// `Init`.
    pub(crate) fn init(&mut self) {
        self.x = [F::ZERO; N];
    }

    /// `delay()`: half the taps per phase.
    pub(crate) const fn delay() -> usize {
        N / 2
    }

    /// `Process(in, out, n)`: `out` takes `K` samples per input sample.
    pub(crate) fn process(&mut self, input: &[F], out: &mut [F]) {
        let len = K * N;
        for (x, y) in input.iter().zip(out.as_chunks_mut::<K>().0) {
            self.x.copy_within(0..N - 1, 1);
            self.x[0] = *x;
            for (phase, o) in y.iter_mut().enumerate() {
                let mut acc = F::ZERO;
                for i in (0..N).rev() {
                    acc = self.x[i] * tap(self.kernel, len, phase + i * K) + acc;
                }
                *o = acc;
            }
        }
    }
}

/// `SampleRateConverter<SRC_DOWN, K, N>`: `K` input samples in, one out, over the last `N` inputs,
/// in the firmware's circular-buffer branch (the one it takes for blocks shorter than `8 N`
/// input samples, which every block of the module's is).
#[derive(Clone, Debug)]
pub(crate) struct Down<const K: usize, const N: usize> {
    /// The history; the firmware keeps a mirrored copy `N` further on, so reading `N` taps from
    /// any position is contiguous: here the reads wrap instead.
    x: [F; N],
    /// The next write position (`x_ptr_`).
    p: usize,
    kernel: &'static [f32],
}

impl<const K: usize, const N: usize> Down<K, N> {
    pub(crate) fn new(kernel: &'static [f32]) -> Self {
        Self { x: [F::ZERO; N], p: N - 1, kernel }
    }

    /// `Init`.
    pub(crate) fn init(&mut self) {
        self.x = [F::ZERO; N];
        self.p = N - 1;
    }

    /// `delay()`: half the taps.
    pub(crate) const fn delay() -> usize {
        N / 2
    }

    /// `Process(in, out, n)`: one output per `K` inputs. Like the firmware, an input whose length
    /// is not a multiple of `K` produces nothing.
    pub(crate) fn process(&mut self, input: &[F], out: &mut [F]) {
        if !input.len().is_multiple_of(K) {
            return;
        }
        for (group, y) in input.as_chunks::<K>().0.iter().zip(out.iter_mut()) {
            for x in group {
                self.x[self.p] = *x;
                self.p = if self.p == 0 { N - 1 } else { self.p - 1 };
            }
            let mut acc = F::ZERO;
            for i in (0..N).rev() {
                acc = self.x[(self.p + 1 + i) % N] * tap(self.kernel, N, i) + acc;
            }
            *y = acc;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{SRC_DOWN_6_48, SRC_UP_6_48};

    /// Up then down by 6 passes a low tone through at unit gain (the kernels' passband), delayed
    /// by both converters.
    #[test]
    fn up_then_down_passes_a_low_tone() {
        let mut up = Up::<6, 8>::new(&SRC_UP_6_48);
        let mut down = Down::<6, 48>::new(&SRC_DOWN_6_48);
        let n = 2000;
        let x: Vec<F> = (0..n).map(|i| F((std::f32::consts::TAU * 1000.0 * i as f32 / 96_000.0).sin())).collect();
        let mut over = vec![F::ZERO; n * 6];
        up.process(&x, &mut over);
        let mut y = vec![F::ZERO; n];
        down.process(&over, &mut y);
        let peak = y[n / 2..].iter().map(|v| v.0.abs()).fold(0.0_f32, f32::max);
        assert!((peak - 1.0).abs() < 0.02, "{peak}");
    }
}
