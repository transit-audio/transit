//! The Tuner (#266): the device leaves the audio alone; the model behind its panel reads a tone.

use super::*;
use crate::{Plugin, plugin_info, plugins};

fn sine(hz: f32, sr: f32, n: usize, amp: f32) -> Vec<f32> {
    (0..n).map(|i| amp * (std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr)).sin() as f32).collect()
}

fn pass_through(frames: usize, channels: usize) {
    let mut t = Tuner::new();
    t.prepare(48_000.0, 512, channels);
    let src: Vec<Vec<f32>> = (0..channels).map(|c| sine(330.0 + 55.0 * c as f32, 48_000.0, frames, 0.7)).collect();
    let mut io = src.clone();
    t.process(&mut io, frames);
    for (a, b) in io.iter().zip(&src) {
        assert!(a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()), "{frames} frames, {channels} channels");
    }
    assert_eq!(t.latency(), 0);
    assert_eq!(t.tail_samples(), 0);
}

#[test]
fn audio_passes_bit_for_bit_with_no_latency() {
    for frames in [64, 512] {
        for channels in [1, 2] {
            pass_through(frames, channels);
        }
    }
}

#[test]
fn a_parameter_change_does_not_touch_the_audio() {
    let mut t = Tuner::new();
    t.prepare(44_100.0, 64, 2);
    assert!(t.set_param("reference_hz", 432.0));
    assert!(t.set_param("hold", 1.0));
    let src = vec![sine(200.0, 44_100.0, 64, 0.3), sine(300.0, 44_100.0, 64, 0.3)];
    let mut io = src.clone();
    t.process(&mut io, 64);
    assert_eq!(io, src);
}

#[test]
fn parameters_have_their_defaults_and_ranges() {
    let t = Tuner::new();
    assert_eq!(t.param("reference_hz"), Some(440.0));
    assert_eq!(t.param("hold"), Some(0.0));
    let info = plugin_info("tuner").expect("the registry lists the tuner");
    assert_eq!((info.name, info.short_name), ("Tuner", "Tuner"));
    assert!(!info.is_instrument);
    let r = info.param("reference_hz").unwrap();
    assert_eq!((r.min, r.max, r.unit), (415.0, 466.0, Unit::Hz));
    assert_eq!(info.params.len(), 2);
    let mut t = Tuner::new();
    t.set_param("reference_hz", 5000.0);
    assert_eq!(t.param("reference_hz"), Some(466.0));
    assert!(t.wants_sample_tap());
}

#[test]
fn the_registry_lists_the_tuner() {
    assert!(plugins().iter().any(|p| p.id == "tuner"));
    assert_eq!(plugins::tuner::TUNER_INFO.id, "tuner");
}

#[test]
fn a_440_hz_tone_reads_a4_in_tune() {
    let sr = 48_000.0;
    let mut m = TunerModel::new(sr);
    let n = m.window_mut().len();
    m.window_mut().copy_from_slice(&sine(440.0, sr, n, 0.5));
    m.analyze(440.0, false);
    let r = m.reading().expect("a reading");
    assert_eq!((r.note, signed_tenths(r.cents)), (69, "+0.0".to_string()));
}

#[test]
fn a_flat_tone_reads_its_cents_and_the_reference_moves_the_reading() {
    let sr = 44_100.0;
    let mut m = TunerModel::new(sr);
    let n = m.window_mut().len();
    let hz = 440.0 * 2.0f32.powf(-20.0 / 1200.0);
    m.window_mut().copy_from_slice(&sine(hz, sr, n, 0.5));
    m.analyze(440.0, false);
    let r = m.reading().unwrap();
    assert_eq!(r.note, 69);
    assert!((r.cents + 20.0).abs() < 0.5, "{}", r.cents);
    // Against a reference of 432 Hz the same tone is sharp.
    let mut m2 = TunerModel::new(sr);
    m2.window_mut().copy_from_slice(&sine(hz, sr, n, 0.5));
    m2.analyze(432.0, false);
    assert!(m2.reading().unwrap().cents > 0.0);
}

