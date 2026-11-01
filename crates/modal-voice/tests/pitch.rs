//! Absolute pitch of every pitch path at the module's rate, with a measurement that refuses aliases
//! (#198 slice 2; the E63 gate class of `docs/contributing/evidence/gates.md`).
//!
//! The modal voice runs at 32 kHz only (the firmware's rate; its filters, delays, envelope and
//! reverb assume it), so its pitch is checked at 32 kHz; the instrument's converter carries it to
//! the session rate and has its own pitch test at every rate. Every path computes its frequency
//! from `lut_midi_to_f_high[(note + 48) * 256 >> 8] * lut_midi_to_f_low[...]`, a table of
//! `440 * 2^((note - 69) / 12) / 32000` (capped at 12 kHz), so MIDI note 69 plays 440 Hz. Each
//! path is measured at notes 45, 69 and 81, the gate held from the first block, from 0.5 s to
//! 2.5 s, on main plus aux. The measurement is the resonator's (`crates/resonator/tests/pitch.rs`):
//! the autocorrelation peak searched within 3% of the expected period only and required to be a
//! real periodicity; refused if the signal also repeats at a half or a third of that lag (an
//! octave or a twelfth up) or if its spectrum near half the pitch is within 12 dB of the peak (an
//! octave down); the frequency then read from the spectrum.
//!
//! The five-string model is a chord, not one pitch (every chord of the firmware's table holds the
//! note, its octave below and three more strings), so it has no single fundamental to measure: its
//! strings are the one-string path's comb, measured here, tuned by `SemitonesToRatio` of the chord
//! table's transpositions. Tolerances are from measurement (2026-11-01, Windows x86-64; the port
//! is bit-identical to the oracle, so these are the firmware's figures): the modal bank and the FM
//! pair are exact, within 0.1 cent; the string goes flat as the note rises through the firmware's
//! approximate comb compensation (-0.85, -1.34 and -4.79 cents at A2, A4 and A5), so it is held to
//! 3, 3 and 8 cents. The string's error is the firmware's own comb tuning, not a port error: do
//! not correct or tighten it in the port without changing the oracle first.

#![forbid(unsafe_code)]

use transit_modal_voice::{MAX_BLOCK_SIZE, ModalVoice, Patch, PerformanceState, ResonatorModel};

/// The notes measured: A2, A4, A5.
const NOTES: [f32; 3] = [45.0, 69.0, 81.0];

const RATE: f32 = transit_modal_voice::SAMPLE_RATE;

/// A patch with every exciter off; each path turns on what it needs.
fn quiet() -> Patch {
    Patch {
        exciter_envelope_shape: 0.5,
        exciter_bow_level: 0.0,
        exciter_bow_timbre: 0.5,
        exciter_blow_level: 0.0,
        exciter_blow_meta: 0.5,
        exciter_blow_timbre: 0.5,
        exciter_strike_level: 0.0,
        exciter_strike_meta: 0.5,
        exciter_strike_timbre: 0.5,
        resonator_geometry: 0.25,
        resonator_brightness: 0.5,
        resonator_damping: 0.9995,
        resonator_position: 0.3,
        space: 0.0,
    }
}

/// (label, model, easter egg, patch, tolerances in cents at the three notes): the modal bank, the string and the FM pair voice.
fn paths() -> Vec<(&'static str, ResonatorModel, bool, Patch, [f64; 3])> {
    // The modal bank struck by the mallet (strike meta 0.5), harmonic (geometry 0.25: no
    // stiffness), at full damping: a long ring.
    let modal = Patch { exciter_strike_level: 0.6, ..quiet() };
    // The string struck by the mallet (geometry 0.25: no dispersion), damping 0.97: the firmware's
    // crossfade to infinite decay.
    let string = Patch { exciter_strike_level: 0.6, resonator_damping: 0.97, ..quiet() };
    // The FM pair: no FM (blow timbre 0), no feedback (bow timbre 0), the second oscillator silent
    // (strike level 0), the filter open (brightness 0.9995) in low-pass (geometry 0).
    let fm = Patch {
        exciter_blow_level: 0.9,
        exciter_blow_timbre: 0.0,
        exciter_bow_timbre: 0.0,
        resonator_geometry: 0.0,
        resonator_brightness: 0.9995,
        resonator_damping: 0.5,
        ..quiet()
    };
    vec![
        ("modal", ResonatorModel::Modal, false, modal, [1.0, 1.0, 1.0]),
        ("string", ResonatorModel::String, false, string, [3.0, 3.0, 8.0]),
        ("fm pair", ResonatorModel::Modal, true, fm, [1.0, 1.0, 1.0]),
    ]
}

fn render(model: ResonatorModel, easter_egg: bool, patch: Patch, note: f32) -> Vec<f64> {
    let mut voice = ModalVoice::new();
    voice.set_model(model);
    voice.set_easter_egg(easter_egg);
    *voice.patch_mut() = patch;
    let frames = (2.5 * RATE) as usize / MAX_BLOCK_SIZE * MAX_BLOCK_SIZE;
    let silence = [0.0_f32; MAX_BLOCK_SIZE];
    let (mut m, mut a) = ([0.0_f32; MAX_BLOCK_SIZE], [0.0_f32; MAX_BLOCK_SIZE]);
    let mut out = Vec::with_capacity(frames);
    let performance = PerformanceState { gate: true, note, modulation: 0.0, strength: 0.5 };
    for _ in 0..frames / MAX_BLOCK_SIZE {
        voice.process(&performance, &silence, &silence, &mut m, &mut a);
        out.extend(m.iter().zip(&a).map(|(m, a)| f64::from(*m) + f64::from(*a)));
    }
    // Two one-pole high-passes at 30 Hz against a slow drift.
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
fn every_path_plays_its_note_at_32_khz() {
    let mut failures = Vec::new();
    for (label, model, easter_egg, patch, tolerances) in paths() {
        for (note, tolerance) in NOTES.iter().zip(tolerances) {
            let expected = 440.0 * 2f64.powf((f64::from(*note) - 69.0) / 12.0);
            let x = render(model, easter_egg, patch, *note);
            let name = format!("{label} at note {note}");
            let hz = pitch_hz(&x, RATE, expected, &name);
            let c = cents(hz, expected);
            println!("{name}: {hz:.3} Hz against {expected:.3} Hz ({c:+.2} cents)");
            if c.abs() > tolerance {
                failures.push(format!("{name}: {c:+.2} cents (tolerance {tolerance})"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{}",
        failures.join(
            "
"
        )
    );
}

#[test]
fn an_octave_or_a_twelfth_is_refused() {
    // The modal path at A4 measured as if it should be A3 or as a twelfth below fails the alias
    // checks: the measurement cannot pass a wrong octave.
    let (_, model, easter_egg, patch, _) = paths()[0];
    let x = render(model, easter_egg, patch, 69.0);
    for wrong in [220.0, 440.0 / 3.0] {
        let r = std::panic::catch_unwind(|| pitch_hz(&x, RATE, wrong, "alias"));
        assert!(r.is_err(), "measuring A4 as {wrong} Hz must fail");
    }
}
