//! `F`: an `f32` whose arithmetic flushes subnormal results to zero, as the oracle's FPU does.
//!
//! The reference is the firmware's C++ as its desktop test harness builds and runs it: the harness
//! turns flush-to-zero on (`rings/test/rings_test.cc`), and so does the oracle around each call into
//! the C++. The module itself does not: its firmware never writes FPSCR, the Cortex-M4 resets with
//! FZ off, and its GCC build fuses multiply-adds, so the hardware's bits differ from both oracle
//! families; the port matches the x86 oracle, which is what the null tests prove. Rust code always runs in the default floating-point mode, where subnormals are kept,
//! and changing that mode under Rust is undefined behavior. So the port reproduces flush-to-zero
//! explicitly: every `+`, `-`, `*` and `/` between `F` values (or an `F` and an `f32`) rounds as
//! IEEE single precision does and then turns a subnormal result into a zero of the same sign, which
//! is what x86 MXCSR.FTZ does to every arithmetic result. Negation, `abs`, comparisons, `min` and
//! `max` never flush, as on the FPU; subnormal inputs are used as they are (FTZ without DAZ).
//!
//! Two edge cases are not reproduced, both far below anything audible: x86 detects tininess after
//! rounding, so a result that rounds up to the smallest normal is kept there as here, but one whose
//! unbounded-exponent rounding is tiny while its denormal rounding reaches the smallest normal is
//! flushed on x86 and kept here; and AArch64's FPCR.FZ (the macOS oracle) also flushes subnormal
//! inputs, which this type does not.
//!
//! Conversions to integers reproduce x86-64's truncating conversions, which the C++ compilers emit
//! for its casts, including out of range (undefined in C++, so only x86 behavior is reproduced).

#![forbid(unsafe_code)]

use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// An `f32` with flush-to-zero arithmetic (see the module documentation).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct F(pub f32);

/// A subnormal value as a zero of the same sign; anything else unchanged.
#[inline(always)]
#[must_use]
pub fn ftz(x: f32) -> f32 {
    if x.is_subnormal() { 0.0_f32.copysign(x) } else { x }
}

impl F {
    /// Zero.
    pub const ZERO: Self = Self(0.0);
    /// One.
    pub const ONE: Self = Self(1.0);

    /// `fabs`.
    #[inline(always)]
    #[must_use]
    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// `sqrtf` (correctly rounded; never subnormal for a normal input).
    #[inline(always)]
    #[must_use]
    pub fn sqrt(self) -> Self {
        Self(ftz(self.0.sqrt()))
    }

    /// `powf` from the platform's math library, as the C++ calls it.
    #[inline(always)]
    #[must_use]
    pub fn powf(self, exponent: Self) -> Self {
        Self(ftz(self.0.powf(exponent.0)))
    }

    /// `std::min(self, other)`: `other < self ? other : self`.
    #[inline(always)]
    #[must_use]
    pub fn min(self, other: Self) -> Self {
        if other.0 < self.0 { other } else { self }
    }

    /// `std::max(self, other)`: `self < other ? other : self`.
    #[inline(always)]
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        if self.0 < other.0 { other } else { self }
    }

    /// `static_cast<int32_t>(x)` as x86-64 computes it (`cvttss2si`): truncation toward zero, and
    /// `i32::MIN` for NaN or a value out of range.
    #[inline(always)]
    #[must_use]
    pub fn trunc_i32(self) -> i32 {
        if self.0 >= -2_147_483_648.0 && self.0 < 2_147_483_648.0 {
            #[allow(clippy::cast_possible_truncation)]
            let i = self.0 as i32;
            i
        } else {
            i32::MIN
        }
    }

    /// `static_cast<uint32_t>(x)` as x86-64 compilers emit it: a 64-bit truncating conversion whose
    /// low 32 bits are kept, so values above `u32::MAX` and negative values wrap; 0 for NaN or a
    /// value beyond the 64-bit range.
    #[inline(always)]
    #[must_use]
    pub fn trunc_u32(self) -> u32 {
        if self.0.abs() < 9.223_372e18 {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let u = (self.0 as i64) as u32;
            u
        } else {
            0
        }
    }

    /// `static_cast<size_t>(x)` for a value the caller knows is small and non-negative.
    #[inline(always)]
    #[must_use]
    pub fn trunc_usize(self) -> usize {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let u = self.0 as usize;
        u
    }

    /// `static_cast<float>(i)` for an `int32_t`.
    #[inline(always)]
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn from_i32(i: i32) -> Self {
        Self(i as f32)
    }

    /// `static_cast<float>(u)` for a `uint32_t` (rounded to nearest, as `cvtsi2ss`).
    #[inline(always)]
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn from_u32(u: u32) -> Self {
        Self(u as f32)
    }

    /// `static_cast<float>(n)` for a small `size_t`.
    #[inline(always)]
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn from_usize(n: usize) -> Self {
        Self(n as f32)
    }
}

