//! Null tests of the pitch shifter and time stretcher (the stretch playback mode: the WSOLA player,
//! its correlator and windows, the low-pass and high-pass filters, the density-driven diffuser) at
//! every quality against the C++ oracle (ADR 0033, #199 slice 2).
//!
//! **The sweep.** Each quality and gate pattern is rendered over the oracle's stretch-mode sweep
//! (`granular_sweep::points_in_mode(Stretch)`: the nine continuous parameters each at five levels
//! around a center, 37 points per pattern; free-running, frozen from the middle to three quarters,
//! and triggered every 25 blocks; 32 kHz; 32-frame blocks; the module's memory sizes; a fresh
//! processor, the same seed and the same input per point, each point the quality's recording plus
//! 0.25 s) by the port and by the oracle on the same runner, all 37 points per test on every run
//! (the whole sweep, both sides, takes seconds, so the CI-time subset is not needed). The int16
//! output must be identical, 0 LSB (the oracle's floor, `-inf`, plus a margin of
//! zero), and the random state after each point equal (the stretch mode draws nothing, so this
//! proves the port does not either). Each point must sound after its first two blocks.
//!
//! **The long runs.** 60 s per quality with every parameter moving, the pitch stepping over the
//! module's range, freeze toggled, triggers, and 10 s of silence (the decays reach the subnormal
//! range there).
//!
//! **Scenarios.** Changes between the granular and the stretch modes in one processor (each
//! benign: one silent block, the filters reset, the correlator's memory cleared, the recording
//! kept); quality changes in the stretch mode; every block size; bypass and silence; the shortest
//! and longest memory sizes with the extreme window sizes and transpositions; full-range ramps.
//!
//! The correlator advances in `Prepare`, once per block, as the oracle's harness orders it (the
//! module runs it from its main loop as often as the CPU allows); both sides follow the harness.
//!
//! Set `GRANULAR_NULL_POINTS` to print the residual and the first differing frame of every sweep
//! point whose label contains its value (empty: every point).

#![forbid(unsafe_code)]

mod common;

use common::{Step, compare_in_mode, port_parameters, port_quality, triangle};
use transit_granular::{BufferSize, Granular, MAX_BLOCK_SIZE, PlaybackMode};
use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::floor::RunnerFamily;
use transit_macro_osc_oracle::granular::{
    FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, GRANULAR_BLOCK_SIZE, MAX_DRY_WET, MAX_SIZE, Parameters, PlaybackMode as OracleMode, Quality,
};
use transit_macro_osc_oracle::granular_sweep::{
    Gate, Point, SEED, center, input_signal, longest_point_frames, point_frames, points_in_mode, render_point,
};
use transit_macro_osc_oracle::residual::residual_i16;

/// The margin over the oracle's floor, dB. Zero: the port is held to bit identity.
const MARGIN_DB: f64 = 0.0;

fn bound_dbfs() -> f64 {
    RunnerFamily::current().self_floor_dbfs().unwrap_or(f64::NEG_INFINITY) + MARGIN_DB
}

fn render_port_point(quality: Quality, point: &Point, input: &[[i16; 2]]) -> (Vec<[i16; 2]>, u32) {
    let mut processor = Granular::new_in_mode(BufferSize::FIRMWARE, port_quality(quality), PlaybackMode::Stretch);
    processor.seed(SEED);
    let input = &input[..point_frames(quality)];
    let blocks = input.len() / MAX_BLOCK_SIZE;
    let mut out = vec![[0_i16; 2]; input.len()];
    for (block, (i, o)) in input.chunks(MAX_BLOCK_SIZE).zip(out.chunks_mut(MAX_BLOCK_SIZE)).enumerate() {
        processor.process(&port_parameters(&point.parameters(block, blocks)), i, o);
    }
    (out, processor.rng_state())
}

fn flat(frames: &[[i16; 2]]) -> Vec<i16> {
    frames.iter().flatten().copied().collect()
}

