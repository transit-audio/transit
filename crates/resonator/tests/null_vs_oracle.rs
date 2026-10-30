//! Null tests of every resonator model at every polyphony against the C++ oracle (ADR 0033, #198).
//!
//! Each model and polyphony is rendered over the oracle's resonator sweep
//! (`resonator_sweep::points`: 159 points, notes 36, 60 and 84, three exciters, each macro at 0,
//! 0.25, 0.5, 0.75 and 1, two FM offsets; 48 kHz; the firmware's 24-frame blocks; a fresh part and
//! the same seed per point) by the port and by the oracle on the same runner. Both outputs must be
//! bit-identical: the bound is the oracle's measured floor for this runner family (`-inf`) plus a
//! margin of zero, and the bits are compared too (a `+0.0` against a `-0.0` fails). The random state
//! after each point must equal the oracle's, so the port draws exactly as often as the firmware.
//! Flush-to-zero, which the oracle runs with, is reproduced in the port's own arithmetic
//! (`transit_resonator::float::F`); the test process keeps Rust's default floating-point mode.
//!
//! Set `RESONATOR_NULL_POINTS` to print the residual and the first differing sample of every point
//! whose label contains its value (empty: every point).

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::floor::RunnerFamily;
use transit_macro_osc_oracle::residual::{Residual, residual_f32};
use transit_macro_osc_oracle::resonator::{RESONATOR_BLOCK_SIZE, ResonatorModel as OracleModel};
use transit_macro_osc_oracle::resonator_sweep::{POINT_FRAMES, Point, SEED, points, render_point};
use transit_resonator::{MAX_BLOCK_SIZE, Patch, PerformanceState, Resonator, ResonatorModel};

/// The margin over the oracle's floor, dB. Zero: the port is held to bit identity.
const MARGIN_DB: f64 = 0.0;

fn bound_dbfs() -> f64 {
    RunnerFamily::current().self_floor_dbfs().unwrap_or(f64::NEG_INFINITY) + MARGIN_DB
}

fn port_model(model: OracleModel) -> ResonatorModel {
    ResonatorModel::ALL[model.index()]
}

struct PortRender {
    out: Vec<f32>,
    aux: Vec<f32>,
    rng_state: u32,
}

fn render_port_point(model: OracleModel, polyphony: usize, point: &Point) -> PortRender {
    let mut part = Resonator::new();
    part.seed(SEED);
    part.set_polyphony(polyphony);
    part.set_model(port_model(model));
    let blocks = POINT_FRAMES / RESONATOR_BLOCK_SIZE;
    let input = point.input_signal(POINT_FRAMES);
    let mut render = PortRender { out: vec![0.0; POINT_FRAMES], aux: vec![0.0; POINT_FRAMES], rng_state: 0 };
    let ins = input.as_chunks::<MAX_BLOCK_SIZE>().0;
    let outs = render.out.as_chunks_mut::<MAX_BLOCK_SIZE>().0;
    let auxes = render.aux.as_chunks_mut::<MAX_BLOCK_SIZE>().0;
    for (block, ((i, o), a)) in ins.iter().zip(outs.iter_mut()).zip(auxes.iter_mut()).enumerate() {
        let (p, m) = point.inputs(block, blocks);
        let performance = PerformanceState {
            strum: p.strum,
            internal_exciter: p.internal_exciter,
            internal_strum: p.internal_strum,
            internal_note: p.internal_note,
            tonic: p.tonic,
            note: p.note,
            fm: p.fm,
            chord: p.chord,
        };
        let patch = Patch { structure: m.structure, brightness: m.brightness, damping: m.damping, position: m.position };
        part.process(&performance, &patch, i, o, a);
    }
    render.rng_state = part.rng_state();
    render
}

fn worse(a: Option<(Residual, String)>, r: Residual, label: String) -> Option<(Residual, String)> {
    match a {
        Some((w, l)) if w.peak_dbfs >= r.peak_dbfs => Some((w, l)),
        _ => Some((r, label)),
    }
}

