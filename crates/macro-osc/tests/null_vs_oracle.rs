//! Null tests of every engine against the C++ oracle (ADR 0033, #195).
//!
//! Each engine is rendered over the oracle's engine-level sweep (`sweep::engine_points`: 78 points,
//! 48 kHz, 12-frame blocks, a fresh engine and the same seed per point) by the port and by the
//! oracle on the same runner, and the peak and RMS residual of out and aux is held to [`bound_dbfs`]:
//! the oracle's measured floor for this runner family plus [`MARGIN_DB`].
//!
//! The margin is zero: every engine nulls bit-identical against the oracle (measured on Windows
//! x86-64 MSVC, 2026-10-29), so any residual at all is a port defect to hunt, never a margin to
//! widen. Flush-to-zero, which the oracle runs with, is reproduced in the port's own code
//! (`transit_macro_osc::utils::ftz`, see the crate README); the test process keeps Rust's default
//! floating-point mode.
//!
//! Set `MACRO_OSC_NULL_POINTS` to print the residual and the first differing sample of every point
//! whose label contains its value (empty: every point).

#![forbid(unsafe_code)]

use transit_macro_osc::engine::{Engine, EngineParameters, TriggerState};
use transit_macro_osc_oracle::floor::RunnerFamily;
use transit_macro_osc_oracle::residual::{Residual, residual_f32};
use transit_macro_osc_oracle::sweep::{ENGINE_POINT_FRAMES, EnginePoint, SEED, engine_points, render_engine_point};
use transit_macro_osc_oracle::{BLOCK_SIZE, Model, Session, Trigger};

/// The margin over the oracle's floor, dB. Zero: the port is held to bit identity.
const MARGIN_DB: f64 = 0.0;

/// The bound every engine is held to, peak and RMS, dBFS: the oracle's self-comparison floor on this
/// runner family plus [`MARGIN_DB`]. A family with no measured floor is held to bit identity too.
fn bound_dbfs() -> f64 {
    RunnerFamily::current().self_floor_dbfs().unwrap_or(f64::NEG_INFINITY) + MARGIN_DB
}

fn port_engine(model: Model) -> Box<dyn Engine> {
    use transit_macro_osc::engine::*;
    use transit_macro_osc::engine2::*;
    use transit_macro_osc::resources::sysex::{SYX_BANK_0, SYX_BANK_1, SYX_BANK_2};
    let n = transit_macro_osc_oracle::MAX_BLOCK_SIZE;
    match model {
        Model::VirtualAnalogFilter => Box::new(virtual_analog_vcf_engine::VirtualAnalogVcfEngine::new()),
        Model::PhaseDistortion => Box::new(phase_distortion_engine::PhaseDistortionEngine::new(n)),
        Model::SixOpFmBankA | Model::SixOpFmBankB | Model::SixOpFmBankC => {
            let mut e = six_op_engine::SixOpEngine::new(n);
            e.load_syx_bank(match model {
                Model::SixOpFmBankA => &SYX_BANK_0,
                Model::SixOpFmBankB => &SYX_BANK_1,
                _ => &SYX_BANK_2,
            });
            Box::new(e)
        }
        Model::WaveTerrain => Box::new(wave_terrain_engine::WaveTerrainEngine::new(n)),
        Model::StringMachine => Box::new(string_machine_engine::StringMachineEngine::new()),
        Model::Chiptune => Box::new(chiptune_engine::ChiptuneEngine::new()),
        Model::VirtualAnalog => Box::new(virtual_analog_engine::VirtualAnalogEngine::new(n)),
        Model::Waveshaping => Box::new(waveshaping_engine::WaveshapingEngine::new()),
        Model::Fm => Box::new(fm_engine::FmEngine::new()),
        Model::GrainFormant => Box::new(grain_engine::GrainEngine::new()),
        Model::Additive => Box::new(additive_engine::AdditiveEngine::new()),
        Model::Wavetable => Box::new(wavetable_engine::WavetableEngine::new()),
        Model::Chord => Box::new(chord_engine::ChordEngine::new()),
        Model::Speech => Box::new(speech_engine::SpeechEngine::new(n)),
        Model::Swarm => Box::new(swarm_engine::SwarmEngine::new()),
        Model::FilteredNoise => Box::new(noise_engine::NoiseEngine::new(n)),
        Model::ParticleNoise => Box::new(particle_engine::ParticleEngine::new(n)),
        Model::InharmonicString => Box::new(string_engine::StringEngine::new(n)),
        Model::ModalResonator => Box::new(modal_engine::ModalEngine::new(n)),
        Model::BassDrum => Box::new(bass_drum_engine::BassDrumEngine::new()),
        Model::SnareDrum => Box::new(snare_drum_engine::SnareDrumEngine::new()),
        Model::HiHat => Box::new(hihat_engine::HihatEngine::new(n)),
    }
}

