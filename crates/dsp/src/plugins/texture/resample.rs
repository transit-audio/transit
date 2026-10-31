//! The Texture's rate edges (#241): streaming sample-rate converters between the session rate and a
//! quality's codec rate, from the windowed-sinc family of the offline resampler
//! ([`crate::offline::resample`]: a Kaiser window with beta 9 over 32 zero crossings each side).
//!
//! **Timing is exact and rational.** With `g = gcd(from, to)`, one input sample lasts `to / g` ticks
//! and one output sample `from / g` ticks. Output sample `k` is the input signal at tick
//! `k · from/g − delay`, worked out from integer counters, so its value depends only on the input
//! samples and `k`, never on how the host split the stream into blocks.
//!
//! **Passband.** The kernel cuts off at 0.94 of the lower Nyquist ([`PASSBAND`]); its transition is
//! about 0.09 of the lower rate wide, centered there, so it is flat to about 0.89 of the lower
//! Nyquist and rejects from about 0.985 of it (Kaiser beta 9: about 90 dB down). Equal rates take an
//! exact copy, no filter.
//!
//! **Latency.** A converter delays by its half length in input samples (the filter is linear phase
//! and causal), plus any `extra` ticks its owner asks for (the device lines its total latency up on
//! a whole number of session samples).

use std::f64::consts::PI;

use crate::offline::{SINC_BETA, SINC_ZC, bessel_i0};

/// Kernel table steps per zero crossing (linear interpolation between them).
const STEPS: usize = 512;
/// The cutoff, as a fraction of the lower of the two Nyquist frequencies.
pub(crate) const PASSBAND: f64 = 0.94;
/// The largest phase table a converter precomputes, in taps; a rate pair needing more (an unusual
/// session rate) reads the kernel table per output sample instead, to the same values.
const MAX_ROWS: usize = 1 << 17;
/// Frames a firmware block can push ahead of what the output reads (the largest codec block).
const AHEAD: usize = transit_texture::MAX_BLOCK_SIZE;

/// The Kaiser-windowed sinc `K(u) = sinc(πu) · w(u / 32)` for `u` in zero crossings, tabulated.
pub(crate) struct Kernel {
    table: Vec<f32>,
}

impl Kernel {
    pub(crate) fn new() -> Self {
        let n = SINC_ZC as usize * STEPS + 2;
        let i0b = bessel_i0(SINC_BETA);
        let table = (0..n)
            .map(|i| {
                let u = i as f64 / STEPS as f64;
                let x = u / SINC_ZC;
                if x >= 1.0 {
                    return 0.0;
                }
                let a = PI * u;
                let sinc = if i == 0 { 1.0 } else { a.sin() / a };
                (sinc * bessel_i0(SINC_BETA * (1.0 - x * x).sqrt()) / i0b) as f32
            })
            .collect();
        Self { table }
    }

    /// `K(|u|)`, linearly interpolated; 0 past the last crossing.
    fn at(&self, u: f64) -> f32 {
        let pos = u.abs() * STEPS as f64;
        let i = pos as usize;
        let f = (pos - i as f64) as f32;
        match (self.table.get(i), self.table.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            _ => 0.0,
        }
    }

