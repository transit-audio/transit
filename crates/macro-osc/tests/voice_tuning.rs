//! `Voice::set_tuning` (#195 slice 3): concert pitch plays every note at its equal-tempered
//! frequency (A4 = 440 Hz) at every rate, on every pitch path (the voice's `a0`, the six-operator
//! voices, LPC speech), and the firmware's tuning stays the firmware's arithmetic bit for bit.

#![forbid(unsafe_code)]

use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc::{CORRECTED_SAMPLE_RATE, REF_SAMPLE_RATE, SampleRate, Tuning};

/// The engine index of the two-operator FM engine (a sine at timbre 0).
const FM: usize = 10;

fn concert_hz(note: f64) -> f64 {
    440.0 * 2.0_f64.powf((note - 69.0) / 12.0)
}

/// The main output of a free-running voice: 2 s, the first `skip` seconds dropped, DC removed.
fn render(rate: SampleRate, tuning: Tuning, engine: usize, note: f32, harmonics: f32, timbre: f32, skip: f64) -> Vec<f64> {
    let mut voice = Voice::new(rate);
    voice.set_tuning(tuning);
    let patch = Patch { note, harmonics, timbre, morph: 0.5, engine, ..Patch::default() };
    let modulations = Modulations::default();
    let frames = (2.0 * rate.hz()) as usize / 12 * 12;
    let mut out = Vec::with_capacity(frames);
    let mut block = [Frame::default(); 12];
    for _ in 0..frames / 12 {
        voice.render(&patch, &modulations, &mut block);
        out.extend(block.iter().map(|f| f64::from(f.out)));
    }
    let out = out.split_off((skip * f64::from(rate.hz())) as usize);
    let mean = out.iter().sum::<f64>() / out.len() as f64;
    out.into_iter().map(|x| x - mean).collect()
}

/// Frequency from the first and last rising zero crossings, interpolated between samples.
fn zero_crossing_hz(x: &[f64], rate: f32) -> f64 {
    let crossings: Vec<f64> =
        x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, w)| i as f64 - w[0] / (w[1] - w[0])).collect();
    assert!(crossings.len() > 100, "{} crossings", crossings.len());
    (crossings.len() - 1) as f64 / ((crossings[crossings.len() - 1] - crossings[0]) / f64::from(rate))
}

