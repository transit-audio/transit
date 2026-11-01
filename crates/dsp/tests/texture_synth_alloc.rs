//! Allocation probe for the Texture Synth instrument (#241 slice 3, ADR 0012 G5): its callback
//! allocates and frees nothing, at a session rate behind the edge converter (44.1 kHz) and at the
//! firmware's own (48 kHz), in host blocks that are not multiples of the firmware's 12-frame block,
//! through notes (legato retriggers and returns), quality changes, Freeze and Seed (held and
//! latched), the shaper switched while keys are held, matrix rows, controllers, All Notes Off, All
//! Sound Off (the synth reset in place), a reset, and a frozen state answered to a reader on
//! another thread (at every host rate a state carries, 8 to 384 kHz, from a block and from the strip's idle service).
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_dsp::create;
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 250;

#[test]
fn the_texture_synth_callback_allocates_nothing() {
    for sr in [44_100.0, 48_000.0] {
        let mut p = create("texture_synth").unwrap();
        p.prepare(sr, BLOCK, 2);
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let edits: [(&str, f32); 20] = [
            ("quality", 0.0),
            ("density", 70.0),
            ("freeze", 1.0),
            ("quality", 1.0),
            ("shaper", 1.0),
            ("freeze", 0.0),
            ("seed", 1.0),
            ("mod1_source", 1.0),
            ("mod1_dest", 15.0),
            ("mod1_amount", 40.0),
            ("seed", 0.0),
            ("seed_latch", 1.0),
            ("quality", 2.0),
            ("mod2_source", 5.0),
            ("mod2_dest", 1.0),
            ("mod2_amount", -30.0),
            ("seed_latch", 0.0),
            ("shaper", 0.0),
            ("mono_grains", 1.0),
            ("quality", 3.0),
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
                        // Two keys: the second retriggers the held gate.
                        p.note_on(3, 48 + (round % 5) as u8, 90);
                        p.note_on(120, 60 + (round % 5) as u8, 70);
                    }
                    3 => {
                        // The newer key first: legato back to the older one, then the gate falls.
                        p.note_off(17, 60 + ((round - 3) % 5) as u8);
                        p.note_off(140, 48 + ((round - 3) % 5) as u8);
                        p.midi(40, [0xB0, 1, (round % 128) as u8]);
                        p.midi(41, [0xE0, 0, (round % 128) as u8]);
                    }
                    5 => p.midi(9, [0xB0, 64, if round % 2 == 0 { 127 } else { 0 }]),
                    _ => {}
                }
                if round == rounds / 3 {
                    p.midi(10, [0xB0, 120, 0]);
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

/// Every host rate a state carries (8 to 384 kHz) behind the module's edge converter, plus the
/// firmware's own.
const STATE_RATES: [f32; 12] =
    [8_000.0, 11_025.0, 16_000.0, 22_050.0, 32_000.0, 44_100.0, 48_000.0, 88_200.0, 96_000.0, 176_400.0, 192_000.0, 384_000.0];

/// A reader on another thread asks for the frozen state while the callback runs: the callback
/// writes it into the reader's preallocated loan without allocating, and the reader gets a state.
/// The strip's idle service (an inactive track's instrument) answers the same way without a block.
fn answered_without_allocating(sr: f32, idle: bool) {
    let mut p = create("texture_synth").unwrap();
    p.prepare(sr, BLOCK, 2);
    let mut reader = p.state_reader().unwrap();
    let mut io = vec![vec![0.0f32; BLOCK]; 2];
    p.note_on(0, 57, 100);
    for _ in 0..40 {
        p.process(&mut io, BLOCK);
    }
    assert!(p.set_param("freeze", 1.0));
    p.process(&mut io, BLOCK);
    let asking = std::thread::spawn(move || reader.save_state());
    let mut total = AllocStats::default();
    // Calls run until the reader has its answer (however long its thread takes to ask).
    while !asking.is_finished() {
        let ((), stats) = measure(|| if idle { p.service_idle_state() } else { p.process(&mut io, BLOCK) });
        total.allocs += stats.allocs;
        total.frees += stats.frees;
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let state = asking.join().unwrap().expect("a frozen state");
    assert!(state.len() > 64 * 1024, "{sr} Hz: {} bytes", state.len());
    assert!(total.is_zero(), "{sr} Hz idle={idle}: {total:?}");
}

#[test]
fn answering_a_frozen_state_allocates_nothing_on_the_callback() {
    for sr in STATE_RATES {
        answered_without_allocating(sr, false);
    }
}

#[test]
fn the_idle_service_answers_a_frozen_state_without_allocating() {
    for sr in STATE_RATES {
        answered_without_allocating(sr, true);
    }
}
