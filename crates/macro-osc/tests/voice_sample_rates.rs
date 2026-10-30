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