/// The fundamental from the autocorrelation peak near `periods` periods of `around_hz` (within 2%),
/// refined by a parabola. Over many periods a waveform rebuilt on a coarser grid (LPC speech runs
/// at 8 kHz and is held to the output rate) no longer pulls the peak to a grid multiple.
fn autocorrelation_hz(x: &[f64], rate: f32, around_hz: f64, periods: f64) -> f64 {
    let rate = f64::from(rate);
    let lag = periods * rate / around_hz;
    let (lo, hi) = ((lag / 1.02) as usize, (lag * 1.02) as usize + 2);
    let r = |lag: usize| {
        let n = x.len() - lag;
        x[..n].iter().zip(&x[lag..]).map(|(a, b)| a * b).sum::<f64>() / n as f64
    };
    let best = (lo..=hi).max_by(|&a, &b| r(a).total_cmp(&r(b))).expect("lags");
    assert!(best > lo && best < hi, "peak at the edge of the search ({lo}..{hi}): {best}");
    let (a, b, c) = (r(best - 1), r(best), r(best + 1));
    periods * rate / (best as f64 + 0.5 * (a - c) / (a - 2.0 * b + c))
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

#[test]
fn concert_tuning_plays_equal_temperament_at_every_rate() {
    for rate in SampleRate::ALL {
        for note in [45.0, 69.0, 93.0] {
            let hz = zero_crossing_hz(&render(rate, Tuning::Concert, FM, note, 0.5, 0.0, 0.05), rate.hz());
            let c = cents(hz, concert_hz(f64::from(note)));
            println!("{rate:?} note {note}: {hz:.4} Hz ({c:+.4} cents)");
            assert!(c.abs() < 0.05, "{rate:?} note {note}: {hz} Hz ({c:+.4} cents)");
        }
    }
}

/// The firmware's tuning plays 48000 / 47872.34 above concert pitch: the measured ratio between the
/// two settings is that factor (4.61 cents) to within 0.01 cent.
#[test]
fn the_tunings_differ_by_the_corrected_rate_factor() {
    let factor = f64::from(REF_SAMPLE_RATE) / f64::from(CORRECTED_SAMPLE_RATE);
    for rate in [SampleRate::Hz48000, SampleRate::Hz96000] {
        let firmware = zero_crossing_hz(&render(rate, Tuning::Firmware, FM, 69.0, 0.5, 0.0, 0.05), rate.hz());
        let concert = zero_crossing_hz(&render(rate, Tuning::Concert, FM, 69.0, 0.5, 0.0, 0.05), rate.hz());
        let off = cents(firmware / concert, factor);
        println!("{rate:?}: firmware {firmware:.4} Hz, concert {concert:.4} Hz, ratio off by {off:+.5} cents");
        assert!(off.abs() < 0.01, "{rate:?}: {firmware} / {concert} against {factor} ({off:+.5} cents)");
    }
}

/// The six-op voices and LPC speech have pitch references of their own; concert tuning lowers them
/// by the same factor.
#[test]
fn six_op_and_speech_follow_the_tuning() {
    let factor = f64::from(REF_SAMPLE_RATE) / f64::from(CORRECTED_SAMPLE_RATE);
    for (engine, harmonics, label) in [(2, 0.0, "six-op bank A"), (15, 0.35, "LPC speech")] {
        let rate = SampleRate::Hz48000;
        let around = concert_hz(69.0) * factor;
        let firmware = autocorrelation_hz(&render(rate, Tuning::Firmware, engine, 69.0, harmonics, 0.5, 0.5), rate.hz(), around, 40.0);
        let concert = autocorrelation_hz(&render(rate, Tuning::Concert, engine, 69.0, harmonics, 0.5, 0.5), rate.hz(), around / factor, 40.0);
        let off = cents(firmware / concert, factor);
        println!("{label}: firmware {firmware:.4} Hz, concert {concert:.4} Hz, ratio off by {off:+.4} cents");
        assert!(off.abs() < 0.25, "{label}: {firmware} / {concert} against {factor} ({off:+.4} cents)");
    }
}

/// Switching to concert pitch and back leaves the firmware's arithmetic bit for bit.
#[test]
fn firmware_tuning_is_the_default_and_round_trips_bit_for_bit() {
    assert_eq!(Tuning::default(), Tuning::Firmware);
    assert_eq!(Tuning::Firmware.pitch_scale().to_bits(), 1.0_f32.to_bits());
    let render_all = |toggle: bool| {
        let mut voice = Voice::new(SampleRate::Hz48000);
        if toggle {
            voice.set_tuning(Tuning::Concert);
            voice.set_tuning(Tuning::Firmware);
            voice.init();
        }
        assert_eq!(voice.tuning(), Tuning::Firmware);
        let mut out = Vec::new();
        let mut block = [Frame::default(); 12];
        for engine in 0..24 {
            let patch = Patch { note: 57.0, engine, ..Patch::default() };
            for _ in 0..40 {
                voice.render(&patch, &Modulations::default(), &mut block);
                out.extend_from_slice(&block);
            }
        }
        out
    };
    assert!(render_all(false) == render_all(true), "the firmware's tuning after a round trip renders other bits");
}

/// The tuning survives `init` (the engines re-derive their references from it).
#[test]
fn the_tuning_survives_init() {
    let rate = SampleRate::Hz48000;
    let mut voice = Voice::new(rate);
    voice.set_tuning(Tuning::Concert);
    voice.init();
    assert_eq!(voice.tuning(), Tuning::Concert);
    let patch = Patch { note: 69.0, harmonics: 0.0, engine: 2, ..Patch::default() };
    let mut a = Vec::new();
    let mut b = Vec::new();
    let mut block = [Frame::default(); 12];
    let mut fresh = Voice::new(rate);
    fresh.set_tuning(Tuning::Concert);
    for _ in 0..400 {
        voice.render(&patch, &Modulations::default(), &mut block);
        a.extend_from_slice(&block);
        fresh.render(&patch, &Modulations::default(), &mut block);
        b.extend_from_slice(&block);
    }
    assert!(a == b, "an initialized concert voice renders as a fresh one");
}
