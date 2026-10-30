//! The granular oracle's residual floor against itself (#199): the same sweep with the same seed
//! renders bit-identical output twice in one process and again in a second process.
//!
//! A failure here means the reference itself is unstable on this runner (an uninitialized read,
//! state leaking between renders, a floating-point mode that differs between processes); the port
//! gate cannot be set until it is fixed. Never rerun it to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::granular::{
    FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, GRANULAR_BLOCK_SIZE, MAX_DRY_WET, MAX_SIZE, PITCH_RANGE, Parameters, Quality,
};
use transit_macro_osc_oracle::granular_sweep::{self, center, input_signal, longest_point_frames, points, render_point};
use transit_macro_osc_oracle::residual::residual_i16;
use transit_macro_osc_oracle::{Error, Session};

const CHILD_ENV: &str = "TRANSIT_GRANULAR_ORACLE_SELF_FLOOR_CHILD";
const LINE_PREFIX: &str = "granular-digest ";

fn flat(frames: &[[i16; 2]]) -> Vec<i16> {
    frames.iter().flatten().copied().collect()
}

#[test]
fn the_firmware_constants_match() {
    let session = Session::lock();
    assert_eq!(session.granular_max_block_size(), GRANULAR_BLOCK_SIZE);
    assert_eq!(session.granular_firmware_sizes(), (FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER));
    let workspace = session.granular_workspace_size();
    // The diffuser, the reverb, then the pitch shifter's 4096 words over the correlator's blocks.
    assert_eq!(workspace, 2048 * 4 + 16384 * 2 + 4096 * 2);
    // The module's blocks leave it room in both layouts, so every quality builds on them.
    for quality in Quality::ALL {
        assert!(session.granular(FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, quality).is_ok(), "{}", quality.label());
    }
}

#[test]
fn same_seed_twice_is_bit_identical() {
    let session = Session::lock();
    let input = input_signal(longest_point_frames());
    let mut compared = 0usize;
    for quality in Quality::ALL {
        for point in points() {
            let a = render_point(&session, quality, &point, &input).unwrap();
            let b = render_point(&session, quality, &point, &input).unwrap();
            let r = residual_i16(&flat(&a), &flat(&b)).unwrap();
            assert!(r.is_exact(), "{} {}: {r}", quality.label(), point.label());
            compared += 1;
        }
    }
    assert_eq!(compared, Quality::ALL.len() * points().len());
}

