//! Small helpers the primitives share (copies of dsp's `util` ones, same code).

/// Converts milliseconds to (fractional) samples, never negative or non-finite.
#[inline]
pub(crate) fn ms_to_samples(ms: f32, sr: f32) -> f32 {
    let s = ms * 0.001 * sr;
    if s.is_finite() { s.max(0.0) } else { 0.0 }
}

/// Flushes denormals and non-finite values in feedback paths to zero.
#[inline]
pub(crate) fn flush(x: f32) -> f32 {
    if x.is_finite() && x.abs() > 1.0e-25 { x } else { 0.0 }
}
