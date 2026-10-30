//! Scenarios the sweep does not reach, against the C++ oracle with the same rule: every output
//! sample bit-identical, and the random state equal at the end (#198).
//!
//! Model and polyphony changes within one part, the note filter under small and large V/OCT moves
//! and FM, full-range macro ramps on every model, every block size from 1 to 24 frames, bypass,
//! a continuous noise input into every model, notes below 11.7 Hz (the strings' resampler) and at
//! the top of the range, and long silences after a strum (subnormal decays, where flush-to-zero
//! decides the bits).

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::residual::residual_f32;
use transit_macro_osc_oracle::resonator::{PerformanceState as OraclePerformance, ResonatorModel as OracleModel, ResonatorPatch};
use transit_resonator::stmlib::random::Random;
use transit_resonator::{Patch, PerformanceState, Resonator, ResonatorModel};

const SEED: u32 = 0x0198_0C0D;

/// One call: settings applied first (as the panel would between blocks), then one block.
#[derive(Clone, Debug)]
struct Step {
    model: Option<usize>,
    polyphony: Option<usize>,
    bypass: Option<bool>,
    performance: OraclePerformance,
    patch: ResonatorPatch,
    input: Vec<f32>,
}

impl Step {
    fn block(performance: OraclePerformance, patch: ResonatorPatch, input: Vec<f32>) -> Self {
        Self { model: None, polyphony: None, bypass: None, performance, patch, input }
    }
}

fn center() -> ResonatorPatch {
    ResonatorPatch { structure: 0.5, brightness: 0.5, damping: 0.5, position: 0.5 }
}

fn internal(note: f32, strum: bool) -> OraclePerformance {
    OraclePerformance { strum, internal_exciter: true, internal_strum: true, tonic: 36.0, note: note - 36.0, ..OraclePerformance::default() }
}

