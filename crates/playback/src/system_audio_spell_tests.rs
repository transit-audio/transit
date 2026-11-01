//! System rows for the device line's lower-envelope fit (#268, the sixth review): spells of
//! lateness that ease off, spells with arrival jitter, periodic lags, a lasting offset, and a
//! seeded random generator of all of them. Simulated time only (`simulate_with`).

use super::*;

/// Run `cb`-frame callbacks for `seconds` at endpoint 0 and -1000 ppm with `opts`, and assert: no
/// dry event, no zeros and nothing discarded after priming; the trim never at its clamp; and the
/// reported drift within 20 ppm (the jitter rows' bound, here at -1000 ppm too) at every callback
/// after 30 s, spells and refills included. No settling after a spell is needed: a late point has
/// no weight in the device line's fit, and after any restart the estimate is held until the
/// refilled line is as well conditioned as the jitter rows assume. Prints the row's figures
/// (`--nocapture`).
fn assert_row(cb: usize, label: &str, seconds: f64, opts: Opts) {
    for true_ppm in [0.0, -1_000.0] {
        let label = format!("cb {cb}, {label}, endpoint {true_ppm:+} ppm");
        let run = simulate_with(&|_| true_ppm, cb, 0.0, seconds, opts);
        println!(
            "row | {label} | worst error {:.1} ppm | max trim {:.0} ppm | over 20 ppm {:.1} s | dry {} zeros {} discarded {} | trim excess {:+.0}",
            run.worst_error, run.max_trim, run.over_s, run.dry, run.zeros, run.discarded, run.trim_excess
        );
        assert_eq!((run.dry, run.zeros), (0, 0), "{label}: ran dry after priming: {run:?}");
        assert_eq!(run.discarded, 0, "{label}: resynchronized: {run:?}");
        assert!(run.max_trim < MAX_TRIM_PPM, "{label}: the trim reached its clamp: {run:?}");
        assert_eq!(run.over_s, 0.0, "{label}: the reading moved: {run:?}");
    }
}

/// The callback due at `t` arrives `delays[i]` ms late, `i` counting the callbacks from 60 s.
fn per_callback(cb: usize, delays: &[f64]) -> impl Fn(f64) -> f64 + '_ {
    move |t| {
        let i = ((t - 60.0) * 48_000.0 / cb as f64).round();
        if i >= 0.0 && (i as usize) < delays.len() { delays[i as usize] * 1e-3 } else { 0.0 }
    }
}

/// A lag from 60 s in `(length s, ms)` segments.
fn lag(segments: &[(f64, f64)]) -> impl Fn(f64) -> f64 + '_ {
    move |t| {
        let mut at = 60.0;
        for &(length, ms) in segments {
            if (at..at + length).contains(&t) {
                return ms * 1e-3;
            }
            at += length;
        }
        0.0
    }
}

#[test]
fn a_spell_of_8192_frame_callbacks_that_eases_off_neither_moves_the_reading_nor_pins_the_trim() {
    // Three callbacks 60 ms late, then 15 and 10, 15 and 15, or 19, 19 and 19 ms: on 1e9e0feb the
    // trim sat at its clamp and the reading at -2000 ppm.
    for (name, delays) in [
        ("60 ms x3 then 15, 10", &[60.0, 60.0, 60.0, 15.0, 10.0][..]),
        ("60 ms x3 then 15, 15", &[60.0, 60.0, 60.0, 15.0, 15.0][..]),
        ("60 ms x3 then 19, 19, 19", &[60.0, 60.0, 60.0, 19.0, 19.0, 19.0][..]),
    ] {
        let delay = per_callback(8_192, delays);
        assert_row(8_192, name, 200.0, Opts { delay: Some(&delay), ..Opts::default() });
    }
}

#[test]
fn a_lag_that_eases_off_neither_moves_the_reading_nor_pins_the_trim() {
    // A 45 ms lag for 0.5 s, then 15 ms for 0.1 s and 18 ms for 1 s: on 1e9e0feb the reading went
    // to -1591..-1715 ppm, and to the clamp at -1000 ppm.
    let segments = [(0.5, 45.0), (0.1, 15.0), (1.0, 18.0)];
    let delay = lag(&segments);
    for cb in [128, 480, 960, 8_192] {
        assert_row(cb, "45 ms lag 0.5 s, 15 ms 0.1 s, 18 ms 1 s", 200.0, Opts { delay: Some(&delay), ..Opts::default() });
    }
}

