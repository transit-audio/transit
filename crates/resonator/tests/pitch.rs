//! Absolute pitch of every pitch path, with a measurement that refuses aliases (#198; the E63 gate
//! class of `docs/contributing/evidence/gates.md`).
//!
//! The resonator runs at 48 kHz only (the firmware's rate; its filters, delays and reverb assume
//! it), so the pitch is checked at 48 kHz. Every model computes its frequency as
//! `SemitonesToRatio(note - 69) * a3` with `a3 = 440 / 48000` (no corrected rate), so MIDI note 69
//! plays 440 Hz; the sympathetic strings reach the same formula through their own note table. Each
//! model is measured at notes 45, 69 and 81 (the quantized strings at 45, 57 and 69), strummed once with an infinite decay, from 0.5 s to
//! 2.5 s, on the main output. The measurement is the macro oscillator's (`crates/macro-osc/tests/
//! voice_pitch.rs`): the autocorrelation peak searched within 3% of the expected period only and
//! required to be a real periodicity; refused if the signal also repeats at a half or a third of that
//! lag (an octave or a twelfth up) or if its spectrum near half the pitch is within 12 dB of the peak
//! (an octave down); the frequency then read from the spectrum.
//!
//! The tolerance is per model and note, from measurement (2026-10-30, Windows x86-64; the port is
//! bit-identical to the oracle, so these are the firmware's figures). The modal and FM paths are
//! exact: within 0.2 cent at every note. The strings' comb is tuned through the firmware's own delay
//! compensation (the FIR's one sample, the IIR's group delay read from a table), which is
//! approximate and goes flat as the note rises, as on the module: within 0.8 cent at A2 and A4 for
//! the plain strings (2.3 cents with the reverb's chorus LFO), 1.3 to 2.5 cents flat at A5 (5.9
//! with the reverb), 6 to 7 cents flat at A6 (measured, not gated). So the string models are held
//! to 3 cents at their first two notes and 8 cents at the third: tight enough for an octave, a
//! twelfth or a wrong sample rate (44.1 against 48 kHz is 147 cents) to fail. The strings' error is
//! the firmware's own comb tuning, not a port error: do not correct or tighten it in the port
//! without changing the oracle first.

#![forbid(unsafe_code)]

use transit_resonator::{MAX_BLOCK_SIZE, Patch, PerformanceState, Resonator, ResonatorModel};

/// The notes measured: A2, A4, A5.
const NOTES: [f32; 3] = [45.0, 69.0, 81.0];

/// The quantized strings' notes: A2, A3, A4. Their chord has two partials a whole tone apart, and
/// at A5 the spectral read can no longer separate the note's partial from their beating.
const CHORD_NOTES: [f32; 3] = [45.0, 57.0, 69.0];

/// Tolerances in cents at the measured notes: the exact paths, and the strings' comb.
const EXACT: [f64; 3] = [1.0, 1.0, 1.0];
const STRING_CENTS: [f64; 3] = [3.0, 3.0, 8.0];

/// (model, polyphony, patch, chord, label, notes, tolerances in cents at those notes).
const MODELS: [(ResonatorModel, usize, Patch, i32, &str, [f32; 3], [f64; 3]); 6] = [
    // Structure 0.25: no stiffness, harmonic modes; the main output carries modes 1, 3, 5...
    (ResonatorModel::Modal, 1, Patch { structure: 0.25, brightness: 0.5, damping: 1.0, position: 0.5 }, 0, "modal", NOTES, EXACT),
    // Structure 0.25: no dispersion. Damping 0.97: infinite decay.
    (
        ResonatorModel::InharmonicString,
        1,
        Patch { structure: 0.25, brightness: 0.5, damping: 0.97, position: 0.3 },
        0,
        "inharmonic string",
        NOTES,
        STRING_CENTS,
    ),
    // Structure 3/7: the second string in unison with the first (four voices: two strings each).
    (
        ResonatorModel::SympatheticStrings,
        4,
        Patch { structure: 3.0 / 7.0, brightness: 0.5, damping: 0.97, position: 0.3 },
        0,
        "sympathetic strings",
        NOTES,
        STRING_CENTS,
    ),
    // Brightness 0: no FM; structure 60/128: the modulator an octave up, so the carrier is the
    // fundamental. Damping 1: the oscillator behavior, no envelope.
    (ResonatorModel::FmVoice, 1, Patch { structure: 60.0 / 128.0, brightness: 0.0, damping: 1.0, position: 0.5 }, 0, "FM voice", NOTES, EXACT),
    // Chord 1 at four voices: the note and a whole tone above (two partials with no common
    // fundamental below the note; the spectral read takes the note's partial).
    (
        ResonatorModel::QuantizedSympatheticStrings,
        4,
        Patch { structure: 0.5, brightness: 0.5, damping: 0.97, position: 0.3 },
        1,
        "quantized sympathetic strings",
        CHORD_NOTES,
        [3.0; 3],
    ),
    (
        ResonatorModel::StringAndReverb,
        1,
        Patch { structure: 0.25, brightness: 0.5, damping: 0.97, position: 0.3 },
        0,
        "string and reverb",
        NOTES,
        STRING_CENTS,
    ),
];

const RATE: f32 = transit_resonator::SAMPLE_RATE;

