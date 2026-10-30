//! Voice-level scenarios the per-engine sweep does not reach, nulled against the C++ oracle
//! (ADR 0033, #195): engine changes within one voice (by the patch and by the engine CV), the level
//! input, the frequency, timbre and morph inputs with their attenuverters, the note input, the
//! speech and chiptune special cases, and blocks other than 12 frames.
//!
//! Each scenario renders a fresh voice on both sides, after seeding with the sweep's seed, and
//! requires identical int16 frames (0 LSB), as `null_vs_oracle_voice.rs` does.

#![forbid(unsafe_code)]

use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc_oracle::residual::residual_i16;
use transit_macro_osc_oracle::sweep::SEED;
use transit_macro_osc_oracle::{self as oracle, MAX_BLOCK_SIZE, Model, SAMPLE_RATE, Session};

/// The inputs of one block: the frame count, the patch and the modulation inputs.
type Block = (usize, oracle::Patch, oracle::Modulations);

fn patch(p: &oracle::Patch) -> Patch {
    Patch {
        note: p.note,
        harmonics: p.harmonics,
        timbre: p.timbre,
        morph: p.morph,
        frequency_modulation_amount: p.frequency_modulation_amount,
        timbre_modulation_amount: p.timbre_modulation_amount,
        morph_modulation_amount: p.morph_modulation_amount,
        engine: p.model.index(),
        decay: p.decay,
        lpg_colour: p.lpg_color,
    }
}

fn modulations(m: &oracle::Modulations) -> Modulations {
    Modulations {
        engine: m.engine,
        note: m.note,
        frequency: m.frequency,
        harmonics: m.harmonics,
        timbre: m.timbre,
        morph: m.morph,
        trigger: m.trigger,
        level: m.level,
        frequency_patched: m.frequency_patched,
        timbre_patched: m.timbre_patched,
        morph_patched: m.morph_patched,
        trigger_patched: m.trigger_patched,
        level_patched: m.level_patched,
    }
}

/// Renders `blocks` on both sides from fresh voices and asserts identical frames.
fn null_scenario(name: &str, blocks: &[Block]) {
    let session = Session::lock();
    session.seed(SEED);
    let mut oracle_voice = session.voice(SAMPLE_RATE).expect("oracle voice");
    let mut port_voice = Voice::new(SAMPLE_RATE);
    port_voice.seed(SEED);
    port_voice.init();

    let (mut oracle_out, mut oracle_aux, mut port_out, mut port_aux) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut oracle_frames = [oracle::Frame::default(); MAX_BLOCK_SIZE];
    let mut port_frames = [Frame::default(); MAX_BLOCK_SIZE];
    for (size, p, m) in blocks {
        oracle_voice.render(p, m, &mut oracle_frames[..*size]).expect("oracle render");
        port_voice.render(&patch(p), &modulations(m), &mut port_frames[..*size]);
        oracle_out.extend(oracle_frames[..*size].iter().map(|f| f.out));
        oracle_aux.extend(oracle_frames[..*size].iter().map(|f| f.aux));
        port_out.extend(port_frames[..*size].iter().map(|f| f.out));
        port_aux.extend(port_frames[..*size].iter().map(|f| f.aux));
    }
    let out = residual_i16(&port_out, &oracle_out).expect("lengths");
    let aux = residual_i16(&port_aux, &oracle_aux).expect("lengths");
    let first = port_out.iter().zip(&oracle_out).chain(port_aux.iter().zip(&oracle_aux)).position(|(a, b)| a != b);
    println!("{name}: {} frames; out {out}; aux {aux}", port_out.len());
    assert!(out.is_exact() && aux.is_exact(), "{name}: out {out}; aux {aux}; first differing value at {first:?} (out frames, then aux)");
}

/// A gate pattern: high for the first `high` blocks of every `period`.
fn gate(block: usize, period: usize, high: usize) -> f32 {
    if block % period < high { 1.0 } else { 0.0 }
}

fn triggered(trigger: f32) -> oracle::Modulations {
    oracle::Modulations { trigger, trigger_patched: true, ..oracle::Modulations::default() }
}

/// Every engine in turn within one voice, triggered, so each change reloads and resets the new
/// engine and resets the out limiter, and the noise engines draw from the voice's one generator
/// where the previous engine left it.
#[test]
fn engine_changes_by_patch() {
    let order: Vec<Model> = Model::ALL.iter().copied().chain(Model::ALL.iter().rev().copied()).collect();
    let mut blocks = Vec::new();
    for (i, model) in order.iter().enumerate() {
        for b in 0..60 {
            let block = i * 60 + b;
            let p = oracle::Patch { note: 48.0 + (i % 5) as f32 * 7.0, decay: 0.6, ..oracle::Patch::centered(*model) };
            blocks.push((12, p, triggered(gate(block, 30, 10))));
        }
    }
    null_scenario("engine changes by patch", &blocks);
}

/// The engine CV moves the selection: free-running it follows the CV block by block (through the
/// hysteresis quantizer); triggered it is sampled on each trigger.
#[test]
fn engine_changes_by_cv() {
    let mut blocks = Vec::new();
    for block in 0..2000 {
        let cv = (block as f32 / 2000.0) * 1.2 - 0.1;
        let base = oracle::Patch { note: 55.0, ..oracle::Patch::centered(Model::VirtualAnalog) };
        let m = if block < 1000 {
            oracle::Modulations { engine: cv, ..oracle::Modulations::default() }
        } else {
            oracle::Modulations { engine: -cv, ..triggered(gate(block, 16, 4)) }
        };
        blocks.push((12, base, m));
    }
    null_scenario("engine changes by cv", &blocks);
}

