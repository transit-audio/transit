//! Double-precision functions (ADR 0055, 2.2). Each transcendental function is one call into `libm`
//! (FreeBSD msun ports, pure Rust), so its result is identical on every platform; its accuracy against
//! arbitrary-precision reference values is measured by the crate's tests. The exact operations are
//! correctly rounded everywhere and are here so a `no_std` caller has them too.

/// Sine of `x` in radians (`libm::sin`).
#[inline]
pub fn sin(x: f64) -> f64 {
    libm::sin(x)
}

/// Cosine of `x` in radians (`libm::cos`).
#[inline]
pub fn cos(x: f64) -> f64 {
    libm::cos(x)
}

/// `(sin(x), cos(x))`: two calls, so it can never disagree with [`sin`] and [`cos`].
#[inline]
pub fn sin_cos(x: f64) -> (f64, f64) {
    (sin(x), cos(x))
}

/// Tangent of `x` in radians (`libm::tan`).
#[inline]
pub fn tan(x: f64) -> f64 {
    libm::tan(x)
}

/// `e^x` (`libm::exp`).
#[inline]
pub fn exp(x: f64) -> f64 {
    libm::exp(x)
}

/// `2^x` (`libm::exp2`).
#[inline]
pub fn exp2(x: f64) -> f64 {
    libm::exp2(x)
}

/// Natural logarithm (`libm::log`).
#[inline]
pub fn ln(x: f64) -> f64 {
    libm::log(x)
}

/// Base-2 logarithm (`libm::log2`).
#[inline]
pub fn log2(x: f64) -> f64 {
    libm::log2(x)
}

/// Base-10 logarithm (`libm::log10`).
#[inline]
pub fn log10(x: f64) -> f64 {
    libm::log10(x)
}

/// `x^y` (`libm::pow`).
#[inline]
pub fn pow(x: f64, y: f64) -> f64 {
    libm::pow(x, y)
}

/// Four-quadrant arctangent of `y / x` in radians (`libm::atan2`; the surround panner's azimuth).
#[inline]
pub fn atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

/// Square root, correctly rounded (`libm::sqrt`: the SSE2 or NEON instruction, or exact software).
#[inline]
pub fn sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}

/// Largest integer at or below `x` (`libm::floor`, exact).
#[inline]
pub fn floor(x: f64) -> f64 {
    libm::floor(x)
}

/// Smallest integer at or above `x` (`libm::ceil`, exact).
#[inline]
pub fn ceil(x: f64) -> f64 {
    libm::ceil(x)
}

/// Nearest integer, halves away from zero like `f64::round` (`libm::round`, exact).
#[inline]
pub fn round(x: f64) -> f64 {
    libm::round(x)
}

/// Integer part, toward zero (`libm::trunc`, exact).
#[inline]
pub fn trunc(x: f64) -> f64 {
    libm::trunc(x)
}

/// `x * a + b` with one rounding (`libm::fma`). Exact everywhere, but a software routine on an x86
/// CPU without FMA, so it stays out of per-sample code unless measured (ADR 0055, 2.2).
#[inline]
pub fn mul_add(x: f64, a: f64, b: f64) -> f64 {
    libm::fma(x, a, b)
}
