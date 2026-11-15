//! Exact values of the reader (ADR 0056, decisions 5.3, 8.2, 8.3 and 8.4): the table's exact ones and
//! zeros, the sample itself at integer positions when q ≤ 1, the direct-index path at q = 1, the
//! response to a constant within its measured limits, the record's own quoted reader outputs, and an
//! independently written exact kernel.

mod common;

use transit_resample::{Interleaved, PHASES, SincTable, TABLE_LEN, direct, read, read_frame, taps};

fn table() -> &'static SincTable {
    SincTable::shared()
}

#[test]
fn the_table_holds_exactly_one_at_zero_and_zero_at_every_other_integer() {
    let t = table();
    let (v, d) = (t.values(), t.deltas());
    assert_eq!((v.len(), d.len()), (TABLE_LEN, TABLE_LEN));
    assert_eq!(v[0].to_bits(), 1.0f32.to_bits());
    for n in 1..=16 {
        assert_eq!(v[n * PHASES].to_bits(), 0.0f32.to_bits(), "T[{}] at u = {n}", n * PHASES);
    }
    for i in 0..TABLE_LEN - 1 {
        assert_eq!(d[i].to_bits(), (v[i + 1] - v[i]).to_bits(), "D[{i}]");
    }
    assert_eq!(d[TABLE_LEN - 1], 0.0);
    // The table's fingerprint (FNV-1a over the bits of T then D), reported for the cross-platform
    // comparison in the build note; not pinned (decision 13.4: in-process before ADR 0055's goldens).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for x in v.iter().chain(d.iter()) {
        for b in x.to_bits().to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    println!("table fingerprint {h:#018x}");
}

#[test]
fn the_reader_returns_the_sample_at_integer_positions_when_q_is_at_most_one() {
    let t = table();
    let src = common::noise(7, 300);
    for q in [1.0, 0.999_999_9, 0.918_75, 0.5, 2f64.powf(-50.0 / 12.0), 1.0 / 32.0] {
        for k in -40i64..340 {
            let y = read(t, &src, k as f64, q);
            let want = if (0..300).contains(&k) { src[k as usize] } else { 0.0 };
            assert_eq!(y.to_bits(), want.to_bits(), "q = {q}, k = {k}");
        }
    }
    // Interleaved stereo: both channels exact.
    let st = common::noise(8, 400);
    let src2 = Interleaved::new(&st, 2);
    let mut out = [0.0f32; 2];
    for k in 0..200i64 {
        read_frame(t, &src2, k as f64, 0.75, &mut out);
        assert_eq!(out[0].to_bits(), st[2 * k as usize].to_bits());
        assert_eq!(out[1].to_bits(), st[2 * k as usize + 1].to_bits());
    }
}

#[test]
fn q_one_is_the_direct_read_bit_for_bit() {
    let t = table();
    let src = common::noise(9, 256);
    for k in -5i64..260 {
        assert_eq!(direct(k as f64, 1.0), Some(k));
        let y = read(t, &src, k as f64, 1.0);
        let want = if (0..256).contains(&k) { src[k as usize] } else { 0.0 };
        assert_eq!(y.to_bits(), want.to_bits(), "k = {k}");
    }
    // Every other rate, and every fractional position, goes through the reader.
    for (x, q) in [(10.5, 1.0), (10.0, 1.000_000_000_000_000_2), (10.0, 0.999_999_999_999_999_9), (10.0, 2.0), (10.0, 0.5), (f64::NAN, 1.0)] {
        assert_eq!(direct(x, q), None, "x = {x}, q = {q}");
    }
}

#[test]
fn a_constant_comes_out_within_its_measured_limits() {
    // Decision 8.4, measured with the full f32 reader and rounded outward: 4.2e-5 at q <= 1, 5.2e-5 for
    // 1 < q < 1.5 and 3.7e-5 for 1.5 <= q <= 32.
    let t = table();
    let ones = vec![1.0f32; 4096];
    for (q, n, limit) in [
        (0.5, 65_536u32, 4.2e-5),
        (1.010_95, 32_768, 5.2e-5),
        (1.5, 32_768, 3.7e-5),
        (1.507_22, 32_768, 3.7e-5),
        (2.0, 32_768, 3.7e-5),
        (32.0, 32_768, 3.7e-5),
    ] {
        let mut worst = 0.0f64;
        for j in 0..n {
            let x = 2000.0 + f64::from(j) / f64::from(n);
            worst = worst.max((f64::from(read(t, &ones, x, q)) - 1.0).abs());
        }
        println!("constant: q = {q}: worst |y - 1| = {worst:.4e} (limit {limit:.1e})");
        assert!(worst <= limit, "q = {q}: {worst:e} > {limit:e}");
    }
}

