//! Absolute pitch of every pitch path at every supported rate (#195, escape E63).
//!
//! The voice has three pitch paths: the common one (`note_to_frequency` against the voice's `a0`,
//! used by every engine but two), the six-op voices' own reference, and the LPC speech synth's own
//! 8 kHz clock and pitch shift. Each is measured free-running at note 69 against the frequency it
//! must play, with a measurement that cannot lock onto an alias:
//!
//! 1. the autocorrelation peak is searched within 3% of the expected period only, and must be a
//!    real periodicity (normalized correlation above 0.3);
//! 2. the signal must not also repeat at a half or a third of that lag (an octave or a twelfth up:
//!    a signal of period T/2 peaks at T too, which is how the slice 2 test passed an octave error);
//! 3. the frequency is then read from the spectrum (the strongest DFT peak within 1.5% of the
//!    autocorrelation's estimate; no lag grid, so LPC's 8 kHz clock cannot bias it) and must be
//!    within 1 cent.
//!
//! The firmware's tuning: the common and six-op paths play `440 * 48000 / 47872.34` for A4 at every
//! rate, as the firmware does at 48 kHz; LPC speech plays 440 (its clock assumes 48 kHz while its
//! pitch shift uses the corrected rate, so at a true 48 kHz the two cancel). Concert tuning: 440 on
//! every path.

#![forbid(unsafe_code)]

use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc::{CORRECTED_SAMPLE_RATE, REF_SAMPLE_RATE, SampleRate, Tuning};

/// (engine, harmonics, timbre, label, own pitch path). Every engine that reads its pitch through
/// the common path does so with `note_to_frequency(note, a0_normalized)`; a representative set is
/// measured, plus both engines with their own path.
const PATHS: [(usize, f32, f32, &str); 10] = [
    (0, 0.5, 0.5, "filtered analog"),
    (1, 0.5, 0.5, "phase distortion"),
    // A bank A patch whose carrier sounds at the note with no operator detune (harmonics picks the
    // patch: 15.5 / 32 / 1.02). Most of the bank sounds an octave below the note (0.5 carrier
    // ratios) or detuned by the patch's own fine tuning, as on the module.
    (2, 15.5 / 32.0 / 1.02, 0.5, "six-op bank A (own reference)"),
    (8, 0.0, 0.5, "virtual analog"),
    (9, 0.5, 0.5, "waveshaper"),
    (10, 0.5, 0.0, "FM"),
    (12, 0.5, 0.5, "additive"),
    (13, 0.5, 0.5, "wavetable"),
    (15, 0.0, 0.5, "naive speech"),
    (15, 0.35, 0.5, "LPC speech (own clock)"),
];

fn render(rate: SampleRate, tuning: Tuning, engine: usize, harmonics: f32, timbre: f32) -> Vec<f64> {
    let mut voice = Voice::new(rate);
    voice.set_tuning(tuning);
    let patch = Patch { note: 69.0, harmonics, timbre, morph: 0.5, engine, ..Patch::default() };
    let frames = (2.0 * rate.hz()) as usize / 12 * 12;
    let mut out = Vec::with_capacity(frames);
    let mut block = [Frame::default(); 12];
    for _ in 0..frames / 12 {
        voice.render(&patch, &Modulations::default(), &mut block);
        out.extend(block.iter().map(|f| f64::from(f.out)));
    }
    let out = out.split_off((0.5 * f64::from(rate.hz())) as usize);
    let mean = out.iter().sum::<f64>() / out.len() as f64;
    out.into_iter().map(|x| x - mean).collect()
}

fn r(x: &[f64], lag: usize) -> f64 {
    let n = x.len() - lag;
    x[..n].iter().zip(&x[lag..]).map(|(a, b)| a * b).sum::<f64>() / n as f64
}

/// The best lag in `[lo, hi]` and its normalized correlation.
fn peak(x: &[f64], r0: f64, lo: f64, hi: f64) -> (usize, f64) {
    (lo.floor() as usize..=hi.ceil() as usize).map(|lag| (lag, r(x, lag) / r0)).max_by(|a, b| a.1.total_cmp(&b.1)).expect("lags")
}

