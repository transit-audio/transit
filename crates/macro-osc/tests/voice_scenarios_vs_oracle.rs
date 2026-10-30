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

/// Out and aux of one render, frame by frame.
struct Render {
    out: Vec<i16>,
    aux: Vec<i16>,
}

/// Renders `blocks` from a fresh oracle voice and a fresh port voice, both seeded with [`SEED`].
fn render_both(blocks: &[Block]) -> (Render, Render) {
    let session = Session::lock();
    session.seed(SEED);
    let mut oracle_voice = session.voice(SAMPLE_RATE).expect("oracle voice");
    let mut port_voice = Voice::new(SAMPLE_RATE);
    port_voice.seed(SEED);
    port_voice.init();

    let mut port = Render { out: Vec::new(), aux: Vec::new() };
    let mut oracle = Render { out: Vec::new(), aux: Vec::new() };
    let mut oracle_frames = [oracle::Frame::default(); MAX_BLOCK_SIZE];
    let mut port_frames = [Frame::default(); MAX_BLOCK_SIZE];
    for (size, p, m) in blocks {
        oracle_voice.render(p, m, &mut oracle_frames[..*size]).expect("oracle render");
        port_voice.render(&patch(p), &modulations(m), &mut port_frames[..*size]);
        oracle.out.extend(oracle_frames[..*size].iter().map(|f| f.out));
        oracle.aux.extend(oracle_frames[..*size].iter().map(|f| f.aux));
        port.out.extend(port_frames[..*size].iter().map(|f| f.out));
        port.aux.extend(port_frames[..*size].iter().map(|f| f.aux));
    }
    (port, oracle)
}

/// Renders `blocks` on both sides from fresh voices and asserts identical frames.
fn null_scenario(name: &str, blocks: &[Block]) {
    let (port, oracle) = render_both(blocks);
    let out = residual_i16(&port.out, &oracle.out).expect("lengths");
    let aux = residual_i16(&port.aux, &oracle.aux).expect("lengths");
    let first = port.out.iter().zip(&oracle.out).chain(port.aux.iter().zip(&oracle.aux)).position(|(a, b)| a != b);
    println!("{name}: {} frames; out {out}; aux {aux}", port.out.len());
    assert!(out.is_exact() && aux.is_exact(), "{name}: out {out}; aux {aux}; first differing value at {first:?} (out frames, then aux)");
}

/// A gate pattern: high for the first `high` blocks of every `period`.
fn gate(block: usize, period: usize, high: usize) -> f32 {
    if block % period < high { 1.0 } else { 0.0 }
}

fn triggered(trigger: f32) -> oracle::Modulations {
    oracle::Modulations { trigger, trigger_patched: true, ..oracle::Modulations::default() }
}

fn is_six_op(model: Model) -> bool {
    matches!(model, Model::SixOpFmBankA | Model::SixOpFmBankB | Model::SixOpFmBankC)
}

/// 60 triggered blocks per engine, in `order`, within one voice.
fn engine_change_blocks(order: &[Model]) -> Vec<Block> {
    let mut blocks = Vec::new();
    for (i, model) in order.iter().enumerate() {
        for b in 0..60 {
            let block = i * 60 + b;
            let p = oracle::Patch { note: 48.0 + (i % 5) as f32 * 7.0, decay: 0.6, ..oracle::Patch::centered(*model) };
            blocks.push((12, p, triggered(gate(block, 30, 10))));
        }
    }
    blocks
}

/// Engine changes within one voice, triggered: each change reloads and resets the new engine and
/// resets the out limiter, and the noise engines draw from the voice's one generator where the
/// previous engine left it. The order switches between the six-op banks first, then visits every
/// engine forward, then every other engine backward and in a scrambled order, so most pairs of
/// engines meet. The six-op engine is not re-entered after another engine that shares its RAM:
/// see [`six_op_reentry_after_a_ram_sharing_engine`].
#[test]
fn engine_changes_by_patch() {
    let others: Vec<Model> = Model::ALL.iter().copied().filter(|m| !is_six_op(*m)).collect();
    let mut scrambled = others.clone();
    let mut state = SEED;
    for i in (1..scrambled.len()).rev() {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        scrambled.swap(i, (state >> 16) as usize % (i + 1));
    }
    let order: Vec<Model> = [Model::SixOpFmBankC, Model::SixOpFmBankA, Model::SixOpFmBankB, Model::SixOpFmBankA]
        .into_iter()
        .chain(Model::ALL)
        .chain(others.iter().rev().copied())
        .chain(scrambled)
        .collect();
    null_scenario("engine changes by patch", &engine_change_blocks(&order));
}

