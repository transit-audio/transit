//! Peak and RMS residual between two renders, in dBFS.
//!
//! The residual is the sample-by-sample difference `a - b`. Float buffers are full scale at 1.0;
//! the voice's int16 frames are full scale at 32768. Bit-identical buffers give `-inf` for both
//! figures. A NaN or infinity in either buffer, where the two differ, gives `+inf`, so a broken
//! render can never pass a gate.

#![forbid(unsafe_code)]

/// Two buffers of different lengths cannot be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LengthMismatch {
    /// Length of the first buffer.
    pub a: usize,
    /// Length of the second buffer.
    pub b: usize,
}

impl std::fmt::Display for LengthMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "residual of buffers with different lengths ({} and {})", self.a, self.b)
    }
}

impl std::error::Error for LengthMismatch {}

/// The residual between two renders.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Residual {
    /// Largest absolute difference, dBFS (`-inf` when there is none).
    pub peak_dbfs: f64,
    /// RMS of the difference, dBFS (`-inf` when there is none).
    pub rms_dbfs: f64,
    /// Index of the first sample with the largest difference, if any differs.
    pub peak_index: Option<usize>,
    /// Number of samples that differ in their bits.
    pub differing: usize,
}

impl Residual {
    /// Both buffers were bit-identical.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.differing == 0
    }

    /// Both figures are at or below `floor_dbfs` (a gate passes).
    #[must_use]
    pub fn within(&self, floor_dbfs: f64) -> bool {
        self.peak_dbfs <= floor_dbfs && self.rms_dbfs <= floor_dbfs
    }
}

impl std::fmt::Display for Residual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "peak {:.2} dBFS, rms {:.2} dBFS, {} samples differ", self.peak_dbfs, self.rms_dbfs, self.differing)?;
        if let Some(i) = self.peak_index {
            write!(f, " (peak at sample {i})")?;
        }
        Ok(())
    }
}

/// `20 log10(x)`, with `-inf` for zero.
#[must_use]
pub fn to_dbfs(amplitude: f64) -> f64 {
    if amplitude == 0.0 { f64::NEG_INFINITY } else { 20.0 * amplitude.log10() }
}

fn residual(len: usize, mut diff: impl FnMut(usize) -> Option<f64>) -> Residual {
    let mut peak = 0.0_f64;
    let mut peak_index = None;
    let mut sum_sq = 0.0_f64;
    let mut differing = 0;
    for i in 0..len {
        let Some(d) = diff(i) else { continue };
        differing += 1;
        let d = if d.is_finite() { d.abs() } else { f64::INFINITY };
        if d > peak || peak_index.is_none() {
            peak = d;
            peak_index = Some(i);
        }
        sum_sq += d * d;
    }
    let rms = if len == 0 { 0.0 } else { (sum_sq / len as f64).sqrt() };
    Residual { peak_dbfs: to_dbfs(peak), rms_dbfs: to_dbfs(rms), peak_index, differing }
}

/// Residual between two float renders (full scale 1.0).
///
/// # Errors
///
/// [`LengthMismatch`] when the lengths differ.
pub fn residual_f32(a: &[f32], b: &[f32]) -> Result<Residual, LengthMismatch> {
    if a.len() != b.len() {
        return Err(LengthMismatch { a: a.len(), b: b.len() });
    }
    Ok(residual(a.len(), |i| (a[i].to_bits() != b[i].to_bits()).then(|| f64::from(a[i]) - f64::from(b[i]))))
}

/// Residual between two int16 renders (full scale 32768).
///
/// # Errors
///
/// [`LengthMismatch`] when the lengths differ.
pub fn residual_i16(a: &[i16], b: &[i16]) -> Result<Residual, LengthMismatch> {
    if a.len() != b.len() {
        return Err(LengthMismatch { a: a.len(), b: b.len() });
    }
    Ok(residual(a.len(), |i| (a[i] != b[i]).then(|| (f64::from(a[i]) - f64::from(b[i])) / 32768.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_buffers_are_exact_at_minus_infinity() {
        let a = [0.5_f32, -0.25, 0.0, 1.0];
        let r = residual_f32(&a, &a).unwrap();
        assert!(r.is_exact());
        assert_eq!(r.peak_dbfs, f64::NEG_INFINITY);
        assert_eq!(r.rms_dbfs, f64::NEG_INFINITY);
        assert_eq!(r.peak_index, None);
        assert!(r.within(f64::NEG_INFINITY));
    }

    #[test]
    fn a_half_scale_step_is_minus_six_dbfs_peak() {
        let a = [0.0_f32; 4];
        let b = [0.0_f32, 0.5, 0.0, 0.0];
        let r = residual_f32(&a, &b).unwrap();
        assert_eq!(r.differing, 1);
        assert_eq!(r.peak_index, Some(1));
        assert!((r.peak_dbfs - 20.0 * 0.5_f64.log10()).abs() < 1e-12);
        // RMS over four samples: sqrt(0.25 / 4) = 0.25.
        assert!((r.rms_dbfs - 20.0 * 0.25_f64.log10()).abs() < 1e-12);
    }

    #[test]
    fn one_lsb_of_int16_is_about_minus_ninety_dbfs() {
        let r = residual_i16(&[0, 0], &[1, 0]).unwrap();
        assert!((r.peak_dbfs - (-90.309)).abs() < 1e-3, "{r}");
    }

    #[test]
    fn a_nan_never_passes() {
        let r = residual_f32(&[0.0], &[f32::NAN]).unwrap();
        assert_eq!(r.peak_dbfs, f64::INFINITY);
        assert!(!r.within(0.0));
    }

    #[test]
    fn different_lengths_are_an_error() {
        assert_eq!(residual_i16(&[0], &[0, 0]), Err(LengthMismatch { a: 1, b: 2 }));
    }

    #[test]
    fn signed_zeros_differ_in_bits_but_not_in_value() {
        let r = residual_f32(&[0.0], &[-0.0]).unwrap();
        assert_eq!(r.differing, 1);
        assert_eq!(r.peak_dbfs, f64::NEG_INFINITY);
    }
}
