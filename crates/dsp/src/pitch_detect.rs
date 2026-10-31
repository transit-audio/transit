//! Monophonic pitch tracking (YIN, de Cheveigné & Kawahara 2002) and note segmentation, for
//! audio-to-MIDI conversion.

/// A detected note: start and end in samples, MIDI pitch, and a velocity estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectedNote {
    pub start: usize,
    pub end: usize,
    pub pitch: u8,
    pub velocity: u8,
}

/// Fundamental frequency of one frame, or None when unvoiced. `threshold` ≈ 0.1–0.2.
pub fn yin(frame: &[f32], sample_rate: f32, min_hz: f32, max_hz: f32, threshold: f32) -> Option<f32> {
    if !(sample_rate.is_finite() && sample_rate > 0.0) || frame.len() < 64 {
        return None;
    }
    let max_tau = ((sample_rate / min_hz.max(20.0)) as usize).min(frame.len() / 2);
    let min_tau = ((sample_rate / max_hz.max(min_hz + 1.0)) as usize).max(2);
    if max_tau <= min_tau.saturating_add(2) {
        return None;
    }
    let w = frame.len() - max_tau;
    let mut d = vec![0.0f32; max_tau + 1];
    for (tau, dv) in d.iter_mut().enumerate().skip(1) {
        let mut sum = 0.0f32;
        for j in 0..w {
            let (Some(a), Some(b)) = (frame.get(j), frame.get(j + tau)) else { break };
            let diff = a - b;
            sum += diff * diff;
        }
        *dv = sum;
    }
    // Cumulative mean normalised difference.
    let mut cmnd = vec![1.0f32; max_tau + 1];
    let mut running = 0.0f32;
    for tau in 1..=max_tau {
        let v = d.get(tau).copied().unwrap_or(0.0);
        running += v;
        if let Some(c) = cmnd.get_mut(tau) {
            *c = if running > 0.0 { v * tau as f32 / running } else { 1.0 };
        }
    }
    let mut tau = min_tau;
    while tau < max_tau {
        let c = cmnd.get(tau).copied().unwrap_or(1.0);
        if c < threshold {
            // Walk to the local minimum.
            while tau + 1 < max_tau && cmnd.get(tau + 1).copied().unwrap_or(1.0) < cmnd.get(tau).copied().unwrap_or(1.0) {
                tau += 1;
            }
            // Parabolic interpolation.
            let (a, b, c) =
                (cmnd.get(tau - 1).copied().unwrap_or(1.0), cmnd.get(tau).copied().unwrap_or(1.0), cmnd.get(tau + 1).copied().unwrap_or(1.0));
            let denom = a - 2.0 * b + c;
            let shift = if denom.abs() > 1e-9 { 0.5 * (a - c) / denom } else { 0.0 };
            let t = tau as f32 + shift.clamp(-1.0, 1.0);
            return (t > 0.0).then_some(sample_rate / t);
        }
        tau += 1;
    }
    None
}

/// Convert mono audio to notes: frames of `hop` samples, pitch per frame, then runs of the same
/// semitone at least `min_frames` long become notes.
pub fn audio_to_notes(audio: &[f32], sample_rate: f32) -> Vec<DetectedNote> {
    let sr = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
    let win = ((sr * 0.046) as usize).clamp(256, 8192);
    let hop = win / 4;
    let min_frames = 3;
    let mut frames: Vec<(usize, Option<u8>, f32)> = Vec::new();
    let mut pos = 0usize;
    while pos + win <= audio.len() && frames.len() < 2_000_000 {
        let Some(frame) = audio.get(pos..pos + win) else { break };
        let rms = (frame.iter().map(|x| x * x).sum::<f32>() / win as f32).sqrt();
        let pitch = if rms > 0.01 {
            yin(frame, sr, 50.0, 1600.0, 0.15).map(|f| (69.0 + 12.0 * (f / 440.0).log2()).round().clamp(0.0, 127.0) as u8)
        } else {
            None
        };
        frames.push((pos, pitch, rms));
        pos += hop;
    }
    let mut notes = Vec::new();
    let mut i = 0;
    while i < frames.len() {
        let Some((start, Some(p), _)) = frames.get(i).copied() else {
            i += 1;
            continue;
        };
        let mut j = i;
        let mut peak = 0.0f32;
        while let Some((_, Some(q), rms)) = frames.get(j).copied() {
            if q != p {
                break;
            }
            peak = peak.max(rms);
            j += 1;
        }
        if j - i >= min_frames {
            let end = frames.get(j).map_or(audio.len(), |f| f.0);
            let velocity = (40.0 + 87.0 * (peak / 0.5).min(1.0)) as u8;
            notes.push(DetectedNote { start, end, pitch: p, velocity: velocity.clamp(1, 127) });
        }
        i = j.max(i + 1);
    }
    notes
}

