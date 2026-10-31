//! transit's same-pitch restart on the six-op models (#263, ADR 0033), against the C++ oracle.
//!
//! The restart is off in the crate, and every other null test runs with it off. These run with it
//! on and show it changes nothing but a same-pitch re-strike: the six-op banks' engine-level and
//! voice-level sweeps (one trigger per point, so no re-strike) and a scenario of re-strikes at
//! other pitches stay at 0 LSB, while a scenario of same-pitch re-strikes differs from the
//! firmware (the restart engages) with the restart on and nulls with it off.

#![forbid(unsafe_code)]

use transit_macro_osc::SampleRate;
use transit_macro_osc::engine::{Engine, EngineParameters, TriggerState};
use transit_macro_osc::engine2::six_op_engine::SixOpEngine;
use transit_macro_osc::resources::sysex::{SYX_BANK_0, SYX_BANK_1, SYX_BANK_2};
use transit_macro_osc::utils::random::Random;
use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc_oracle::residual::residual_i16;
use transit_macro_osc_oracle::sweep::{
    ENGINE_POINT_FRAMES, SEED, VOICE_POINT_FRAMES, engine_points, render_engine_point, render_voice_point, voice_points,
};
use transit_macro_osc_oracle::{self as oracle, BLOCK_SIZE, MAX_BLOCK_SIZE, Model, SAMPLE_RATE, Session};

const SIX_OP: [Model; 3] = [Model::SixOpFmBankA, Model::SixOpFmBankB, Model::SixOpFmBankC];

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

fn port_voice(restart: bool) -> Voice<'static> {
    let mut voice = Voice::new(SampleRate::Hz48000);
    voice.set_same_pitch_restart(restart);
    voice.seed(SEED);
    voice.init();
    voice
}

/// Every engine-level sweep point of the six-op banks with the restart on: the same bits and the
/// same random state as the firmware.
#[test]
fn the_six_op_engine_sweep_nulls_with_the_restart_on() {
    let session = Session::lock();
    for model in SIX_OP {
        let points = engine_points();
        for point in &points {
            let oracle = render_engine_point(&session, model, point).expect("oracle render");
            let oracle_rng = session.rng_state();
            let mut engine = SixOpEngine::new(MAX_BLOCK_SIZE);
            engine.load_syx_bank(match model {
                Model::SixOpFmBankA => &SYX_BANK_0,
                Model::SixOpFmBankB => &SYX_BANK_1,
                _ => &SYX_BANK_2,
            });
            engine.set_same_pitch_restart(true);
            let mut rng = Random::new(SEED);
            engine.init(transit_macro_osc::REF_SAMPLE_RATE);
            engine.reset();
            let blocks = ENGINE_POINT_FRAMES / BLOCK_SIZE;
            let (mut out, mut aux) = (vec![0.0f32; ENGINE_POINT_FRAMES], vec![0.0f32; ENGINE_POINT_FRAMES]);
            for (block, (o, a)) in out.as_chunks_mut::<BLOCK_SIZE>().0.iter_mut().zip(aux.as_chunks_mut::<BLOCK_SIZE>().0.iter_mut()).enumerate() {
                let p = point.parameters(block, blocks);
                let parameters = EngineParameters {
                    trigger: TriggerState(u8::try_from(p.trigger.0).expect("trigger bits fit a byte")),
                    note: p.note,
                    timbre: p.timbre,
                    morph: p.morph,
                    harmonics: p.harmonics,
                    accent: p.accent,
                    a0_normalized: transit_macro_osc::A0,
                };
                let mut enveloped = false;
                engine.render(&parameters, &mut rng, o, a, &mut enveloped);
            }
            let same = |port: &[f32], oracle: &[f32]| port.iter().zip(oracle).all(|(a, b)| a.to_bits() == b.to_bits());
            assert!(
                same(&out, &oracle.out) && same(&aux, &oracle.aux),
                "{} {}: the restart changed a point with no re-strike",
                model.label(),
                point.label()
            );
            assert_eq!(rng.state(), oracle_rng, "{} {}: random state", model.label(), point.label());
        }
        println!("{}: {} engine points at 0 LSB with the restart on", model.label(), points.len());
    }
}

