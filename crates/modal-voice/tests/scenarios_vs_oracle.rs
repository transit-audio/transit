//! The port against the oracle over long and irregular runs (#198 slice 2): a 20-second performance
//! per configuration with every control moving, model and voice changes mid-run, every block size
//! from 1 to 16, long silences after a ringing strike (where the flush-to-zero decides the tail),
//! bypass, and seeds. Bit for bit, random state included.
//!
//! A failure is a port defect to root-cause against the C++, never a flake: never rerun to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::modal_voice::{MAX_CONTROL, MAX_STRIKE_TIMBRE, ModalVoiceModel, ModalVoicePatch, Performance};
use transit_macro_osc_oracle::modal_voice_sweep::{Configuration, center};
use transit_macro_osc_oracle::residual::residual_f32;
use transit_modal_voice::{MAX_BLOCK_SIZE, ModalVoice, Patch, PerformanceState, ResonatorModel};

/// One block of a scenario.
#[derive(Clone, Copy, Debug)]
struct Step {
    size: usize,
    performance: Performance,
    patch: ModalVoicePatch,
    configuration: Option<Configuration>,
    bypass: bool,
    /// Both inputs carry a test signal (otherwise silence).
    inputs: bool,
}

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

fn port_configure(voice: &mut ModalVoice, configuration: Configuration) {
    match configuration {
        Configuration::Model(m) => {
            voice.set_easter_egg(false);
            voice.set_model(match m {
                ModalVoiceModel::Modal => ResonatorModel::Modal,
                ModalVoiceModel::String => ResonatorModel::String,
                ModalVoiceModel::Strings => ResonatorModel::Strings,
            });
        }
        Configuration::EasterEgg => voice.set_easter_egg(true),
    }
}

/// A deterministic input signal for frame `n` (a decaying click every 4000 frames plus low noise).
fn input_sample(n: usize, salt: u32) -> f32 {
    let mut x = u32::try_from(n % 1_000_003).unwrap_or(0).wrapping_mul(2_654_435_761) ^ salt;
    x ^= x >> 13;
    #[allow(clippy::cast_precision_loss)]
    let noise = ((x >> 8) as f32 / 16_777_216.0 - 0.5) * 0.05;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let click = 0.7 * 0.995_f32.powi((n % 4000) as i32);
    (noise + click).clamp(-1.0, 1.0)
}

/// Runs `steps` through the oracle and the port from fresh parts seeded alike and compares
/// everything.
fn compare(name: &str, seed: u32, serial: Option<[u32; 3]>, first: Configuration, steps: &[Step]) {
    let session = Session::lock();
    session.seed(seed);
    let mut oracle = session.modal_voice().unwrap();
    let mut port = ModalVoice::new();
    port.set_random_state(seed);
    match first {
        Configuration::Model(m) => oracle.set_model(m),
        Configuration::EasterEgg => oracle.set_easter_egg(true),
    }
    port_configure(&mut port, first);
    if let Some(words) = serial {
        oracle.seed(words);
        port.seed(&words);
    }
    let total: usize = steps.iter().map(|s| s.size).sum();
    let (mut om, mut oa, mut pm, mut pa) = (vec![0.0; total], vec![0.0; total], vec![0.0; total], vec![0.0; total]);
    let mut at = 0;
    for step in steps {
        if let Some(c) = step.configuration {
            match c {
                Configuration::Model(m) => {
                    oracle.set_easter_egg(false);
                    oracle.set_model(m);
                }
                Configuration::EasterEgg => oracle.set_easter_egg(true),
            }
            port_configure(&mut port, c);
        }
        oracle.set_bypass(step.bypass);
        port.set_bypass(step.bypass);
        let r = at..at + step.size;
        let blow: Vec<f32> = r.clone().map(|n| if step.inputs { input_sample(n, 0x51) } else { 0.0 }).collect();
        let strike: Vec<f32> = r.clone().map(|n| if step.inputs { input_sample(n, 0xA7) } else { 0.0 }).collect();
        oracle.process(&step.performance, &step.patch, &blow, &strike, &mut om[r.clone()], &mut oa[r.clone()]).unwrap();
        *port.patch_mut() = port_patch(&step.patch);
        let p = step.performance;
        let performance = PerformanceState { gate: p.gate, note: p.note, modulation: p.modulation, strength: p.strength };
        port.process(&performance, &blow, &strike, &mut pm[r.clone()], &mut pa[r]);
        at += step.size;
    }
    for (which, x, y) in [("main", &pm, &om), ("aux", &pa, &oa)] {
        let r = residual_f32(x, y).unwrap();
        let first = x.iter().zip(y.iter()).position(|(a, b)| a.to_bits() != b.to_bits());
        assert!(r.is_exact(), "{name} {which}: {r}, first differing frame {first:?} of {total}");
    }
    assert_eq!(port.random_state(), session.rng_state(), "{name}: random state");
    let peak = om.iter().chain(&oa).fold(0.0_f32, |m, s| m.max(s.abs()));
    assert!(peak > 1e-3, "{name}: the scenario must sound (peak {peak})");
}

fn blocks(seconds: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = (seconds * 32_000.0 / 16.0) as usize;
    n
}

