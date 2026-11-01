use super::*;

/// Feed `seconds` of a 48 kHz device in `cb`-frame callbacks and an endpoint `ppm` fast in
/// 480-frame packets, both on exact times, and return the estimate in ppm.
fn run(ppm: f64, cb: u64, seconds: f64, arrival_jitter_ns: i64) -> Option<f64> {
    let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
    let (mut device, mut packet, mut seed) = (0u64, 0u64, 7u64);
    let ep_rate = 48_000.0 * (1.0 + ppm * 1e-6);
    while (device as f64) < seconds * 48_000.0 {
        let t = device as f64 / 48_000.0;
        while (packet * 480) as f64 / ep_rate <= t {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let jitter = if arrival_jitter_ns > 0 { (seed >> 33) as i64 % arrival_jitter_ns } else { 0 };
            let time_ns = ((packet * 480) as f64 / ep_rate * 1e9) as i64 + jitter;
            est.endpoint(Obs { first_frame: packet * 480, frames: 480, time_ns, kind: Kind::Position, domain: Domain::Qpc });
            packet += 1;
        }
        est.device(device, (t * 1e9) as i64, Kind::Host, Domain::Qpc);
        device += cb;
    }
    est.drift().map(|d| d * 1e6)
}

#[test]
fn the_estimate_reads_the_drift_whatever_the_callback() {
    for cb in [128, 480, 960, 2048] {
        for ppm in [0.0, 50.0, -50.0, 1_000.0, -1_000.0] {
            let got = run(ppm, cb, 30.0, 0).expect("an estimate after 30 s");
            assert!((got - ppm).abs() < 1.0, "cb {cb}, {ppm} ppm read as {got}");
        }
    }
}

#[test]
fn arrival_times_jittered_by_three_milliseconds_still_read_within_twenty_ppm() {
    for ppm in [0.0, 200.0, -500.0] {
        let got = run(ppm, 480, 60.0, 3_000_000).expect("an estimate");
        assert!((got - ppm).abs() < 20.0, "{ppm} ppm read as {got}");
    }
}

#[test]
fn no_estimate_before_two_seconds_and_a_change_of_kind_restarts_the_line_but_holds_the_estimate() {
    assert_eq!(run(100.0, 480, 1.5, 0), None);
    let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
    for k in 0..400u64 {
        let t = (k * 10_000_000) as i64;
        est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
        est.device(k * 480, t, Kind::Host, Domain::Qpc);
    }
    let held = est.drift().expect("4 s of points");
    assert!(held.abs() < 1e-9);
    est.device(400 * 480, 4_000_000_000, Kind::Arrival, Domain::Qpc);
    assert_eq!(est.drift(), Some(held), "the restarted device line is short: the estimate is held");
    assert_eq!(est.device.len, 1);
}

#[test]
fn a_pair_on_different_clock_domains_holds_the_estimate_instead_of_computing_one() {
    let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
    for k in 0..600u64 {
        let t = (k * 10_000_000) as i64;
        est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
        est.device(k * 480, t, Kind::Host, Domain::Driver);
    }
    assert_eq!(est.drift(), None, "no estimate from a pair that shares no clock");
    for k in 600..1200u64 {
        let t = (k * 10_000_000) as i64;
        est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
        est.device(k * 480, t, Kind::Host, Domain::Qpc);
    }
    let held = est.drift().expect("the device line is on QPC again and has refilled");
    assert!(held.abs() < 1e-9);
    for k in 1200..1800u64 {
        let t = (k * 10_000_000) as i64;
        est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
        est.device(k * 480, t, Kind::Host, Domain::Driver);
    }
    assert_eq!(est.drift(), Some(held), "mixed again: the last estimate is held");
}

#[test]
fn a_step_in_time_with_no_frames_restarts_the_line_and_the_reading_holds() {
    for stall_s in [0.5, 2.0, 8.0] {
        let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
        let step = (stall_s * 1e9) as i64;
        for k in 0..2000u64 {
            // 20 s of both sides on exact time; at 10 s the endpoint's time steps and its frames do not.
            let t = (k * 10_000_000) as i64;
            let endpoint_t = if k < 1000 { t } else { t + step };
            est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: endpoint_t, kind: Kind::Position, domain: Domain::Qpc });
            est.device(k * 480, t, Kind::Host, Domain::Qpc);
            if let Some(d) = est.drift() {
                assert!(d.abs() < 1e-6, "{stall_s} s stall: the reading left its bound ({d}) at point {k}");
            }
            if k == 1000 {
                assert_eq!(est.endpoint.len, 1, "{stall_s} s stall: the endpoint line restarted at the step");
            }
        }
    }
}

