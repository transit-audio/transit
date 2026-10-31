//! Allocation probe for the Cross Modulator insert (#200, ADR 0012 G5): its callback allocates and
//! frees nothing, keyed and with no key, at session rates that convert (44.1 kHz) and that do not
//! (96 kHz), in host blocks that are not multiples of the firmware's 60 frames, through parameter
//! edits made on the audio thread (every algorithm region, the vocoder's freeze, the oscillator's
//! shapes and frequency, the carrier mode, the swap, dry/wet and output) and a reset.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_dsp::create;
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 250;

#[test]
fn the_cross_modulator_callback_allocates_nothing() {
    for sr in [44_100.0, 96_000.0] {
        let mut p = create("cross_modulator").unwrap();
        p.prepare(sr, BLOCK, 2);
        assert!(p.has_key_input());
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let mut key = vec![vec![0.0f32; BLOCK]; 2];
        let edits: [(&str, f32); 14] = [
            ("algorithm", 0.0),
            ("algorithm", 1.5),
            ("timbre", 90.0),
            ("algorithm", 4.5),
            ("algorithm", 5.4),
            ("algorithm", 6.5),
            ("algorithm", 8.0),
            ("shape", 2.0),
            ("carrier", 1.0),
            ("frequency", 1500.0),
            ("swap", 1.0),
            ("dry_wet", 40.0),
            ("output", -6.0),
            ("algorithm", 3.0),
        ];
        let mut phase = 0.0f32;
        let mut total = transit_rt::testing::AllocStats::default();
        for round in 0..(edits.len() * 12) {
            for (n, (s, k)) in io[0].iter_mut().zip(key[0].iter_mut()).enumerate() {
                phase += 0.031;
                *s = 0.4 * phase.sin() + if n % 97 == 0 { 0.3 } else { 0.0 };
                *k = 0.5 * (phase * 0.37).sin();
            }
            let (l, r) = io.split_at_mut(1);
            r[0].copy_from_slice(&l[0]);
            let (l, r) = key.split_at_mut(1);
            r[0].copy_from_slice(&l[0]);
            let keyed = round % 5 != 4;
            let ((), stats) = measure(|| {
                if round % 12 == 0 {
                    let (id, v) = edits[round / 12];
                    assert!(p.set_param(id, v));
                }
                if round == edits.len() * 6 {
                    p.reset();
                }
                if keyed {
                    p.process_with_key(&mut io, &key, BLOCK);
                } else {
                    p.process(&mut io, BLOCK);
                }
                let _ = p.latency();
            });
            total.allocs += stats.allocs;
            total.frees += stats.frees;
            assert!(io.iter().flatten().all(|v| v.is_finite()));
        }
        assert!(total.is_zero(), "{sr} Hz: {total:?}");
    }
}
