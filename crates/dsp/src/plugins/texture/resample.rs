//! The Texture's rate edges (#241): streaming sample-rate converters between the session rate and a
//! quality's codec rate, from the windowed-sinc family of the offline resampler
//! ([`crate::offline::resample`]: a Kaiser window with beta 9 over 32 zero crossings each side).
//!
//! **Timing is exact and rational.** With `g = gcd(from, to)`, one input sample lasts `to / g` ticks
//! and one output sample `from / g` ticks. Output sample `k` is the input signal at tick
//! `k · from/g − delay`, worked out from integer counters, so its value depends only on the input
//! samples and `k`, never on how the host split the stream into blocks.
//!
//! **Passband.** The kernel is a sinc cut off at 0.91 of the lower Nyquist ([`PASSBAND`]) under a
//! Kaiser window (beta 9) whose half length is held at the offline family's 32 zero crossings of a
//! 0.94 cutoff ([`LENGTH_CUTOFF`]), so the converters' lengths, and the latency table, are those of
//! the first design while the stopband now starts at the lower Nyquist. Measured per converter
//! (96 to 48 kHz and 44.1 to 24 kHz, the tests below): flat within 0.1 dB to 0.85 of the lower
//! Nyquist, -3 dB at about 0.896, at least 90 dB down from the lower Nyquist on; a tone just above
//! the lower Nyquist (1.004 of it) folds back more than 90 dB down. Content between 0.85 and 1.0 of
//! the codec's Nyquist is rolled off (the top 1.8 kHz at 24 kHz). Equal rates take an exact copy,
//! no filter.
//!
//! **Subnormals.** A resampled output is flushed to zero under 1e-25 ([`crate::util::flush`], a
//! threshold compare): a converter's tail decaying into silence would otherwise hand subnormal
//! samples downstream. The equal-rate copy is never flushed, so the firmware-exact path stays a
//! copy bit for bit.
//!
//! **Latency.** A converter delays by its half length in input samples (the filter is linear phase
//! and causal), plus any `extra` ticks its owner asks for (the device lines its total latency up on
//! a whole number of session samples).

use std::f64::consts::PI;

use crate::offline::{SINC_BETA, SINC_ZC, bessel_i0};
use crate::util::flush;

/// Table steps per zero crossing of the sinc, and per 1/32 of the window (linear interpolation
/// between them).
const STEPS: usize = 512;
/// The cutoff, as a fraction of the lower of the two Nyquist frequencies.
pub(crate) const PASSBAND: f64 = 0.91;
/// The cutoff the window's length is set from: the window spans 32 zero crossings of a sinc cut
/// off here (the first design's cutoff, which fixed the latency table).
const LENGTH_CUTOFF: f64 = 0.94;
/// The largest phase table a converter precomputes, in taps; a rate pair needing more (an unusual
/// session rate) reads the kernel table per output sample instead, to the same values.
const MAX_ROWS: usize = 1 << 17;
/// Frames a firmware block can push ahead of what the output reads (the largest codec block).
const AHEAD: usize = transit_texture::MAX_BLOCK_SIZE;

/// The sinc `sinc(πu)` for `u` in zero crossings and the Kaiser window `w(x)` for `x` in `[0, 1]`,
/// tabulated.
pub(crate) struct Kernel {
    sinc: Vec<f32>,
    window: Vec<f32>,
}

impl Kernel {
    pub(crate) fn new() -> Self {
        let n = SINC_ZC as usize * STEPS + 2;
        let sinc = (0..n)
            .map(|i| {
                let a = PI * i as f64 / STEPS as f64;
                (if i == 0 { 1.0 } else { a.sin() / a }) as f32
            })
            .collect();
        let i0b = bessel_i0(SINC_BETA);
        let last = n - 2;
        let window = (0..n)
            .map(|i| {
                let x = i as f64 / last as f64;
                if x >= 1.0 { 0.0 } else { (bessel_i0(SINC_BETA * (1.0 - x * x).sqrt()) / i0b) as f32 }
            })
            .collect();
        Self { sinc, window }
    }

    /// `table` at position `|pos|` (in table steps), linearly interpolated; 0 past its end.
    fn lookup(table: &[f32], pos: f64) -> f32 {
        let pos = pos.abs();
        let i = pos as usize;
        let f = (pos - i as f64) as f32;
        match (table.get(i), table.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            _ => 0.0,
        }
    }

