//! Allocation probe for the Resonator instrument (#198 slice 3, ADR 0012 G5): its callback
//! allocates and frees nothing, at a session rate behind the edge converter (44.1 kHz) and at the
//! firmware's own (48 kHz), in host blocks that are not multiples of the firmware's 24-frame block,
//! through notes (chords that queue strums), parameter edits made on the audio thread (every model,
//! every polyphony, the macros, the shaper switched while keys are held, matrix rows), controllers,
//! all-notes-off and a reset.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_dsp::create;
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 250;

#[test]
fn the_resonator_callback_allocates_nothing() {
    for sr in [44_100.0, 48_000.0] {
        let mut p = create("resonator").unwrap();
        p.prepare(sr, BLOCK, 2);
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let edits: [(&str, f32); 18] = [
            ("model", 1.0),
            ("polyphony", 0.0),
            ("structure", 80.0),
            ("model", 2.0),
            ("shaper", 1.0),
            ("polyphony", 1.0),
            ("mod1_source", 1.0),
            ("mod1_dest", 3.0),
            ("mod1_amount", 40.0),
            ("model", 3.0),
            ("damping", 95.0),
            ("model", 4.0),
            ("polyphony", 2.0),
            ("shaper", 0.0),
            ("model", 5.0),
            ("spread", 0.0),
            ("tonic", 30.0),
            ("model", 0.0),
        ];
        let mut total = AllocStats::default();
        let rounds = edits.len() * 10;
        for round in 0..rounds {
            let ((), stats) = measure(|| {
                if round % 10 == 0 {
                    let (id, v) = edits[round / 10];
                    assert!(p.set_param(id, v));
                }
                match round % 7 {
                    0 => {
                        // A chord: three strums queue for consecutive blocks.
                        for (i, key) in [48u8, 55, 64].into_iter().enumerate() {
                            p.note_on(i * 3, key + (round % 5) as u8, 90);
                        }
                    }
                    3 => {
                        for key in [48u8, 55, 64] {
                            p.note_off(17, key + ((round - 3) % 5) as u8);
                        }
                        p.midi(40, [0xB0, 1, (round % 128) as u8]);
                        p.midi(41, [0xE0, 0, (round % 128) as u8]);
                    }
                    5 => p.midi(9, [0xB0, 64, if round % 2 == 0 { 127 } else { 0 }]),
                    _ => {}
                }
                if round == rounds / 2 {
                    p.all_notes_off();
                }
                if round == rounds * 3 / 4 {
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

/// The shaper switched on while a key is held (the crossfade-in path), switched off again under
/// the held key (the fade-out path), then All Notes Off and All Sound Off, all inside the
/// measured window.
#[test]
fn the_shaper_fade_in_and_the_panics_allocate_nothing() {
    for sr in [44_100.0, 48_000.0] {
        let mut p = create("resonator").unwrap();
        p.prepare(sr, BLOCK, 2);
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        // Warm up with the key held and the shaper off, outside the window.
        p.note_on(0, 60, 100);
        for _ in 0..6 {
            p.process(&mut io, BLOCK);
        }
        let mut total = AllocStats::default();
        for round in 0..40 {
            let ((), stats) = measure(|| {
                match round {
                    2 => assert!(p.set_param("shaper", 1.0)),
                    12 => assert!(p.set_param("shaper", 0.0)),
                    20 => {
                        assert!(p.set_param("shaper", 1.0));
                        p.note_on(4, 64, 100);
                    }
                    28 => p.midi(10, [0xB0, 123, 0]),
                    30 => p.note_on(0, 67, 100),
                    34 => p.midi(10, [0xB0, 120, 0]),
                    _ => {}
                }
                p.process(&mut io, BLOCK);
            });
            total.allocs += stats.allocs;
            total.frees += stats.frees;
            assert!(io.iter().flatten().all(|v| v.is_finite()));
        }
        assert!(total.is_zero(), "{sr} Hz: {total:?}");
    }
}