/// Both sides on exact time, 480 frames per 10 ms, for `points` points; `device_late(k)` is how
/// late the device's point `k` arrives, in ns. Returns the estimate after each point and the
/// device line's length.
fn run_device_late(points: u64, device_late: &dyn Fn(u64) -> i64) -> (Box<RateEstimate>, Vec<(Option<f64>, usize)>) {
    let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
    let mut seen = Vec::new();
    for k in 0..points {
        let t = (k * 10_000_000) as i64;
        est.endpoint(Obs { first_frame: k * 480, frames: 480, time_ns: t, kind: Kind::Position, domain: Domain::Qpc });
        est.device(k * 480, t + device_late(k), Kind::Host, Domain::Qpc);
        seen.push((est.drift(), est.device.len));
    }
    (est, seen)
}

#[test]
fn a_late_arrival_that_the_next_blocks_catch_up_is_ignored_and_the_reading_holds() {
    for late_ms in [21, 45, 60, 120, 500] {
        // Point 1000 arrives late; the points due inside the delay arrive with it.
        let late = late_ms * 1_000_000;
        let (_, seen) = run_device_late(2000, &|k| {
            let behind = (k as i64 - 1000) * 10_000_000;
            if (0..late).contains(&behind) { late - behind } else { 0 }
        });
        for (k, (drift, len)) in seen.iter().enumerate() {
            if let Some(d) = drift {
                assert!(d.abs() < 1e-6, "{late_ms} ms late: the reading left its bound ({d}) at point {k}");
            }
            if k > 0 && k < 2000 {
                assert!(*len >= seen[k - 1].1, "{late_ms} ms late: the device line restarted at point {k}");
            }
        }
        assert!(seen[1999].0.is_some());
    }
}

#[test]
fn a_lasting_offset_on_an_arrival_timed_line_restarts_it_once_confirmed() {
    // From point 1000 on, every device point is 200 ms late: the device lost frames. The late
    // points are stored and given no weight by the envelope fit; once they have stayed above the
    // band for 3 s (and 32 points) the line restarts on the point that confirms it.
    let (est, seen) = run_device_late(2000, &|k| if k >= 1000 { 200_000_000 } else { 0 });
    let restarts: Vec<usize> = (1..2000).filter(|&k| seen[k].1 < seen[k - 1].1).collect();
    assert_eq!(restarts, [1300], "the offset is confirmed 3 s after it began, once");
    assert_eq!(seen[1299].1, 1300, "the late points are stored until then");
    for (k, (drift, _)) in seen.iter().enumerate() {
        if let Some(d) = drift {
            assert!(d.abs() < 1e-6, "the reading left its bound ({d}) at point {k}: held while the line refills");
        }
    }
    assert_eq!(est.device.len, 700);
}

#[test]
fn a_lasting_offset_that_ends_restarts_the_line_again_on_the_first_point_back_on_time() {
    // 8192-frame callbacks (170.7 ms apart): callbacks 400 to 699 are 60 ms late (a lasting
    // offset), then the device is on time again. The offset is confirmed after 32 callbacks above
    // the band (5.3 s), and callback 700, 60 ms below the envelope of the restarted line, restarts
    // it again: no delay puts a point below the envelope, so the points before it carried one.
    let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
    let (mut restarts, mut prev_len, mut last_estimate) = (Vec::new(), 0usize, None);
    for k in 0..1400u64 {
        let t = (k * 8192) as f64 / 48_000.0;
        let late = if (400..700).contains(&k) { 0.06 } else { 0.0 };
        est.device(k * 8192, ((t + late) * 1e9) as i64, Kind::Host, Domain::Qpc);
        let packets = ((k * 8192) / 480)..(((k + 1) * 8192) / 480);
        for p in packets {
            est.endpoint(Obs { first_frame: p * 480, frames: 480, time_ns: (p * 10_000_000) as i64, kind: Kind::Position, domain: Domain::Qpc });
        }
        if est.device.len < prev_len {
            restarts.push(k);
        }
        prev_len = est.device.len;
        if let Some(d) = est.drift() {
            assert!(d.abs() < 1e-6, "the reading left its bound ({d}) at callback {k}: held while the line refills");
            last_estimate = Some(k);
        }
    }
    assert_eq!(restarts, [431, 700], "confirmed on the 32nd callback above the band, then on the first callback back on time");
    assert_eq!(last_estimate, Some(1399));
    assert_eq!(est.device.len, 700, "the line refilled from callback 700");
}

