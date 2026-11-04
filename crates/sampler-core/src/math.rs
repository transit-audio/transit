//! The float functions the core needs beyond `core` (ADR 0037 Tier B). With `std` each is the
//! inherent method (the desktop arithmetic, and the legacy sampler's); without `std` each is `libm`'s.

#[cfg(feature = "std")]
mod imp {
    #[inline]
    pub(crate) fn powf(x: f32, y: f32) -> f32 {
        x.powf(y)
    }
    #[inline]
    pub(crate) fn pow(x: f64, y: f64) -> f64 {
        x.powf(y)
    }
    #[inline]
    pub(crate) fn round(x: f64) -> f64 {
        x.round()
    }
    #[inline]
    pub(crate) fn floor(x: f64) -> f64 {
        x.floor()
    }
}

#[cfg(not(feature = "std"))]
mod imp {
    pub(crate) use libm::{floor, pow, powf, round};
}

pub(crate) use imp::*;