#[test]
fn spells_with_one_and_three_milliseconds_of_device_jitter_neither_move_the_reading_nor_pin_the_trim() {
    // The committed chained spells and the 8192-frame runs of late callbacks, and a 45 ms lag,
    // each with 1 and 3 ms of device arrival jitter: on 1e9e0feb they read up to -327 ppm.
    for jitter_ms in [1.0, 3.0] {
        for cb in [128, 480, 960] {
            for (pattern, every, delay_ms, length) in
                [("45 ms late every 50 ms for 0.5 s", 0.05, 45.0, 0.5), ("30 ms late every 35 ms for 1 s", 0.035, 30.0, 1.0)]
            {
                let opts = Opts {
                    late: Some((60.0, every, delay_ms * 1e-3)),
                    late_stop: Some(60.0 + length),
                    device_jitter_ms: jitter_ms,
                    ..Opts::default()
                };
                assert_row(cb, &format!("{pattern}, device jitter {jitter_ms} ms"), 200.0, opts);
            }
        }
        let period = 8_192.0 / 48_000.0;
        for n in [3.0, 4.0] {
            let opts =
                Opts { late: Some((60.0, period, 0.06)), late_stop: Some(60.0 + n * period - 1e-6), device_jitter_ms: jitter_ms, ..Opts::default() };
            assert_row(8_192, &format!("{n} callbacks in a row 60 ms late, device jitter {jitter_ms} ms"), 200.0, opts);
        }
        let segments = [(0.5, 45.0)];
        let delay = lag(&segments);
        for cb in [128, 480, 960, 8_192] {
            assert_row(
                cb,
                &format!("45 ms lag 0.5 s, device jitter {jitter_ms} ms"),
                200.0,
                Opts { delay: Some(&delay), device_jitter_ms: jitter_ms, ..Opts::default() },
            );
        }
    }
}

#[test]
fn a_periodic_lag_that_eases_off_and_a_forty_percent_lag_neither_move_the_reading_nor_run_dry() {
    // A 45 ms lag for 400 ms easing to 15 ms for 100 ms, every 5 s (79 restarts and the reading
    // over 20 ppm for 170 s on 1e9e0feb); a 45 ms lag for 400 ms of every second (the estimate
    // never formed, and the reader ran dry 5 to 7 times at -1000 ppm).
    let taper = |t: f64| {
        let phase = (t - 1.0).rem_euclid(5.0);
        if t < 1.0 {
            0.0
        } else if phase < 0.4 {
            0.045
        } else if phase < 0.5 {
            0.015
        } else {
            0.0
        }
    };
    // From 1 s: a lag while the reader first primes makes it run dry once at 128 frames, the
    // ring's start-up and not the estimate's (the reading stays exact; the sixth review saw the
    // same, 352 frames discarded, for a 50 ms hiccup every 200 ms from 0 s on 1e9e0feb).
    let duty = |t: f64| if t >= 1.0 && t.rem_euclid(1.0) < 0.4 { 0.045 } else { 0.0 };
    for cb in [128, 480, 960, 8_192] {
        assert_row(cb, "45-to-15 ms lag every 5 s", 200.0, Opts { delay: Some(&taper), ..Opts::default() });
        assert_row(cb, "45 ms lag for 40 % of every second", 200.0, Opts { delay: Some(&duty), ..Opts::default() });
    }
}

#[test]
fn a_lasting_offset_of_half_a_second_with_device_jitter_keeps_the_reading() {
    // Every callback from 60 s on 500 ms late, with 3 ms of jitter (on 1e9e0feb the reading was
    // 119 to 313 ppm off for 2 to 10 s). The ring sees the device fall 500 ms behind, which is
    // more than it holds: the reader resynchronizes once (the discard and the overflow are the
    // ring's, not the estimate's). Asserted: no dry event, at most that one resynchronization's
    // discard, the trim never at its clamp, and the reading within 20 ppm at every callback after
    // 30 s.
    let delay = |t: f64| if t >= 60.0 { 0.5 } else { 0.0 };
    for cb in [128, 480, 960, 8_192] {
        for true_ppm in [0.0, -1_000.0] {
            let label = format!("cb {cb}, +500 ms lasting offset, device jitter 3 ms, endpoint {true_ppm:+} ppm");
            let run = simulate_with(&|_| true_ppm, cb, 0.0, 200.0, Opts { delay: Some(&delay), device_jitter_ms: 3.0, ..Opts::default() });
            println!(
                "row | {label} | worst error {:.1} ppm | max trim {:.0} ppm | over 20 ppm {:.1} s | dry {} zeros {} discarded {}",
                run.worst_error, run.max_trim, run.over_s, run.dry, run.zeros, run.discarded
            );
            assert_eq!(run.dry, 0, "{label}: ran dry: {run:?}");
            assert!(run.discarded <= 24_000, "{label}: more than one ring's resynchronization: {run:?}");
            assert!(run.max_trim < MAX_TRIM_PPM, "{label}: the trim reached its clamp: {run:?}");
            assert_eq!(run.over_s, 0.0, "{label}: the reading moved: {run:?}");
        }
    }
}

