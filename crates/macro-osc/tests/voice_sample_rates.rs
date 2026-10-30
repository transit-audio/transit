//! The voice at every supported sample rate plays each note at the frequency it plays at 48 kHz
//! (#195). The oracle renders at 48 kHz only, so the other rates are checked for pitch: a
//! free-running two-operator FM voice with no modulation (a sine) is rendered for two seconds and
//! its frequency measured from the first and last rising zero crossings (interpolated between
//! samples), which must be within 1 cent of the firmware's 48 kHz pitch, the note's equal-tempered
//! frequency scaled by 48000 / 47872.34 (the firmware computes pitch against its corrected rate).

#![forbid(unsafe_code)]

use transit_macro_osc::SampleRate;
use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};

/// The engine index of the two-operator FM engine.
const FM: usize = 10;

/// The firmware's pitch for `note` at 48 kHz, in hertz.
fn expected_hz(note: f32) -> f64 {
    440.0 * 2.0_f64.powf((f64::from(note) - 69.0) / 12.0) * (48_000.0 / f64::from(transit_macro_osc::CORRECTED_SAMPLE_RATE))
}

/// The frequency of the voice's main output for `note` at `rate`, measured over two seconds.
fn measured_hz(rate: SampleRate, note: f32) -> f64 {
    let mut voice = Voice::new(rate);
    voice.init();
    let patch = Patch { note, harmonics: 0.5, timbre: 0.0, morph: 0.5, engine: FM, ..Patch::default() };
    let modulations = Modulations::default();
    let frames = (2.0 * rate.hz()) as usize / 12 * 12;
    let mut out = Vec::with_capacity(frames);
    let mut block = [Frame::default(); 12];
    for _ in 0..frames / 12 {
        voice.render(&patch, &modulations, &mut block);
        out.extend(block.iter().map(|f| f64::from(f.out)));
    }
    // Skip the first 50 ms (the engine's parameter smoothing), then remove the DC offset.
    let out = &out[(0.05 * f64::from(rate.hz())) as usize..];
    let mean = out.iter().sum::<f64>() / out.len() as f64;
    let crossings: Vec<f64> = out
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0] - mean < 0.0 && w[1] - mean >= 0.0)
        .map(|(i, w)| i as f64 + (mean - w[0]) / (w[1] - w[0]))
        .collect();
    assert!(crossings.len() > 100, "{rate:?} note {note}: {} crossings", crossings.len());
    let periods = (crossings.len() - 1) as f64;
    periods / ((crossings[crossings.len() - 1] - crossings[0]) / f64::from(rate.hz()))
}

#[test]
fn every_rate_plays_the_firmware_pitch_within_one_cent() {
    for rate in SampleRate::ALL {
        for note in [45.0, 69.0, 93.0] {
            let (measured, expected) = (measured_hz(rate, note), expected_hz(note));
            let cents = 1200.0 * (measured / expected).log2();
            println!("{rate:?} note {note}: {measured:.4} Hz, expected {expected:.4} Hz, {cents:+.4} cents");
            assert!(cents.abs() < 1.0, "{rate:?} note {note}: {measured} Hz against {expected} Hz ({cents:+.3} cents)");
        }
    }
}

#[test]
fn rates_round_trip_through_hertz() {
    for rate in SampleRate::ALL {
        assert_eq!(SampleRate::from_hz(rate.hz()), Some(rate));
    }
    assert_eq!(SampleRate::from_hz(22_050.0), None);
    assert_eq!(SampleRate::default(), SampleRate::Hz48000);
}

/// The main output of a free-running voice on `engine` at note 69 and `rate`: 2 s, the first 0.5 s
/// skipped (smoothing, the six-op envelope settling), DC removed.
fn render_free(rate: SampleRate, engine: usize, harmonics: f32) -> Vec<f64> {
    let mut voice = Voice::new(rate);
    let patch = Patch { note: 69.0, harmonics, timbre: 0.5, morph: 0.5, engine, ..Patch::default() };
    let modulations = Modulations::default();
    let frames = (2.0 * rate.hz()) as usize / 12 * 12;
    let mut out = Vec::with_capacity(frames);
    let mut block = [Frame::default(); 12];
    for _ in 0..frames / 12 {
        voice.render(&patch, &modulations, &mut block);
        out.extend(block.iter().map(|f| f64::from(f.out)));
    }
    let out = out.split_off((0.5 * f64::from(rate.hz())) as usize);
    let mean = out.iter().sum::<f64>() / out.len() as f64;
    out.into_iter().map(|x| x - mean).collect()
}

/// The fundamental of `x` in hertz, from the normalized autocorrelation peak within a quarter
/// octave of `around_hz`, refined by a parabola through the peak and its neighbors (the waveform
/// need not be a sine).
fn autocorrelation_hz(x: &[f64], rate: f32, around_hz: f64) -> f64 {
    let rate = f64::from(rate);
    let period = rate / around_hz;
    let (lo, hi) = ((period / 1.19) as usize, (period * 1.19) as usize + 2);
    let r = |lag: usize| {
        let n = x.len() - lag;
        x[..n].iter().zip(&x[lag..]).map(|(a, b)| a * b).sum::<f64>() / n as f64
    };
    let best = (lo..=hi).max_by(|&a, &b| r(a).total_cmp(&r(b))).expect("lags");
    assert!(best > lo && best < hi, "peak at the edge of the search ({lo}..{hi}): {best}");
    let (a, b, c) = (r(best - 1), r(best), r(best + 1));
    let offset = 0.5 * (a - c) / (a - 2.0 * b + c);
    rate / (best as f64 + offset)
}

/// The two pitch paths besides `a0`: the six-op engine computes against the corrected rate scaled by
/// rate / 48000, and LPC speech (harmonics 0.35: the vowel table scanned by morph, free-running, so
/// stationary) its pitch shift against the same corrected rate. At 96 kHz each plays within 1 cent of
/// its 48 kHz pitch.
#[test]
fn six_op_and_speech_keep_their_48_khz_pitch_at_96_khz() {
    for (engine, harmonics, label) in [(2, 0.0, "six-op bank A"), (15, 0.35, "LPC speech")] {
        let at_48 = autocorrelation_hz(&render_free(SampleRate::Hz48000, engine, harmonics), 48_000.0, expected_hz(69.0));
        let at_96 = autocorrelation_hz(&render_free(SampleRate::Hz96000, engine, harmonics), 96_000.0, at_48);
        let cents = 1200.0 * (at_96 / at_48).log2();
        println!("{label}: {at_48:.4} Hz at 48 kHz, {at_96:.4} Hz at 96 kHz, {cents:+.4} cents");
        assert!(cents.abs() < 1.0, "{label}: {at_48} Hz at 48 kHz, {at_96} Hz at 96 kHz ({cents:+.3} cents)");
    }
}