impl Neg for F {
    type Output = Self;
    #[inline(always)]
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl PartialEq<f32> for F {
    #[inline(always)]
    fn eq(&self, other: &f32) -> bool {
        self.0 == *other
    }
}

impl PartialOrd<f32> for F {
    #[inline(always)]
    fn partial_cmp(&self, other: &f32) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(other)
    }
}

macro_rules! flushing_op {
    ($trait:ident, $method:ident, $assign_trait:ident, $assign_method:ident, $op:tt) => {
        impl $trait for F {
            type Output = F;
            #[inline(always)]
            fn $method(self, rhs: F) -> F {
                F(ftz(self.0 $op rhs.0))
            }
        }
        impl $trait<f32> for F {
            type Output = F;
            #[inline(always)]
            fn $method(self, rhs: f32) -> F {
                F(ftz(self.0 $op rhs))
            }
        }
        impl $trait<F> for f32 {
            type Output = F;
            #[inline(always)]
            fn $method(self, rhs: F) -> F {
                F(ftz(self $op rhs.0))
            }
        }
        impl $assign_trait for F {
            #[inline(always)]
            fn $assign_method(&mut self, rhs: F) {
                *self = *self $op rhs;
            }
        }
        impl $assign_trait<f32> for F {
            #[inline(always)]
            fn $assign_method(&mut self, rhs: f32) {
                *self = *self $op rhs;
            }
        }
    };
}

flushing_op!(Add, add, AddAssign, add_assign, +);
flushing_op!(Sub, sub, SubAssign, sub_assign, -);
flushing_op!(Mul, mul, MulAssign, mul_assign, *);
flushing_op!(Div, div, DivAssign, div_assign, /);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnormal_results_flush_to_a_signed_zero() {
        let tiny = F(f32::MIN_POSITIVE);
        assert_eq!((tiny * 0.5).0.to_bits(), 0.0_f32.to_bits());
        assert_eq!((-tiny * 0.5).0.to_bits(), (-0.0_f32).to_bits());
        assert_eq!((tiny / 3.0).0.to_bits(), 0.0_f32.to_bits());
        assert_eq!((tiny - F(f32::MIN_POSITIVE * 1.5)).0.to_bits(), (-0.0_f32).to_bits());
        // Normal results are IEEE single precision, unchanged.
        assert_eq!((F(0.1) + 0.2).0.to_bits(), (0.1_f32 + 0.2_f32).to_bits());
        // Inputs are not flushed (FTZ without DAZ): a subnormal input times 2^24 is normal.
        let sub = f32::from_bits(1);
        assert_eq!((F(sub) * 16_777_216.0).0, sub * 16_777_216.0);
    }

    #[test]
    fn conversions_are_x86_truncations() {
        assert_eq!(F(-2.7).trunc_i32(), -2);
        assert_eq!(F(3e9).trunc_i32(), i32::MIN);
        assert_eq!(F(f32::NAN).trunc_i32(), i32::MIN);
        assert_eq!(F(4_294_967_296.0).trunc_u32(), 0);
        assert_eq!(F(4_294_967_808.0).trunc_u32(), 512);
        assert_eq!(F(-1.0).trunc_u32(), u32::MAX);
        assert_eq!(F(3_000_000_000.0).trunc_u32(), 3_000_000_000);
    }

    #[test]
    fn min_and_max_return_an_operand_as_std_does() {
        assert_eq!(F(0.0).min(F(-0.0)).0.to_bits(), 0.0_f32.to_bits());
        assert_eq!(F(-0.0).max(F(0.0)).0.to_bits(), (-0.0_f32).to_bits());
        assert_eq!(F(1.0).min(F(0.5)), F(0.5));
        assert_eq!(F(1.0).max(F(0.5)), F(1.0));
    }
}
