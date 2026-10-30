//! Scenarios against the C++ oracle where the sweep does not reach (ADR 0033, #199): quality
//! changes in one processor, every block size, bypass and silence, other memory sizes (the
//! shortest and the longest the firmware's arithmetic supports, with the longest transposed
//! grains), and full-range parameter ramps. Every one must match the oracle at 0 LSB.

#![forbid(unsafe_code)]

mod common;

use common::{Step, compare, triangle};
use transit_granular::BufferSize;
use transit_macro_osc_oracle::granular::{
    FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, GRANULAR_BLOCK_SIZE, MAX_DRY_WET, MAX_SIZE, Parameters, Quality,
};
use transit_macro_osc_oracle::granular_sweep::{center, input_signal};

fn check(label: &str, large: usize, small: usize, quality: Quality, input: &[[i16; 2]], steps: &[Step]) {
    if let Some((frame, port, oracle)) = compare(label, large, small, quality, 0x0199_5CE7, input, steps) {
        panic!("{label}: first difference at frame {frame}: port {port:?}, oracle {oracle:?}");
    }
}

fn blocks(n: usize, p: Parameters) -> impl Iterator<Item = Step> {
    std::iter::repeat_n(Step::Block(GRANULAR_BLOCK_SIZE, p), n)
}

/// The quality cycles through all four every 1.5 s in one processor: each change clears the
/// buffers (one silent block) and re-initializes the effects and the grains, as on the module.
#[test]
fn quality_changes_in_one_processor() {
    let input = input_signal(16 * 32_000);
    let p = Parameters { position: 0.1, ..center() };
    let mut steps = Vec::new();
    for round in 0..10 {
        steps.push(Step::Quality(Quality::ALL[(round * 3 + 1) % 4]));
        steps.extend(blocks(1500, p));
    }
    check("quality changes", FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, Quality::Stereo, &input, &steps);
}