#[test]
fn a_line_restarted_on_a_lasting_offset_gives_the_points_with_more_delay_no_weight() {
    // A 200 ms offset from point 1000, confirmed at point 1300, which carries 15 ms more (a spell
    // on top of the offset, easing off by 0.5 ms a point): the restarted line's lower envelope is
    // the offset, so the refilled line gives the spell and its anchor no weight.
    let (mut est, seen) = run_device_late(2600, &|k| match k {
        1000..1300 => 200_000_000,
        1300..1330 => 200_000_000 + (1330 - k as i64) * 500_000,
        1330.. => 200_000_000,
        _ => 0,
    });
    assert_eq!(seen[1300].1, 1, "the line restarted on the confirming point, 15 ms later than the offset");
    for (k, (drift, _)) in seen.iter().enumerate() {
        if let Some(d) = drift {
            assert!(d.abs() < 1e-6, "the reading left its bound ({d}) at point {k}");
        }
    }
    assert!((1301..2600).all(|k| seen[k].1 >= seen[k - 1].1), "no restart after it");
    let fresh = est.device.fit().expect("the refilled line fits").0;
    // The taper's last two points, 1 and 0.5 ms late, lie inside the band and keep their weight:
    // that is all a delay can move the fit by (0.37 ppm here), against 15 ms at full leverage.
    assert!((fresh / 48_000.0 - 1.0).abs() < 1e-6, "the refilled line's own fit reads the device within 1 ppm: {fresh}");
}

#[test]
fn a_device_a_little_fast_against_the_time_base_still_grows_its_line() {
    // 128-frame callbacks whose times run 50 ppm short: every point is a little earlier than
    // the newest one predicts at the nominal rate. Nothing restarts the line on that, so it grows
    // and reads the drift.
    let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
    for k in 0..11_250u64 {
        let t = (k * 128) as f64 / 48_000.0;
        est.device(k * 128, (t * (1.0 - 50e-6) * 1e9) as i64, Kind::Host, Domain::Qpc);
        let packet = (t * 100.0) as u64;
        est.endpoint(Obs {
            first_frame: packet * 480,
            frames: 480,
            time_ns: (packet * 10_000_000) as i64,
            kind: Kind::Position,
            domain: Domain::Qpc,
        });
    }
    assert!(est.device.len >= 2000, "the device line grew: {}", est.device.len);
    let got = est.drift().expect("an estimate after 30 s") * 1e6;
    assert!((got + 50.0).abs() < 1.0, "the device timed 50 ppm fast reads the endpoint 50 ppm slow: {got}");
}

/// What [`probe`] saw after 30 s.
#[derive(Debug)]
struct Probe {
    /// The reading's largest error, in ppm, and the time it was more than 20 ppm off.
    worst_ppm: f64,
    over_s: f64,
    /// Restarts of the device line, and callbacks without an estimate.
    restarts: u32,
    missing: u32,
}

