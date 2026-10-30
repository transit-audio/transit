//! Voice-level null tests of every engine against the C++ oracle (ADR 0033, #195).
//!
//! Each engine is rendered through a whole voice over the oracle's voice-level sweep
//! (`sweep::voice_points`: 84 points, 48 kHz, 12-frame blocks, a fresh voice and the same seed per
//! point) by the port and by the oracle on the same runner: the trigger delay, the decay envelope,
//! the low-pass gate envelope, the engine's render, the limiter, the low-pass gate and the int16
//! conversion. The int16 frames, out and aux, must be identical: zero LSB of difference on every
//! sample of every point. There is no tolerance, so any difference at all is a port defect to hunt
//! against the C++, never a bound to widen.
//!
//! Flush-to-zero, which the oracle runs with, is reproduced in the port's own code (see the crate
//! README); the test process keeps Rust's default floating-point mode.
//!
//! Set `MACRO_OSC_NULL_POINTS` to print the residual and the first differing frame of every point
//! whose label contains its value (empty: every point).

#![forbid(unsafe_code)]

use transit_macro_osc::voice::{Frame, Modulations, Patch, Voice};
use transit_macro_osc_oracle::residual::{Residual, residual_i16};
use transit_macro_osc_oracle::sweep::{SEED, VOICE_POINT_FRAMES, VoicePoint, render_voice_point, voice_points};
use transit_macro_osc_oracle::{self as oracle, BLOCK_SIZE, Model, SAMPLE_RATE, Session};

/// The oracle's patch as the port's.
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

/// The oracle's modulation inputs as the port's.
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

/// Renders one voice-level point from a fresh port voice, as `sweep::render_voice_point` renders it
/// from a fresh oracle voice: seed, init, then 12-frame blocks.
fn render_port_point(model: Model, point: &VoicePoint) -> Vec<Frame> {
    let mut voice = Voice::new(SAMPLE_RATE);
    voice.seed(SEED);
    voice.init();
    let blocks = VOICE_POINT_FRAMES / BLOCK_SIZE;
    let mut frames = vec![Frame::default(); VOICE_POINT_FRAMES];
    for (block, chunk) in frames.as_chunks_mut::<BLOCK_SIZE>().0.iter_mut().enumerate() {
        let (p, m) = point.inputs(model, block, blocks);
        voice.render(&patch(&p), &modulations(&m), chunk);
    }
    frames
}

fn worse(a: Option<(Residual, String)>, r: Residual, label: String) -> Option<(Residual, String)> {
    match a {
        Some((w, l)) if (w.peak_dbfs, w.differing) >= (r.peak_dbfs, r.differing) => Some((w, l)),
        _ => Some((r, label)),
    }
}

/// Renders the whole voice-level sweep of `model` on both sides and asserts identical frames.
fn null_voice(model: Model) {
    let session = Session::lock();
    let filter = std::env::var("MACRO_OSC_NULL_POINTS").ok();
    let (mut worst_out, mut worst_aux) = (None, None);
    let mut failing = 0usize;
    let points = voice_points();
    for point in &points {
        let oracle = render_voice_point(&session, model, point).expect("oracle render");
        let port = render_port_point(model, point);
        let (port_out, port_aux): (Vec<i16>, Vec<i16>) = port.iter().map(|f| (f.out, f.aux)).unzip();
        let (oracle_out, oracle_aux): (Vec<i16>, Vec<i16>) = oracle.iter().map(|f| (f.out, f.aux)).unzip();
        let out = residual_i16(&port_out, &oracle_out).expect("lengths");
        let aux = residual_i16(&port_aux, &oracle_aux).expect("lengths");
        if !(out.is_exact() && aux.is_exact()) {
            failing += 1;
        }
        if filter.as_ref().is_some_and(|f| point.label().contains(f.as_str())) {
            println!(
                "{} {}: out {out}{}; aux {aux}{}",
                model.label(),
                point.label(),
                first_diff(&port_out, &oracle_out),
                first_diff(&port_aux, &oracle_aux)
            );
        }
        worst_out = worse(worst_out, out, point.label());
        worst_aux = worse(worst_aux, aux, point.label());
    }
    let (wo, lo) = worst_out.expect("points");
    let (wa, la) = worst_aux.expect("points");
    println!("{}: worst out {wo} at {lo}; worst aux {wa} at {la}; {failing} of {} points differ", model.label(), points.len());
    assert_eq!(failing, 0, "{}: {failing} points differ; worst out {wo} at {lo}; worst aux {wa} at {la}", model.label());
}

macro_rules! null_tests {
    ($($name:ident => $model:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                null_voice(Model::$model);
            }
        )*
    };
}

null_tests! {
    virtual_analog_filter => VirtualAnalogFilter,
    phase_distortion => PhaseDistortion,
    six_op_fm_bank_a => SixOpFmBankA,
    six_op_fm_bank_b => SixOpFmBankB,
    six_op_fm_bank_c => SixOpFmBankC,
    wave_terrain => WaveTerrain,
    string_machine => StringMachine,
    chiptune => Chiptune,
    virtual_analog => VirtualAnalog,
    waveshaping => Waveshaping,
    fm => Fm,
    grain_formant => GrainFormant,
    additive => Additive,
    wavetable => Wavetable,
    chord => Chord,
    speech => Speech,
    swarm => Swarm,
    filtered_noise => FilteredNoise,
    particle_noise => ParticleNoise,
    inharmonic_string => InharmonicString,
    modal_resonator => ModalResonator,
    bass_drum => BassDrum,
    snare_drum => SnareDrum,
    hi_hat => HiHat,
}

/// The first frame whose value differs, as `, first at <index> (port <a>, oracle <b>)`.
fn first_diff(port: &[i16], oracle: &[i16]) -> String {
    port.iter().zip(oracle).position(|(a, b)| a != b).map(|i| format!(", first at {i} (port {}, oracle {})", port[i], oracle[i])).unwrap_or_default()
}