/// Every block size from 1 to 32 frames (even sizes at low fidelity, where the 2:1 converters take
/// frames in pairs), with freeze and triggers on some blocks.
#[test]
fn every_block_size() {
    let input = input_signal(6 * 32_000);
    for quality in Quality::ALL {
        let sizes: Vec<usize> = if quality.low_fidelity() { (2..=32).step_by(2).collect() } else { (1..=32).collect() };
        let mut steps = Vec::new();
        let mut frames = 0;
        let mut i = 0;
        while frames + 32 <= input.len() {
            let n = sizes[i % sizes.len()];
            let p = Parameters { position: 0.05, freeze: (i / 200) % 3 == 2, trigger: i.is_multiple_of(41), ..center() };
            steps.push(Step::Block(n, p));
            frames += n;
            i += 1;
        }
        check(&format!("{} block sizes", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
    }
}

/// Bypass copies the input and records nothing; silence outputs zeros and records nothing; both
/// toggled mid-stream.
#[test]
fn bypass_and_silence() {
    let input = input_signal(8 * 32_000);
    let p = Parameters { position: 0.2, feedback: 0.5, reverb: 0.6, ..center() };
    let mut steps = Vec::new();
    steps.extend(blocks(2000, p));
    steps.push(Step::Bypass(true));
    steps.extend(blocks(1000, p));
    steps.push(Step::Bypass(false));
    steps.extend(blocks(1500, p));
    steps.push(Step::Silence(true));
    steps.extend(blocks(1000, p));
    steps.push(Step::Silence(false));
    steps.extend(blocks(2000, p));
    for quality in Quality::ALL {
        check(&format!("{} bypass and silence", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
    }
}

/// The memory sizes `BufferSize::from_seconds` gives at its two ends and in between, each nulled at
/// every quality, with the oldest position and the largest transposition (grains a quarter of the
/// buffer long, whose 16.16 phase nears 2^31 at the longest 8-bit mono buffer).
#[test]
fn other_memory_sizes() {
    let input = input_signal(10 * 32_000);
    for seconds in [BufferSize::MIN_SECONDS, 0.9, BufferSize::MAX_SECONDS] {
        let size = BufferSize::from_seconds(seconds);
        for quality in Quality::ALL {
            let frames = (size.samples(transit_granular::Quality::ALL[quality.index()]) * if quality.low_fidelity() { 2 } else { 1 }).min(9 * 32_000);
            let mut steps = Vec::new();
            steps.extend(blocks(frames / GRANULAR_BLOCK_SIZE, Parameters { position: 1.0, ..center() }));
            steps.extend(blocks(600, Parameters { position: 1.0, size: MAX_SIZE, pitch: 48.0, ..center() }));
            steps.extend(blocks(600, Parameters { position: 0.0, size: MAX_SIZE, pitch: 47.5, density: 1.0, ..center() }));
            steps.extend(blocks(600, Parameters { position: 1.0, size: MAX_SIZE, pitch: -48.0, density: 0.0, ..center() }));
            let label = format!("{} {seconds} s ({} + {} bytes)", quality.label(), size.large(), size.small());
            check(&label, size.large(), size.small(), quality, &input, &steps);
        }
    }
}

/// Every parameter ramped over its whole range, each at its own rate, at every quality.
#[test]
fn full_range_ramps() {
    let input = input_signal(12 * 32_000);
    let steps: Vec<Step> = (0..12_000_usize)
        .map(|b| {
            #[allow(clippy::cast_precision_loss)]
            let t = b as f32 / 1000.0;
            Step::Block(
                GRANULAR_BLOCK_SIZE,
                Parameters {
                    position: triangle(t, 1.3),
                    size: triangle(t, 2.9) * MAX_SIZE,
                    pitch: -48.0 + 96.0 * triangle(t, 3.7),
                    density: triangle(t, 0.7),
                    texture: triangle(t, 1.1),
                    dry_wet: triangle(t, 4.1) * MAX_DRY_WET,
                    stereo_spread: triangle(t, 0.9),
                    feedback: triangle(t, 5.3),
                    reverb: triangle(t, 2.3),
                    freeze: (b / 300) % 4 == 3,
                    trigger: b.is_multiple_of(29),
                    gate: false,
                },
            )
        })
        .collect();
    for quality in Quality::ALL {
        check(&format!("{} ramps", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
    }
}

/// `from_seconds` at the firmware's length gives the module's blocks, and every size it gives is
/// one the port and the oracle both accept.
#[test]
fn buffer_sizes_follow_the_firmware() {
    assert_eq!(BufferSize::from_seconds(32_696.0 / 32_000.0), BufferSize::FIRMWARE);
    assert_eq!(BufferSize::FIRMWARE.large(), FIRMWARE_LARGE_BUFFER);
    assert_eq!(BufferSize::FIRMWARE.small(), FIRMWARE_SMALL_BUFFER);
    for seconds in [0.0, BufferSize::MIN_SECONDS, 1.0, BufferSize::MAX_SECONDS, 10.0, f32::NAN] {
        let size = BufferSize::from_seconds(seconds);
        assert_eq!(BufferSize::new(size.large(), size.small()), Some(size), "{seconds}");
    }
    let session = transit_macro_osc_oracle::Session::lock();
    assert_eq!(BufferSize::WORKSPACE, session.granular_workspace_size());
    for seconds in [BufferSize::MIN_SECONDS, BufferSize::MAX_SECONDS] {
        let size = BufferSize::from_seconds(seconds);
        assert!(session.granular(size.large(), size.small(), Quality::MonoLowFidelity).is_ok(), "{seconds}");
    }
    let firmware = BufferSize::FIRMWARE;
    let lengths: Vec<usize> = transit_granular::Quality::ALL.iter().map(|q| firmware.samples(*q)).collect();
    assert_eq!(lengths, [32_696, 59_384, 65_400, 118_776]);
    assert!(BufferSize::new(BufferSize::MAX_LARGE + 4, 65_408).is_none());
    assert!(BufferSize::new(118_784, BufferSize::WORKSPACE - 4).is_none());
}