/// `seconds` of a 48 kHz device in `cb`-frame callbacks against an endpoint `ppm` fast in
/// 480-frame packets on exact times. The callback due at `t` arrives `delay(t)` seconds late, plus
/// 0 to `jitter_ms` (uniform, deterministic from `seed`), and never before the one before it (a
/// block that comes back on time catches up behind a late one, as on WASAPI and ASIO).
fn probe(cb: u64, ppm: f64, seconds: f64, delay: &dyn Fn(f64) -> f64, jitter_ms: f64, seed: u64) -> Probe {
    let mut est = Box::new(RateEstimate::new(48_000.0, 48_000.0));
    let ep_rate = 48_000.0 * (1.0 + ppm * 1e-6);
    let (mut packet, mut state, mut arrived, mut prev_len) = (0u64, seed, 0.0f64, 0usize);
    let mut seen = Probe { worst_ppm: 0.0, over_s: 0.0, restarts: 0, missing: 0 };
    for k in 0..(seconds * 48_000.0 / cb as f64) as u64 {
        let t = (k * cb) as f64 / 48_000.0;
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        let at = (t + delay(t) + (state >> 11) as f64 / (1u64 << 53) as f64 * jitter_ms * 1e-3).max(arrived);
        arrived = at;
        while (packet * 480) as f64 / ep_rate <= at {
            let time_ns = ((packet * 480) as f64 / ep_rate * 1e9) as i64;
            est.endpoint(Obs { first_frame: packet * 480, frames: 480, time_ns, kind: Kind::Position, domain: Domain::Qpc });
            packet += 1;
        }
        est.device(k * cb, (at * 1e9) as i64, Kind::Host, Domain::Qpc);
        seen.restarts += u32::from(est.device.len < prev_len);
        prev_len = est.device.len;
        let drift = est.drift();
        if t >= 30.0 {
            match drift {
                Some(d) => {
                    let error = (d * 1e6 - ppm).abs();
                    seen.worst_ppm = seen.worst_ppm.max(error);
                    seen.over_s += if error > 20.0 { cb as f64 / 48_000.0 } else { 0.0 };
                }
                None => seen.missing += 1,
            }
        }
    }
    seen
}

/// A callback-indexed schedule from 60 s: the `i`th callback due from then arrives `delays[i]` ms
/// late.
fn per_callback(cb: u64, delays: &'static [f64]) -> impl Fn(f64) -> f64 {
    move |t| {
        let i = ((t - 60.0) * 48_000.0 / cb as f64).round();
        if i >= 0.0 && (i as usize) < delays.len() { delays[i as usize] * 1e-3 } else { 0.0 }
    }
}

