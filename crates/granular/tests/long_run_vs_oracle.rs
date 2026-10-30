//! A long run of the granular processor at every quality against the C++ oracle (ADR 0033, #199):
//! 60 s of input in the firmware's 32-frame blocks, with every parameter moving, freeze toggled
//! every few seconds, triggers, and 10 s of silence in the middle (the slow decays reach the
//! subnormal range there, where the port's flush-to-zero must match the oracle's). The int16 output
//! must be identical, 0 LSB, from the first frame to the last, and the random state at the end must
//! equal the oracle's.

#![forbid(unsafe_code)]

mod common;

use common::{Step, compare, triangle};
use transit_macro_osc_oracle::granular::{
    FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, GRANULAR_BLOCK_SIZE, MAX_DRY_WET, MAX_SIZE, Parameters, Quality,
};
use transit_macro_osc_oracle::granular_sweep::input_signal;

/// 60 s at 32 kHz.
const FRAMES: usize = 60 * 32_000;

/// Silence from 30 s to 40 s.
const SILENCE: std::ops::Range<usize> = 30 * 32_000..40 * 32_000;

const PITCHES: [f32; 5] = [-12.0, -5.0, 0.0, 7.0, 12.0];

/// The parameters of block `block`: triangles of co-prime periods (seconds) over each range, the
/// pitch stepping every 3 s, freeze toggled at irregular multiples of 1.25 s, a trigger every 37
/// blocks.
fn parameters(block: usize) -> Parameters {
    #[allow(clippy::cast_precision_loss)]
    let t = (block * GRANULAR_BLOCK_SIZE) as f32 / 32_000.0;
    let segment = block / 1250;
    Parameters {
        position: triangle(t, 7.0),
        size: triangle(t, 11.0) * MAX_SIZE,
        pitch: PITCHES[(block / 3000) % PITCHES.len()],
        density: triangle(t, 13.0),
        texture: triangle(t, 5.0),
        dry_wet: (0.5 + 0.5 * triangle(t, 23.0)) * MAX_DRY_WET,
        stereo_spread: triangle(t, 9.0),
        feedback: 0.9 * triangle(t, 17.0),
        reverb: triangle(t, 19.0),
        freeze: (segment * 7 + segment / 3) % 3 == 1,
        trigger: block.is_multiple_of(37),
        gate: false,
    }
}

fn long_run(quality: Quality) {
    let mut input = input_signal(FRAMES);
    input[SILENCE].fill([0, 0]);
    let steps: Vec<Step> = (0..FRAMES / GRANULAR_BLOCK_SIZE).map(|b| Step::Block(GRANULAR_BLOCK_SIZE, parameters(b))).collect();
    let label = format!("{} 60 s", quality.label());
    if let Some((frame, port, oracle)) = compare(&label, FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, 0x0199_0060, &input, &steps) {
        #[allow(clippy::cast_precision_loss)]
        let seconds = frame as f64 / 32_000.0;
        panic!("{label}: first difference at frame {frame} ({seconds:.3} s): port {port:?}, oracle {oracle:?}");
    }
}

#[test]
fn stereo_60_s() {
    long_run(Quality::Stereo);
}

#[test]
fn mono_60_s() {
    long_run(Quality::Mono);
}

#[test]
fn stereo_low_fidelity_60_s() {
    long_run(Quality::StereoLowFidelity);
}

#[test]
fn mono_low_fidelity_60_s() {
    long_run(Quality::MonoLowFidelity);
}

/// The schedule above freezes and unfreezes several times, and the silence reaches the decays.
#[test]
fn the_schedule_toggles_freeze_and_covers_the_ranges() {
    let blocks = FRAMES / GRANULAR_BLOCK_SIZE;
    let toggles = (1..blocks).filter(|&b| parameters(b).freeze != parameters(b - 1).freeze).count();
    assert!(toggles >= 10, "{toggles} freeze toggles");
    let max = |f: fn(&Parameters) -> f32| (0..blocks).map(|b| f(&parameters(b))).fold(f32::MIN, f32::max);
    let min = |f: fn(&Parameters) -> f32| (0..blocks).map(|b| f(&parameters(b))).fold(f32::MAX, f32::min);
    assert!(min(|p| p.position) < 0.01 && max(|p| p.position) > 0.99);
    assert!(min(|p| p.density) < 0.01 && max(|p| p.density) > 0.99);
    assert!(max(|p| p.texture) > 0.99 && max(|p| p.feedback) > 0.89);
    assert!((0..blocks).all(|b| transit_macro_osc_oracle::granular::parameters_in_range(&parameters(b))));
}
