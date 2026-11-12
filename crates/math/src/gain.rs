//! Exact gain conversions (ADR 0055, 2.9): decibels and the pan law computed in `f64` through the seam
//! and rounded once to `f32`, one path with no special cases.
//!
//! [`db_to_gain`] returns the correctly rounded `f32` of 10^(dB/20) at every value of the 0.1 dB and
//! 0.01 dB grids from -144 to +48 dB and at every whole decade (0 dB is exactly 1, +20 dB exactly 10,
//! +40 dB exactly 100), checked against arbitrary-precision references by the crate's tests. The `f64`
//! result's error stays below 2^-47.9 relative up to the +200 dB clamp, so an arbitrary `f32` input can
//! misround only when its exact result lies that close to an `f32` rounding midpoint (a handful of the
//! roughly 2.2e9 inputs, measured exhaustively by the crate's ignored near-tie test); such a result is
//! still within one rounding step of exact and still identical on every platform.

use crate::consts::{DB_TO_LOG2_F64, LOG2_TO_DB_F64};
use crate::f64 as m64;

/// The silence floor: anything at or below -144 dB (and NaN) converts to a gain of exactly 0.0, and
/// [`gain_to_db`] never reports less.
pub const MIN_DB: f32 = -144.0;

/// The top of [`db_to_gain`]'s range: larger inputs, +infinity included, convert as +200 dB (a gain of
/// exactly 1e10). Callers keep their own tighter caps.
pub const MAX_DB: f32 = 200.0;

/// Decibels to linear gain: `exp2(dB · log2(10)/20)` in `f64`, rounded once to `f32`. NaN and anything
/// at or below [`MIN_DB`] are exactly 0.0; the input is clamped at [`MAX_DB`].
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    if db.is_nan() || db <= MIN_DB {
        return 0.0;
    }
    m64::exp2(f64::from(db.min(MAX_DB)) * DB_TO_LOG2_F64) as f32
}

/// Linear gain (sign ignored) to decibels: `20·log10(2) · log2(|g|)` in `f64`, rounded once to `f32`
/// and floored at [`MIN_DB`]. NaN is [`MIN_DB`]; an infinite gain is `f32::MAX`. Exact powers of ten
/// give exact multiples of 20 (`gain_to_db(10.0)` is 20.0).
#[inline]
pub fn gain_to_db(g: f32) -> f32 {
    let a = f32::from_bits(g.to_bits() & 0x7fff_ffff);
    if a.is_nan() {
        return MIN_DB;
    }
    if a.is_infinite() {
        return f32::MAX;
    }
    ((LOG2_TO_DB_F64 * m64::log2(f64::from(a))) as f32).max(MIN_DB)
}

/// The pan law's reference: cos(pi/4) is -3.0103 dB, so a gain raised to `center_db / -3.0103` puts a
/// centered source at `center_db`.
const QUARTER_TURN_DB: f64 = -3.010_3;

/// Left and right gains of a mono source at `pan` in -1 (hard left) ..= 1 (hard right) under the law
/// whose center attenuation is `center_db` (-3, -4.5 or -6 dB: `cos` and `sin` of the quarter-turn
/// angle raised to `center_db / -3.0103`; 0 dB: the balance law, where the far side fades out
/// linearly). The angle, `cos`, `sin` and the power are `f64` through the seam, each gain rounded once
/// to `f32`. NaN pans center; pans beyond the ends clamp; the hard ends are exactly (1, 0) and (0, 1).
pub fn pan_gains(pan: f32, center_db: f32) -> (f32, f32) {
    let pan = if pan.is_nan() { 0.0 } else { pan.clamp(-1.0, 1.0) };
    if pan <= -1.0 {
        return (1.0, 0.0);
    }
    if pan >= 1.0 {
        return (0.0, 1.0);
    }
    if center_db == 0.0 {
        return ((1.0 - pan).min(1.0), (1.0 + pan).min(1.0));
    }
    let theta = (f64::from(pan) + 1.0) * core::f64::consts::FRAC_PI_4;
    let exp = f64::from(center_db) / QUARTER_TURN_DB;
    let l = m64::pow(m64::cos(theta).max(0.0), exp);
    let r = m64::pow(m64::sin(theta).max(0.0), exp);
    (l as f32, r as f32)
}
