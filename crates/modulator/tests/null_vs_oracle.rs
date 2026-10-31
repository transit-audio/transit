//! The port against the oracle over the fixed sweep (ADR 0033 gate, #200): every point, the main
//! and aux outputs, bit for bit (the floor is exact: `transit-macro-osc-oracle`'s
//! `tests/modulator_self_floor.rs`). A failure names the point; it is a defect to hunt, never a
//! run to repeat.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::modulator::{self as oracle, MODULATOR_BLOCK_SIZE};
use transit_macro_osc_oracle::modulator_sweep::{MOTION_FRAMES, Motion, Point, SEED, center, input_signal, points, render_point};
use transit_macro_osc_oracle::residual::residual_i16;
use transit_macro_osc_oracle::{Session, subset};
use transit_modulator::{BLOCK_SIZE, CarrierShape, Modulator, Parameters};

fn carrier(c: oracle::CarrierShape) -> CarrierShape {
    match c {
        oracle::CarrierShape::External => CarrierShape::External,
        oracle::CarrierShape::Sine => CarrierShape::Sine,
        oracle::CarrierShape::Triangle => CarrierShape::Triangle,
        oracle::CarrierShape::Saw => CarrierShape::Saw,
    }
}

fn port_parameters(p: &oracle::Parameters) -> Parameters {
    Parameters { drive: p.drive, algorithm: p.algorithm, timbre: p.timbre, note: p.note, carrier: carrier(p.carrier) }
}

fn flat(frames: &[[i16; 2]]) -> Vec<i16> {
    frames.iter().flatten().copied().collect()
}

/// The port over one point, in blocks of `block` frames.
fn render_port(point: &Point, input: &[[i16; 2]], block: usize) -> Vec<[i16; 2]> {
    let input = &input[..point.frames()];
    let mut m = Modulator::with_seed(SEED);
    let blocks = input.len() / MODULATOR_BLOCK_SIZE;
    let mut out = vec![[0_i16; 2]; input.len()];
    for (k, (i, o)) in input.chunks(block).zip(out.chunks_mut(block)).enumerate() {
        // The parameters change per 60-frame block of the sweep.
        let p = point.parameters(k * block / MODULATOR_BLOCK_SIZE, blocks);
        m.process(&port_parameters(&p), i, o);
    }
    out
}

#[test]
fn the_constants_match_the_oracle() {
    let session = Session::lock();
    let (block, _, oversampling) = session.modulator_constants();
    assert_eq!(block, BLOCK_SIZE);
    assert_eq!(oversampling, transit_modulator::OVERSAMPLING);
    assert_eq!(oracle::MODULATOR_SAMPLE_RATE, transit_modulator::SAMPLE_RATE);
}

#[test]
fn every_point_nulls() {
    let input = input_signal(MOTION_FRAMES);
    let sweep = subset::strided("modulator null", points());
    let mut failures = Vec::new();
    for point in &sweep {
        let expected = {
            let session = Session::lock();
            render_point(&session, point, &input).unwrap()
        };
        let got = render_port(point, &input, BLOCK_SIZE);
        let r = residual_i16(&flat(&expected), &flat(&got)).unwrap();
        if !r.is_exact() {
            failures.push(format!("{}: {r}", point.label()));
        }
    }
    assert!(failures.is_empty(), "{} of {} points differ:\n{}", failures.len(), sweep.len(), failures.join("\n"));
}

/// The oracle takes any whole number of 12-frame groups up to 60; so does the port, to the same
/// bits (the parameters held, so the block size is all that changes).
#[test]
fn shorter_blocks_null_too() {
    let input = input_signal(MOTION_FRAMES);
    for p in [center(oracle::CarrierShape::External, 0.3125), center(oracle::CarrierShape::Triangle, 0.875), center(oracle::CarrierShape::Saw, 0.69)]
    {
        for block in [12, 24, 36, 48] {
            let frames = 12_000;
            let expected = {
                let session = Session::lock();
                session.seed(SEED);
                let mut m = session.modulator().unwrap();
                let mut out = vec![[0_i16; 2]; frames];
                for (i, o) in input[..frames].chunks(block).zip(out.chunks_mut(block)) {
                    m.process(&p, i, o).unwrap();
                }
                out
            };
            let mut m = Modulator::with_seed(SEED);
            let mut got = vec![[0_i16; 2]; frames];
            for (i, o) in input[..frames].chunks(block).zip(got.chunks_mut(block)) {
                m.process(&port_parameters(&p), i, o);
            }
            let r = residual_i16(&flat(&expected), &flat(&got)).unwrap();
            assert!(r.is_exact(), "{} in {block}-frame blocks: {r}", Point { base: p, motion: Motion::Static }.label());
        }
    }
}

/// A block the firmware cannot render whole is refused: silence out, the state untouched.
#[test]
fn a_refused_block_leaves_the_state_alone() {
    let input = input_signal(1200);
    let p = port_parameters(&center(oracle::CarrierShape::Sine, 0.5));
    let mut a = Modulator::new();
    let mut b = Modulator::new();
    let mut junk = vec![[7_i16; 2]; 13];
    b.process(&p, &input[..13], &mut junk);
    assert!(junk.iter().all(|f| *f == [0, 0]));
    let mut ya = vec![[0_i16; 2]; 1200];
    let mut yb = vec![[0_i16; 2]; 1200];
    for ((i, oa), ob) in input.chunks(60).zip(ya.chunks_mut(60)).zip(yb.chunks_mut(60)) {
        a.process(&p, i, oa);
        b.process(&p, i, ob);
    }
    assert_eq!(ya, yb);
}

/// `reset` is a fresh modulator: after a dirtied run, the same output as a new one.
#[test]
fn reset_equals_new() {
    let input = input_signal(24_000);
    let point = Point { base: center(oracle::CarrierShape::Saw, 0.0), motion: Motion::Jumps };
    let mut dirty = Modulator::with_seed(SEED);
    let mut scratch = vec![[0_i16; 2]; 60];
    for (k, i) in input.chunks(60).enumerate() {
        dirty.process(&port_parameters(&point.parameters(k, 400)), i, &mut scratch);
    }
    dirty.reset(SEED);
    let fresh = render_port(&Point { base: center(oracle::CarrierShape::Saw, 0.9), motion: Motion::Static }, &input, 60);
    let mut again = vec![[0_i16; 2]; 24_000];
    let p = port_parameters(&center(oracle::CarrierShape::Saw, 0.9));
    for (i, o) in input.chunks(60).zip(again.chunks_mut(60)) {
        dirty.process(&p, i, o);
    }
    assert_eq!(fresh, again);
}
