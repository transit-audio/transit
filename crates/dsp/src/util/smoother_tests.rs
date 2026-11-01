//! Sample-count bounds for parameter settling; no wall-clock timing.

use crate::util::{Rng, Smoother, time_coef};

fn check_settle(start: f32, target: f32, ms: f32, sr: f32, block: usize) -> usize {
    let mut smoother = Smoother::new(start);
    smoother.set_time(ms, sr);
    smoother.set(target);
    // A 1 -> 0.01 step takes about 13.86 constants to reach the existing 1e-6
    // tolerance at 20 ms / 48 kHz. Twelve cannot bound every moving transition.
    let limit = (16.0 * ms * 0.001 * sr).ceil() as usize;
    let mut elapsed = 0;
    while smoother.settling() && elapsed < limit {
        let before = smoother.value();
        let n = block.min(limit - elapsed);
        let value = if block == 1 { smoother.next() } else { smoother.advance(n) };
        let distance = (f64::from(value) - f64::from(target)).abs();
        let previous_distance = (f64::from(before) - f64::from(target)).abs();
        assert!(distance <= previous_distance, "{start}->{target}, {ms} ms / {sr} Hz, block {block}, sample {elapsed}: moved away");
        assert!((start.min(target)..=start.max(target)).contains(&value), "overshot {target}");
        elapsed += n;
    }
    assert!(!smoother.settling(), "{start}->{target}, {ms} ms / {sr} Hz, block {block}: still settling at {}", smoother.value());
    assert_eq!(smoother.value().to_bits(), target.to_bits());
    for _ in 0..256 {
        assert_eq!(smoother.next().to_bits(), target.to_bits());
        assert_eq!(smoother.advance(block).to_bits(), target.to_bits());
        assert!(!smoother.settling());
    }
    elapsed
}

#[test]
fn target_sweep_settles_monotonically_and_stays_exact() {
    let mut targets = vec![0.01, 0.05, 0.1, 0.2, 0.45, 0.5, 0.75, 0.9, 1.0];
    let mut rng = Rng::new(279);
    targets.extend((0..4).map(|_| 0.01 + (rng.next_u32() >> 8) as f32 * (0.99 / 16_777_216.0)));
    let mut worst_constants = 0.0f32;
    let mut cases = 0;
    for target in targets {
        for start in [0.0, 1.0] {
            for ms in [20.0, 1000.0] {
                for sr in [44_100.0, 48_000.0, 96_000.0] {
                    let elapsed = check_settle(start, target, ms, sr, 1);
                    worst_constants = worst_constants.max(elapsed as f32 / (ms * 0.001 * sr));
                    cases += 1;
                }
            }
        }
    }
    println!("{cases} sample-rate transitions settled; maximum {worst_constants} time constants");
}

#[test]
fn block_updates_and_non_normalized_targets_settle() {
    for (start, target) in [(1.0, 0.01), (0.0, 0.45), (1.0, 0.0), (-1.0, -0.45), (0.0, -0.75), (1000.0, 440.0), (-1000.0, 440.0)] {
        for ms in [20.0, 1000.0] {
            for sr in [44_100.0, 48_000.0, 96_000.0] {
                for block in [1, 32, 256] {
                    check_settle(start, target, ms, sr, block);
                }
            }
        }
    }
}

#[test]
fn issue_279_reaches_point_45_exactly() {
    let mut smoother = Smoother::new(0.0);
    smoother.set_time(20.0, 48_000.0);
    smoother.set(0.45);
    for _ in 0..11_520 {
        smoother.next();
    }
    assert!(!smoother.settling(), "after 12 time constants: value={}, target=0.45, distance={}", smoother.value(), (smoother.value() - 0.45).abs());
    for _ in 0..256 {
        assert_eq!(smoother.next().to_bits(), 0.45f32.to_bits());
        assert!(!smoother.settling());
    }
}

/// Replays `chunks` (cycled) against an independent copy of the old recurrence.
/// Until the smoother snaps, every value must match it bit for bit.
fn check_original_bits(start: f32, target: f32, ms: f32, sr: f32, chunks: &[usize]) {
    let mut smoother = Smoother::new(start);
    smoother.set_time(ms, sr);
    smoother.set(target);
    let coef = time_coef(ms, sr);
    let mut original = start;
    let mut snapped = false;
    let mut elapsed = 0usize;
    let mut index = 0usize;
    while elapsed < 1_536_000 {
        let block = chunks[index % chunks.len()];
        index += 1;
        elapsed += block;
        let k = if block == 1 { coef } else { coef.powi(block as i32) };
        let previous = original;
        original = target + (original - target) * k;
        let within_tolerance = (original - target).abs() <= 1.0e-6 * (1.0 + target.abs());
        let value = if block == 1 { smoother.next() } else { smoother.advance(block) };
        if !smoother.settling() {
            assert_eq!(value.to_bits(), target.to_bits());
            assert!(within_tolerance || original == previous, "only a tolerance or no-progress snap may change bits");
            snapped = true;
            break;
        }
        assert!(!within_tolerance);
        assert_eq!(value.to_bits(), original.to_bits(), "{start}->{target}, {ms} ms / {sr} Hz, chunks {chunks:?}");
    }
    assert!(snapped, "{start}->{target}, {ms} ms / {sr} Hz, chunks {chunks:?}");
}

#[test]
fn every_step_before_the_snap_keeps_the_original_bits() {
    for (start, target) in [(0.0, 0.45), (1.0, 0.01), (-1.0, -0.45), (1000.0, 440.0)] {
        // Uniform blocks at 1 s / 96 kHz and at 20 ms / 48 kHz.
        for (ms, sr) in [(1000.0, 96_000.0), (20.0, 48_000.0)] {
            for block in [1usize, 32, 256] {
                check_original_bits(start, target, ms, sr, &[block]);
            }
        }
        // The EQ's pattern: 32-frame chunks closed by a short tail of 1 to 8 frames.
        for tail in 1..=8usize {
            check_original_bits(start, target, 20.0, 48_000.0, &[32, 32, 32, tail]);
            check_original_bits(start, target, 20.0, 48_000.0, &[32, tail]);
        }
    }
}

#[test]
fn advancing_one_sample_matches_next_through_the_snap() {
    let mut sample = Smoother::new(0.0);
    sample.set_time(20.0, 48_000.0);
    sample.set(0.45);
    let mut block = sample;
    for _ in 0..11_520 {
        assert_eq!(sample.next().to_bits(), block.advance(1).to_bits());
        assert_eq!(sample.settling(), block.settling());
    }
    assert!(!block.settling());
}

#[test]
fn an_empty_block_does_not_trigger_the_no_progress_snap() {
    let mut smoother = Smoother::new(1.0);
    smoother.set_time(1000.0, 96_000.0);
    smoother.set(0.45);
    for _ in 0..256 {
        assert_eq!(smoother.advance(0).to_bits(), 1.0f32.to_bits());
        assert!(smoother.settling());
    }
}