/// The first sample whose bits differ, as `, first at <index> (port <a>, oracle <b>)`.
fn first_diff(port: &[f32], oracle: &[f32]) -> String {
    port.iter()
        .zip(oracle)
        .position(|(a, b)| a.to_bits() != b.to_bits())
        .map(|i| format!(", first at {i} (port {:e}, oracle {:e})", port[i], oracle[i]))
        .unwrap_or_default()
}

/// Renders the whole sweep of `model` at `polyphony` on both sides and asserts the bound.
fn null_model(model: OracleModel, polyphony: usize) {
    let session = Session::lock();
    let bound = bound_dbfs();
    let filter = std::env::var("RESONATOR_NULL_POINTS").ok();
    let (mut worst_out, mut worst_aux) = (None, None);
    let mut failing = 0usize;
    let points = points();
    for point in &points {
        let oracle = render_point(&session, model, polyphony, point).expect("oracle render");
        let oracle_rng_state = session.rng_state();
        let port = render_port_point(model, polyphony, point);
        if port.rng_state != oracle_rng_state {
            failing += 1;
            println!("{} p{polyphony} {}: random state {:#010x}, oracle {oracle_rng_state:#010x}", model.label(), point.label(), port.rng_state);
        }
        let out = residual_f32(&port.out, &oracle.out).expect("lengths");
        let aux = residual_f32(&port.aux, &oracle.aux).expect("lengths");
        let exact_required = bound == f64::NEG_INFINITY;
        if !out.within(bound) || !aux.within(bound) || (exact_required && !(out.is_exact() && aux.is_exact())) {
            failing += 1;
        }
        if filter.as_ref().is_some_and(|f| point.label().contains(f.as_str())) {
            println!(
                "{} p{polyphony} {}: out {out}{}; aux {aux}{}",
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
    println!("{} p{polyphony}: worst out {wo} at {lo}; worst aux {wa} at {la}; {failing} of {} points over the bound", model.label(), points.len());
    assert_eq!(failing, 0, "{} p{polyphony}: {failing} points over {bound} dBFS; worst out {wo} at {lo}; worst aux {wa} at {la}", model.label());
}

macro_rules! null_tests {
    ($($name:ident => $model:ident, $polyphony:literal;)*) => {
        $(
            #[test]
            fn $name() {
                null_model(OracleModel::$model, $polyphony);
            }
        )*
    };
}

null_tests! {
    modal_1 => Modal, 1;
    modal_2 => Modal, 2;
    modal_3 => Modal, 3;
    modal_4 => Modal, 4;
    sympathetic_strings_1 => SympatheticStrings, 1;
    sympathetic_strings_2 => SympatheticStrings, 2;
    sympathetic_strings_3 => SympatheticStrings, 3;
    sympathetic_strings_4 => SympatheticStrings, 4;
    inharmonic_string_1 => InharmonicString, 1;
    inharmonic_string_2 => InharmonicString, 2;
    inharmonic_string_3 => InharmonicString, 3;
    inharmonic_string_4 => InharmonicString, 4;
    fm_voice_1 => FmVoice, 1;
    fm_voice_2 => FmVoice, 2;
    fm_voice_3 => FmVoice, 3;
    fm_voice_4 => FmVoice, 4;
    quantized_sympathetic_strings_1 => QuantizedSympatheticStrings, 1;
    quantized_sympathetic_strings_2 => QuantizedSympatheticStrings, 2;
    quantized_sympathetic_strings_3 => QuantizedSympatheticStrings, 3;
    quantized_sympathetic_strings_4 => QuantizedSympatheticStrings, 4;
    string_and_reverb_1 => StringAndReverb, 1;
    string_and_reverb_2 => StringAndReverb, 2;
    string_and_reverb_3 => StringAndReverb, 3;
    string_and_reverb_4 => StringAndReverb, 4;
}