/// The fundamental near `expected_hz`, refusing octave and twelfth aliases (see the module doc).
fn pitch_hz(x: &[f64], rate: f32, expected_hz: f64, label: &str) -> f64 {
    let rate = f64::from(rate);
    let r0 = r(x, 0);
    let t = rate / expected_hz;
    let (best, rb) = peak(x, r0, t / 1.03, t * 1.03);
    assert!(rb > 0.3, "{label}: no periodicity at the expected period ({rb:.3})");
    for k in [2.0, 3.0] {
        let tk = best as f64 / k;
        let (_, rk) = peak(x, r0, tk / 1.03, tk * 1.03);
        assert!(rk < rb - 0.1, "{label}: repeats at 1/{k} of the period ({rk:.3} against {rb:.3}): an alias, not the fundamental");
    }
    dft_peak_hz(x, rate, rate / best as f64)
}

/// The frequency of the strongest spectral peak within 1.5% of `around_hz`: a Hann-windowed DFT
/// over the whole render, scanned in 0.05 Hz steps and refined by a parabola through the log
/// magnitudes. Unlike a lag search it has no sample grid, so neither LPC speech's 8 kHz clock nor a
/// detuned operator pair biases it.
fn dft_peak_hz(x: &[f64], rate: f64, around_hz: f64) -> f64 {
    let n = x.len();
    let w: Vec<f64> = x.iter().enumerate().map(|(i, v)| v * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).collect();
    let mag = |f: f64| {
        let (s, c) = (std::f64::consts::TAU * f / rate).sin_cos();
        let (mut re, mut im, mut cr, mut ci) = (0.0, 0.0, 1.0f64, 0.0f64);
        for (i, v) in w.iter().enumerate() {
            re += v * cr;
            im += v * ci;
            let next = (cr * c - ci * s, cr * s + ci * c);
            (cr, ci) = next;
            if i % 4096 == 4095 {
                // Renormalize the rotation against drift.
                let m = (cr * cr + ci * ci).sqrt();
                (cr, ci) = (cr / m, ci / m);
            }
        }
        (re * re + im * im).sqrt().max(1e-30).ln()
    };
    let step = 0.05;
    let (lo, hi) = (around_hz / 1.015, around_hz * 1.015);
    let steps = ((hi - lo) / step) as usize;
    let mags: Vec<f64> = (0..=steps).map(|k| mag(lo + k as f64 * step)).collect();
    let k = (1..steps).max_by(|a, b| mags[*a].total_cmp(&mags[*b])).expect("steps");
    let (a, m, c) = (mags[k - 1], mags[k], mags[k + 1]);
    lo + (k as f64 + 0.5 * (a - c) / (a - 2.0 * m + c)) * step
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

fn check(tuning: Tuning, expected: impl Fn(&str) -> f64) {
    let mut worst = 0.0f64;
    for rate in SampleRate::ALL {
        for (engine, harmonics, timbre, label) in PATHS {
            let want = expected(label);
            let hz = pitch_hz(&render(rate, tuning, engine, harmonics, timbre), rate.hz(), want, label);
            let c = cents(hz, want);
            println!("{tuning:?} {rate:?} {label}: {hz:.4} Hz against {want:.4} ({c:+.4} cents)");
            assert!(c.abs() < 1.0, "{tuning:?} {rate:?} {label}: {hz} Hz against {want} Hz ({c:+.3} cents)");
            worst = worst.max(c.abs());
        }
    }
    println!("{tuning:?}: worst {worst:.4} cents");
}

#[test]
fn every_pitch_path_plays_the_firmware_pitch_at_every_rate() {
    let firmware = 440.0 * f64::from(REF_SAMPLE_RATE) / f64::from(CORRECTED_SAMPLE_RATE);
    check(Tuning::Firmware, |label| if label.starts_with("LPC") { 440.0 } else { firmware });
}

/// The measurement refuses an octave error: the same render checked against half its frequency
/// fails (the signal repeats at half the expected lag).
#[test]
fn the_measurement_refuses_an_octave_alias() {
    let x = render(SampleRate::Hz48000, Tuning::Concert, 10, 0.5, 0.0);
    let refused = std::panic::catch_unwind(|| pitch_hz(&x, 48_000.0, 220.0, "FM checked an octave low"));
    assert!(refused.is_err(), "a 440 Hz sine passed as 220 Hz");
}
