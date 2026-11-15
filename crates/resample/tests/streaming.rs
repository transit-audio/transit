//! What a streaming or block-cut caller relies on (ADR 0056, decisions 8.3, 8.5, 8.6 and 8.8): the
//! output depends only on the source frames, the positions and the rates, never on how the work is cut
//! into blocks; a caller holding only `halo(q)` frames each side of the read position gets the same
//! output as one with the whole source; the tap set is exact, contiguous and bounded; a tap outside the
//! source reads zero.

mod common;

use transit_resample::{Frames, Interleaved, MAX_RATE, SincTable, halo, reach, read, read_frame, taps};

/// The rates the four callers name: device and sampler rate pairs both ways (44.1, 48 and 96 kHz),
/// transpositions up to +24 semitones, and both combined.
const RATES: &[f64] = &[
    44_100.0 / 48_000.0,
    48_000.0 / 44_100.0,
    48_000.0 / 96_000.0,
    96_000.0 / 48_000.0,
    44_100.0 / 96_000.0,
    96_000.0 / 44_100.0,
    4.0,
    44_100.0 / 48_000.0 * 4.0,
    96_000.0 / 44_100.0 * 4.0,
];

/// A ring of the frames a streaming caller has received: `[base, received)` of a stereo stream. A frame
/// before the stream's start reads 0; a frame the ring no longer or not yet holds reads NaN, so any
/// read outside the halo poisons the output.
struct Ring<'a> {
    all: &'a [f32],
    base: i64,
    received: i64,
}

impl Frames for Ring<'_> {
    fn channels(&self) -> usize {
        2
    }

    fn sample(&self, channel: usize, frame: i64) -> f32 {
        if frame < 0 {
            0.0
        } else if frame < self.base || frame >= self.received {
            f32::NAN
        } else {
            self.all[2 * frame as usize + channel]
        }
    }
}

/// Positions and rates for `n` outputs: `x_{i+1} = x_i + q_i` (the caller's arithmetic, done once).
fn path(x0: f64, n: usize, rate: impl Fn(usize) -> f64) -> Vec<(f64, f64)> {
    let mut x = x0;
    (0..n)
        .map(|i| {
            let q = rate(i);
            let p = (x, q);
            x += q;
            p
        })
        .collect()
}

fn whole(table: &SincTable, src: &[f32], path: &[(f64, f64)]) -> Vec<[f32; 2]> {
    let s = Interleaved::new(src, 2);
    path.iter()
        .map(|&(x, q)| {
            let mut o = [0.0f32; 2];
            read_frame(table, &s, x, q, &mut o);
            o
        })
        .collect()
}

/// The same outputs from a ring fed in irregular blocks, each output produced as soon as its halo has
/// arrived, the ring dropping everything older than the halo of the oldest position still owed.
fn streamed(table: &SincTable, src: &[f32], path: &[(f64, f64)], blocks: &[usize]) -> Vec<[f32; 2]> {
    let frames = (src.len() / 2) as i64;
    let mut ring = Ring { all: src, base: 0, received: 0 };
    let mut out = Vec::with_capacity(path.len());
    let mut b = 0;
    while out.len() < path.len() {
        ring.received = (ring.received + blocks[b % blocks.len()] as i64).min(frames);
        b += 1;
        while let Some(&(x, q)) = path.get(out.len()) {
            let h = halo(q) as i64;
            let centre = x.floor() as i64;
            if centre + h >= ring.received {
                break;
            }
            ring.base = ring.base.max(centre - h);
            let mut o = [0.0f32; 2];
            read_frame(table, &ring, x, q, &mut o);
            out.push(o);
        }
        assert!(ring.received < frames || out.len() == path.len(), "the source ran out");
    }
    out
}

fn same(a: &[[f32; 2]], b: &[[f32; 2]], what: &str) {
    assert_eq!(a.len(), b.len());
    for (i, (p, q)) in a.iter().zip(b).enumerate() {
        assert!(!q[0].is_nan() && !q[1].is_nan(), "{what}: output {i} read outside the halo");
        assert_eq!((p[0].to_bits(), p[1].to_bits()), (q[0].to_bits(), q[1].to_bits()), "{what}: output {i}");
    }
}

