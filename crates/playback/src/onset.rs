//! Onset of a sound on an audio return, by sample position, and the summary of a run of trials:
//! the detector the External Instrument's Measure (ADR 0020 decision 3) and `tools/audio-bench`
//! (`--midi-roundtrip`, `--placement`) share, so the app and the bench time a synth one way.
//!
//! A synth note is not a known waveform, so the click's matched filter ([`crate::hardware_probe`],
//! the loop probe) cannot find it. Instead: a short causal envelope (the mean absolute value over
//! [`window`] samples, about 0.25 ms), a threshold 12 dB over the envelope's peak in a stretch of
//! noise floor measured before any note, and the first crossing of it; then a refinement to the
//! first sample where the envelope exceeds 10 percent of its peak in the [`PEAK_MS`] after that
//! crossing (the attack's rise, independent of the floor). The refined sample is the onset; the
//! distance between the two crossings is the detector's uncertainty. The causal envelope never
//! reports an onset before the sound starts, and lags it by at most one window.
//!
//! Pure functions on the control side; nothing here runs on an audio thread.

/// The threshold is this many times the reference floor's envelope peak (+12 dB).
pub const FLOOR_FACTOR: f32 = 4.0;
/// A pre-send window counts as at the noise floor while its envelope peak is at most this many
/// times the reference floor's (+6 dB).
pub const QUIET_FACTOR: f32 = 2.0;
/// The threshold is never below this level (-60 dBFS), so a digitally silent floor still needs a
/// real signal.
pub const MIN_THRESHOLD: f32 = 0.001;
/// A reference floor louder than this (-20 dBFS) is not a floor: a drone, a held note or the wrong input.
pub const MAX_FLOOR: f32 = 0.1;
/// The refinement looks for the attack's peak this long after the threshold crossing.
pub const PEAK_MS: u32 = 20;
/// The refinement's level: this fraction of the attack's peak.
pub const REFINE_FRACTION: f32 = 0.1;
/// A trial further than this many standard deviations from the mean is listed as an outlier.
pub const OUTLIER_SIGMAS: f64 = 2.0;

/// The envelope window at `rate`: about 0.25 ms, at least one sample.
#[must_use]
pub fn window(rate: u32) -> usize {
    usize::try_from(rate / 4000).unwrap_or(1).max(1)
}

/// Magnitude of one sample; a non-finite sample counts as full scale (a broken sample is not silence).
fn magnitude(x: f32) -> f32 {
    if x.is_finite() { x.abs() } else { 1.0 }
}

/// The envelope at sample `n`: the mean magnitude of the `w` samples ending at `n` (fewer at the
/// start of `samples`, divided by `w` all the same).
#[must_use]
pub fn envelope_at(samples: &[f32], n: usize, w: usize) -> f32 {
    let w = w.max(1);
    let start = (n + 1).saturating_sub(w);
    samples.get(start..=n).map_or(0.0, |s| s.iter().map(|&x| magnitude(x)).sum::<f32>() / w as f32)
}

/// The level of the return over a stretch with no note.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floor {
    /// Peak of the envelope (the first `w - 1` samples, whose window is partial, are left out).
    pub peak: f32,
    /// Root mean square of the samples.
    pub rms: f32,
}

/// The floor of `samples` with envelope window `w` (silence when empty).
#[must_use]
pub fn noise_floor(samples: &[f32], w: usize) -> Floor {
    let peak = (w.max(1) - 1..samples.len()).map(|n| envelope_at(samples, n, w)).fold(0.0f32, f32::max);
    let sum: f64 = samples.iter().map(|&x| f64::from(magnitude(x)).powi(2)).sum();
    let rms = if samples.is_empty() { 0.0 } else { (sum / samples.len() as f64).sqrt() as f32 };
    Floor { peak, rms }
}

/// A stretch that should be quiet is louder than [`MAX_FLOOR`]: a drone, a held note, a release
/// still sounding, or the wrong input. `peak` is its envelope peak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TooLoud {
    pub peak: f32,
}

/// The level an onset's envelope must exceed over the `reference` floor, or [`TooLoud`] when that
/// floor is too loud to measure over.
pub fn threshold(reference: Floor) -> Result<f32, TooLoud> {
    if reference.peak > MAX_FLOOR {
        return Err(TooLoud { peak: reference.peak });
    }
    Ok((reference.peak * FLOOR_FACTOR).max(MIN_THRESHOLD))
}

/// Whether the window before a send (`pre`) is quiet enough to measure over: a window louder
/// than [`MAX_FLOOR`] is a release still sounding (or something else playing). A window merely
/// above the floor is measured with a raised threshold ([`levels`]).
pub fn not_sounding(pre: Floor) -> Result<(), TooLoud> {
    if pre.peak > MAX_FLOOR {
        return Err(TooLoud { peak: pre.peak });
    }
    Ok(())
}

