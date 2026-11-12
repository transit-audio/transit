//! Single-precision functions (ADR 0055, 2.2). Each transcendental function is one call into `libm`
//! (FreeBSD msun ports, pure Rust), so its result is identical on every platform; its accuracy against
//! the `f64` counterpart is measured by the crate's tests. The exact operations are correctly rounded
//! everywhere and are here so a `no_std` caller has them too.

/// Sine of `x` in radians (`libm::sinf`).
#[inline]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

/// Cosine of `x` in radians (`libm::cosf`).
#[inline]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

/// `(sin(x), cos(x))`: two calls, so it can never disagree with [`sin`] and [`cos`].
#[inline]
pub fn sin_cos(x: f32) -> (f32, f32) {
    (sin(x), cos(x))
}

/// Tangent of `x` in radians (`libm::tanf`).
#[inline]
pub fn tan(x: f32) -> f32 {
    libm::tanf(x)
}

/// `e^x` (`libm::expf`).
#[inline]
pub fn exp(x: f32) -> f32 {
    libm::expf(x)
}

/// `2^x` (`libm::exp2f`).
#[inline]
pub fn exp2(x: f32) -> f32 {
    libm::exp2f(x)
}

/// Natural logarithm (`libm::logf`).
#[inline]
pub fn ln(x: f32) -> f32 {
    libm::logf(x)
}

/// Base-2 logarithm (`libm::log2f`).
#[inline]
pub fn log2(x: f32) -> f32 {
    libm::log2f(x)
}

/// Base-10 logarithm (`libm::log10f`).
#[inline]
pub fn log10(x: f32) -> f32 {
    libm::log10f(x)
}

/// `x^y` (`libm::powf`).
#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}

/// Hyperbolic tangent (`libm::tanhf`).
#[inline]
pub fn tanh(x: f32) -> f32 {
    libm::tanhf(x)
}

/// Four-quadrant arctangent of `y / x` in radians (`libm::atan2f`).
#[inline]
pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

/// Square root, correctly rounded (`libm::sqrtf`: the SSE2 or NEON instruction, or exact software).
#[inline]
pub fn sqrt(x: f32) -> f32 {
    libm::sqrtf(x)
}

/// Largest integer at or below `x` (`libm::floorf`, exact).
#[inline]
pub fn floor(x: f32) -> f32 {
    libm::floorf(x)
}

/// Smallest integer at or above `x` (`libm::ceilf`, exact).
#[inline]
pub fn ceil(x: f32) -> f32 {
    libm::ceilf(x)
}

/// Nearest integer, halves away from zero like `f32::round` (`libm::roundf`, exact).
#[inline]
pub fn round(x: f32) -> f32 {
    libm::roundf(x)
}

/// Integer part, toward zero (`libm::truncf`, exact).
#[inline]
pub fn trunc(x: f32) -> f32 {
    libm::truncf(x)
}

/// `x * a + b` with one rounding (`libm::fmaf`). Exact everywhere, but a software routine on an x86
/// CPU without FMA, so it stays out of per-sample code unless measured (ADR 0055, 2.2).
#[inline]
pub fn mul_add(x: f32, a: f32, b: f32) -> f32 {
    libm::fmaf(x, a, b)
}
