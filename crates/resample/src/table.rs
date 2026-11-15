//! The kernel and its table (ADR 0056, decisions 8.1 and 8.2).

use alloc::boxed::Box;
use alloc::vec;

/// Zero crossings a side: the kernel is 32 taps wide at a rate of 1 or below (decision 8.1).
pub const ZERO_CROSSINGS: usize = 16;
/// Table points per zero crossing, per unit of `u` (decision 8.2).
pub const PHASES: usize = 512;
/// Table entries: `h(i / 512)` for `i` in `0..=16·512` (8,193).
pub const TABLE_LEN: usize = ZERO_CROSSINGS * PHASES + 1;
/// The Kaiser window's shape, `0.1102·(80 − 8.7)` (the design formula for 80 dB; a design figure, not
/// a bound), evaluated in binary64 as written: 7.85726.
pub const BETA: f64 = 0.1102 * (80.0 - 8.7);
/// Terms of the `I0` power series (decision 8.2).
pub const I0_TERMS: usize = 60;

/// The modified Bessel function of the first kind, order 0, by its power series
/// `Σ ((z/2)^k / k!)²` for `k` in `0..60`, each term from the previous one as `term·y/k²` with
/// `y = (z/2)²`, summed in ascending `k` in binary64.
#[must_use]
pub fn bessel_i0(z: f64) -> f64 {
    let half = z * 0.5;
    let y = half * half;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..I0_TERMS {
        let kk = (k * k) as f64;
        term = term * y / kk;
        sum += term;
    }
    sum
}

/// The kernel `h(u) = sinc(u)·I0(β·sqrt(1 − (u/16)²)) / I0(β)` for |u| < 16, 0 beyond, 1 at 0, in
/// binary64 through the portable seam: `sinc(u) = sin(πu)/(πu)`, then `sinc·(I0(…)/I0(β))`.
#[must_use]
pub fn kernel(u: f64) -> f64 {
    let a = if u < 0.0 { -u } else { u };
    if a == 0.0 {
        return 1.0;
    }
    if a.is_nan() || a >= ZERO_CROSSINGS as f64 {
        return 0.0;
    }
    let pu = core::f64::consts::PI * a;
    let sinc = transit_math::f64::sin(pu) / pu;
    let r = a / ZERO_CROSSINGS as f64;
    let window = bessel_i0(BETA * transit_math::f64::sqrt(1.0 - r * r)) / bessel_i0(BETA);
    sinc * window
}

/// The reader's table: `T[i] = h(i/512)` rounded once to `f32` (exactly 1 at 0 and exactly 0 at every
/// nonzero integer), and the delta table `D[i] = T[i+1] − T[i]` in `f32` (`D[8192] = 0`, never read).
/// 64 KiB, built once off the audio thread and shared read-only.
pub struct SincTable {
    t: Box<[f32]>,
    d: Box<[f32]>,
}

impl SincTable {
    /// Build the table (allocates; about 8,193 kernel evaluations). Never on the audio thread.
    #[must_use]
    pub fn new() -> SincTable {
        let mut t = vec![0.0f32; TABLE_LEN];
        for (i, v) in t.iter_mut().enumerate() {
            *v = if i == 0 {
                1.0
            } else if i % PHASES == 0 {
                0.0
            } else {
                kernel(i as f64 / PHASES as f64) as f32
            };
        }
        let mut d = vec![0.0f32; TABLE_LEN];
        for (dv, pair) in d.iter_mut().zip(t.windows(2)) {
            *dv = pair[1] - pair[0];
        }
        SincTable { t: t.into_boxed_slice(), d: d.into_boxed_slice() }
    }

    /// The process-wide table, built on first use. Call it once off the audio thread (when a plan or a
    /// voice pool is built) and hand the reference to the real-time code; later calls are one atomic
    /// load.
    #[cfg(feature = "std")]
    #[must_use]
    pub fn shared() -> &'static SincTable {
        static TABLE: std::sync::OnceLock<SincTable> = std::sync::OnceLock::new();
        TABLE.get_or_init(SincTable::new)
    }

    /// The interpolated coefficient at `u` in `[0, 16)` (decision 8.3): `i = floor(512·u)`, the fraction
    /// `512·u − i` rounded to `f32`, then `T[i] + f·D[i]` in `f32`, no fused multiply-add. `u` outside
    /// `[0, 16)` (or not a number) gives 0.
    #[inline]
    #[must_use]
    pub fn coefficient(&self, u: f64) -> f32 {
        let v = u * PHASES as f64;
        if v.is_nan() || v < 0.0 || v >= (TABLE_LEN - 1) as f64 {
            return 0.0;
        }
        // `v` is in [0, 8192): truncation is the floor, exactly.
        let i = v as usize;
        let f = (v - i as f64) as f32;
        self.t[i] + f * self.d[i]
    }

    /// `u` in `[0, 16)` as its table index `floor(512·u)` and its fraction rounded to `f32`.
    #[inline]
    pub(crate) fn split(&self, u: f64) -> (usize, f32) {
        let v = u * PHASES as f64;
        let i = v as usize;
        (i, (v - i as f64) as f32)
    }

    /// `T[i] + f·D[i]` in `f32`.
    #[inline]
    pub(crate) fn at(&self, i: usize, f: f32) -> f32 {
        self.t[i] + f * self.d[i]
    }

    /// The table `T`, 8,193 entries.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.t
    }

    /// The delta table `D`, 8,193 entries (the last is 0 and never read).
    #[must_use]
    pub fn deltas(&self) -> &[f32] {
        &self.d
    }
}

impl Default for SincTable {
    fn default() -> SincTable {
        SincTable::new()
    }
}