/// The level input: the low-pass gate follows the compressed level (ProcessLP) and the accent is
/// the compressed level, with and without the trigger input, on a plain engine and on an
/// already-enveloped one.
#[test]
fn level_input() {
    let mut blocks = Vec::new();
    for (i, model) in [Model::Wavetable, Model::BassDrum, Model::FilteredNoise, Model::ModalResonator].into_iter().enumerate() {
        for b in 0..400 {
            let block = i * 400 + b;
            let level = ((block as f32) * 0.05).sin().abs() * 1.4 - 0.2;
            let p = oracle::Patch { note: 50.0, lpg_color: 0.3 * i as f32, decay: 0.2 + 0.2 * i as f32, ..oracle::Patch::centered(model) };
            let m = oracle::Modulations { level, level_patched: true, trigger: gate(block, 50, 20), trigger_patched: b >= 200, ..oracle::Modulations::default() };
            blocks.push((12, p, m));
        }
    }
    null_scenario("level input", &blocks);
}

/// The frequency, timbre and morph inputs with their attenuverters, patched and unpatched (then
/// the internal envelope or the defaults stand in), the harmonics and note inputs, and the note
/// smoothing over two blocks.
#[test]
fn modulation_inputs() {
    let amounts = [-1.0, -0.4, 0.0, 0.03, 0.5, 1.0];
    let mut blocks = Vec::new();
    for (i, model) in [Model::Fm, Model::PhaseDistortion, Model::Chord, Model::Swarm].into_iter().enumerate() {
        for b in 0..480 {
            let block = i * 480 + b;
            let phase = block as f32 * 0.031;
            let p = oracle::Patch {
                note: 40.0 + i as f32 * 9.0,
                frequency_modulation_amount: amounts[b / 80],
                timbre_modulation_amount: amounts[(b / 80 + 2) % 6],
                morph_modulation_amount: amounts[(b / 80 + 4) % 6],
                ..oracle::Patch::centered(model)
            };
            let patched = b % 160 >= 80;
            let m = oracle::Modulations {
                note: (phase * 0.7).sin() * 12.0,
                frequency: phase.sin(),
                harmonics: (phase * 1.3).cos() * 0.4,
                timbre: (phase * 0.9).sin() * 0.8,
                morph: (phase * 1.7).cos(),
                frequency_patched: patched,
                timbre_patched: !patched,
                morph_patched: patched,
                ..triggered(gate(block, 40, 12))
            };
            blocks.push((12, p, m));
        }
    }
    null_scenario("modulation inputs", &blocks);
}

/// Speech: the internal envelope amplitude follows harmonics, prosody and speed follow the
/// frequency and morph attenuverters when triggered. Chiptune: the built-in envelope takes the
/// timbre attenuverter as its shape when triggered and the timbre input is unpatched.
#[test]
fn speech_and_chiptune_special_cases() {
    let mut blocks = Vec::new();
    for b in 0..1200 {
        let shape = [-0.6, -0.1, 0.0, 0.2, 0.7, 1.0][b / 200];
        let p = oracle::Patch {
            harmonics: (b % 300) as f32 / 300.0,
            frequency_modulation_amount: shape,
            morph_modulation_amount: -shape,
            ..oracle::Patch::centered(Model::Speech)
        };
        let m = oracle::Modulations { frequency_patched: b % 400 >= 300, morph_patched: b % 400 >= 350, trigger_patched: b % 600 < 500, ..triggered(gate(b, 60, 20)) };
        blocks.push((12, p, m));
    }
    for b in 0..1200 {
        let shape = [-0.6, -0.1, 0.0, 0.2, 0.7, 1.0][b / 200];
        let p = oracle::Patch { timbre_modulation_amount: shape, note: 48.0, ..oracle::Patch::centered(Model::Chiptune) };
        let m = oracle::Modulations { timbre_patched: b % 400 >= 300, trigger_patched: b % 600 < 500, ..triggered(gate(b, 60, 20)) };
        blocks.push((12, p, m));
    }
    null_scenario("speech and chiptune special cases", &blocks);
}

/// Blocks of 1 to 24 frames: the firmware's time constants assume its 12-frame block whatever the
/// size of a call, and the engines render any size up to 24.
#[test]
fn block_sizes() {
    let sizes = [12, 1, 24, 7, 12, 5, 24, 13, 2, 12, 24, 11];
    let mut blocks = Vec::new();
    for (i, model) in [Model::VirtualAnalogFilter, Model::SixOpFmBankA, Model::Speech, Model::ParticleNoise, Model::SnareDrum].into_iter().enumerate() {
        for b in 0..300 {
            let block = i * 300 + b;
            let p = oracle::Patch { note: 45.0 + i as f32 * 6.0, decay: 0.4, ..oracle::Patch::centered(model) };
            blocks.push((sizes[block % sizes.len()], p, triggered(gate(block, 37, 9))));
        }
    }
    null_scenario("block sizes", &blocks);
}
