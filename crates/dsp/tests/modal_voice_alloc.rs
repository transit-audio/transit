//! Allocation probe for the Modal Voice instrument (#198 slice 2, ADR 0012 G5): its callback
//! allocates and frees nothing, at session rates behind the edge converter (44.1 and 48 kHz) and at
//! the firmware's own (32 kHz), in host blocks that are not multiples of the firmware's 16-frame
//! block, through notes (retriggers while held, legato releases), parameter edits made on the audio
//! thread (every model, the FM pair voice, the exciters and the resonator, the shaper switched with
//! the gate up, matrix rows), controllers (the pedal, the wheel, the bend, pressure), CC 120 and
//! CC 123, all-notes-off and a reset.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_dsp::create;
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 250;

#[test]
fn the_modal_voice_callback_allocates_nothing() {
    for sr in [44_100.0, 48_000.0, 32_000.0] {
        let mut p = create("modal_voice").unwrap();
        p.prepare(sr, BLOCK, 2);
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let edits: [(&str, f32); 20] = [
            ("model", 1.0),
            ("bow_level", 60.0),
            ("geometry", 80.0),
            ("model", 2.0),
            ("shaper", 1.0),
            ("blow_level", 90.0),
            ("mod1_source", 1.0),
            ("mod1_dest", 3.0),
            ("mod1_amount", 40.0),
            ("model", 3.0),
            ("damping", 95.0),
            ("mod2_source", 8.0),
            ("mod2_dest", 15.0),
            ("mod2_amount", -50.0),
            ("space", 190.0),
            ("shaper", 0.0),
            ("mallet", 95.0),
            ("velocity", 30.0),
            ("contour", 10.0),
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
                        // Three notes: the second and third retrigger the held gate.
                        for (i, key) in [48u8, 55, 64].into_iter().enumerate() {
                            p.note_on(i * 3, key + (round % 5) as u8, 90);
                        }
                    }
                    3 => {
                        for key in [64u8, 55, 48] {
                            p.note_off(17, key + ((round - 3) % 5) as u8);
                        }
                        p.midi(40, [0xB0, 1, (round % 128) as u8]);
                        p.midi(41, [0xE0, 0, (round % 128) as u8]);
                        p.midi(42, [0xD0, (round % 128) as u8, 0]);
                    }
                    5 => p.midi(9, [0xB0, 64, if round % 2 == 0 { 127 } else { 0 }]),
                    _ => {}
                }
                if round == rounds / 3 {
                    p.midi(3, [0xB0, 123, 0]);
                }
                if round == rounds / 2 {
                    p.all_notes_off();
                }
                if round == rounds * 2 / 3 {
                    p.midi(5, [0xB0, 120, 0]);
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

/// The shaper switched on while the gate is up (the crossfade-in path), switched off again under
/// the held key (the fade-out path), then All Notes Off and All Sound Off, all inside the measured
/// window.
#[test]
fn the_shaper_fades_and_the_panics_allocate_nothing() {
    for sr in [44_100.0, 32_000.0] {
        let mut p = create("modal_voice").unwrap();
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
