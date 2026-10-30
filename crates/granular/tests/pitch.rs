//! Grain transposition at the port's one rate, 32 kHz (the E63 gate class, ADR 0033 notes): one
//! triggered grain reading a 440 Hz sine at -12, 0, +7 and +12 semitones must come out at the
//! expected absolute frequency, measured with an alias-rejecting search (the peak searched near the
//! expected frequency only, refused if the spectrum at half of it, or at the input frequency when
//! transposed, is within 12 dB of the peak). One grain alone is measured because overlapping grains
//! restart the sine at unrelated phases, which spreads the spectrum into lines at the grain rate.
//! The host-rate path (resampling to and from 32 kHz) is the effect's, with its own pitch tests at
//! every rate.

#![forbid(unsafe_code)]

use transit_granular::{BufferSize, Granular, MAX_BLOCK_SIZE, Parameters, Quality, SAMPLE_RATE};

const INPUT_HZ: f64 = 440.0;

/// The block that triggers the grain: after 2 s, so the buffer holds the sine.
const TRIGGER_BLOCK: usize = 2000;

/// Hann-windowed DFT magnitude of `x` at `hz`.
fn magnitude(x: &[f64], hz: f64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let n = x.len() as f64;
    let w = 2.0 * std::f64::consts::PI * hz / f64::from(SAMPLE_RATE);
    let (mut re, mut im) = (0.0, 0.0);
    for (i, s) in x.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let t = i as f64;
        let hann = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * t / n).cos();
        re += s * hann * (w * t).cos();
        im -= s * hann * (w * t).sin();
    }
    (re * re + im * im).sqrt()
}

/// The left output from the trigger on, while the grain plays (it ends where the output returns to
/// silence; density 0.5 seeds no other grain, and the reverb, feedback and diffuser are off).
fn render(quality: Quality, pitch: f32) -> Vec<f64> {
    let mut g = Granular::new(BufferSize::FIRMWARE, quality);
    let p = Parameters { position: 0.0, size: 1.0, pitch, density: 0.5, texture: 0.6, dry_wet: 1.0, ..Parameters::default() };
    let frames = (TRIGGER_BLOCK + 1000) * MAX_BLOCK_SIZE;
    let input: Vec<[i16; 2]> = (0..frames)
        .map(|i| {
            #[allow(clippy::cast_precision_loss)]
            let s = (2.0 * std::f64::consts::PI * INPUT_HZ * i as f64 / f64::from(SAMPLE_RATE)).sin();
            #[allow(clippy::cast_possible_truncation)]
            let v = (s * 12_000.0) as i16;
            [v, v]
        })
        .collect();
    let mut out = vec![[0_i16; 2]; frames];
    for (block, (i, o)) in input.chunks(MAX_BLOCK_SIZE).zip(out.chunks_mut(MAX_BLOCK_SIZE)).enumerate() {
        g.process(&Parameters { trigger: block == TRIGGER_BLOCK, ..p }, i, o);
    }
    let grain: Vec<f64> = out[TRIGGER_BLOCK * MAX_BLOCK_SIZE..].iter().map(|f| f64::from(f[0])).collect();
    let end = grain.iter().rposition(|s| *s != 0.0).unwrap_or(0) + 1;
    grain[..end].to_vec()
}

#[test]
fn a_grain_transposes_to_the_expected_absolute_frequency() {
    for quality in [Quality::Stereo, Quality::MonoLowFidelity] {
        for pitch in [-12.0_f32, 0.0, 7.0, 12.0] {
            let x = render(quality, pitch);
            assert!(x.len() > 2000, "{quality:?} {pitch:+}: the grain played {} frames", x.len());
            let expected = INPUT_HZ * 2_f64.powf(f64::from(pitch) / 12.0);
            // The peak, searched within ±50 cents of the expected frequency in 0.25-cent steps.
            let (cents, peak) = (-200..=200)
                .map(|c| {
                    let cents = f64::from(c) * 0.25;
                    (cents, magnitude(&x, expected * 2_f64.powf(cents / 1200.0)))
                })
                .fold((0.0, 0.0), |best, m| if m.1 > best.1 { m } else { best });
            let db = |m: f64| 20.0 * (m / peak).log10();
            let half = db(magnitude(&x, expected / 2.0));
            println!(
                "{quality:?} {pitch:+} st: {:.2} Hz ({cents:+.2} cents) over {} frames, half {half:.1} dB",
                expected * 2_f64.powf(cents / 1200.0),
                x.len()
            );
            assert!(cents.abs() <= 1.0, "{quality:?} {pitch:+}: {cents:+.2} cents off");
            assert!(half < -12.0, "{quality:?} {pitch:+}: half the frequency is only {half:.1} dB down");
            if pitch != 0.0 {
                let original = db(magnitude(&x, INPUT_HZ));
                assert!(original < -12.0, "{quality:?} {pitch:+}: the input frequency is only {original:.1} dB down");
            }
        }
    }
}