    /// The tap weights for one output phase: `fc · K(fc · ((j − i) + r / lp))` for `i` in
    /// `0..=2j`, where tap `i` multiplies input sample `base − 2j + i`.
    fn row(&self, fc: f64, j: i64, r: i64, lp: i64, out: &mut [f32]) {
        let frac = r as f64 / lp as f64;
        for (i, w) in out.iter_mut().enumerate() {
            let t = (j - i as i64) as f64 + frac;
            *w = (fc * f64::from(self.at(fc * t))) as f32;
        }
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// The cutoff relative to the input's Nyquist and the half length in input samples, for a
/// conversion `from` Hz to `to` Hz.
fn design(from: u32, to: u32) -> (f64, i64) {
    let fc = if to < from { PASSBAND * f64::from(to) / f64::from(from) } else { PASSBAND };
    (fc, (SINC_ZC / fc).ceil() as i64)
}

/// The half length, input samples, of a conversion `from` Hz to `to` Hz (0 for equal rates): the
/// converter's own delay.
pub(crate) fn half_taps(from: u32, to: u32) -> i64 {
    if from == to { 0 } else { design(from, to).1 }
}

/// Ticks per input sample and per output sample of a conversion `from` Hz to `to` Hz.
pub(crate) fn ticks(from: u32, to: u32) -> (i64, i64) {
    let g = gcd(u64::from(from), u64::from(to));
    (i64::try_from(u64::from(to) / g).unwrap_or(1), i64::try_from(u64::from(from) / g).unwrap_or(1))
}

/// One stereo converter. Allocates at construction only.
pub(crate) struct Converter {
    identity: bool,
    /// Ticks per input sample and per output sample.
    lp: i64,
    mp: i64,
    /// Half length: taps reach `j` input samples either side of the output's time.
    j: i64,
    fc: f64,
    /// `lp` rows of `2j + 1` weights, or empty when computed per output into `scratch`.
    rows: Vec<f32>,
    scratch: Vec<f32>,
    hist: [Vec<f32>; 2],
    mask: usize,
    /// Input samples pushed.
    n_in: i64,
    /// The next output's newest input sample and its phase (ticks past it).
    base: i64,
    rem: i64,
    start: (i64, i64),
}

impl Converter {
    /// A converter `from` Hz to `to` Hz delayed by its half length plus `extra` ticks.
    pub(crate) fn new(kernel: &Kernel, from: u32, to: u32, extra: i64) -> Self {
        let identity = from == to;
        let (lp, mp) = if identity { (1, 1) } else { ticks(from, to) };
        let (fc, j) = if identity { (1.0, 0) } else { design(from, to) };
        let taps = usize::try_from(2 * j + 1).unwrap_or(1);
        let lp_rows = usize::try_from(lp).unwrap_or(usize::MAX);
        let rows = if !identity && lp_rows.saturating_mul(taps) <= MAX_ROWS {
            let mut rows = vec![0.0; lp_rows * taps];
            for (r, row) in rows.chunks_mut(taps).enumerate() {
                kernel.row(fc, j, r as i64, lp, row);
            }
            rows
        } else {
            Vec::new()
        };
        let len = (2 * taps + 4 * AHEAD + 64).next_power_of_two();
        // Output 0 is the input at tick `−extra` (and the half length on top).
        let t0 = -extra.max(0);
        let base = t0.div_euclid(lp);
        let start = (base, t0 - base * lp);
        Self {
            identity,
            lp,
            mp,
            j,
            fc,
            rows,
            scratch: vec![0.0; taps],
            hist: [vec![0.0; len], vec![0.0; len]],
            mask: len - 1,
            n_in: 0,
            base: start.0,
            rem: start.1,
            start,
        }
    }

    /// Back to its state after construction: no input, the history silent.
    pub(crate) fn reset(&mut self) {
        self.hist.iter_mut().for_each(|h| h.fill(0.0));
        self.n_in = 0;
        (self.base, self.rem) = self.start;
    }

    /// Input samples of delay: the half length.
    #[cfg(test)]
    pub(crate) fn half_taps(&self) -> i64 {
        self.j
    }

    pub(crate) fn push(&mut self, x: [f32; 2]) {
        let at = (self.n_in as usize) & self.mask;
        for (h, v) in self.hist.iter_mut().zip(x) {
            if let Some(s) = h.get_mut(at) {
                *s = v;
            }
        }
        self.n_in += 1;
    }

    /// True when the next output's inputs have all arrived.
    pub(crate) fn ready(&self) -> bool {
        self.n_in > self.base
    }

    /// The next output sample (taking it advances the stream). Read before [`Converter::ready`],
    /// it uses whatever the history holds; the device never does that (its tests count it).
    pub(crate) fn pull(&mut self, kernel: &Kernel) -> [f32; 2] {
        let y = if self.identity {
            let at = (self.base as usize) & self.mask;
            if self.base < 0 { [0.0; 2] } else { [0, 1].map(|c| self.hist[c].get(at).copied().unwrap_or(0.0)) }
        } else {
            let taps = self.scratch.len();
            let r = usize::try_from(self.rem).unwrap_or(0);
            let row: &[f32] = if self.rows.is_empty() {
                kernel.row(self.fc, self.j, self.rem, self.lp, &mut self.scratch);
                &self.scratch
            } else {
                self.rows.get(r * taps..(r + 1) * taps).unwrap_or(&[])
            };
            // Input sample `base − 2j + i` meets tap `i`; a sample before the first is silence (the
            // history starts zeroed and is longer than the span read here).
            let first = self.base - 2 * self.j;
            let mut acc = [0.0f32; 2];
            for (i, w) in row.iter().enumerate() {
                let at = ((first + i as i64) as usize) & self.mask;
                for (a, h) in acc.iter_mut().zip(&self.hist) {
                    *a += h.get(at).copied().unwrap_or(0.0) * w;
                }
            }
            acc
        };
        self.rem += self.mp;
        self.base += self.rem / self.lp;
        self.rem %= self.lp;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(c: &mut Converter, k: &Kernel, x: &[f32], outs: usize) -> Vec<f32> {
        let mut y = Vec::new();
        let mut i = 0;
        while y.len() < outs {
            while !c.ready() {
                c.push([x.get(i).copied().unwrap_or(0.0); 2]);
                i += 1;
            }
            y.push(c.pull(k)[0]);
        }
        y
    }

    #[test]
    fn equal_rates_copy_bit_for_bit_after_the_extra_delay() {
        let k = Kernel::new();
        let mut c = Converter::new(&k, 48_000, 48_000, 11);
        let x: Vec<f32> = (0..200).map(|i| if i % 7 == 0 { -0.0 } else { (i as f32 * 0.37).sin() }).collect();
        let y = run(&mut c, &k, &x, 150);
        assert!(y[..11].iter().all(|v| v.to_bits() == 0));
        assert!(y[11..].iter().zip(&x).all(|(a, b)| a.to_bits() == b.to_bits()));
    }

    #[test]
    fn a_tone_keeps_its_frequency_and_level_through_each_conversion() {
        let k = Kernel::new();
        for (from, to) in [(44_100, 24_000), (24_000, 44_100), (48_000, 32_000), (32_000, 96_000), (96_000, 24_000), (44_056, 48_000)] {
            let f = 1000.0f64;
            let x: Vec<f32> = (0..20_000).map(|i| (2.0 * PI * f * i as f64 / f64::from(from)).sin() as f32).collect();
            let mut c = Converter::new(&k, from, to, 0);
            let outs = (12_000.0 * f64::from(to) / f64::from(from)) as usize;
            let y = run(&mut c, &k, &x, outs);
            // Once the filter has settled, the output is the tone at the output rate, delayed by
            // the half length (input samples).
            let d = c.half_taps() as f64 / f64::from(from);
            let worst = (outs / 4..outs).map(|n| (f64::from(y[n]) - (2.0 * PI * f * (n as f64 / f64::from(to) - d)).sin()).abs()).fold(0.0, f64::max);
            assert!(worst < 2e-3, "{from} -> {to}: off by {worst}");
        }
    }

    #[test]
    fn the_phase_table_and_the_per_sample_kernel_agree() {
        let k = Kernel::new();
        let mut cached = Converter::new(&k, 44_100, 32_000, 5);
        let mut direct = Converter::new(&k, 44_100, 32_000, 5);
        direct.rows.clear();
        let x: Vec<f32> = (0..4000).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        let a = run(&mut cached, &k, &x, 2500);
        let b = run(&mut direct, &k, &x, 2500);
        assert!(a.iter().zip(&b).all(|(p, q)| p.to_bits() == q.to_bits()));
    }
}