/// The oracle's trigger bits, which are the firmware's `TriggerState` bit set, as the port's.
fn trigger(t: Trigger) -> TriggerState {
    TriggerState(u8::try_from(t.0).expect("trigger bits fit a byte"))
}

struct PortRender {
    out: Vec<f32>,
    aux: Vec<f32>,
}

fn render_port_point(model: Model, point: &EnginePoint) -> PortRender {
    let mut engine = port_engine(model);
    engine.seed(SEED);
    engine.init(transit_macro_osc::REF_SAMPLE_RATE);
    engine.reset();
    let blocks = ENGINE_POINT_FRAMES / BLOCK_SIZE;
    let mut render = PortRender { out: vec![0.0; ENGINE_POINT_FRAMES], aux: vec![0.0; ENGINE_POINT_FRAMES] };
    let (outs, _) = render.out.as_chunks_mut::<BLOCK_SIZE>();
    let (auxes, _) = render.aux.as_chunks_mut::<BLOCK_SIZE>();
    for (block, (out, aux)) in outs.iter_mut().zip(auxes.iter_mut()).enumerate() {
        let p = point.parameters(block, blocks);
        let parameters = EngineParameters {
            trigger: trigger(p.trigger),
            note: p.note,
            timbre: p.timbre,
            morph: p.morph,
            harmonics: p.harmonics,
            accent: p.accent,
            a0_normalized: transit_macro_osc::A0,
        };
        let mut enveloped = false;
        engine.render(&parameters, out, aux, &mut enveloped);
    }
    render
}

fn worse(a: Option<(Residual, String)>, r: Residual, label: String) -> Option<(Residual, String)> {
    match a {
        Some((w, l)) if w.peak_dbfs >= r.peak_dbfs => Some((w, l)),
        _ => Some((r, label)),
    }
}

/// Renders the whole engine-level sweep of `model` on both sides and asserts the bound.
fn null_engine(model: Model) {
    let session = Session::lock();
    let bound = bound_dbfs();
    let filter = std::env::var("MACRO_OSC_NULL_POINTS").ok();
    let (mut worst_out, mut worst_aux) = (None, None);
    let mut failing = 0usize;
    let points = engine_points();
    for point in &points {
        let oracle = render_engine_point(&session, model, point).expect("oracle render");
        let port = render_port_point(model, point);
        let out = residual_f32(&port.out, &oracle.out).expect("lengths");
        let aux = residual_f32(&port.aux, &oracle.aux).expect("lengths");
        // Bits, not values: a +0.0 against -0.0 is a residual of -inf dBFS but still a difference.
        let exact_required = bound == f64::NEG_INFINITY;
        if !out.within(bound) || !aux.within(bound) || (exact_required && !(out.is_exact() && aux.is_exact())) {
            failing += 1;
        }
        if filter.as_ref().is_some_and(|f| point.label().contains(f.as_str())) {
            println!(
                "{} {}: out {out}{}; aux {aux}{}",
                model.label(),
                point.label(),
                first_diff(&port.out, &oracle.out),
                first_diff(&port.aux, &oracle.aux)
            );
        }
        worst_out = worse(worst_out, out, point.label());
        worst_aux = worse(worst_aux, aux, point.label());
    }
    let (wo, lo) = worst_out.expect("points");
    let (wa, la) = worst_aux.expect("points");
    println!("{}: worst out {wo} at {lo}; worst aux {wa} at {la}; {failing} of {} points over the bound", model.label(), points.len());
    assert_eq!(failing, 0, "{}: {failing} points over {bound} dBFS; worst out {wo} at {lo}; worst aux {wa} at {la}", model.label());
}

macro_rules! null_tests {
    ($($name:ident => $model:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                null_engine(Model::$model);
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

/// The first sample whose bits differ, as `, first at <index> (port <a>, oracle <b>)`.
fn first_diff(port: &[f32], oracle: &[f32]) -> String {
    port.iter()
        .zip(oracle)
        .position(|(a, b)| a.to_bits() != b.to_bits())
        .map(|i| format!(", first at {i} (port {:e}, oracle {:e})", port[i], oracle[i]))
        .unwrap_or_default()
}
