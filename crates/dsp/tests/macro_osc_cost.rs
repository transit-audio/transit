//! The Macro Oscillator's render cost per voice (#196): what one sounding voice costs per host
//! block at 48 kHz, so racks (ADR 0034) have a budget. A measurement, not a gate: ignored, so CI
//! never runs it and it never passes or fails on time. Run it in release on a quiet machine:
//!
//! `cargo test --release -p transit-dsp --test macro_osc_cost -- --ignored --nocapture`
//!
//! Each figure is the median of 9 runs of 400 host blocks of 256 frames with every voice held
//! (a chord of N notes, retriggered so no voice goes idle), divided by N; the realtime share is
//! that time over the block's 5.33 ms.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCK: usize = 256;
const BLOCKS: usize = 400;
const RUNS: usize = 9;

/// Median microseconds per host block for `voices` voices held on `model`.
fn per_block_us(model: f32, voices: usize) -> f64 {
    let mut m = transit_dsp::create("macro_oscillator").unwrap();
    m.set_param("voices", (voices - 1) as f32);
    m.set_param("model", model);
    m.prepare(SR, BLOCK, 2);
    let mut io = vec![vec![0.0f32; BLOCK]; 2];
    let mut times = Vec::with_capacity(RUNS);
    let mut loud = 0.0f32;
    for _ in 0..RUNS {
        for k in 0..voices {
            m.note_on(0, 40 + 3 * k as u8, 100);
        }
        // timing: value the measured figures are printed, never compared with a bound
        let t = Instant::now();
        for _ in 0..BLOCKS {
            m.process(&mut io, BLOCK);
            loud = loud.max(io[0].iter().fold(0.0f32, |a, v| a.max(v.abs())));
        }
        times.push(t.elapsed().as_secs_f64() * 1e6 / BLOCKS as f64);
        for k in 0..voices {
            m.note_off(0, 40 + 3 * k as u8);
        }
        m.process(&mut io, BLOCK);
    }
    assert!(loud > 0.0, "model {model} with {voices} voices sounds");
    assert_eq!(m.voice_activity() & 0xFFFF, (1u32 << voices) - 1, "every voice sounding");
    times.sort_by(f64::total_cmp);
    times[RUNS / 2]
}

#[test]
#[ignore = "timing: measurement, run in release with --ignored --nocapture"]
fn timing_render_cost_per_voice() {
    let block_us = BLOCK as f64 / f64::from(SR) * 1e6;
    let models: [(f32, &str); 8] = [
        (8.0, "Virtual analog"),
        (10.0, "FM"),
        (2.0, "Six-op FM 1"),
        (13.0, "Wavetable"),
        (14.0, "Chord"),
        (15.0, "Speech"),
        (20.0, "Modal"),
        (18.0, "Particle noise"),
    ];
    println!("| Model | 1 voice | 8 voices | 16 voices | per voice (16) | share of one core per voice |");
    println!("|---|---|---|---|---|---|");
    let mut all = Vec::new();
    for (model, name) in models {
        let (one, eight, sixteen) = (per_block_us(model, 1), per_block_us(model, 8), per_block_us(model, 16));
        let per = sixteen / 16.0;
        all.push(per);
        println!("| {name} | {one:.1} us | {eight:.1} us | {sixteen:.1} us | {per:.1} us | {:.2}% |", 100.0 * per / block_us);
    }
    // Every model at one voice, for the worst case.
    let mut worst = (0.0f64, 0usize);
    for model in 0..24 {
        let us = per_block_us(model as f32, 1);
        if us > worst.0 {
            worst = (us, model);
        }
    }
    all.sort_by(f64::total_cmp);
    println!(
        "per 256-frame block at 48 kHz ({block_us:.0} us): median voice {:.1} us ({:.2}%), worst model at one voice: {} {:.1} us ({:.2}%)",
        all[all.len() / 2],
        100.0 * all[all.len() / 2] / block_us,
        transit_dsp::plugins::macro_osc::MODELS[worst.1],
        worst.0,
        100.0 * worst.0 / block_us
    );
}