/// The one documented difference from the firmware at the voice level. The firmware's engines share
/// one 16 KB RAM arena (`Voice::Init` frees the allocator before each engine's `Init`), and the
/// six-op engine's staggered-render accumulator lives in it and is not cleared by `Reset`, so the
/// first block after the voice returns to a six-op bank mixes in whatever the engines rendered in
/// between left at that address (the string machine's ensemble delay, the particle diffuser, the
/// string, speech and swarm state, and, on a 64-bit host, the wavetable engine's pointer map).
/// The port's engines own their memory, so the six-op engine resumes from its own accumulator,
/// which is what the firmware does whenever nothing overwrote it (re-entry from another six-op
/// bank, or after the arena-free engines: the bank switches and the forward pass of
/// [`engine_changes_by_patch`]). Emulating the overlap would mean reproducing every engine's byte
/// layout, host pointer size included, to replay garbage for one block, so the port does not.
///
/// This test pins the difference to exactly that block, for every engine between two six-op
/// segments: every frame outside the first block of the second six-op segment is identical; that
/// block is identical after the engines that leave the accumulator alone and differs after the six
/// that overwrite it (measured on 64-bit hosts, as every CI runner is).
#[test]
fn six_op_reentry_after_a_ram_sharing_engine() {
    const OVERWRITE: [Model; 6] =
        [Model::StringMachine, Model::Wavetable, Model::Speech, Model::Swarm, Model::ParticleNoise, Model::InharmonicString];
    let reentry = 2 * 60 * 12..2 * 60 * 12 + 12;
    for between in Model::ALL.into_iter().filter(|m| !is_six_op(*m)) {
        let (port, oracle) = render_both(&engine_change_blocks(&[Model::SixOpFmBankC, between, Model::SixOpFmBankC]));
        for (channel, p, o) in [("out", &port.out, &oracle.out), ("aux", &port.aux, &oracle.aux)] {
            for (i, (a, b)) in p.iter().zip(o.iter()).enumerate() {
                assert!(
                    reentry.contains(&i) || a == b,
                    "{}: {channel} frame {i} differs (port {a}, oracle {b}) outside the re-entry block",
                    between.label()
                );
            }
            let differs = p[reentry.clone()] != o[reentry.clone()];
            assert_eq!(differs, OVERWRITE.contains(&between), "{}: {channel} re-entry block differs: {differs}", between.label());
        }
    }
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
            let m = oracle::Modulations {
                level,
                level_patched: true,
                trigger: gate(block, 50, 20),
                trigger_patched: b >= 200,
                ..oracle::Modulations::default()
            };
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
        let m = oracle::Modulations {
            frequency_patched: b % 400 >= 300,
            morph_patched: b % 400 >= 350,
            trigger_patched: b % 600 < 500,
            ..triggered(gate(b, 60, 20))
        };
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
    for (i, model) in [Model::VirtualAnalogFilter, Model::SixOpFmBankA, Model::Speech, Model::ParticleNoise, Model::SnareDrum].into_iter().enumerate()
    {
        for b in 0..300 {
            let block = i * 300 + b;
            let p = oracle::Patch { note: 45.0 + i as f32 * 6.0, decay: 0.4, ..oracle::Patch::centered(model) };
            blocks.push((sizes[block % sizes.len()], p, triggered(gate(block, 37, 9))));
        }
    }
    null_scenario("block sizes", &blocks);
}

/// Every engine with its three macros ramped over their whole range, one engine per fresh voice:
/// the sweep holds each macro at five values, so tables between them (chord, bank, patch and
/// wavetable indices, speech words) are reached here.
#[test]
fn macro_ramps_every_engine() {
    for model in Model::ALL {
        let mut blocks = Vec::new();
        for b in 0..1200 {
            let x = b as f32 / 1199.0;
            let p = oracle::Patch {
                note: 36.0 + 48.0 * ((b / 300) as f32 / 3.0),
                harmonics: x,
                timbre: 1.0 - x,
                morph: (x * 7.0).fract(),
                decay: 0.7,
                ..oracle::Patch::centered(model)
            };
            let m = if b % 600 < 300 { oracle::Modulations::default() } else { triggered(gate(b, 25, 8)) };
            blocks.push((12, p, m));
        }
        null_scenario(&format!("macro ramps, {}", model.label()), &blocks);
    }
}
