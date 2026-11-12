//! Shared helpers for the tests: grids, rates, the deviation measure.

#![allow(dead_code)]

/// The rates every sweep runs (decision 12): 44.1, 48, 96 and 192 kHz.
pub(crate) const RATES: [f64; 4] = [44_100.0, 48_000.0, 96_000.0, 192_000.0];
/// One rate of each outer class (decision 12).
pub(crate) const OUTER_RATES: [f64; 2] = [32_000.0, 384_000.0];
/// The Q list of decision 2.5.
pub(crate) const QS: [f64; 10] = [0.1, 0.2, 0.5, 0.707, 1.0, 2.0, 4.0, 10.0, 20.0, 40.0];

/// `n` log-spaced values from `lo` to `hi` inclusive.
pub(crate) fn log_grid(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    (0..n).map(|i| lo * (hi / lo).powf(i as f64 / (n - 1) as f64)).collect()
}

/// The 30 band frequencies of the slice 1 sweep, 20 Hz to 20 kHz.
pub(crate) fn band_freqs() -> Vec<f64> {
    log_grid(20.0, 20_000.0, 30)
}

/// Gains −30 to +30 dB in 3 dB steps, without 0 (the identity).
pub(crate) fn gains() -> Vec<f64> {
    (-10..=10).filter(|&k| k != 0).map(|k| 3.0 * k as f64).collect()
}

/// The deviation grid of decision 2.5: `20·(f_top/20)^(i/999)`, i = 0 … 999, `f_top` = min(20 kHz,
/// 0.45·fs), with each point's half-angle sine and cosine.
pub(crate) struct EvalGrid {
    pub(crate) f: Vec<f64>,
    pub(crate) sc: Vec<(f64, f64)>,
}

impl EvalGrid {
    pub(crate) fn new(fs: f64) -> Self {
        Self::points(fs, 1000)
    }

    pub(crate) fn points(fs: f64, n: usize) -> Self {
        let top = 20_000f64.min(0.45 * fs);
        let f = log_grid(20.0, top, n);
        let sc = f.iter().map(|&f| {
            let h = std::f64::consts::PI * f / fs;
            (h.sin(), h.cos())
        });
        Self { sc: sc.collect(), f }
    }
}

pub(crate) fn db(p: f64) -> f64 {
    10.0 * p.log10()
}

/// A deterministic xorshift generator for test signals.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in [−1, 1).
    pub(crate) fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
}

/// A stereo test signal: two decorrelated noises at −6 dBFS plus a few quiet and extreme samples.
pub(crate) fn noise(frames: usize, channels: usize, seed: u64) -> Vec<Vec<f32>> {
    let mut r = Rng(seed | 1);
    (0..channels).map(|_| (0..frames).map(|_| (0.5 * r.uniform()) as f32).collect()).collect()
}

/// A used, on band.
pub(crate) fn band(kind: crate::BandType, freq_hz: f64, gain_db: f64, q: f64) -> crate::BandParams {
    crate::BandParams { used: true, on: true, kind, freq_hz, gain_db, q, ..crate::BandParams::default() }
}
