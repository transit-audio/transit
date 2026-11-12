//! The published response (ADR 0054 decision 8): pure functions over stored band values, the sample
//! rate and the phase mode, computed with the engine's own design code, so the curve is exactly what
//! the engine converges to once smoothing settles. The panel calls them on the UI thread, one
//! frequency per pixel column.

use crate::design::{self, Used};
use crate::math::{self, TAU};
use crate::{BANDS, BandParams, CurveChannel, PhaseMode, sane_rate};

/// The highest frequency a response is given for, as a fraction of the rate (decision 12): above it a
/// digital response would draw a mirror image.
const TOP: f64 = 0.49;

fn designed(b: &BandParams, fs: f64) -> design::Design {
    design::design(b.kind, b.slope, Used::from_stored(b.freq_hz, b.gain_db, b.q, fs), fs)
}

/// Writes `|H|²` in dB of the product of `designs` at each frequency of `freqs` into `out`. A
/// frequency above 0.49 of the rate, a negative frequency and every non-finite frequency (NaN and
/// both infinities) get NaN (no value: the panel draws nothing there; decision 8's contract). Zero
/// and exactly 0.49 of the rate are valid.
fn evaluate(designs: &[design::Design], fs: f64, freqs: &[f64], out: &mut [f64]) {
    for (o, &f) in out.iter_mut().zip(freqs) {
        // Inside [0, top] or no value; a comparison with NaN is false, so NaN lands here too.
        if !(0.0..=TOP * fs).contains(&f) {
            *o = f64::NAN;
            continue;
        }
        let half = 0.5 * TAU * f / fs;
        let (s, co) = (math::sin(half), math::cos(half));
        let mut p = 1.0;
        for d in designs {
            p *= d.power(s, co);
        }
        *o = math::power_to_db(p);
    }
}

/// One band's own response in dB at each frequency (Zero latency: the matched sections' magnitude),
/// whether or not the band is used or on. A frequency outside 0 to 0.49 of the rate, or not finite, gets NaN.
pub fn band_response_db(band: &BandParams, sample_rate: f64, mode: PhaseMode, freqs: &[f64], out: &mut [f64]) {
    let PhaseMode::ZeroLatency = mode;
    let fs = sane_rate(sample_rate);
    evaluate(&[designed(band, fs)], fs, freqs, out);
}

/// The composite response in dB of the used and on bands on `channel`'s path (decision 8) at each
/// frequency. A frequency outside 0 to 0.49 of the rate, or not finite, gets NaN.
pub fn response_db(bands: &[BandParams], sample_rate: f64, mode: PhaseMode, channel: CurveChannel, freqs: &[f64], out: &mut [f64]) {
    let PhaseMode::ZeroLatency = mode;
    let fs = sane_rate(sample_rate);
    let mut designs = [design::Design::identity(); BANDS];
    let mut n = 0;
    for b in bands.iter().take(BANDS) {
        if b.audible() && channel.includes(b.place) {
            designs[n] = designed(b, fs);
            n += 1;
        }
    }
    evaluate(&designs[..n], fs, freqs, out);
}
