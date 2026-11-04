//! The float functions the primitives need beyond `core` (ADR 0037 Tier B). With `std` each is the
//! inherent method the code called before, so the desktop arithmetic is the same code bit for bit;
//! without `std` each is `libm`'s.

#[cfg(feature = "std")]
mod imp {
    #[inline]
    pub(crate) fn sinf(x: f32) -> f32 {
        x.sin()
    }
    #[inline]
    pub(crate) fn cosf(x: f32) -> f32 {
        x.cos()
    }
    #[inline]
    pub(crate) fn tanf(x: f32) -> f32 {
        x.tan()
    }
    #[inline]
    pub(crate) fn expf(x: f32) -> f32 {
        x.exp()
    }
    #[inline]
    pub(crate) fn exp2f(x: f32) -> f32 {
        x.exp2()
    }
    #[inline]
    pub(crate) fn lnf(x: f32) -> f32 {
        x.ln()
    }
    #[inline]
    pub(crate) fn log2f(x: f32) -> f32 {
        x.log2()
    }
    #[inline]
    pub(crate) fn powf(x: f32, y: f32) -> f32 {
        x.powf(y)
    }
    #[inline]
    pub(crate) fn tan(x: f64) -> f64 {
        x.tan()
    }
    #[inline]
    pub(crate) fn sqrt(x: f64) -> f64 {
        x.sqrt()
    }
    #[inline]
    pub(crate) fn floor(x: f64) -> f64 {
        x.floor()
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    pub(crate) use libm::{cosf, exp2f, expf, floor, log2f, powf, sinf, sqrt, tan, tanf};

    #[inline]
    pub(crate) fn lnf(x: f32) -> f32 {
        libm::logf(x)
    }
}

pub(crate) use imp::*;
