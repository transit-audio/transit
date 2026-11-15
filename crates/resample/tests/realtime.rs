//! Real time (ADR 0056, decisions 8.8 and 13.1): reading neither allocates nor runs an unbounded loop.
//! A counting allocator (`transit-rt`'s test support, a dev-dependency) watches reads at every rate
//! band, the largest rate, refused inputs and the direct path; a counting source checks that one output
//! frame reads at most `2·16·max(1, q) + 1` frames a channel, each once.

mod common;

use std::cell::Cell;

use transit_resample::{Frames, Interleaved, MAX_RATE, SincTable, direct, halo, read_frame, taps};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

#[test]
fn reading_does_not_allocate() {
    let t = SincTable::shared();
    let src = common::noise(5, 2 * 8192);
    let s = Interleaved::new(&src, 2);
    let rates = [1.0 / 32.0, 0.5, 0.918_75, 1.0, 1.088_4, 1.5, 2.0, 4.0, 16.47, MAX_RATE, 0.0, f64::NAN, 64.0];
    let (sum, stats) = measure(|| {
        let mut out = [0.0f32; 2];
        let mut sum = 0.0f32;
        for &q in &rates {
            for j in 0..64 {
                let x = 4000.0 + f64::from(j) * 0.37;
                read_frame(t, &s, x, q, &mut out);
                sum += out[0] + out[1];
                sum += direct(x, q).map_or(0.0, |k| k as f32);
                sum += halo(q) as f32;
            }
        }
        sum
    });
    assert!(sum.is_finite());
    assert!(stats.is_zero(), "{stats:?}");
}

/// A stereo source that counts every frame asked of it.
struct Counting {
    calls: Cell<usize>,
    last: Cell<i64>,
}

impl Frames for Counting {
    fn channels(&self) -> usize {
        2
    }

    fn sample(&self, channel: usize, frame: i64) -> f32 {
        if channel == 0 {
            assert!(frame > self.last.get(), "frame {frame} asked twice or out of order");
            self.last.set(frame);
        }
        self.calls.set(self.calls.get() + 1);
        1.0
    }
}

#[test]
fn one_frame_reads_a_bounded_ascending_tap_set() {
    let t = SincTable::shared();
    for q in [1.0 / 32.0, 0.5, 1.0, 1.000_001, 1.5, 2.0, 4.0, 16.47, MAX_RATE] {
        let m = q.max(1.0);
        for j in 0..50 {
            let x = 123.0 + f64::from(j) * 0.173;
            let src = Counting { calls: Cell::new(0), last: Cell::new(i64::MIN) };
            let mut out = [0.0f32; 2];
            read_frame(t, &src, x, q, &mut out);
            let per_channel = src.calls.get() / 2;
            assert_eq!(per_channel, taps(x, q).count(), "q = {q}, x = {x}");
            assert!(per_channel as f64 <= 2.0 * 16.0 * m + 1.0, "q = {q}, x = {x}: {per_channel} taps");
        }
    }
}