/// A seeded pattern of arrival delays for the generator: device jitter, spells and lasting
/// offsets.
#[derive(Debug)]
struct Pattern {
    cb: usize,
    true_ppm: f64,
    jitter_ms: f64,
    /// `(start s, length s, lateness ms, shape)`: 0 a constant lag, 1 a linear taper to 0, 2 the
    /// lateness for half the spell and a third of it after, 3 the lateness then 15 ms for the
    /// last fifth.
    spells: Vec<(f64, f64, f64, u8)>,
    /// `(start s, length s, ms)`: every callback inside is that much later as well.
    offsets: Vec<(f64, f64, f64)>,
}

impl Pattern {
    fn new(seed: u64, seconds: f64) -> Self {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x268;
        let mut next = move || {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let cb = [128, 480, 960, 8_192][(next() * 4.0) as usize];
        let true_ppm = if next() < 0.5 { 0.0 } else { -1_000.0 };
        let jitter_ms = 3.0 * next();
        let (mut spells, mut t) = (Vec::new(), 1.0 + 9.0 * next());
        while t < seconds {
            let length = 0.1 + 1.9 * next();
            spells.push((t, length, 5.0 + 75.0 * next(), (next() * 4.0) as u8));
            t += length + 1.0 + 14.0 * next();
        }
        // Lasting offsets one after another, never overlapping: two that overlap and end apart
        // leave a net step of their difference, which can be under the band (K97).
        let (mut offsets, mut at) = (Vec::new(), 40.0 + 80.0 * next());
        for _ in 0..(next() * 3.0) as usize {
            let length = 5.0 + 35.0 * next();
            if at + length < seconds - 30.0 {
                offsets.push((at, length, 5.0 + 20.0 * next()));
            }
            at += length + 5.0 + 55.0 * next();
        }
        Self { cb, true_ppm, jitter_ms, spells, offsets }
    }

    fn delay(&self, t: f64) -> f64 {
        let mut late = 0.0f64;
        for &(at, length, ms, shape) in &self.spells {
            if (at..at + length).contains(&t) {
                let u = (t - at) / length;
                late = late.max(match shape {
                    0 => ms,
                    1 => ms * (1.0 - u),
                    2 if u < 0.5 => ms,
                    2 => ms / 3.0,
                    _ if u < 0.8 => ms,
                    _ => ms.min(15.0),
                });
            }
        }
        let offset: f64 = self.offsets.iter().filter(|(at, length, _)| (*at..at + length).contains(&t)).map(|(_, _, ms)| ms).sum();
        (late + offset) * 1e-3
    }
}

#[test]
fn fifty_seeded_patterns_of_spells_jitter_and_lasting_offsets_neither_move_the_reading_nor_pin_the_trim() {
    // Per seed: the callback size (128, 480, 960 or 8192), the endpoint (0 or -1000 ppm), device
    // jitter of 0 to 3 ms, spells every 1 to 15 s of 0.1 to 2 s at 5 to 80 ms in four shapes, and
    // up to two lasting offsets of 5 to 25 ms for 5 to 40 s that start and end, one after the
    // other; 300 s. Lasting offsets within the 1 ms band are K97; over 25 ms the ring itself
    // resynchronizes when they start and runs dry when they end, which is not the estimate's
    // doing.
    let mut failures = Vec::new();
    for seed in 0..50 {
        let pattern = Pattern::new(seed, 300.0);
        let delay = |t: f64| pattern.delay(t);
        let opts = Opts { delay: Some(&delay), device_jitter_ms: pattern.jitter_ms, ..Opts::default() };
        let run = simulate_with(&|_| pattern.true_ppm, pattern.cb, 0.0, 300.0, opts);
        println!(
            "seed {seed} | cb {} {:+} ppm jitter {:.2} ms, {} spells, offsets {:?} | worst error {:.1} ppm | max trim {:.0} | over {:.1} s | dry {} zeros {} discarded {}",
            pattern.cb,
            pattern.true_ppm,
            pattern.jitter_ms,
            pattern.spells.len(),
            pattern.offsets,
            run.worst_error,
            run.max_trim,
            run.over_s,
            run.dry,
            run.zeros,
            run.discarded
        );
        if run.dry > 0 || run.zeros > 0 || run.discarded > 0 || run.max_trim >= MAX_TRIM_PPM || run.over_s > 0.0 {
            failures.push(format!("seed {seed}: {pattern:?}: {run:?}"));
        }
    }
    assert!(failures.is_empty(), "{} of 50 seeds failed:\n{}", failures.len(), failures.join("\n"));
}