/// The threshold and the refinement's guard of a send whose pre-send window peaked at `pre`,
/// over the noise floor `floor`: the floor's threshold, raised over a pre-send window more than
/// 6 dB above the floor (a release still sounding must not count as the onset).
pub fn levels(floor: f32, pre: f32) -> Result<(f32, f32), TooLoud> {
    let base = threshold(Floor { peak: floor, rms: 0.0 })?;
    let raised = pre > floor * QUIET_FACTOR;
    let threshold = if raised { base.max(pre * FLOOR_FACTOR) } else { base };
    let level = if raised { pre } else { floor };
    Ok((threshold, (level * QUIET_FACTOR).min(threshold)))
}

/// The first sample at or after `from` whose envelope exceeds `threshold`.
#[must_use]
pub fn first_crossing(samples: &[f32], from: usize, w: usize, threshold: f32) -> Option<usize> {
    (from..samples.len()).find(|&n| envelope_at(samples, n, w) > threshold)
}

/// An onset found on the envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    /// The first crossing of the floor threshold.
    pub crossing: usize,
    /// The first crossing of [`REFINE_FRACTION`] of the attack's peak: the onset.
    pub at: usize,
    /// The envelope's peak in the [`PEAK_MS`] after the threshold crossing.
    pub peak: f32,
}

impl Onset {
    /// The detector's uncertainty: the distance between the two crossings, in samples.
    #[must_use]
    pub fn uncertainty(&self) -> usize {
        self.at.abs_diff(self.crossing)
    }
}

/// Refine the threshold `crossing` of `samples`: the envelope's peak over the `peak_len` samples
/// from it, then the first sample from `from` on whose envelope exceeds both
/// [`REFINE_FRACTION`] of that peak and `guard` (a level the floor stays under).
#[must_use]
pub fn refine(samples: &[f32], from: usize, crossing: usize, w: usize, peak_len: usize, guard: f32) -> Onset {
    let end = crossing.saturating_add(peak_len).min(samples.len());
    let peak = (crossing..end).map(|n| envelope_at(samples, n, w)).fold(0.0f32, f32::max);
    let level = (peak * REFINE_FRACTION).max(guard);
    let at = (from.min(crossing)..end).find(|&n| envelope_at(samples, n, w) > level).unwrap_or(crossing);
    Onset { crossing, at, peak }
}

/// A level in dBFS for messages ("-inf dBFS" for silence).
#[must_use]
pub fn dbfs(level: f32) -> String {
    if level > 0.0 { format!("{:.1} dBFS", 20.0 * level.log10()) } else { "-inf dBFS".to_string() }
}

/// The distribution of a run's latencies, in samples.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub mean: f64,
    pub median: f64,
    pub min: u64,
    pub max: u64,
    /// Sample standard deviation (0 for one trial).
    pub std_dev: f64,
    /// 1-based trial numbers further than [`OUTLIER_SIGMAS`] standard deviations from the mean.
    pub outliers: Vec<usize>,
}