#[test]
fn the_records_quoted_reader_outputs_are_reproduced() {
    let t = table();
    // Decision 8.4: at q = 32, position fraction 0.7249755859375 of a constant 1.0 gives 0.9999704360961914.
    let ones = vec![1.0f32; 4096];
    let y = read(t, &ones, 2000.7249755859375, 32.0);
    println!("constant at q = 32, fraction 0.7249755859375: {y:.16}");
    assert_eq!(f64::from(y), 0.999_970_436_096_191_4);
    // Decision 6.3: at q = 2 the reader's output at position 298 of the ramp x[i] = i is 297.99399.
    let ramp: Vec<f32> = (0..1000).map(|i| i as f32).collect();
    let y = read(t, &ramp, 298.0, 2.0);
    println!("ramp at q = 2, position 298: {y:.8}");
    assert!((f64::from(y) - 297.993_99).abs() < 5e-6, "{y}");
}

#[test]
fn the_table_matches_an_independently_written_kernel() {
    // Decision 8.4's coefficient figure: the interpolated table is within 1.7e-6 of the exact kernel
    // (worst found 1.6263e-6 near u = 7.5/512), 64 points per table step.
    let t = table();
    let mut worst = (0.0f64, 0.0f64);
    for i in 0..(TABLE_LEN - 1) {
        for s in 0..64 {
            let u = (i as f64 + f64::from(s) / 64.0) / PHASES as f64;
            let e = (f64::from(t.coefficient(u)) - common::exact_kernel(u)).abs();
            if e > worst.0 {
                worst = (e, u);
            }
        }
    }
    println!("table: worst |coefficient - h| = {:.4e} at u = {:.6} ({:.2}/512)", worst.0, worst.1, worst.1 * 512.0);
    assert!(worst.0 <= 1.7e-6, "{worst:?}");
}

#[test]
fn the_reader_matches_a_reference_that_sums_the_exact_kernel() {
    // The reader against `Σ s_k·h(|x − k|/m)/m` in binary64 with the independently written kernel: the
    // difference is bounded by the coefficient figure (1.7e-6 a tap, scaled by 1/m) plus the f32
    // rounding of the products and the sum.
    let t = table();
    let src = common::noise(11, 3000);
    let mut worst_rel = 0.0f64;
    for q in [0.5, 0.918_75, 1.088_435_374_149_659_8, 1.5, 2.0, 2.176_870_748_299_32, 3.675, 4.0, 16.47, 32.0] {
        for j in 0..64 {
            let x = 1400.0 + f64::from(j) * 0.618_034;
            let y = f64::from(read(t, &src, x, q));
            let r = common::exact_read(&src, x, q);
            let m = q.max(1.0);
            let abs_sum: f64 = taps(x, q).map(|k| f64::from(src[k as usize].abs())).sum();
            let n = taps(x, q).count() as f64;
            let bound = abs_sum * (1.7e-6 / m + (n + 2.0) * f64::from(f32::EPSILON) / m);
            let e = (y - r).abs();
            worst_rel = worst_rel.max(e / bound);
            assert!(e <= bound, "q = {q}, x = {x}: |{y} - {r}| = {e:e} > {bound:e}");
        }
    }
    println!("reference: worst error / bound = {worst_rel:.3}");
}

/// Decision 8.3 written out literally, tap by tap, from the published tables: the reference the
/// crate's read (whose q <= 1 path reads the table at a fixed stride) must equal bit for bit.
fn literal_8_3(t: &SincTable, src: &[f32], channels: usize, x: f64, q: f64) -> Vec<f32> {
    let (tv, dv) = (t.values(), t.deltas());
    let m = if q > 1.0 { q } else { 1.0 };
    let frames = (src.len() / channels) as i64;
    let mut acc = vec![0.0f32; channels];
    let lo = (x - 16.0 * m).floor() as i64 - 2;
    let hi = (x + 16.0 * m).ceil() as i64 + 2;
    for k in lo..=hi {
        let u = (x - k as f64).abs() / m;
        if u >= 16.0 {
            continue;
        }
        let i = (512.0 * u).floor() as usize;
        let f = (512.0 * u - i as f64) as f32;
        let c = tv[i] + f * dv[i];
        for (ch, a) in acc.iter_mut().enumerate() {
            let s = if (0..frames).contains(&k) { src[k as usize * channels + ch] } else { 0.0 };
            *a += s * c;
        }
    }
    if q > 1.0 {
        for a in &mut acc {
            *a *= (1.0 / q) as f32;
        }
    }
    acc
}