/// A control value moving slowly through its whole range: a triangle of period `period` blocks.
#[allow(clippy::cast_precision_loss)]
fn sweep(block: usize, period: usize, phase: usize, top: f32) -> f32 {
    let t = ((block + phase) % period) as f32 / period as f32;
    let tri = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
    (tri * top).min(top)
}

/// The 20-second performance: notes every half second over four octaves, gates of varying length,
/// every control and the strength moving on its own period, the FM input wobbling.
fn performance_steps(seconds: f32) -> Vec<Step> {
    let notes = [36.0_f32, 48.0, 55.0, 60.0, 63.0, 67.0, 72.0, 79.0, 84.0, 43.0, 70.0, 58.0];
    (0..blocks(seconds))
        .map(|b| {
            let note_index = b / 1000;
            let gate_length = [700, 300, 950, 120][note_index % 4];
            let patch = ModalVoicePatch {
                envelope_shape: sweep(b, 9000, 0, MAX_CONTROL),
                bow_level: sweep(b, 7000, 1000, MAX_CONTROL),
                bow_timbre: sweep(b, 5300, 0, MAX_CONTROL),
                blow_level: sweep(b, 8100, 4000, MAX_CONTROL),
                blow_meta: sweep(b, 6100, 0, MAX_CONTROL),
                blow_timbre: sweep(b, 4300, 2000, MAX_CONTROL),
                strike_level: sweep(b, 6700, 3000, MAX_CONTROL),
                strike_meta: sweep(b, 12_000, 0, MAX_CONTROL),
                strike_timbre: sweep(b, 3900, 0, MAX_STRIKE_TIMBRE),
                geometry: sweep(b, 11_000, 500, MAX_CONTROL),
                brightness: sweep(b, 5900, 2500, MAX_CONTROL),
                damping: sweep(b, 7700, 6000, MAX_CONTROL),
                position: sweep(b, 3100, 0, MAX_CONTROL),
                space: sweep(b, 13_000, 0, 2.0),
            };
            #[allow(clippy::cast_precision_loss)]
            let wobble = ((b % 400) as f32 / 400.0 - 0.5) * 2.0;
            let performance = Performance {
                gate: b % 1000 < gate_length,
                note: notes[note_index % notes.len()],
                modulation: wobble,
                strength: sweep(b, 2700, 0, 1.0),
            };
            Step { size: MAX_BLOCK_SIZE, performance, patch, configuration: None, bypass: false, inputs: false }
        })
        .collect()
}

#[test]
fn a_twenty_second_performance_nulls_in_every_configuration() {
    for configuration in Configuration::ALL {
        compare(&format!("performance {}", configuration.label()), 0x1985_0001, None, configuration, &performance_steps(20.0));
    }
}

#[test]
fn model_and_voice_changes_mid_run_null() {
    let mut steps = performance_steps(12.0);
    let order = [
        Configuration::Model(ModalVoiceModel::String),
        Configuration::EasterEgg,
        Configuration::Model(ModalVoiceModel::Strings),
        Configuration::Model(ModalVoiceModel::Modal),
        Configuration::EasterEgg,
        Configuration::Model(ModalVoiceModel::String),
    ];
    for (i, c) in order.iter().enumerate() {
        steps[(i + 1) * 3500 + 77].configuration = Some(*c);
    }
    compare("model changes", 0x1985_0002, Some([1, 2, 3]), Configuration::Model(ModalVoiceModel::Modal), &steps);
}

#[test]
fn every_block_size_nulls() {
    for configuration in Configuration::ALL {
        let mut steps = performance_steps(4.0);
        for (i, s) in steps.iter_mut().enumerate() {
            s.size = 1 + (i * 7) % MAX_BLOCK_SIZE;
        }
        compare(&format!("block sizes {}", configuration.label()), 0x1985_0003, None, configuration, &steps);
    }
}

#[test]
fn long_silences_after_a_strike_null() {
    // A high strike into a fully damped resonator, the gate released, then 20 s with nothing but
    // the tail: the decay reaches the subnormal range, where the oracle's flush-to-zero decides
    // every bit (the port's `F`).
    for configuration in Configuration::ALL {
        let mut patch = center();
        patch.bow_level = 0.0;
        patch.blow_level = 0.0;
        patch.strike_level = 0.9;
        patch.damping = 0.0;
        patch.space = 0.0;
        let steps: Vec<Step> = (0..blocks(21.0))
            .map(|b| Step {
                size: MAX_BLOCK_SIZE,
                performance: Performance { gate: b < 200, note: 96.0, modulation: 0.0, strength: 0.8 },
                patch,
                configuration: None,
                bypass: false,
                inputs: false,
            })
            .collect();
        compare(&format!("silence {}", configuration.label()), 0x1985_0004, None, configuration, &steps);
    }
}

#[test]
fn bypass_and_patched_inputs_null() {
    let mut steps = performance_steps(6.0);
    for (i, s) in steps.iter_mut().enumerate() {
        s.inputs = true;
        s.bypass = (4000..5000).contains(&i);
    }
    compare("inputs and bypass", 0x1985_0005, Some([0xFEED, 0xBEEF, 0xCAFE]), Configuration::Model(ModalVoiceModel::Modal), &steps);
    let mut easter = performance_steps(4.0);
    for s in &mut easter {
        s.inputs = true;
    }
    compare("inputs easter egg", 0x1985_0006, None, Configuration::EasterEgg, &easter);
}