/// What [`PitchDetector::detect`] found: the fundamental and how sure the detector is of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub hz: f32,
    /// Height of the chosen peak of the normalized autocorrelation, 0..1 (1 is a pure periodic tone).
    pub clarity: f32,
}

/// The clarity under which an estimate is rejected.
pub const DEFAULT_CLARITY: f32 = 0.8;
/// The octave-error guard: the lowest lag whose peak is within this fraction of the highest wins.
const PEAK_CUTOFF: f32 = 0.9;

/// The window the Tuner analyzes, in samples: about 85 ms, a power of two.
#[must_use]
pub fn window_for(sample_rate: f32) -> usize {
    if sample_rate > 70_000.0 { 8192 } else { 4096 }
}

/// McLeod pitch method (McLeod & Wyvill 2005) for the Tuner: the normalized square difference
/// function of one window, computed through the FFT so a detection stays cheap, with the key
/// maxima picked by the lowest-lag rule and a parabolic interpolation of the chosen peak.
///
/// Everything it needs is allocated in [`PitchDetector::new`]; `detect` allocates nothing. It runs
/// on the UI thread, never in the audio callback.
pub struct PitchDetector {
    sample_rate: f32,
    win: usize,
    fwd: std::sync::Arc<dyn rustfft::Fft<f32>>,
    inv: std::sync::Arc<dyn rustfft::Fft<f32>>,
    buf: Vec<rustfft::num_complex::Complex<f32>>,
    scratch: Vec<rustfft::num_complex::Complex<f32>>,
    /// The NSDF for lags `0..=win / 2`.
    nsdf: Vec<f32>,
}

impl PitchDetector {
    /// A detector with the window [`window_for`] gives `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self::with_window(sample_rate, window_for(sample_rate))
    }

    /// A detector with a window of `window` samples (rounded up to a power of two, 512 to 16384).
    #[must_use]
    pub fn with_window(sample_rate: f32, window: usize) -> Self {
        let win = window.clamp(512, 16_384).next_power_of_two();
        let sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        let mut planner = rustfft::FftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(win * 2);
        let inv = planner.plan_fft_inverse(win * 2);
        let zero = rustfft::num_complex::Complex::new(0.0, 0.0);
        let scratch = vec![zero; fwd.get_inplace_scratch_len().max(inv.get_inplace_scratch_len())];
        PitchDetector { sample_rate, win, fwd, inv, buf: vec![zero; win * 2], scratch, nsdf: vec![0.0; win / 2 + 1] }
    }

    /// Samples of input one detection reads (the newest `window` of a longer slice).
    #[must_use]
    pub fn window(&self) -> usize {
        self.win
    }

    /// The fundamental of the newest [`PitchDetector::window`] samples of `x`, or `None` when there
    /// is no clear pitch: too short an input, silence, noise, or a peak under `min_clarity`.
    pub fn detect(&mut self, x: &[f32], min_clarity: f32) -> Option<Estimate> {
        let win = self.win;
        let x = x.get(x.len().checked_sub(win)?..)?;
        let mean = x.iter().sum::<f32>() / win as f32;
        if !mean.is_finite() {
            return None;
        }
        for (b, s) in self.buf.iter_mut().zip(x.iter().map(|s| s - mean).chain(std::iter::repeat(0.0))) {
            *b = rustfft::num_complex::Complex::new(s, 0.0);
        }
        self.fwd.process_with_scratch(&mut self.buf, &mut self.scratch);
        for b in &mut self.buf {
            *b = rustfft::num_complex::Complex::new(b.norm_sqr(), 0.0);
        }
        self.inv.process_with_scratch(&mut self.buf, &mut self.scratch);
        // r(tau) = buf[tau].re / (2 * win); m(tau) = sum of x[j]^2 + x[j + tau]^2 over the overlap.
        let scale = 1.0 / (2 * win) as f32;
        let mut m: f64 = 2.0 * x.iter().map(|s| f64::from((s - mean) * (s - mean))).sum::<f64>();
        if !(m > 1e-12) {
            return None;
        }
        let at = |i: usize| x.get(i).map_or(0.0, |s| f64::from((s - mean) * (s - mean)));
        for tau in 0..self.nsdf.len() {
            if tau > 0 {
                m -= at(tau - 1) + at(win - tau);
            }
            let r = self.buf.get(tau).map_or(0.0, |c| c.re) * scale;
            if let Some(n) = self.nsdf.get_mut(tau) {
                *n = if m > 1e-12 { (2.0 * f64::from(r) / m) as f32 } else { 0.0 };
            }
        }
        self.pick(min_clarity)
    }

    /// Picks the key maximum: the lowest lag within [`PEAK_CUTOFF`] of the highest, interpolated.
    fn pick(&self, min_clarity: f32) -> Option<Estimate> {
        let n = &self.nsdf;
        let last = n.len() - 1;
        let v = |i: usize| n.get(i).copied().unwrap_or(0.0);
        // Past the lobe around lag 0: the first negative-going zero crossing.
        let mut start = 1;
        while start < last && v(start) > 0.0 {
            start += 1;
        }
        let mut top = 0.0f32;
        let mut from = start;
        while let Some((_, y, next)) = next_peak(n, from) {
            top = top.max(y);
            from = next;
        }
        if top <= 0.0 {
            return None;
        }
        let mut from = start;
        let lag = loop {
            let (lag, y, next) = next_peak(n, from)?;
            if y >= PEAK_CUTOFF * top {
                break lag;
            }
            from = next;
        };
        let (y0, y1, y2) = (v(lag - 1), v(lag), v(lag + 1));
        let denom = y0 - 2.0 * y1 + y2;
        let shift = if denom < -1e-9 { (0.5 * (y0 - y2) / denom).clamp(-1.0, 1.0) } else { 0.0 };
        let clarity = (y1 - 0.25 * (y0 - y2) * shift).min(1.0);
        let period = lag as f32 + shift;
        (clarity >= min_clarity && period > 0.0).then(|| Estimate { hz: self.sample_rate / period, clarity })
    }
}