fn render(model: ResonatorModel, polyphony: usize, patch: Patch, chord: i32, note: f32) -> Vec<f64> {
    let mut part = Resonator::new();
    part.set_polyphony(polyphony);
    part.set_model(model);
    let frames = (2.5 * RATE) as usize / MAX_BLOCK_SIZE * MAX_BLOCK_SIZE;
    let input = [0.0_f32; MAX_BLOCK_SIZE];
    let (mut o, mut a) = ([0.0_f32; MAX_BLOCK_SIZE], [0.0_f32; MAX_BLOCK_SIZE]);
    let mut out = Vec::with_capacity(frames);
    for block in 0..frames / MAX_BLOCK_SIZE {
        let performance = PerformanceState {
            strum: block == 0,
            internal_exciter: true,
            internal_strum: true,
            tonic: 36.0,
            note: note - 36.0,
            chord,
            ..PerformanceState::default()
        };
        part.process(&performance, &patch, &input, &mut o, &mut a);
        out.extend(o.iter().zip(&a).map(|(o, a)| f64::from(*o) + f64::from(*a)));
    }
    // Two one-pole high-passes at 30 Hz: at high notes the strings' tone is quiet next to a slow
    // drift that would otherwise dominate the autocorrelation.
    let k = (-std::f64::consts::TAU * 30.0 / f64::from(RATE)).exp();
    for _ in 0..2 {
        let (mut x1, mut y1) = (0.0, 0.0);
        for s in &mut out {
            let y = k * (y1 + *s - x1);
            x1 = *s;
            y1 = y;
            *s = y;
        }
    }
    out.split_off((0.5 * RATE) as usize)
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
    let (hz, peak_mag) = dft_peak(x, rate, rate / best as f64);
    // An octave down: a signal whose fundamental is half the expected pitch, with a strong second
    // harmonic, passes the lag checks above (it repeats at the expected period too). Its spectrum
    // near half the measured pitch must be at least 12 dB under the peak.
    let (_, half_mag) = dft_peak(x, rate, hz / 2.0);
    let below_db = 20.0 * (peak_mag - half_mag) / std::f64::consts::LN_10;
    assert!(below_db > 12.0, "{label}: the spectrum at half the pitch is only {below_db:.1} dB under the peak: an octave-down alias");
    hz
}

/// The frequency of the strongest spectral peak within 1.5% of `around_hz` and its log magnitude: a
/// Hann-windowed DFT over the whole render, scanned in 0.05 Hz steps and refined by a parabola
/// through the log magnitudes. Unlike a lag search it has no sample grid, so a comb tuned between
/// whole samples is read exactly.
fn dft_peak(x: &[f64], rate: f64, around_hz: f64) -> (f64, f64) {
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
    let d = 0.5 * (a - c) / (a - 2.0 * m + c);
    (lo + (k as f64 + d) * step, m - 0.25 * (a - c) * d)
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

#[test]
fn every_model_plays_its_note_at_48_khz() {
    let mut failures = Vec::new();
    for (model, polyphony, patch, chord, label, notes, tolerances) in MODELS {
        for (note, tolerance) in notes.into_iter().zip(tolerances) {
            let want = 440.0 * 2f64.powf((f64::from(note) - 69.0) / 12.0);
            let label = format!("{label} at note {note}");
            let hz = pitch_hz(&render(model, polyphony, patch, chord, note), RATE, want, &label);
            let c = cents(hz, want);
            println!("{label}: {hz:.4} Hz against {want:.4} ({c:+.4} cents, tolerance {tolerance})");
            if c.abs() >= tolerance {
                failures.push(format!("{label}: {hz} Hz against {want} Hz ({c:+.3} cents)"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The measurement refuses an octave error on the resonator's own output: the modal model at A4
/// checked as A3 fails (the signal repeats at half the expected lag).
#[test]
fn the_measurement_refuses_an_octave_alias() {
    let (model, polyphony, patch, chord, ..) = MODELS[0];
    let x = render(model, polyphony, patch, chord, 69.0);
    let refused = std::panic::catch_unwind(|| pitch_hz(&x, RATE, 220.0, "modal checked an octave low"));
    assert!(refused.is_err(), "a 440 Hz resonance passed as 220 Hz");
}

/// The measurement refuses an octave-down error: a 220 Hz tone whose second harmonic is 10.5 dB
/// stronger than its fundamental repeats at the 440 Hz period, but its spectrum at 220 Hz is within
/// 12 dB of the 440 Hz peak.
#[test]
fn the_measurement_refuses_an_octave_down_alias() {
    let rate = 48_000.0f64;
    let x: Vec<f64> = (0..72_000)
        .map(|i| {
            let t = i as f64 / rate;
            0.3 * (std::f64::consts::TAU * 220.0 * t).sin() + (std::f64::consts::TAU * 440.0 * t).sin()
        })
        .collect();
    let refused = std::panic::catch_unwind(|| pitch_hz(&x, 48_000.0, 440.0, "a 220 Hz tone checked at 440 Hz"));
    let message = refused.expect_err("a 220 Hz tone with a strong second harmonic passed as 440 Hz");
    let text = message.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(text.contains("octave-down"), "refused for the octave-down check, not another one: {text}");
}