/// Every voice-level sweep point of the six-op banks with the restart on: identical int16 frames.
#[test]
fn the_six_op_voice_sweep_nulls_with_the_restart_on() {
    let session = Session::lock();
    for model in SIX_OP {
        let points = voice_points();
        for point in &points {
            let oracle = render_voice_point(&session, model, point).expect("oracle render");
            let mut voice = port_voice(true);
            let blocks = VOICE_POINT_FRAMES / BLOCK_SIZE;
            let mut frames = vec![Frame::default(); VOICE_POINT_FRAMES];
            for (block, chunk) in frames.as_chunks_mut::<BLOCK_SIZE>().0.iter_mut().enumerate() {
                let (p, m) = point.inputs(model, block, blocks);
                voice.render(&patch(&p), &modulations(&m), chunk);
            }
            let port: Vec<(i16, i16)> = frames.iter().map(|f| (f.out, f.aux)).collect();
            let oracle: Vec<(i16, i16)> = oracle.iter().map(|f| (f.out, f.aux)).collect();
            assert!(port == oracle, "{} {}: the restart changed a point with no re-strike", model.label(), point.label());
        }
        println!("{}: {} voice points at 0 LSB with the restart on", model.label(), points.len());
    }
}

/// Renders `blocks` (patch, modulations) from a fresh oracle voice and a fresh port voice with the
/// restart `restart`: whether the int16 frames are identical.
fn nulls(blocks: &[(oracle::Patch, oracle::Modulations)], restart: bool) -> bool {
    let session = Session::lock();
    session.seed(SEED);
    let mut oracle_voice = session.voice(SAMPLE_RATE).expect("oracle voice");
    let mut port_voice = port_voice(restart);
    let mut oracle_frames = [oracle::Frame::default(); BLOCK_SIZE];
    let mut port_frames = [Frame::default(); BLOCK_SIZE];
    let (mut port, mut reference) = (Vec::new(), Vec::new());
    for (p, m) in blocks {
        oracle_voice.render(p, m, &mut oracle_frames).expect("oracle render");
        port_voice.render(&patch(p), &modulations(m), &mut port_frames);
        reference.extend(oracle_frames.iter().map(|f| (f.out, f.aux)));
        port.extend(port_frames.iter().map(|f| (f.out, f.aux)));
    }
    let out = residual_i16(&port.iter().map(|f| f.0).collect::<Vec<_>>(), &reference.iter().map(|f| f.0).collect::<Vec<_>>()).expect("lengths");
    println!("restart {restart}: out {out}");
    port == reference
}

/// Re-strikes of each six-op bank every 30 blocks, gate high for 25 (a 1.25 ms gap), with level
/// patched as transit's instrument drives it: the note of each strike from `notes`, in turn.
fn restrikes(notes: &[f32]) -> Vec<(oracle::Patch, oracle::Modulations)> {
    let mut blocks = Vec::new();
    for model in SIX_OP {
        for b in 0..600 {
            let note = notes[(b / 30) % notes.len()];
            let p = oracle::Patch { note: 0.0, ..oracle::Patch::centered(model) };
            let trigger = if b % 30 < 25 { 1.0 } else { 0.0 };
            let m = oracle::Modulations { note, trigger, trigger_patched: true, level: 0.8, level_patched: true, ..oracle::Modulations::default() };
            blocks.push((p, m));
        }
    }
    blocks
}

/// Re-strikes that each change the note by a semitone or more are firmware re-strikes: they null
/// with the restart on.
#[test]
fn restrikes_at_other_pitches_null_with_the_restart_on() {
    assert!(nulls(&restrikes(&[48.0, 55.0, 49.0, 60.0, 59.0]), true));
}

/// Same-pitch re-strikes are where the restart deviates from the firmware: with it off they null,
/// with it on they do not.
#[test]
fn same_pitch_restrikes_deviate_only_with_the_restart_on() {
    let blocks = restrikes(&[48.0]);
    assert!(nulls(&blocks, false), "the restart off is the firmware");
    assert!(!nulls(&blocks, true), "the restart on engages at a same-pitch re-strike");
}
