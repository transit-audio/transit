//! The Resonator's render cost (#198 slice 3; the slice 1 review asked for the cost of the port's
//! flush-to-zero arithmetic in the instrument's evidence): what the instrument costs per host
//! block, every voice ringing, for each model at polyphony 1, 2 and 4, at 48 kHz (no conversion)
//! and at 44.1 kHz (behind the edge converter). A measurement, not a gate: ignored, so CI never
//! runs it and it never passes or fails on time. Run it in release on a quiet machine:
//!
//! `cargo test --release -p transit-dsp --test resonator_cost -- --ignored --nocapture`
//!
//! Each figure is the median of 9 runs of 400 host blocks of 256 frames after a strum on every
//! voice (the part renders every voice every block whether it rings or not, so the cost does not
//! depend on the notes); the realtime share is that time over the block's duration.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

const BLOCK: usize = 256;
const BLOCKS: usize = 400;
const RUNS: usize = 9;

/// Median microseconds per host block at `sr` for `model` at polyphony choice `polyphony`.
fn per_block_us(sr: f32, model: usize, polyphony: usize, shaper: bool) -> f64 {
    let mut m = transit_dsp::create("resonator").unwrap();
    m.set_param("model", model as f32);
    m.set_param("polyphony", polyphony as f32);
    m.set_param("damping", 90.0);
    m.set_param("shaper", if shaper { 1.0 } else { 0.0 });
    if shaper {
        for (id, v) in
            [("mod1_source", 1.0), ("mod1_dest", 3.0), ("mod1_amount", 20.0), ("mod2_source", 2.0), ("mod2_dest", 7.0), ("mod2_amount", 30.0)]
        {
            m.set_param(id, v);
        }
    }
    m.prepare(sr, BLOCK, 2);
    let mut io = vec![vec![0.0f32; BLOCK]; 2];
    let mut times = Vec::with_capacity(RUNS);
    let mut loud = 0.0f32;
    for _ in 0..RUNS {
        for k in 0..4u8 {
            m.note_on(usize::from(k) * 30, 45 + 7 * k, 100);
        }
        // timing: value the measured figures are printed, never compared with a bound
        let t = Instant::now();
        for _ in 0..BLOCKS {
            m.process(&mut io, BLOCK);
            loud = loud.max(io[0].iter().fold(0.0f32, |a, v| a.max(v.abs())));
        }
        times.push(t.elapsed().as_secs_f64() * 1e6 / BLOCKS as f64);
    }
    assert!(loud > 0.0, "model {model} sounds");
    times.sort_by(f64::total_cmp);
    times[RUNS / 2]
}

#[test]
#[ignore = "timing: measurement, run in release with --ignored --nocapture"]
fn timing_render_cost() {
    let models = ["Modal", "Sympathetic Strings", "Inharmonic String", "FM Voice", "Chord Strings", "String and Reverb"];
    for sr in [48_000.0f32, 44_100.0] {
        let block_us = BLOCK as f64 / f64::from(sr) * 1e6;
        println!("{sr} Hz, {BLOCK}-frame blocks ({block_us:.0} us):");
        println!("| Model | polyphony 1 | 2 | 4 | 4 with the shaper and two rows | share of one core (4) |");
        println!("|---|---|---|---|---|---|");
        for (model, name) in models.iter().enumerate() {
            let (one, two, four) = (per_block_us(sr, model, 0, false), per_block_us(sr, model, 1, false), per_block_us(sr, model, 2, false));
            let shaped = per_block_us(sr, model, 2, true);
            println!("| {name} | {one:.1} us | {two:.1} us | {four:.1} us | {shaped:.1} us | {:.2}% |", 100.0 * four / block_us);
        }
    }
}
