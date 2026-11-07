//! A streaming windowed-sinc sample-rate converter for the Opus writer, which encodes at 48 kHz
//! whatever the session's rate (#238). It is the same converter as `transit_dsp::offline::resample`
//! (a Kaiser-windowed sinc, 32 zero crossings each side, beta 9, 512 table steps per crossing, the
//! cutoff at 0.97 of the input Nyquist when converting up and 0.94 of the output Nyquist when
//! converting down, input outside the signal read as silence), fed a block at a time: this crate
//! sits below transit-dsp in the layer order, and a bounce never holds the whole mix in memory.
//!
//! Output sample `n` is read at input time `n * from / to`, computed exactly in integers, so a
//! long file never drifts; the output is `round(input * to / from)` samples long.

use std::f64::consts::PI;

const ZERO_CROSSINGS: f64 = 32.0;
const TABLE_STEPS: usize = 512;
const BETA: f64 = 9.0;

/// Modified Bessel function of the first kind, order 0 (the Kaiser window).
fn bessel_i0(x: f64) -> f64 {
    let y = x * x / 4.0;
    let (mut sum, mut term) = (1.0f64, 1.0f64);
    for k in 1..64 {
        term *= y / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-12 {
            break;
        }
    }
    sum
}

pub(crate) struct Resampler {
    from: u64,
    to: u64,
    table: Vec<f32>,
    half_width: f64,
    /// Input history per channel; `hist[c][0]` is input sample `base`.
    hist: Vec<Vec<f32>>,
    base: u64,
    /// Input samples received per channel.
    received: u64,
    /// The next output sample to produce.
    next: u64,
}

impl Resampler {
    pub(crate) fn new(from: u32, to: u32, channels: usize) -> Self {
        let ratio = f64::from(to) / f64::from(from.max(1));
        let cutoff = if ratio < 1.0 { ratio * 0.94 } else { 0.97 }.clamp(0.01, 1.0);
        let half_width = ZERO_CROSSINGS / cutoff;
        let n = (half_width * TABLE_STEPS as f64).ceil() as usize + 2;
        let i0b = bessel_i0(BETA);
        let table = (0..n)
            .map(|i| {
                let t = i as f64 / TABLE_STEPS as f64;
                let x = t / half_width;
                if x >= 1.0 {
                    return 0.0;
                }
                let arg = PI * cutoff * t;
                let sinc = if arg.abs() < 1e-12 { 1.0 } else { arg.sin() / arg };
                let w = bessel_i0(BETA * (1.0 - x * x).max(0.0).sqrt()) / i0b;
                (cutoff * sinc * w) as f32
            })
            .collect();
        Resampler {
            from: u64::from(from.max(1)),
            to: u64::from(to.max(1)),
            table,
            half_width,
            hist: vec![Vec::new(); channels],
            base: 0,
            received: 0,
            next: 0,
        }
    }

    #[inline]
    fn kernel(&self, t: f64) -> f32 {
        let pos = t.abs() * TABLE_STEPS as f64;
        let i = pos as usize;
        let f = (pos - i as f64) as f32;
        match (self.table.get(i), self.table.get(i + 1)) {
            (Some(a), Some(b)) => a + (b - a) * f,
            (Some(a), None) => *a,
            _ => 0.0,
        }
    }

    /// The output length for `input` input samples.
    pub(crate) fn output_len(&self, input: u64) -> u64 {
        ((u128::from(input) * u128::from(self.to) + u128::from(self.from) / 2) / u128::from(self.from)) as u64
    }

    /// The input window of output sample `n`: its first and last input index (inclusive, may be
    /// negative or past the input) and its input time's fraction.
    fn window(&self, n: u64) -> (i64, i64, u64, f64) {
        let num = u128::from(n) * u128::from(self.from);
        let whole = (num / u128::from(self.to)) as u64;
        let frac = (num % u128::from(self.to)) as f64 / self.to as f64;
        let lo = whole as i64 + (frac - self.half_width).ceil() as i64;
        let hi = whole as i64 + (frac + self.half_width).floor() as i64;
        (lo, hi, whole, frac)
    }