/// Renders `steps` on both sides from fresh parts and asserts identical bits.
fn compare(label: &str, steps: &[Step]) {
    let session = Session::lock();
    session.seed(SEED);
    let mut oracle = session.resonator().expect("oracle part");
    let mut port = Resonator::new();
    port.seed(SEED);
    let (mut o_out, mut o_aux, mut p_out, mut p_aux) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for step in steps {
        if let Some(m) = step.model {
            oracle.set_model(OracleModel::ALL[m]);
            port.set_model(ResonatorModel::ALL[m]);
        }
        if let Some(p) = step.polyphony {
            oracle.set_polyphony(p).expect("polyphony");
            port.set_polyphony(p);
        }
        if let Some(b) = step.bypass {
            oracle.set_bypass(b);
            port.set_bypass(b);
        }
        let n = step.input.len();
        let (mut oo, mut oa, mut po, mut pa) = (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
        oracle.process(&step.performance, &step.patch, &step.input, &mut oo, &mut oa).expect("oracle block");
        let p = &step.performance;
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
        let m = &step.patch;
        let patch = Patch { structure: m.structure, brightness: m.brightness, damping: m.damping, position: m.position };
        port.process(&performance, &patch, &step.input, &mut po, &mut pa);
        o_out.extend(oo);
        o_aux.extend(oa);
        p_out.extend(po);
        p_aux.extend(pa);
    }
    let out = residual_f32(&p_out, &o_out).expect("lengths");
    let aux = residual_f32(&p_aux, &o_aux).expect("lengths");
    let first = |p: &[f32], o: &[f32]| p.iter().zip(o).position(|(a, b)| a.to_bits() != b.to_bits());
    println!("{label}: {} frames; out {out}; aux {aux}", o_out.len());
    assert!(
        out.is_exact() && aux.is_exact(),
        "{label}: out {out} (first {:?}); aux {aux} (first {:?})",
        first(&p_out, &o_out),
        first(&p_aux, &o_aux)
    );
    assert_eq!(port.rng_state(), session.rng_state(), "{label}: random state");
}

/// `blocks` blocks of 24 frames with the same inputs, the first one strummed if `strum`.
fn hold(blocks: usize, note: f32, patch: ResonatorPatch, strum: bool) -> Vec<Step> {
    (0..blocks).map(|b| Step::block(internal(note, strum && b == 0), patch, vec![0.0; 24])).collect()
}

#[test]
fn model_changes_within_one_part() {
    for polyphony in [1, 2, 3, 4] {
        let mut steps = Vec::new();
        let order = [0, 1, 2, 3, 4, 5, 4, 3, 2, 1, 0, 3, 5, 0, 2, 4, 1];
        for (k, m) in order.into_iter().enumerate() {
            let mut first = hold(40, 48.0 + (k % 5) as f32 * 7.0, center(), true);
            first[0].model = Some(m);
            if k == 0 {
                first[0].polyphony = Some(polyphony);
            }
            steps.extend(first);
        }
        compare(&format!("model changes p{polyphony}"), &steps);
    }
}

#[test]
fn polyphony_changes_under_strums() {
    for model in 0..6 {
        let mut steps = Vec::new();
        for (k, p) in [1, 4, 3, 2, 1, 3, 4, 2].into_iter().enumerate() {
            for b in 0..60 {
                let mut s = Step::block(internal(40.0 + (b % 7) as f32 * 3.0, b % 10 == 0), center(), vec![0.0; 24]);
                if b == 0 {
                    s.polyphony = Some(p);
                    if k == 0 {
                        s.model = Some(model);
                    }
                }
                steps.push(s);
            }
        }
        compare(&format!("polyphony changes m{model}"), &steps);
    }
}

#[test]
fn the_note_filter_under_small_and_large_moves_and_fm() {
    for model in 0..6 {
        for polyphony in [1, 4] {
            let mut steps = Vec::new();
            for b in 0..600 {
                // A slow glide (steps under the 0.4 semitone threshold), jumps every 100 blocks,
                // strums every 37, and a triangle on the FM input.
                let glide = (b % 100) as f32 * 0.03;
                let jump = (b / 100) as f32 * 5.0;
                let tri = ((b % 64) as f32 - 32.0).abs() / 32.0;
                let mut p = internal(45.0 + jump + glide, b % 37 == 0);
                p.fm = tri * 6.0 - 3.0;
                p.chord = (b / 60) % 11;
                let mut s = Step::block(p, ResonatorPatch { structure: 0.3, ..center() }, vec![0.0; 24]);
                if b == 0 {
                    s.model = Some(model);
                    s.polyphony = Some(polyphony);
                }
                steps.push(s);
            }
            compare(&format!("note filter m{model} p{polyphony}"), &steps);
        }
    }
}

#[test]
fn full_range_macro_ramps_on_every_model() {
    for model in 0..6 {
        for polyphony in [1, 2] {
            let mut steps = Vec::new();
            let blocks = 800;
            for b in 0..blocks {
                let t = b as f32 / (blocks - 1) as f32;
                // Each macro ramps at its own rate, so the ramps cross at different points.
                let patch =
                    ResonatorPatch { structure: t, brightness: (t * 3.0).fract(), damping: 1.0 - (t * 2.0).fract(), position: (t * 5.0).fract() };
                let mut s = Step::block(internal(50.0, b % 25 == 0), patch, vec![0.0; 24]);
                s.performance.chord = ((t * 10.0) + 0.5) as i32;
                if b == 0 {
                    s.model = Some(model);
                    s.polyphony = Some(polyphony);
                }
                steps.push(s);
            }
            compare(&format!("macro ramps m{model} p{polyphony}"), &steps);
        }
    }
}

#[test]
fn every_block_size_from_1_to_24() {
    for model in 0..6 {
        let mut steps = Vec::new();
        for (k, size) in (1..=24).chain((1..=24).rev()).enumerate() {
            let mut s = Step::block(internal(57.0, k % 9 == 0), center(), vec![0.0; size]);
            if k == 0 {
                s.model = Some(model);
                s.polyphony = Some(3);
            }
            steps.push(s);
        }
        compare(&format!("block sizes m{model}"), &steps);
    }
}

#[test]
fn bypass_and_a_continuous_noise_input() {
    for model in 0..6 {
        for polyphony in [1, 4] {
            let mut noise = Random::new(5);
            let mut steps = Vec::new();
            for b in 0..500 {
                let input: Vec<f32> = (0..24).map(|_| (noise.get_float().0 * 2.0 - 1.0) * 0.3).collect();
                let performance = OraclePerformance { strum: b % 50 == 0, tonic: 36.0, note: 12.0 + (b / 50) as f32, ..OraclePerformance::default() };
                let mut s = Step::block(performance, ResonatorPatch { brightness: 0.8, ..center() }, input);
                if b == 0 {
                    s.model = Some(model);
                    s.polyphony = Some(polyphony);
                }
                if b == 200 {
                    s.bypass = Some(true);
                }
                if b == 230 {
                    s.bypass = Some(false);
                }
                steps.push(s);
            }
            compare(&format!("noise input m{model} p{polyphony}"), &steps);
        }
    }
}

#[test]
fn notes_below_the_delay_line_and_at_the_top() {
    for model in 0..6 {
        for note in [0.0_f32, 6.0, 12.0, 100.0, 118.0] {
            let mut steps = hold(400, note, ResonatorPatch { damping: 0.8, ..center() }, true);
            steps[0].model = Some(model);
            steps[200].performance.strum = true;
            compare(&format!("note {note} m{model}"), &steps);
        }
    }
}

/// Long silences after a strum: the resonators, strings, follower, limiter and reverb decay into the
/// subnormal range, where flush-to-zero decides the bits (the 100 ms sweep never gets there).
#[test]
fn long_silences_decay_through_the_subnormal_range() {
    for model in 0..6 {
        for polyphony in [1, 4] {
            for (damping, seconds) in [(0.0_f32, 20_usize), (0.6, 20)] {
                let blocks = seconds * 48_000 / 24;
                let patch = ResonatorPatch { damping, brightness: 0.3, ..center() };
                let mut steps = hold(blocks, 60.0, patch, true);
                steps[0].model = Some(model);
                steps[0].polyphony = Some(polyphony);
                compare(&format!("silence m{model} p{polyphony} d{damping} {seconds} s"), &steps);
            }
        }
    }
}