/// Mean, median, min, max, standard deviation and outliers of `latencies` (None when empty).
#[must_use]
pub fn summarize(latencies: &[u64]) -> Option<Summary> {
    let (&min, &max) = (latencies.iter().min()?, latencies.iter().max()?);
    let n = latencies.len();
    let mean = latencies.iter().map(|&l| l as f64).sum::<f64>() / n as f64;
    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();
    let median = if n % 2 == 1 { sorted[n / 2] as f64 } else { (sorted[n / 2 - 1] + sorted[n / 2]) as f64 / 2.0 };
    let std_dev = if n > 1 { (latencies.iter().map(|&l| (l as f64 - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt() } else { 0.0 };
    let outliers = latencies
        .iter()
        .enumerate()
        .filter(|(_, l)| std_dev > 0.0 && (**l as f64 - mean).abs() > OUTLIER_SIGMAS * std_dev)
        .map(|(i, _)| i + 1)
        .collect();
    Some(Summary { n, mean, median, min, max, std_dev, outliers })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Deterministic noise in `-amplitude..=amplitude`.
    fn noise(n: usize, amplitude: f32, seed: u32) -> Vec<f32> {
        let mut x = seed.wrapping_mul(2_654_435_761).max(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32 * 2.0 - 1.0) * amplitude
            })
            .collect()
    }

    #[test]
    fn the_threshold_sits_above_the_noise_floor_and_the_onset_is_refined_on_the_attack() {
        let w = window(48_000);
        assert_eq!(w, 12);
        let reference = noise_floor(&noise(4800, 0.004, 7), w);
        assert!(reference.peak > 0.0005 && reference.peak < 0.004, "{reference:?}");
        assert!(reference.rms > 0.001 && reference.rms < 0.004, "{reference:?}");
        let level = threshold(reference).unwrap();
        assert!((level - reference.peak * FLOOR_FACTOR).abs() < 1e-6);
        // The same noise continues, then a note whose attack rises linearly over 200 samples from 300.
        let mut trial = noise(2000, 0.004, 9);
        for (i, x) in trial.iter_mut().enumerate().skip(300) {
            *x += 0.5 * ((i - 300) as f32 / 200.0).min(1.0) * if i.is_multiple_of(2) { 1.0 } else { -1.0 };
        }
        not_sounding(noise_floor(&trial[..300], w)).unwrap();
        assert_eq!(first_crossing(&trial[..300], 0, w, level), None, "the noise alone never crosses");
        let crossing = first_crossing(&trial, 0, w, level).unwrap();
        assert!((300..330).contains(&crossing), "threshold crossing at {crossing}");
        let onset = refine(&trial, 0, crossing, w, 960, level / 2.0);
        // The envelope peaks at 0.5 (plus noise); 10 percent of it is reached 20 samples into the
        // ramp, plus the half window the causal envelope lags.
        assert!((onset.peak - 0.5).abs() < 0.01, "{onset:?}");
        assert!((320..334).contains(&onset.at), "{onset:?}");
        assert!(onset.at >= 300, "never before the sound starts");
        assert_eq!(onset.uncertainty(), onset.at - crossing);
    }

    #[test]
    fn a_fast_attack_lands_within_one_window_of_its_start() {
        let w = window(48_000);
        let mut trial = noise(1000, 0.002, 3);
        for (i, x) in trial.iter_mut().enumerate().skip(500) {
            *x = if i.is_multiple_of(2) { 0.5 } else { -0.5 };
        }
        let level = threshold(noise_floor(&trial[..400], w)).unwrap();
        let crossing = first_crossing(&trial, 0, w, level).unwrap();
        let onset = refine(&trial, 0, crossing, w, 960, level / 2.0);
        assert_eq!(crossing, 500);
        assert!((500..500 + w).contains(&onset.at), "{onset:?}");
    }

    #[test]
    fn a_silent_floor_still_needs_a_real_signal_and_a_loud_one_is_refused() {
        assert_eq!(threshold(noise_floor(&[0.0; 64], 4)).unwrap(), MIN_THRESHOLD);
        assert_eq!(threshold(noise_floor(&[], 4)).unwrap(), MIN_THRESHOLD);
        assert_eq!(first_crossing(&[0.0005, -0.0009, 0.0011], 0, 1, MIN_THRESHOLD), Some(2));
        let loud = threshold(noise_floor(&noise(480, 0.3, 3), 4)).unwrap_err();
        assert!(loud.peak > MAX_FLOOR, "{loud:?}");
        not_sounding(noise_floor(&noise(480, 0.02, 6), 4)).unwrap();
        let ringing = not_sounding(noise_floor(&noise(480, 0.4, 6), 4)).unwrap_err();
        assert!(ringing.peak > MAX_FLOOR, "{ringing:?}");
        assert_eq!(first_crossing(&[0.0, f32::NAN], 0, 1, 0.5), Some(1), "a broken sample is not silence");
        assert_eq!(dbfs(0.0), "-inf dBFS");
        assert_eq!(dbfs(1.0), "0.0 dBFS");
    }

    #[test]
    fn the_threshold_rises_over_a_ringing_pre_send_window_only() {
        let (quiet, _) = levels(0.0005, 0.0006).unwrap();
        assert_eq!(quiet, 0.002, "the floor's threshold");
        let (ringing, guard) = levels(0.0005, 0.004).unwrap();
        assert_eq!((ringing, guard), (0.016, 0.008));
        assert!(levels(0.2, 0.0).is_err(), "a floor above -20 dBFS");
    }

    #[test]
    fn the_summary_lists_mean_median_spread_and_outliers() {
        let m = summarize(&[100, 102, 98, 100, 101, 99, 100, 140, 100, 100]).unwrap();
        assert_eq!((m.n, m.min, m.max, m.median), (10, 98, 140, 100.0));
        assert!((m.mean - 104.0).abs() < 1e-9);
        assert!(m.std_dev > 12.0 && m.std_dev < 13.0, "{m:?}");
        assert_eq!(m.outliers, vec![8]);
        let one = summarize(&[5]).unwrap();
        assert_eq!((one.std_dev, one.outliers.len(), one.median), (0.0, 0, 5.0));
        assert_eq!(summarize(&[1, 2]).unwrap().median, 1.5);
        assert_eq!(summarize(&[955, 954, 956, 955, 990, 955, 955, 954]).unwrap().median, 955.0, "the median resists a stray note");
        assert!(summarize(&[]).is_none());
    }
}
