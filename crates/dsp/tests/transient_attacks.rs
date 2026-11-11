//! Where `detect_transients` places low, clipped attacks (a drum break's limited kicks) under a
//! steady bed of high noise, and that a kick's body is never read as onsets (#468). The signals
//! are built from an integer noise source, integer-period triangles and linear envelopes only.

use transit_dsp::offline::detect_transients;

const SR: f32 = 48_000.0;

fn at(ms: usize) -> usize {
    ms * 48
}

fn noise(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    *seed as f32 / u32::MAX as f32 * 2.0 - 1.0
}

/// A steady bed of high noise (first difference of white noise) at `level`, `len` frames long.
fn bed(len: usize, level: f32, seed: &mut u32) -> Vec<f32> {
    let mut prev = 0.0;
    (0..len)
        .map(|_| {
            let w = noise(seed);
            let d = w - prev;
            prev = w;
            level * d
        })
        .collect()
}

/// A limited kick: triangle cycles starting at a zero crossing whose period grows by 60 frames a
/// cycle from 320 (150 Hz) to 800 (60 Hz), as a kick's pitch falls, scaled by `drive` and clipped
/// at 0.9 (a drive above 0.9 flattens its tops), fading linearly to nothing over `len` frames. Almost all of its
/// energy is low; its attack barely shows in high-passed energy, and the falling pitch moves its
/// clipped harmonics across bands through the body.
fn kick(len: usize, drive: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(len);
    let mut period = 320usize;
    while out.len() < len {
        for j in 0..period {
            let p = (j + period / 4) % period;
            out.push(4.0 * (p as f32 / period as f32 - 0.5).abs() - 1.0);
        }
        period = (period + 60).min(800);
    }
    out.truncate(len);
    out.iter().enumerate().map(|(i, tri)| (drive * (len - i) as f32 / len as f32 * tri).clamp(-0.9, 0.9)).collect()
}

/// A snare-like hit: high noise falling linearly to nothing over `len` frames.
fn hit(len: usize, level: f32, seed: &mut u32) -> Vec<f32> {
    let b = bed(len, level, seed);
    b.iter().enumerate().map(|(i, v)| v * (len - i) as f32 / len as f32).collect()
}

fn put(x: &mut [f32], from: usize, sig: &[f32]) {
    for (o, v) in x.iter_mut().skip(from).zip(sig) {
        *o += v;
    }
}

/// Every found position lies at or before its attack, by at most `early` frames, and never
/// after it by more than `late` frames; one found position per attack, nothing else.
fn placed(found: &[usize], attacks: &[usize], early: usize, late: usize) -> bool {
    found.len() == attacks.len() && found.iter().zip(attacks).all(|(f, a)| *f + early >= *a && *f <= *a + late)
}

/// Every found position is within `tol` frames of an expected one, one per expected position,
/// nothing else.
fn near(found: &[usize], expected: &[usize], tol: usize) -> bool {
    found.len() == expected.len() && found.iter().zip(expected).all(|(f, e)| f.abs_diff(*e) <= tol)
}

/// Audio that starts on a loud kick (with a quiet hit 35 ms into its body, as a break's kick
/// often has) gives the markers it gives when silence comes before it: the start is read as the
/// audio plays, after silence, so the kick's own rise masks its body the way any hit's does,
/// instead of the body's first bump reading as an onset.
#[test]
fn audio_starting_on_an_attack_reads_as_if_silence_came_before_it() {
    let mut seed = 11;
    let mut x = bed(at(1500), 0.02, &mut seed);
    put(&mut x, 0, &kick(at(350), 3.0));
    put(&mut x, at(35), &hit(at(60), 0.05, &mut seed));
    for a in [at(600), at(1100)] {
        put(&mut x, a, &hit(at(120), 0.5, &mut seed));
    }
    let lead = at(250);
    let mut after_silence = vec![0.0; lead];
    after_silence.extend_from_slice(&x);
    let found = detect_transients(&[x], SR, 0.5);
    let shifted: Vec<usize> = detect_transients(&[after_silence], SR, 0.5).into_iter().filter(|p| *p > lead).map(|p| p - lead).collect();
    assert!(near(&found, &shifted, at(1)), "{found:?} vs {shifted:?} (after silence)");
    assert!(near(&found, &[at(600), at(1100)], at(1)), "{found:?}");
}

/// Kicks whose bodies last until the next one, under a steady bed of high noise: one marker per
/// kick (the bodies give none), each at or just before its kick, never after it.
#[test]
fn a_kick_under_a_noise_bed_is_placed_at_its_start_never_after() {
    let mut seed = 5;
    let mut x = bed(at(2400), 0.05, &mut seed);
    let attacks = [at(300), at(800), at(1300), at(1800)];
    for a in attacks {
        put(&mut x, a, &kick(at(480), 3.0));
    }
    let found = detect_transients(&[x], SR, 0.5);
    assert!(placed(&found, &attacks, at(3), at(1)), "{found:?} vs {attacks:?}");
}

/// A hat 8 ms ahead of an unclipped kick, the pair under one flux peak: the kick's plain edge is the
/// stronger, but the marker stays on the hat, the first attack (a marker never moves later than an
/// attack it already sits on).
#[test]
fn a_hat_just_ahead_of_a_kick_keeps_the_marker_on_the_hat() {
    let mut seed = 9;
    let mut x = bed(at(2000), 0.01, &mut seed);
    let attacks = [at(300), at(800), at(1300)];
    for a in attacks {
        put(&mut x, a, &hit(at(40), 0.05, &mut seed));
        put(&mut x, a + at(8), &kick(at(400), 0.9));
    }
    let found = detect_transients(&[x], SR, 0.5);
    assert!(placed(&found, &attacks, at(3), at(1)), "{found:?} vs {attacks:?}");
}
