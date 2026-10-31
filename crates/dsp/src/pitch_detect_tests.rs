//! The Tuner detector (#266): accuracy against sines, a sawtooth, a square, and the inputs that
//! must give no estimate. Cents are measured against the exact frequency of the generated tone.

use super::*;

fn cents(found: f32, want: f32) -> f32 {
    1200.0 * (found / want).log2()
}

fn sine(hz: f32, sr: f32, n: usize, amp: f32) -> Vec<f32> {
    (0..n).map(|i| amp * (std::f64::consts::TAU * f64::from(hz) * i as f64 / f64::from(sr)).sin() as f32).collect()
}

/// A sawtooth from its first partials (1/k), those under 0.45 of the rate.
fn saw(hz: f32, sr: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(sr);
            let sum: f64 = (1..=12)
                .filter(|k| f64::from(hz) * f64::from(*k) < f64::from(sr) * 0.45)
                .map(|k| (std::f64::consts::TAU * f64::from(hz) * f64::from(k) * t).sin() / f64::from(k))
                .sum();
            sum as f32 * 0.4
        })
        .collect()
}

fn square(hz: f32, sr: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let phase = (f64::from(hz) * i as f64 / f64::from(sr)).fract();
            if phase < 0.5 { 0.4 } else { -0.4 }
        })
        .collect()
}

#[test]
fn the_window_follows_the_rate() {
    assert_eq!(window_for(44_100.0), 4096);
    assert_eq!(window_for(48_000.0), 4096);
    assert_eq!(window_for(88_200.0), 8192);
    assert_eq!(window_for(96_000.0), 8192);
    assert_eq!(PitchDetector::new(96_000.0).window(), 8192);
    assert_eq!(PitchDetector::new(f32::NAN).window(), 4096);
}

#[test]
fn sines_from_55_to_3520_hz_are_within_a_cent_at_every_rate() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        let mut d = PitchDetector::new(sr);
        for hz in [55.0f32, 110.0, 440.0, 1760.0, 3520.0] {
            let x = sine(hz, sr, d.window() + 1000, 0.5);
            let e = d.detect(&x, DEFAULT_CLARITY).unwrap_or_else(|| panic!("no estimate for {hz} Hz at {sr}"));
            let c = cents(e.hz, hz);
            println!("table sine {hz} Hz at {sr}: {c:+.3} cents, clarity {:.4}", e.clarity);
            assert!(c.abs() < 1.0, "{hz} Hz at {sr}: {c} cents");
        }
    }
}

#[test]
fn a_low_bass_note_is_within_three_cents_with_the_larger_window() {
    let sr = 48_000.0;
    let mut d = PitchDetector::with_window(sr, 8192);
    let x = sine(41.2, sr, 8192, 0.5);
    let e = d.detect(&x, DEFAULT_CLARITY).unwrap();
    let c = cents(e.hz, 41.2);
    println!("table bass 41.2 Hz at 48000 (8192 window): {c:+.3} cents");
    assert!(c.abs() < 3.0, "{c}");
}

#[test]
fn the_real_bass_case_41_hz_at_48_khz_with_the_window_the_device_uses() {
    for sr in [44_100.0f32, 48_000.0] {
        let mut d = PitchDetector::new(sr);
        assert_eq!(d.window(), 4096, "the device's window at {sr}");
        for hz in [41.2f32, 41.0] {
            let x = sine(hz, sr, d.window(), 0.5);
            let e = d.detect(&x, DEFAULT_CLARITY).unwrap();
            let c = cents(e.hz, hz);
            println!("table bass {hz} Hz at {sr} (4096 window): {c:+.3} cents");
            assert!(c.abs() < 3.0, "{hz} at {sr}: {c}");
        }
    }
}

#[test]
fn a_detuned_sawtooth_and_square_have_no_octave_error() {
    let detune = 2.0f32.powf(15.0 / 1200.0);
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        let mut d = PitchDetector::new(sr);
        let n = d.window() + 500;
        let hz = 110.0 * detune;
        let e = d.detect(&saw(hz, sr, n), DEFAULT_CLARITY).unwrap();
        let c = cents(e.hz, hz);
        println!("table saw {hz} Hz at {sr}: {c:+.3} cents");
        assert!(c.abs() < 2.0, "saw at {sr}: {c} cents");
        let hz = 220.0 * detune;
        let e = d.detect(&square(hz, sr, n), DEFAULT_CLARITY).unwrap();
        let c = cents(e.hz, hz);
        println!("table square {hz} Hz at {sr}: {c:+.3} cents");
        assert!(c.abs() < 2.0, "square at {sr}: {c} cents");
    }
}

#[test]
fn silence_and_noise_give_no_estimate() {
    let mut d = PitchDetector::new(48_000.0);
    assert_eq!(d.detect(&[0.0; 4096], DEFAULT_CLARITY), None);
    let mut s = 0x1234_5678u32;
    let noise: Vec<f32> = (0..4096)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect();
    assert_eq!(d.detect(&noise, DEFAULT_CLARITY), None, "white noise is not a pitch");
}

#[test]
fn too_short_or_non_finite_input_gives_no_estimate() {
    let mut d = PitchDetector::new(48_000.0);
    assert_eq!(d.detect(&[], DEFAULT_CLARITY), None);
    assert_eq!(d.detect(&sine(440.0, 48_000.0, 100, 0.5), DEFAULT_CLARITY), None);
    let mut x = sine(440.0, 48_000.0, 4096, 0.5);
    x[7] = f32::NAN;
    assert_eq!(d.detect(&x, DEFAULT_CLARITY), None);
}

#[test]
fn the_newest_window_of_a_longer_input_is_read() {
    let mut d = PitchDetector::new(48_000.0);
    let mut x = sine(220.0, 48_000.0, 3000, 0.5);
    x.extend(sine(440.0, 48_000.0, 4096, 0.5));
    let e = d.detect(&x, DEFAULT_CLARITY).unwrap();
    assert!(cents(e.hz, 440.0).abs() < 1.0);
}

#[test]
fn nearest_note_names_a4_at_the_reference_and_follows_it() {
    assert_eq!(nearest_note(440.0, 440.0), Some((69, 0.0)));
    let (n, c) = nearest_note(442.0, 440.0).unwrap();
    assert_eq!(n, 69);
    assert!((c - 7.85).abs() < 0.05, "{c}");
    assert_eq!(nearest_note(415.0, 415.0), Some((69, 0.0)));
    let (n, c) = nearest_note(261.6256, 440.0).unwrap();
    assert_eq!(n, 60);
    assert!(c.abs() < 0.01);
    assert_eq!(nearest_note(0.0, 440.0), None);
    assert_eq!(nearest_note(f32::NAN, 440.0), None);
    assert_eq!(nearest_note(440.0, 0.0), None);
}
