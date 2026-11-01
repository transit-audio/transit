//! The port against the oracle (ADR 0033 gate, #198 slice 2): every configuration (the three
//! resonator models and the FM pair voice) over the oracle's fixed sweep, bit for bit (0 LSB on the
//! float main and aux outputs), and the random state equal after every point.
//!
//! A failure is a port defect to root-cause against the C++, never a flake: never rerun to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::modal_voice::{ModalVoiceModel, ModalVoicePatch};
use transit_macro_osc_oracle::modal_voice_sweep::{Configuration, POINT_FRAMES, Point, SEED, points, render_point};
use transit_macro_osc_oracle::residual::residual_f32;
use transit_macro_osc_oracle::subset;
use transit_modal_voice::{MAX_BLOCK_SIZE, ModalVoice, Patch, PerformanceState, ResonatorModel};

fn port_patch(p: &ModalVoicePatch) -> Patch {
    Patch {
        exciter_envelope_shape: p.envelope_shape,
        exciter_bow_level: p.bow_level,
        exciter_bow_timbre: p.bow_timbre,
        exciter_blow_level: p.blow_level,
        exciter_blow_meta: p.blow_meta,
        exciter_blow_timbre: p.blow_timbre,
        exciter_strike_level: p.strike_level,
        exciter_strike_meta: p.strike_meta,
        exciter_strike_timbre: p.strike_timbre,
        resonator_geometry: p.geometry,
        resonator_brightness: p.brightness,
        resonator_damping: p.damping,
        resonator_position: p.position,
        space: p.space,
    }
}

fn configure(voice: &mut ModalVoice, configuration: Configuration) {
    match configuration {
        Configuration::Model(ModalVoiceModel::Modal) => voice.set_model(ResonatorModel::Modal),
        Configuration::Model(ModalVoiceModel::String) => voice.set_model(ResonatorModel::String),
        Configuration::Model(ModalVoiceModel::Strings) => voice.set_model(ResonatorModel::Strings),
        Configuration::EasterEgg => voice.set_easter_egg(true),
    }
}

/// The port rendering one sweep point: (main, aux, random state after).
fn render_port(configuration: Configuration, point: &Point) -> (Vec<f32>, Vec<f32>, u32) {
    let mut voice = ModalVoice::new();
    voice.set_random_state(SEED);
    configure(&mut voice, configuration);
    if let Some(words) = point.serial {
        voice.seed(&words);
    }
    *voice.patch_mut() = port_patch(&point.patch);
    let blocks = POINT_FRAMES / MAX_BLOCK_SIZE;
    let (blow, strike) = point.inputs(POINT_FRAMES);
    let (mut main, mut aux) = (vec![0.0_f32; POINT_FRAMES], vec![0.0_f32; POINT_FRAMES]);
    for block in 0..blocks {
        let p = point.performance(block, blocks);
        let performance = PerformanceState { gate: p.gate, note: p.note, modulation: p.modulation, strength: p.strength };
        let r = block * MAX_BLOCK_SIZE..(block + 1) * MAX_BLOCK_SIZE;
        voice.process(&performance, &blow[r.clone()], &strike[r.clone()], &mut main[r.clone()], &mut aux[r]);
    }
    (main, aux, voice.random_state())
}

fn null(configuration: Configuration) {
    let session = Session::lock();
    let kept = subset::strided(&format!("modal voice null, {}", configuration.label()), points());
    let mut failures = Vec::new();
    for point in &kept {
        let reference = render_point(&session, configuration, point).unwrap();
        let reference_state = session.rng_state();
        let (main, aux, state) = render_port(configuration, point);
        for (which, x, y) in [("main", &main, &reference.main), ("aux", &aux, &reference.aux)] {
            let r = residual_f32(x, y).unwrap();
            if !r.is_exact() {
                let first = x.iter().zip(y.iter()).position(|(a, b)| a.to_bits() != b.to_bits()).unwrap_or(0);
                failures.push(format!("{} {} {which}: {r}, first differing frame {first}", configuration.label(), point.label()));
            }
        }
        if state != reference_state {
            failures.push(format!("{} {}: random state {state:#x}, oracle {reference_state:#x}", configuration.label(), point.label()));
        }
    }
    assert!(failures.is_empty(), "{} of {} checks differ:\n{}", failures.len(), kept.len() * 3, failures.join("\n"));
}

#[test]
fn modal_nulls() {
    null(Configuration::Model(ModalVoiceModel::Modal));
}

#[test]
fn string_nulls() {
    null(Configuration::Model(ModalVoiceModel::String));
}

#[test]
fn strings_nulls() {
    null(Configuration::Model(ModalVoiceModel::Strings));
}

#[test]
fn fm_pair_voice_nulls() {
    null(Configuration::EasterEgg);
}