    /// Appends output sample `n` of every channel, interleaved, to `out`.
    fn emit(&self, n: u64, out: &mut Vec<f32>) {
        let (lo, hi, whole, frac) = self.window(n);
        for ch in &self.hist {
            let mut acc = 0.0f32;
            let mut j = lo.max(self.base as i64);
            let end = hi.min(self.received as i64 - 1);
            while j <= end {
                let t = (whole as i64 - j) as f64 + frac;
                if let Some(&v) = ch.get((j - self.base as i64) as usize) {
                    acc += v * self.kernel(t);
                }
                j += 1;
            }
            out.push(acc);
        }
    }

    /// Takes the first `frames` frames of `block` (one `Vec` per channel; missing samples are
    /// silence) and appends every output sample whose window is complete, interleaved, to `out`.
    pub(crate) fn push(&mut self, block: &[Vec<f32>], frames: usize, out: &mut Vec<f32>) {
        for (c, h) in self.hist.iter_mut().enumerate() {
            let src = block.get(c).map_or(&[][..], |v| &v[..v.len().min(frames)]);
            h.extend_from_slice(src);
            h.resize(h.len() + (frames - src.len()), 0.0);
        }
        self.received += frames as u64;
        loop {
            let (_, hi, _, _) = self.window(self.next);
            if hi >= self.received as i64 {
                break;
            }
            self.emit(self.next, out);
            self.next += 1;
        }
        self.trim();
    }

    /// Appends the remaining output samples (reading silence past the input), up to the output
    /// length of all the input received.
    pub(crate) fn finish(&mut self, out: &mut Vec<f32>) {
        let total = self.output_len(self.received);
        while self.next < total {
            self.emit(self.next, out);
            self.next += 1;
        }
        self.trim();
    }

    /// Drops the history no later output sample reads.
    fn trim(&mut self) {
        let (lo, _, _, _) = self.window(self.next);
        let keep_from = u64::try_from(lo.max(0)).unwrap_or(0).min(self.received);
        if keep_from > self.base {
            let drop = (keep_from - self.base) as usize;
            for h in &mut self.hist {
                h.drain(..drop.min(h.len()));
            }
            self.base = keep_from;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: u32, to: u32, input: &[f32], block: usize) -> Vec<f32> {
        let mut r = Resampler::new(from, to, 1);
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            r.push(&[chunk.to_vec()], chunk.len(), &mut out);
        }
        r.finish(&mut out);
        out
    }

    #[test]
    fn the_output_length_rounds_the_ratio() {
        let r = Resampler::new(44_100, 48_000, 1);
        assert_eq!(r.output_len(44_100), 48_000);
        assert_eq!(r.output_len(22_050), 24_000);
        assert_eq!(r.output_len(1), 1);
        assert_eq!(r.output_len(0), 0);
    }

    #[test]
    fn block_size_does_not_change_the_output() {
        let input: Vec<f32> = (0..10_000).map(|i| ((i as f32) * 0.031).sin() * 0.7).collect();
        let whole = run(44_100, 48_000, &input, input.len());
        assert_eq!(whole.len(), 10_884);
        for block in [1, 7, 64, 1024, 4096] {
            assert_eq!(run(44_100, 48_000, &input, block), whole, "block {block}");
        }
    }

    #[test]
    fn a_sine_keeps_its_frequency_and_level() {
        // 1 kHz at 44.1 kHz to 48 kHz: compare with the ideal sine at the output rate, away from
        // the edges (the converter reads silence before the start).
        let input: Vec<f32> = (0..44_100).map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44_100.0).sin() as f32 * 0.5).collect();
        let out = run(44_100, 48_000, &input, 512);
        let mut err = 0.0f64;
        for (n, &v) in out.iter().enumerate().skip(2_000).take(40_000) {
            let ideal = (2.0 * std::f64::consts::PI * 1000.0 * n as f64 / 48_000.0).sin() * 0.5;
            err = err.max((f64::from(v) - ideal).abs());
        }
        assert!(err < 1e-3, "max error {err}");
    }

    #[test]
    fn converting_down_works_too() {
        let input: Vec<f32> = (0..96_000).map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 96_000.0).sin() as f32 * 0.5).collect();
        let out = run(96_000, 48_000, &input, 333);
        assert_eq!(out.len(), 48_000);
        let peak = out.iter().skip(1000).take(40_000).fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
    }
}