/// The highest point of the next positive region of `n` at or after `from`, as (lag, value, where
/// to look next). A region cut off by the end of the lags has no peak to interpolate and is skipped.
fn next_peak(n: &[f32], from: usize) -> Option<(usize, f32, usize)> {
    let last = n.len().checked_sub(1)?;
    let v = |i: usize| n.get(i).copied().unwrap_or(0.0);
    let mut t = from.max(1);
    while t < last && v(t) <= 0.0 {
        t += 1;
    }
    let mut best = t;
    while t < last && v(t) > 0.0 {
        if v(t) > v(best) {
            best = t;
        }
        t += 1;
    }
    (t < last && v(best) > 0.0).then(|| (best, v(best), t))
}

/// The nearest note to `hz` and how far off it is: MIDI note number (A4 = 69 at `reference_hz`)
/// and cents, -50 to +50. `None` for a frequency that is not positive and finite.
#[must_use]
pub fn nearest_note(hz: f32, reference_hz: f32) -> Option<(i32, f32)> {
    if !(hz.is_finite() && hz > 0.0 && reference_hz.is_finite() && reference_hz > 0.0) {
        return None;
    }
    let semis = 69.0 + 12.0 * f64::from(hz / reference_hz).log2();
    let note = semis.round();
    Some((note as i32, ((semis - note) * 100.0) as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(f: f32, secs: f32, sr: f32) -> Vec<f32> {
        (0..(secs * sr) as usize).map(|i| (i as f32 * f * std::f32::consts::TAU / sr).sin() * 0.5).collect()
    }

    #[test]
    fn yin_finds_a440() {
        let t = tone(440.0, 0.1, 48_000.0);
        let f = yin(&t[..2048], 48_000.0, 50.0, 1600.0, 0.15).unwrap();
        assert!((f - 440.0).abs() < 2.0, "{f}");
    }

    #[test]
    fn melody_to_notes() {
        let sr = 48_000.0;
        let mut a = tone(261.63, 0.4, sr);
        a.extend(vec![0.0; 4800]);
        a.extend(tone(392.0, 0.4, sr));
        let n = audio_to_notes(&a, sr);
        let pitches: Vec<u8> = n.iter().map(|x| x.pitch).collect();
        assert_eq!(pitches, vec![60, 67], "{n:?}");
    }

    #[test]
    fn silence_and_garbage() {
        assert!(audio_to_notes(&vec![0.0; 48_000], 48_000.0).is_empty());
        assert!(audio_to_notes(&[], f32::NAN).is_empty());
        assert!(yin(&[0.0; 10], 48_000.0, 50.0, 1600.0, 0.15).is_none());
    }

    #[test]
    fn huge_finite_rate_is_unvoiced() {
        // #241: the shortest lag saturates to usize::MAX; that is "no pitch", not an overflow.
        assert!(yin(&[0.0; 64], 1.0e30, 50.0, 1600.0, 0.15).is_none());
        assert!(yin(&[0.5; 8192], f32::MAX, 50.0, 1600.0, 0.15).is_none());
    }

    #[test]
    fn audio_to_notes_survives_huge_finite_rate() {
        assert!(audio_to_notes(&[0.5; 8192], 1.0e30).is_empty());
    }
}

#[cfg(test)]
#[path = "pitch_detect_tests.rs"]
mod detector_tests;