/// A lag from 60 s in `(length s, ms)` segments: every callback due inside a segment arrives that
/// late.
fn lag(segments: &'static [(f64, f64)]) -> impl Fn(f64) -> f64 {
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

/// Assert the reading within 20 ppm at every callback after 30 s, an estimate at each of them, and
/// `restarts` restarts of the device line.
fn assert_probe(label: &str, seen: &Probe, restarts: u32) {
    assert!(seen.worst_ppm <= 20.0, "{label}: the reading left its 20 ppm bound: {seen:?}");
    assert_eq!(seen.missing, 0, "{label}: no estimate after 30 s: {seen:?}");
    assert_eq!(seen.restarts, restarts, "{label}: restarts: {seen:?}");
}

#[test]
fn a_spell_at_8192_frames_that_eases_off_moves_neither_the_reading_nor_the_line() {
    // The sixth review of #268: three callbacks 60 ms late, then 15 and 10, 15 and 15, or 19, 19
    // and 19 ms; and a ramp of 19, 38, 15 and 15 ms. Read 1285 to 6337 ppm off on 1e9e0feb.
    const TAPERS: [(&str, &[f64]); 4] = [
        ("60 ms x3, then 15, 10", &[60.0, 60.0, 60.0, 15.0, 10.0]),
        ("60 ms x3, then 15, 15", &[60.0, 60.0, 60.0, 15.0, 15.0]),
        ("60 ms x3, then 19, 19, 19", &[60.0, 60.0, 60.0, 19.0, 19.0, 19.0]),
        ("19, 38, 15, 15 ms", &[19.0, 38.0, 15.0, 15.0]),
    ];
    for (name, delays) in TAPERS {
        for ppm in [0.0, -1_000.0] {
            let seen = probe(8_192, ppm, 200.0, &per_callback(8_192, delays), 0.0, 1);
            assert_probe(&format!("8192 frames, {name}, endpoint {ppm:+} ppm"), &seen, 0);
        }
    }
}

#[test]
fn a_lag_that_eases_off_moves_neither_the_reading_nor_the_line() {
    // A 45 ms lag for 0.5 s that eases to 15 ms for 0.1 s, or to 18 ms for 1 s. Read 1591 to
    // 13 234 ppm off at 128, 480 and 960 frames on 1e9e0feb.
    const LAGS: [(&str, &[(f64, f64)]); 2] =
        [("45 ms 0.5 s, 15 ms 0.1 s", &[(0.5, 45.0), (0.1, 15.0)]), ("45 ms 0.5 s, 18 ms 1 s", &[(0.5, 45.0), (1.0, 18.0)])];
    for cb in [128, 480, 960, 8_192] {
        for (name, segments) in LAGS {
            for ppm in [0.0, -1_000.0] {
                let seen = probe(cb, ppm, 200.0, &lag(segments), 0.0, 1);
                assert_probe(&format!("{cb} frames, {name}, endpoint {ppm:+} ppm"), &seen, 0);
            }
        }
    }
}

#[test]
fn a_lag_with_one_or_three_milliseconds_of_jitter_moves_neither_the_reading_nor_the_line() {
    // A 45 ms lag for 0.5 s with 1 or 3 ms of arrival jitter. Read 25 to 439 ppm off for up to
    // 10.6 s on 1e9e0feb (the restarted line's anchor carried the jitter).
    for cb in [128, 480, 960, 8_192] {
        for jitter_ms in [1.0, 3.0] {
            for ppm in [0.0, -1_000.0] {
                let seen = probe(cb, ppm, 200.0, &lag(&[(0.5, 45.0)]), jitter_ms, 7);
                assert_probe(&format!("{cb} frames, 45 ms lag, jitter {jitter_ms} ms, endpoint {ppm:+} ppm"), &seen, 0);
            }
        }
    }
}

#[test]
fn a_lasting_offset_of_half_a_second_with_three_milliseconds_of_jitter_restarts_the_line_once() {
    // Every callback from 60 s on 500 ms late (a device that lost frames), with 3 ms of jitter.
    // Read 119 to 313 ppm off for 2 to 10 s on 1e9e0feb. The line restarts once, 3 s in (5.5 s at
    // 8192 frames), and the estimate is held until the refilled line is well conditioned.
    for cb in [128, 480, 960, 8_192] {
        for ppm in [0.0, -1_000.0] {
            let seen = probe(cb, ppm, 200.0, &|t| if t >= 60.0 { 0.5 } else { 0.0 }, 3.0, 7);
            assert_probe(&format!("{cb} frames, +500 ms lasting offset, jitter 3 ms, endpoint {ppm:+} ppm"), &seen, 1);
        }
    }
}

#[test]
fn a_periodic_lag_that_eases_off_and_a_forty_percent_lag_never_restart_the_line() {
    // The sixth review's worst periodic pattern (a 45 ms lag for 400 ms easing to 15 ms for
    // 100 ms, every 5 s: 79 restarts and the reading over 20 ppm for all 170 s after 30 s on
    // 1e9e0feb), and a 45 ms lag for 400 ms of every second (the estimate never formed).
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
    let duty = |t: f64| if t.rem_euclid(1.0) < 0.4 { 0.045 } else { 0.0 };
    for cb in [128, 480, 960, 8_192] {
        for ppm in [0.0, -1_000.0] {
            assert_probe(&format!("{cb} frames, periodic 45-to-15 ms lag, endpoint {ppm:+} ppm"), &probe(cb, ppm, 200.0, &taper, 0.0, 7), 0);
            assert_probe(&format!("{cb} frames, 40 % lag, endpoint {ppm:+} ppm"), &probe(cb, ppm, 200.0, &duty, 0.0, 7), 0);
        }
    }
}

#[test]
fn lasting_offsets_over_the_band_that_begin_and_end_read_within_twenty_ppm() {
    // Every callback from 60 s to 100 s late by `ms` (2 ms and up without jitter, 5 ms and up
    // with 3 ms of it): the line restarts when the offset is confirmed and again on the first
    // callback back below the envelope, and the estimate is held while it refills. (Smaller
    // offsets are K100.)
    for cb in [128, 480, 960, 8_192] {
        for (ms, jitter_ms) in [(2.0, 0.0), (5.0, 0.0), (19.0, 0.0), (5.0, 3.0), (10.0, 3.0), (19.0, 3.0)] {
            let seen = probe(cb, 0.0, 200.0, &move |t| if (60.0..100.0).contains(&t) { ms * 1e-3 } else { 0.0 }, jitter_ms, 11);
            let label = format!("{cb} frames, {ms} ms offset for 40 s, jitter {jitter_ms} ms");
            assert!(seen.worst_ppm <= 20.0 && seen.missing == 0, "{label}: {seen:?}");
            assert!(seen.restarts >= 2, "{label}: the offset's start and end restart the line: {seen:?}");
        }
    }
}