#[test]
fn quiet_input_is_no_signal_and_the_display_lets_go_after_a_few_misses() {
    let sr = 48_000.0;
    let mut m = TunerModel::new(sr);
    let n = m.window_mut().len();
    m.window_mut().copy_from_slice(&sine(440.0, sr, n, 0.5));
    m.analyze(440.0, false);
    assert!(m.reading().is_some());
    // -60 dBFS RMS: a clean sine, but under the gate.
    m.window_mut().copy_from_slice(&sine(440.0, sr, n, 0.001 * std::f32::consts::SQRT_2));
    m.analyze(440.0, false);
    m.analyze(440.0, false);
    assert!(m.reading().is_some(), "two misses keep the last note");
    m.analyze(440.0, false);
    assert_eq!(m.reading(), None, "the third lets go");
    // The same tone at -40 dBFS RMS is read.
    m.window_mut().copy_from_slice(&sine(440.0, sr, n, 0.01 * std::f32::consts::SQRT_2));
    m.analyze(440.0, false);
    assert!(m.reading().is_some());
}

#[test]
fn hold_freezes_the_reading() {
    let sr = 48_000.0;
    let mut m = TunerModel::new(sr);
    let n = m.window_mut().len();
    m.window_mut().copy_from_slice(&sine(440.0, sr, n, 0.5));
    m.analyze(440.0, false);
    let held = m.reading();
    m.window_mut().copy_from_slice(&sine(220.0, sr, n, 0.5));
    for _ in 0..8 {
        m.analyze(440.0, true);
    }
    assert_eq!(m.reading(), held);
    m.analyze(440.0, false);
    assert_eq!(m.reading().unwrap().note, 57);
}

#[test]
fn a_new_note_replaces_the_median_at_once() {
    let sr = 48_000.0;
    let mut m = TunerModel::new(sr);
    let n = m.window_mut().len();
    m.window_mut().copy_from_slice(&sine(440.0, sr, n, 0.5));
    for _ in 0..5 {
        m.analyze(440.0, false);
    }
    m.window_mut().copy_from_slice(&sine(329.63, sr, n, 0.5));
    m.analyze(440.0, false);
    assert_eq!(m.reading().unwrap().note, 64, "no averaging with the old note");
}

#[test]
fn cents_read_to_one_decimal_with_no_negative_zero() {
    assert_eq!(signed_tenths(-0.04), "+0.0");
    assert_eq!(signed_tenths(12.34), "+12.3");
    assert_eq!(signed_tenths(-7.86), "-7.9");
}

#[test]
fn the_registry_helper_agrees_with_every_plugin_about_wanting_a_sample_tap() {
    for info in plugins() {
        let asks = crate::create(info.id).is_some_and(|p| p.wants_sample_tap());
        assert_eq!(crate::wants_sample_tap(info.id), asks, "{}", info.id);
    }
    assert!(crate::wants_sample_tap("tuner") && !crate::wants_sample_tap("gain") && !crate::wants_sample_tap("clap:x"));
}

#[test]
fn rounds_with_no_new_frames_are_misses_and_three_let_go_of_the_note() {
    let mut m = TunerModel::new(48_000.0);
    let w = sine(440.0, 48_000.0, m.window_mut().len(), 0.5);
    m.window_mut().copy_from_slice(&w);
    m.analyze(440.0, false);
    assert_eq!(m.reading().map(|r| (r.note, signed_tenths(r.cents))), Some((69, "+0.0".to_string())));
    m.miss();
    m.miss();
    assert!(m.reading().is_some(), "two misses keep the note");
    m.miss();
    assert!(m.reading().is_none(), "the third lets go");
    // A new tone starts the median over from nothing.
    m.analyze(440.0, false);
    assert_eq!(m.reading().map(|r| (r.note, signed_tenths(r.cents))), Some((69, "+0.0".to_string())));
}
