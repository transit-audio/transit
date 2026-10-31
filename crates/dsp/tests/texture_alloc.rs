//! Allocation probe for the Texture insert (#241, ADR 0012 G5): its callback allocates and frees
//! nothing, at a session rate that resamples (44.1 kHz) and at one that does not (48 kHz), in host
//! blocks that are not multiples of any firmware block, through parameter edits made on the audio
//! thread: every quality (a codec rate change), mono input, the buffer length (the in-place rebuild),
//! the freeze, the seed and trigger gates, the input gain, and a reset.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_dsp::create;
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 250;

#[test]
fn the_texture_callback_allocates_nothing() {
    for sr in [44_100.0, 48_000.0] {
        let mut p = create("texture").unwrap();
        p.prepare(sr, BLOCK, 2);
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let edits: [(&str, f32); 16] = [
            ("freeze", 1.0),
            ("quality", 0.0),
            ("seed_latch", 0.0),
            ("seed", 1.0),
            ("quality", 2.0),
            ("buffer", 4.0),
            ("trigger_input", 1.0),
            ("trigger", 1.0),
            ("mono_input", 1.0),
            ("input_gain", 1.0),
            ("input_level", 12.0),
            ("quality", 1.0),
            ("size", 100.0),
            ("buffer", 1.0),
            ("freeze", 0.0),
            ("quality", 3.0),
        ];
        let mut phase = 0.0f32;
        let mut total = transit_rt::testing::AllocStats::default();
        for round in 0..(edits.len() * 12) {
            for (n, s) in io[0].iter_mut().enumerate() {
                phase += 0.031;
                *s = 0.4 * phase.sin() + if n % 97 == 0 { 0.3 } else { 0.0 };
            }
            let (l, r) = io.split_at_mut(1);
            r[0].copy_from_slice(&l[0]);
            let ((), stats) = measure(|| {
                if round % 12 == 0 {
                    let (id, v) = edits[round / 12];
                    assert!(p.set_param(id, v));
                }
                if round == edits.len() * 6 {
                    p.reset();
                }
                p.process(&mut io, BLOCK);
                let _ = p.latency();
            });
            total.allocs += stats.allocs;
            total.frees += stats.frees;
            assert!(io.iter().flatten().all(|v| v.is_finite()));
        }
        assert!(total.is_zero(), "{sr} Hz: {total:?}");
    }
}
