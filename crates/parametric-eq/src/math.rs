//! The transcendental functions of the device (ADR 0054 decision 11), all in f64 and only these four:
//! `exp2`, `log2`, `sin` and `cos`. Every rendered path calls them through this module, so the
//! portable-math record (#437, #277) can replace them in one place. None runs per sample: they run
//! when a band is designed (control rate, moving bands only), when a parameter is set, and in
//! `prepare`. With `std` each is the inherent method; without `std` each is `libm`'s.
//!
//! `sqrt` is here too because `core` has no square root, but it is not a transcendental: IEEE 754
//! rounds it exactly on every platform (and `libm`'s is correctly rounded), so it needs no seam.

/// `log2(e)`, for `e^x = 2^(x * LOG2_E)`.
pub(crate) const LOG2_E: f64 = core::f64::consts::LOG2_E;
/// `log2(10)`, for `10^x = 2^(x * LOG2_10)` and `log10(x) = log2(x) / LOG2_10`.
pub(crate) const LOG2_10: f64 = core::f64::consts::LOG2_10;
/// Decibels per doubling of amplitude, `20 * log10(2)`: a gain in dB is `2^(dB / LOG2_TO_DB)`.
pub(crate) const LOG2_TO_DB: f64 = 6.020_599_913_279_624;
/// pi.
pub(crate) const PI: f64 = core::f64::consts::PI;
/// 2 pi.
pub(crate) const TAU: f64 = core::f64::consts::TAU;

#[cfg(feature = "std")]
mod imp {
    #[inline]
    pub(crate) fn exp2(x: f64) -> f64 {
        x.exp2()
    }
    #[inline]
    pub(crate) fn log2(x: f64) -> f64 {
        x.log2()
    }
    #[inline]
    pub(crate) fn sin(x: f64) -> f64 {
        x.sin()
    }
    #[inline]
    pub(crate) fn cos(x: f64) -> f64 {
        x.cos()
    }
    #[inline]
    pub(crate) fn sqrt(x: f64) -> f64 {
        x.sqrt()
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    pub(crate) use libm::{cos, exp2, log2, sin, sqrt};
}

pub(crate) use imp::*;

/// A gain in dB as a linear amplitude factor.
#[inline]
pub(crate) fn db_to_gain(db: f64) -> f64 {
    exp2(db / LOG2_TO_DB)
}

/// A power ratio in dB, `10 * log10(p)`.
#[inline]
pub(crate) fn power_to_db(p: f64) -> f64 {
    10.0 * log2(p) / LOG2_10
}