#[test]
fn the_output_does_not_depend_on_how_the_request_is_cut_into_blocks() {
    let t = SincTable::shared();
    let src = common::noise(21, 2 * 40_000);
    let cuts: [&[usize]; 4] = [&[1], &[7, 64, 3], &[513], &[64, 1, 1, 200, 31]];
    for &q in RATES {
        let p = path(37.25, 3000, |_| q);
        let reference = whole(t, &src, &p);
        for c in cuts {
            same(&reference, &streamed(t, &src, &p, c), &format!("q = {q}, blocks {c:?}"));
        }
    }
    // A rate that moves every sample, as a drift servo moves it (50 and 1000 ppm), and the same around 2.
    for (centre, ppm) in [(1.0, 50.0), (1.0, 1000.0), (2.0, 1000.0), (44_100.0 / 48_000.0, 50.0)] {
        let p = path(5.5, 4000, |i| centre * (1.0 + ppm * 1e-6 * (i as f64 / 300.0).sin()));
        let reference = whole(t, &src, &p);
        for c in cuts {
            same(&reference, &streamed(t, &src, &p, c), &format!("drift {ppm} ppm about {centre}, blocks {c:?}"));
        }
    }
}

#[test]
fn the_tap_set_is_exact_contiguous_and_bounded() {
    let mut xs = vec![0.0, 0.5, -0.25, 15.999_999, 16.0, -16.0, 1000.0, 1000.0000001, 999.9999999];
    xs.extend(common::noise(3, 200).iter().map(|&v| f64::from(v) * 5000.0));
    xs.extend([2f64.powi(40) - 0.5, 2f64.powi(40) + 0.75, -(2f64.powi(40)) + 0.125]);
    let mut qs = RATES.to_vec();
    qs.extend([1.0, 1.0 / 32.0, 1.000_000_001, 1.5, 16.47, MAX_RATE]);
    for &q in &qs {
        let m = q.max(1.0);
        for &x in &xs {
            let got = taps(x, q);
            let centre = x.floor() as i64;
            let brute: Vec<i64> = (centre - 1100..=centre + 1100).filter(|&k| ((x - k as f64).abs() / m) < 16.0).collect();
            assert_eq!(got.clone().collect::<Vec<_>>(), brute, "x = {x}, q = {q}");
            assert!((got.end - got.start) as f64 <= 2.0 * reach(q) + 1.0, "x = {x}, q = {q}: {got:?}");
            for k in got {
                assert!((k - centre).unsigned_abs() as usize <= halo(q), "x = {x}, q = {q}, k = {k}");
            }
        }
    }
    // Refused positions and rates read nothing.
    for (x, q) in [(0.0, 0.0), (0.0, -1.0), (0.0, MAX_RATE * 1.000_001), (f64::NAN, 1.0), (f64::INFINITY, 1.0), (0.0, f64::NAN)] {
        assert!(taps(x, q).is_empty(), "x = {x}, q = {q}");
    }
}

#[test]
fn taps_outside_the_source_read_zero() {
    // Decision 8.5 at the source's two ends: an impulse on the first and on the last frame, read at
    // positions all around them, equals the same impulses inside a longer zero-padded source.
    let t = SincTable::shared();
    const F: usize = 40;
    const PAD: usize = 1200;
    let mut src = vec![0.0f32; F];
    src[0] = 1.0;
    src[F - 1] = -0.5;
    src[F / 2] = 0.25;
    let mut padded = vec![0.0f32; F + 2 * PAD];
    padded[PAD..PAD + F].copy_from_slice(&src);
    for &q in RATES.iter().chain(&[1.0, 0.25, MAX_RATE]) {
        // Positions on a grid of 3/8 frame, so `x + PAD` is exact and the tap distances are identical.
        let r = reach(q);
        let start = (-r - 2.0).floor();
        let mut j = 0.0;
        while start + j * 0.375 < F as f64 + r + 2.0 {
            let x = start + j * 0.375;
            let a = read(t, &src, x, q);
            let b = read(t, &padded, x + PAD as f64, q);
            assert_eq!(a.to_bits(), b.to_bits(), "q = {q}, x = {x}");
            j += 1.0;
        }
    }
}