fn null_quality(quality: Quality, gate: Gate) {
    let filter = std::env::var("GRANULAR_NULL_POINTS").ok();
    let input = input_signal(longest_point_frames());
    let bound = bound_dbfs();
    let points: Vec<Point> = points_in_mode(OracleMode::Stretch).into_iter().filter(|p| p.gate == gate).collect();
    assert_eq!(points.len(), 37);
    let mut failures = Vec::new();
    for point in &points {
        let (oracle, oracle_rng) = {
            let session = Session::lock();
            let out = render_point(&session, quality, point, &input).unwrap();
            (out, session.rng_state())
        };
        let (port, port_rng) = render_port_point(quality, point, &input);
        let r = residual_i16(&flat(&port), &flat(&oracle)).unwrap();
        let label = format!("{} {}", quality.label(), point.label());
        let first = port.iter().zip(&oracle).position(|(a, b)| a != b);
        if filter.as_deref().is_some_and(|f| label.contains(f)) {
            println!("{label}: {r}, first differing frame {first:?}, rng port {port_rng:08x} oracle {oracle_rng:08x}");
        }
        if !r.within(bound) || first.is_some() {
            let at = first.map(|i| format!(", first at frame {i} (port {:?}, oracle {:?})", port[i], oracle[i])).unwrap_or_default();
            failures.push(format!("{label}: {r}{at}"));
        } else if port_rng != oracle_rng {
            failures.push(format!("{label}: identical output but the random state differs (port {port_rng:08x}, oracle {oracle_rng:08x})"));
        }
        // Every point sounds after its first two blocks (the first still carries the dry input
        // while dry/wet ramps from 0): the windows play from the first block on.
        if !oracle[2 * MAX_BLOCK_SIZE..].iter().any(|f| *f != [0, 0]) {
            failures.push(format!("{label}: silent"));
        }
    }
    assert!(failures.is_empty(), "{} of {} points differ:\n{}", failures.len(), points.len(), failures.join("\n"));
}

macro_rules! null_tests {
    ($($name:ident: $quality:ident, $gate:ident;)*) => {
        $(
            #[test]
            fn $name() {
                null_quality(Quality::$quality, Gate::$gate);
            }
        )*
    };
}

null_tests! {
    stretch_stereo_free: Stereo, Free;
    stretch_stereo_freeze: Stereo, Freeze;
    stretch_stereo_trigger: Stereo, Trigger;
    stretch_mono_free: Mono, Free;
    stretch_mono_freeze: Mono, Freeze;
    stretch_mono_trigger: Mono, Trigger;
    stretch_stereo_lofi_free: StereoLowFidelity, Free;
    stretch_stereo_lofi_freeze: StereoLowFidelity, Freeze;
    stretch_stereo_lofi_trigger: StereoLowFidelity, Trigger;
    stretch_mono_lofi_free: MonoLowFidelity, Free;
    stretch_mono_lofi_freeze: MonoLowFidelity, Freeze;
    stretch_mono_lofi_trigger: MonoLowFidelity, Trigger;
}

// --- Long runs -----------------------------------------------------------------------------------

/// 60 s at 32 kHz.
const FRAMES: usize = 60 * 32_000;

/// Silence from 30 s to 40 s.
const SILENCE: std::ops::Range<usize> = 30 * 32_000..40 * 32_000;

/// The pitch steps every 3 s over the module's whole range, including the octave-limited slews
/// between its ends.
const PITCHES: [f32; 7] = [0.0, 12.0, -12.0, 48.0, -48.0, 7.0, -24.0];

/// The parameters of block `block`: triangles of co-prime periods (seconds) over each range, the
/// pitch stepping every 3 s, freeze toggled at irregular multiples of 1.25 s, a trigger every 37
/// blocks for the first half and every 211 after.
fn long_parameters(block: usize) -> Parameters {
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
        trigger: if block < FRAMES / GRANULAR_BLOCK_SIZE / 2 { block.is_multiple_of(37) } else { block.is_multiple_of(211) },
        gate: false,
    }
}

