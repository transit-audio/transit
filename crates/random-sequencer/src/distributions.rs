//! Samples from the beta distributions by their precomputed inverse cdfs:
//! `marbles/random/distributions.h` (`BetaDistributionSample`, `FastBetaDistributionSample`).

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::{DIST_ICDF_4_3, DISTRIBUTIONS, ICDF_SIZE};
use crate::stmlib::{index, integral_fractional, interpolate_clamped};

const NUM_BIAS_VALUES: f32 = 5.0;
const NUM_RANGE_VALUES: usize = 9;
const ICDF_TABLE_SIZE: f32 = 128.0;

/// `TransitInterpolateIcdf` of the oracle's `patches/all/0011`: `Interpolate(table + offset, index,
/// 128)` with both reads clamped to the 387-entry table (a random component of exactly 1.0 reads
/// one past it at a nonzero fraction upstream).
fn icdf(table: &[f32; ICDF_SIZE], offset: usize, uniform: F) -> F {
    let (integral, fractional) = integral_fractional(uniform * ICDF_TABLE_SIZE);
    let i = offset as i64 + i64::from(integral);
    let last = ICDF_SIZE - 1;
    let a = F(table[index(i, last + 1)]);
    let b = F(table[index(i + 1, last + 1)]);
    a + (b - a) * fractional
}

/// `BetaDistributionSample(uniform, spread, bias)`: tables exist for biases up to 0.5, the rest by
/// symmetry; the lowest and highest 5 % use finer sub-tables.
#[must_use]
pub fn beta_sample(uniform: F, spread: F, bias: F) -> F {
    let (mut uniform, mut bias) = (uniform, bias);
    let flip = bias > 0.5;
    if flip {
        uniform = 1.0 - uniform;
        bias = 1.0 - bias;
    }
    let bias = bias * ((NUM_BIAS_VALUES - 1.0) * 2.0);
    let spread = spread * (NUM_RANGE_VALUES as f32 - 1.0);
    let (bias_integral, bias_fractional) = integral_fractional(bias);
    let (spread_integral, spread_fractional) = integral_fractional(spread);
    let cell = i64::from(bias_integral) * (NUM_RANGE_VALUES as i64 + 1) + i64::from(spread_integral);
    let mut offset = 0_usize;
    if uniform <= 0.05 {
        offset = ICDF_TABLE_SIZE as usize + 1;
        uniform *= 20.0;
    } else if uniform >= 0.95 {
        offset = 2 * (ICDF_TABLE_SIZE as usize + 1);
        uniform = (uniform - 0.95) * 20.0;
    }
    let table = |c: i64| DISTRIBUTIONS[index(c, DISTRIBUTIONS.len())];
    let x1y1 = icdf(table(cell), offset, uniform);
    let x2y1 = icdf(table(cell + 1), offset, uniform);
    let x1y2 = icdf(table(cell + NUM_RANGE_VALUES as i64 + 1), offset, uniform);
    let x2y2 = icdf(table(cell + NUM_RANGE_VALUES as i64 + 2), offset, uniform);
    let y1 = x1y1 + (x2y1 - x1y1) * spread_fractional;
    let y2 = x1y2 + (x2y2 - x1y2) * spread_fractional;
    let y = y1 + (y2 - y1) * bias_fractional;
    if flip { 1.0 - y } else { y }
}

/// `FastBetaDistributionSample(uniform)`: a beta(3, 3) with a fatter tail (the jitter's).
#[must_use]
pub fn fast_beta_sample(uniform: F) -> F {
    interpolate_clamped(DIST_ICDF_4_3, 0, uniform, ICDF_TABLE_SIZE)
}