#[test]
fn read_frame_is_decision_8_3_bit_for_bit() {
    let t = table();
    let data = common::noise(13, 2 * 2500);
    let src = Interleaved::new(&data, 2);
    let mut xs: Vec<f64> = vec![-20.0, -15.5, -0.25, 0.0, 0.5, 3.0, 1249.0, 1249.9999999, 2499.0, 2500.25, 2515.0];
    xs.extend(common::noise(14, 300).iter().map(|&v| 1250.0 + f64::from(v) * 1300.0));
    let mut qs = vec![1.0, 0.999_999_999_9, 0.918_75, 0.5, 0.459_375, 1.0 / 32.0, 1.000_000_001, 1.088_435_374_149_659_8, 1.5, 2.0];
    qs.extend([2.176_870_748_299_32, 3.675, 4.0, 8.707_482_993_197_28, 16.47, 32.0]);
    let mut out = [0.0f32; 2];
    for &q in &qs {
        for &x in &xs {
            read_frame(t, &src, x, q, &mut out);
            let want = literal_8_3(t, &data, 2, x, q);
            assert_eq!((out[0].to_bits(), out[1].to_bits()), (want[0].to_bits(), want[1].to_bits()), "x = {x}, q = {q}");
        }
    }
    // Within 16 frames of zero, on both sides, `x − k` can round, so the per-tap fractions of 8.3 need
    // not share their bits: positions with full-precision fractions where they do not, read through an
    // impulse on the tap whose fraction differs (its coefficient is then the output itself), and through
    // the stereo noise. The review's six crafted positions first, then a search.
    let mut near: Vec<(f64, i64)> =
        [0x400f_fdff_ffea_0001u64, 0x400f_fdff_3f1a_0001, 0x400f_fcff_ffe2_0001, 0x4007_fdff_ffea_0001, 0x4007_fdff_3f1a_0001, 0x4007_fcff_ffe2_0001]
            .into_iter()
            .map(|b| (f64::from_bits(b), 12))
            .collect();
    let crafted = near.len();
    near.extend(fraction_splits_near_zero(400));
    assert!(near.len() - crafted >= 100, "the search found only {} positions", near.len() - crafted);
    assert!(near.iter().any(|&(x, _)| x < 0.0) && near.iter().any(|&(x, _)| x > 0.0));
    let mut impulse = vec![0.0f32; 64];
    for &(x, k) in &near {
        impulse.fill(0.0);
        impulse[k as usize] = 1.0;
        for q in [1.0, 0.5, 1.0 / 32.0] {
            let y = read(t, &impulse, x, q);
            let want = literal_8_3(t, &impulse, 1, x, q)[0];
            assert_eq!(y.to_bits(), want.to_bits(), "x = {x:?} ({:#018x}), impulse at {k}, q = {q}", x.to_bits());
            read_frame(t, &src, x, q, &mut out);
            let want = literal_8_3(t, &data, 2, x, q);
            assert_eq!((out[0].to_bits(), out[1].to_bits()), (want[0].to_bits(), want[1].to_bits()), "x = {x:?}, noise, q = {q}");
        }
    }
    println!("near zero: {crafted} crafted and {} searched positions", near.len() - crafted);
}

/// The f32 fraction decision 8.3 forms for one tap at q <= 1: `512·|x − k| − floor(512·|x − k|)`.
fn tap_fraction(x: f64, k: i64) -> f32 {
    let v = 512.0 * (x - k as f64).abs();
    (v - v.floor()) as f32
}

/// Up to `want` positions in (−16, 16) with full-precision fractions where some tap at frame >= 0 has an
/// f32 fraction different from the nearest tap on its side of x (the value a fixed stride would reuse),
/// with that tap. Candidates aim the per-tap fraction near an f32 rounding midpoint, where the last
/// bits of an inexact `x − k` decide the rounding; a deterministic integer generator picks the tap, the
/// table step, the midpoint, the offset and the side.
fn fraction_splits_near_zero(want: usize) -> Vec<(f64, i64)> {
    let mut s: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        s >> 16
    };
    let mut found = Vec::new();
    for _ in 0..200_000 {
        if found.len() >= want {
            break;
        }
        let k = (next() % 28) as i64; // the tap whose fraction is aimed at, frames 0 to 27
        let step = (next() % 8192) as f64; // table step of 512·|x − k|
        let mid = ((next() % (1 << 23)) as f64 + 0.5) / (1u64 << 24) as f64 + 0.5; // an f32 midpoint in [0.5, 1)
        let off = ((next() % 7) as f64 - 3.0) / (1u64 << 42) as f64;
        let d = (step + mid + off) / 512.0;
        let x = if next() % 2 == 0 { k as f64 - d } else { k as f64 + d };
        if x.is_nan() || x.abs() >= 16.0 || d >= 16.0 || x == x.floor() {
            continue;
        }
        let below = x.floor() as i64;
        let (first, last) = (below - 15, below + 16);
        let left: Vec<i64> = (first..=below).filter(|&j| (x - j as f64).abs() < 16.0).collect();
        let right: Vec<i64> = (below + 1..=last).filter(|&j| (x - j as f64).abs() < 16.0).collect();
        let split = |side: &[i64], reference: i64| side.iter().copied().find(|&j| j >= 0 && tap_fraction(x, j) != tap_fraction(x, reference));
        let hit = left.first().and_then(|&r| split(&left, r)).or_else(|| right.first().and_then(|&r| split(&right, r)));
        if let Some(j) = hit {
            found.push((x, j));
        }
    }
    found
}