#[test]
fn a_second_process_renders_the_same_bits() {
    let here = {
        let session = Session::lock();
        granular_sweep::digest_lines(&session).unwrap()
    };
    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .args(["child_prints_the_sweep_digests", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "child failed ({}):\n{stdout}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
    // libtest prints `test <name> ... ` without a newline before a captured-off test's output, so
    // the first digest shares its line with that header: match the prefix anywhere in a line.
    let there: Vec<&str> = stdout.lines().filter_map(|l| l.find(LINE_PREFIX).map(|i| &l[i + LINE_PREFIX.len()..])).collect();
    assert_eq!(there.len(), here.len(), "the child printed {} digest lines, expected {}", there.len(), here.len());
    let differing: Vec<_> = here.iter().zip(&there).filter(|(a, b)| a.as_str() != **b).map(|(a, b)| format!("here  {a}\nthere {b}")).collect();
    assert!(differing.is_empty(), "{} of {} points differ between processes:\n{}", differing.len(), here.len(), differing.join("\n"));
}

/// The second process of `a_second_process_renders_the_same_bits`. Does nothing unless that test
/// started it.
#[test]
#[ignore = "run as a child process by a_second_process_renders_the_same_bits"]
fn child_prints_the_sweep_digests() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let session = Session::lock();
    for line in granular_sweep::digest_lines(&session).unwrap() {
        println!("{LINE_PREFIX}{line}");
    }
}

/// The grains' random seeding and panning draw from the shared random state, so the seed must
/// reach the output.
#[test]
fn the_seed_reaches_the_grains() {
    let session = Session::lock();
    // Position 0: the grains play the last fraction of a second, so they sound within 0.75 s.
    let input = input_signal(24_000);
    let point = granular_sweep::Point {
        base: Parameters { position: 0.0, ..center() },
        gate: granular_sweep::Gate::Free,
        mode: transit_macro_osc_oracle::granular::PlaybackMode::Granular,
    };
    let render = |seed: u32| {
        session.seed(seed);
        let mut processor = session.granular(FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, Quality::Stereo).unwrap();
        let mut out = vec![[0_i16; 2]; input.len()];
        for (block, (i, o)) in input.chunks(GRANULAR_BLOCK_SIZE).zip(out.chunks_mut(GRANULAR_BLOCK_SIZE)).enumerate() {
            processor.process(&point.parameters(block, 750), i, o).unwrap();
        }
        flat(&out)
    };
    let a = render(7);
    let b = render(7);
    let c = render(8);
    assert!(residual_i16(&a, &b).unwrap().is_exact());
    assert!(!residual_i16(&a, &c).unwrap().is_exact(), "a different seed must change the grains");
}

/// Every quality sounds at the center in both modes (prints the peak level per quality, pattern and
/// mode).
#[test]
fn every_quality_sounds() {
    let session = Session::lock();
    let input = input_signal(longest_point_frames());
    for quality in Quality::ALL {
        for point in points().into_iter().filter(|p| p.base == center()) {
            let out = render_point(&session, quality, &point, &input).unwrap();
            // From the second block on: the first still carries the dry input while dry/wet ramps
            // from its initial 0 (a silent wet path once passed this test with the dry block alone).
            let peak = out[2 * GRANULAR_BLOCK_SIZE..].iter().flatten().map(|s| s.unsigned_abs()).max().unwrap_or(0);
            println!("{} {}: peak {:.1} dBFS", quality.label(), point.label(), 20.0 * (f64::from(peak) / 32768.0).log10());
            assert!(peak > 100, "{} {}: peak {peak}", quality.label(), point.label());
        }
    }
}

/// Inputs the C++ would index a table with out of bounds are refused before the call (FFI invariant
/// 4), and so are memory sizes without room for the workspace or the longest grain, and blocks the
/// processor cannot take. The sweep's own extremes are accepted.
#[test]
fn out_of_range_inputs_are_refused_before_the_cxx() {
    let session = Session::lock();
    let workspace = session.granular_workspace_size();
    assert_eq!(session.granular(FIRMWARE_SMALL_BUFFER, FIRMWARE_SMALL_BUFFER, Quality::Stereo).err(), Some(Error::BadParameter));
    assert_eq!(session.granular(workspace * 2 - 4, workspace, Quality::Stereo).err(), Some(Error::BadParameter));
    assert_eq!(session.granular(FIRMWARE_LARGE_BUFFER + 2, FIRMWARE_SMALL_BUFFER, Quality::Stereo).err(), Some(Error::BadParameter));
    assert!(session.granular(workspace * 2, workspace, Quality::MonoLowFidelity).is_ok());

    let mut processor = session.granular(FIRMWARE_LARGE_BUFFER, FIRMWARE_SMALL_BUFFER, Quality::Stereo).unwrap();
    let input = [[0_i16; 2]; GRANULAR_BLOCK_SIZE];
    let mut out = [[0_i16; 2]; GRANULAR_BLOCK_SIZE];
    let ok = center();
    assert_eq!(processor.process(&ok, &input, &mut out), Ok(()));
    let top = Parameters {
        position: 1.0,
        size: MAX_SIZE,
        pitch: *PITCH_RANGE.end(),
        density: 1.0,
        texture: 1.0,
        dry_wet: MAX_DRY_WET,
        stereo_spread: 1.0,
        feedback: 1.0,
        reverb: 1.0,
        freeze: true,
        trigger: true,
        gate: true,
    };
    assert_eq!(processor.process(&top, &input, &mut out), Ok(()));
    let bottom = Parameters {
        position: 0.0,
        size: 0.0,
        pitch: *PITCH_RANGE.start(),
        density: 0.0,
        texture: 0.0,
        dry_wet: 0.0,
        stereo_spread: 0.0,
        feedback: 0.0,
        reverb: 0.0,
        ..ok
    };
    assert_eq!(processor.process(&bottom, &input, &mut out), Ok(()));
    let bad = [
        Parameters { size: 1.0, ..ok },
        Parameters { dry_wet: 1.0, ..ok },
        Parameters { position: 1.01, ..ok },
        Parameters { pitch: 48.5, ..ok },
        Parameters { pitch: -49.0, ..ok },
        Parameters { density: -0.01, ..ok },
        Parameters { texture: f32::NAN, ..ok },
        Parameters { stereo_spread: f32::INFINITY, ..ok },
        Parameters { feedback: 1.5, ..ok },
        Parameters { reverb: f32::NEG_INFINITY, ..ok },
    ];
    for p in bad {
        assert_eq!(processor.process(&p, &input, &mut out), Err(Error::BadParameter), "{p:?}");
    }
    assert_eq!(processor.process(&ok, &input[..0], &mut out[..0]), Err(Error::BadBlock));
    assert_eq!(processor.process(&ok, &input[..3], &mut out[..4]), Err(Error::BadBlock));
    assert_eq!(processor.process(&ok, &input[..3], &mut out[..3]), Ok(()));
    processor.set_quality(Quality::StereoLowFidelity);
    assert_eq!(processor.process(&ok, &input[..3], &mut out[..3]), Err(Error::BadBlock), "odd blocks at a low-fidelity quality");
    assert_eq!(processor.process(&ok, &input[..4], &mut out[..4]), Ok(()));
}