fn check(label: &str, large: usize, small: usize, quality: Quality, input: &[[i16; 2]], steps: &[Step]) {
    if let Some((frame, port, oracle)) = compare_in_mode(label, large, small, quality, OracleMode::Stretch, 0x0199_0052, input, steps) {
        #[allow(clippy::cast_precision_loss)]
        let seconds = frame as f64 / 32_000.0;
        panic!("{label}: first difference at frame {frame} ({seconds:.3} s): port {port:?}, oracle {oracle:?}");
    }
}

fn long_run(quality: Quality) {
    let mut input = input_signal(FRAMES);
    input[SILENCE].fill([0, 0]);
    let steps: Vec<Step> = (0..FRAMES / GRANULAR_BLOCK_SIZE).map(|b| Step::Block(GRANULAR_BLOCK_SIZE, long_parameters(b))).collect();
    check(&format!("stretch {} 60 s", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
}

#[test]
fn stretch_stereo_60_s() {
    long_run(Quality::Stereo);
}

#[test]
fn stretch_mono_60_s() {
    long_run(Quality::Mono);
}

#[test]
fn stretch_stereo_low_fidelity_60_s() {
    long_run(Quality::StereoLowFidelity);
}

#[test]
fn stretch_mono_low_fidelity_60_s() {
    long_run(Quality::MonoLowFidelity);
}

/// The long-run schedule toggles freeze several times, reaches both pitch ends and the ranges.
#[test]
fn the_long_schedule_covers_the_ranges() {
    let blocks = FRAMES / GRANULAR_BLOCK_SIZE;
    let toggles = (1..blocks).filter(|&b| long_parameters(b).freeze != long_parameters(b - 1).freeze).count();
    assert!(toggles >= 10, "{toggles} freeze toggles");
    let pitches: std::collections::BTreeSet<i32> = (0..blocks).map(|b| long_parameters(b).pitch as i32).collect();
    assert!(pitches.contains(&48) && pitches.contains(&-48), "{pitches:?}");
    assert!((0..blocks).all(|b| transit_macro_osc_oracle::granular::parameters_in_range(&long_parameters(b))));
}

// --- Scenarios -----------------------------------------------------------------------------------

fn blocks(n: usize, p: Parameters) -> impl Iterator<Item = Step> {
    std::iter::repeat_n(Step::Block(GRANULAR_BLOCK_SIZE, p), n)
}

/// The mode changes between granular and stretch every 0.6 to 1.9 s in one processor, at every
/// quality: each change is benign (one silent block, then the filters reset and the correlator's
/// memory cleared; the recording is kept).
#[test]
fn mode_changes_in_one_processor() {
    let input = input_signal(14 * 32_000);
    for quality in Quality::ALL {
        let mut steps = Vec::new();
        for round in 0..12_usize {
            let mode = if round.is_multiple_of(2) { OracleMode::Granular } else { OracleMode::Stretch };
            steps.push(Step::Mode(mode));
            let p = Parameters { position: 0.1 * (round % 5) as f32, pitch: [0.0, 7.0, -12.0][round % 3], density: 0.6, ..center() };
            steps.extend(blocks(600 + 113 * (round % 7), p));
        }
        check(&format!("stretch {} mode changes", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
    }
}

/// The quality cycles through all four every 1.5 s in the stretch mode: each change clears the
/// buffers (one silent block) and re-initializes the players, the correlator and the effects.
#[test]
fn quality_changes_in_the_stretch_mode() {
    let input = input_signal(16 * 32_000);
    let p = Parameters { position: 0.3, pitch: 5.0, ..center() };
    let mut steps = Vec::new();
    for round in 0..10 {
        steps.push(Step::Quality(Quality::ALL[(round * 3 + 1) % 4]));
        steps.extend(blocks(1500, p));
    }
    check("stretch quality changes", FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, Quality::Stereo, &input, &steps);
}

/// Every block size from 1 to 32 frames (even sizes at low fidelity), with freeze and triggers on
/// some blocks: the correlator advances once per block, whatever its length.
#[test]
fn every_block_size_in_the_stretch_mode() {
    let input = input_signal(6 * 32_000);
    for quality in Quality::ALL {
        let sizes: Vec<usize> = if quality.low_fidelity() { (2..=32).step_by(2).collect() } else { (1..=32).collect() };
        let mut steps = Vec::new();
        let mut frames = 0;
        let mut i = 0;
        while frames + 32 <= input.len() {
            let n = sizes[i % sizes.len()];
            let p = Parameters { position: 0.05, pitch: -7.0, freeze: (i / 200) % 3 == 2, trigger: i.is_multiple_of(41), ..center() };
            steps.push(Step::Block(n, p));
            frames += n;
            i += 1;
        }
        check(&format!("stretch {} block sizes", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
    }
}

/// Bypass and silence toggled mid-stream in the stretch mode.
#[test]
fn bypass_and_silence_in_the_stretch_mode() {
    let input = input_signal(8 * 32_000);
    let p = Parameters { position: 0.2, pitch: 12.0, feedback: 0.5, reverb: 0.6, ..center() };
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
        check(&format!("stretch {} bypass and silence", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
    }
}

/// The shortest and longest memory sizes, each at every quality, with the window at its longest and
/// shortest, the transposition at both ends of the module's range, and the oldest position (where
/// the windows read furthest back).
#[test]
fn other_memory_sizes_and_extremes_in_the_stretch_mode() {
    let input = input_signal(10 * 32_000);
    for seconds in [BufferSize::MIN_SECONDS, BufferSize::MAX_SECONDS] {
        let size = BufferSize::from_seconds(seconds);
        for quality in Quality::ALL {
            let mut steps = Vec::new();
            steps.extend(blocks(1500, Parameters { position: 1.0, size: MAX_SIZE, pitch: 48.0, ..center() }));
            steps.extend(blocks(1500, Parameters { position: 1.0, size: 0.0, pitch: -48.0, ..center() }));
            steps.extend(blocks(1500, Parameters { position: 0.0, size: MAX_SIZE, pitch: -48.0, texture: 0.0, ..center() }));
            steps.extend(blocks(1500, Parameters { position: 0.0, size: 0.0, pitch: 48.0, texture: 1.0, density: 1.0, ..center() }));
            let label = format!("stretch {} {seconds} s ({} + {} bytes)", quality.label(), size.large(), size.small());
            check(&label, size.large(), size.small(), quality, &input, &steps);
        }
    }
}

/// Every parameter ramped over its whole range, each at its own rate, at every quality.
#[test]
fn full_range_ramps_in_the_stretch_mode() {
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
        check(&format!("stretch {} ramps", quality.label()), FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality, &input, &steps);
    }
}

/// `reset` returns a processor to what the constructor builds, without allocating: after a dirty
/// run in either mode, a reset processor renders what a new one renders, bit for bit.
#[test]
fn a_reset_processor_renders_what_a_new_one_renders() {
    let input = input_signal(4 * 32_000);
    for quality in transit_granular::Quality::ALL {
        for mode in [PlaybackMode::Granular, PlaybackMode::Stretch] {
            let p = port_parameters(&Parameters { position: 0.2, pitch: 7.0, feedback: 0.6, reverb: 0.5, ..center() });
            let run = |g: &mut Granular| {
                let mut out = vec![[0_i16; 2]; input.len()];
                for (k, (i, o)) in input.chunks(MAX_BLOCK_SIZE).zip(out.chunks_mut(MAX_BLOCK_SIZE)).enumerate() {
                    g.process(&transit_granular::Parameters { trigger: k.is_multiple_of(50), ..p }, i, o);
                }
                out
            };
            let mut dirty = Granular::new_in_mode(BufferSize::FIRMWARE, quality, mode);
            dirty.seed(99);
            let _ = run(&mut dirty);
            dirty.reset();
            let mut fresh = Granular::new_in_mode(BufferSize::FIRMWARE, quality, mode);
            let a = run(&mut dirty);
            let b = run(&mut fresh);
            assert!(a.iter().any(|f| f[0].unsigned_abs() > 64), "{quality:?} {mode:?}: silent");
            assert_eq!(a, b, "{quality:?} {mode:?}");
            assert_eq!(dirty.rng_state(), fresh.rng_state(), "{quality:?} {mode:?}");
        }
    }
}