    /// The tap weights for one output phase: `fc · sinc(π·fc·t) · w(t / span)` with
    /// `t = (j − i) + r / lp` for `i` in `0..=2j`, where tap `i` multiplies input sample
    /// `base − 2j + i` and the window reaches `span` input samples either side.
    fn row(&self, fc: f64, span: f64, j: i64, r: i64, lp: i64, out: &mut [f32]) {
        let frac = r as f64 / lp as f64;
        let wscale = (self.window.len() - 2) as f64 / span;
        for (i, w) in out.iter_mut().enumerate() {
            let t = (j - i as i64) as f64 + frac;
            let win = if t.abs() >= span { 0.0 } else { Self::lookup(&self.window, t * wscale) };
            let sinc = Self::lookup(&self.sinc, fc * t * STEPS as f64);
            *w = (fc * f64::from(sinc) * f64::from(win)) as f32;
        }
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// The cutoff relative to the input's Nyquist, the window's half length and the converter's
/// half length (both in input samples), for a conversion `from` Hz to `to` Hz.
fn design(from: u32, to: u32) -> (f64, f64, i64) {
    let ratio = if to < from { f64::from(to) / f64::from(from) } else { 1.0 };
    let span = SINC_ZC / (LENGTH_CUTOFF * ratio);
    (PASSBAND * ratio, span, span.ceil() as i64)
}

/// The half length, input samples, of a conversion `from` Hz to `to` Hz (0 for equal rates): the
/// converter's own delay.
pub(crate) fn half_taps(from: u32, to: u32) -> i64 {
    if from == to { 0 } else { design(from, to).2 }
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
    span: f64,
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
        let (fc, span, j) = if identity { (1.0, 0.0, 0) } else { design(from, to) };
        let taps = usize::try_from(2 * j + 1).unwrap_or(1);
        let lp_rows = usize::try_from(lp).unwrap_or(usize::MAX);
        let rows = if !identity && lp_rows.saturating_mul(taps) <= MAX_ROWS {
            let mut rows = vec![0.0; lp_rows * taps];
            for (r, row) in rows.chunks_mut(taps).enumerate() {
                kernel.row(fc, span, j, r as i64, lp, row);
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
            span,
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
                kernel.row(self.fc, self.span, self.j, self.rem, self.lp, &mut self.scratch);
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
            acc.map(flush)
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

    /// The level, dB, of what a converter `from` Hz to `to` Hz puts out for a unit sine at `rel`
    /// times the lower Nyquist, once settled: above the lower Nyquist, going down, that output is
    /// the alias alone.
    fn level_db(k: &Kernel, from: u32, to: u32, rel: f64) -> f64 {
        let f = rel * f64::from(from.min(to)) / 2.0;
        let mut c = Converter::new(k, from, to, 0);
        let settle = 4 * c.half_taps() as usize * to as usize / from as usize + 64;
        let outs = settle + 24_000;
        let n_in = outs * from as usize / to as usize + 4 * c.half_taps() as usize + 64;
        let x: Vec<f32> = (0..n_in).map(|i| (2.0 * PI * f * i as f64 / f64::from(from)).sin() as f32).collect();
        let y = run(&mut c, k, &x, outs);
        let power = y[settle..].iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / (outs - settle) as f64;
        10.0 * (2.0 * power).max(1e-30).log10()
    }

    /// Going down, a tone just above the lower Nyquist (1.004 and 1.02 of it) leaves an alias at
    /// least 80 dB down, and one at 0.85 of it passes within 0.2 dB; going up, 0.85 passes too.
    #[test]
    fn the_stopband_starts_at_the_lower_nyquist_and_0_85_of_it_passes() {
        let k = Kernel::new();
        for (from, to) in [(96_000, 48_000), (44_100, 24_000), (48_000, 32_000), (44_100, 32_000), (88_200, 24_000)] {
            for rel in [1.004, 1.02] {
                let db = level_db(&k, from, to, rel);
                assert!(db <= -80.0, "{from} -> {to}: an input at {rel} of the lower Nyquist aliases at {db:.1} dB");
            }
            let db = level_db(&k, from, to, 0.85);
            assert!(db.abs() <= 0.2, "{from} -> {to}: 0.85 of the lower Nyquist at {db:.2} dB");
        }
        for (from, to) in [(24_000, 44_100), (32_000, 96_000), (48_000, 88_200)] {
            let db = level_db(&k, from, to, 0.85);
            assert!(db.abs() <= 0.2, "{from} -> {to}: 0.85 of the lower Nyquist at {db:.2} dB");
        }
    }

    /// The figures the module documentation states, measured through the 96 to 48 kHz and the
    /// 44.1 to 24 kHz down converters: the -0.1 dB and -3 dB points, the level at the lower
    /// Nyquist, and the worst alias of an input from 1.001 to 1.1 of it.
    #[test]
    fn the_documented_response_figures_hold() {
        let k = Kernel::new();
        for (from, to) in [(96_000, 48_000), (44_100, 24_000)] {
            let scan = |lo: f64, hi: f64, step: f64| {
                let n = ((hi - lo) / step).round() as usize;
                (0..=n).map(|i| lo + i as f64 * step).map(|r| (r, level_db(&k, from, to, r))).collect::<Vec<_>>()
            };
            let pass = scan(0.80, 0.92, 0.002);
            let edge = pass.iter().find(|(_, db)| *db < -0.1).map_or(0.0, |p| p.0);
            let half = pass.iter().find(|(_, db)| *db < -3.0).map_or(0.0, |p| p.0);
            let at_nyquist = level_db(&k, from, to, 0.999).max(level_db(&k, from, to, 1.001));
            let worst = scan(1.001, 1.1, 0.003).into_iter().map(|p| p.1).fold(f64::MIN, f64::max);
            println!("{from} -> {to}: -0.1 dB at {edge:.3}, -3 dB at {half:.3}, {at_nyquist:.1} dB at the Nyquist, worst alias {worst:.1} dB");
            assert!(edge >= 0.85 && (0.88..=0.91).contains(&half), "{from} -> {to}: edges {edge}, {half}");
            assert!(at_nyquist <= -85.0 && worst <= -85.0, "{from} -> {to}: {at_nyquist:.1} dB, {worst:.1} dB");
        }
    }

    /// A resampled output decaying into silence is flushed to zero, never subnormal.
    #[test]
    fn a_decaying_tail_comes_out_without_subnormals() {
        let k = Kernel::new();
        let mut c = Converter::new(&k, 44_100, 24_000, 0);
        let mut x = vec![0.0f32; 20_000];
        x[100] = 1.0;
        x[101] = 1e-30;
        for v in x.iter_mut().skip(102).take(200) {
            *v = 1e-36;
        }
        let y = run(&mut c, &k, &x, 10_000);
        assert!(y.iter().any(|v| v.abs() > 0.1));
        assert!(y.iter().all(|v| *v == 0.0 || v.is_normal()), "a subnormal sample");
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
