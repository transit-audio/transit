//! The texture oracle's residual floor against itself (#241): the same sweep with the same seed
//! renders bit-identical output twice in one process and again in a second process.
//!
//! A failure here means the reference itself is unstable on this runner (an uninitialized read,
//! state leaking between renders, a floating-point mode that differs between processes); the port
//! gate cannot be set until it is fixed. Never rerun it to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::residual::residual_f32;
use transit_macro_osc_oracle::texture::{
    Controls, INPUT_LIMIT, Media, TEXTURE_FIRMWARE_BUFFER_BITS, TEXTURE_LONG_BUFFER_BITS, TEXTURE_MAX_BLOCK_SIZE,
};
use transit_macro_osc_oracle::texture_sweep::{
    self, Gate, INPUT_RUNS, Point, Quality, center, delay_center, input_signal, longest_point_frames, points, render_input_run, render_point,
    render_synth_point, synth_center, synth_points,
};
use transit_macro_osc_oracle::{Error, Session, subset};

const CHILD_ENV: &str = "TRANSIT_TEXTURE_ORACLE_SELF_FLOOR_CHILD";
const LINE_PREFIX: &str = "texture-digest ";

fn flat(frames: &[[f32; 2]]) -> Vec<f32> {
    frames.iter().flatten().copied().collect()
}

#[test]
fn the_firmware_constants_match() {
    let session = Session::lock();
    assert_eq!(
        session.texture_constants(),
        [(TEXTURE_MAX_BLOCK_SIZE, TEXTURE_FIRMWARE_BUFFER_BITS), (TEXTURE_MAX_BLOCK_SIZE, TEXTURE_LONG_BUFFER_BITS)]
    );
    for quality in Quality::all() {
        assert!(session.texture(quality.media, quality.mono).is_ok(), "{}", quality.label());
        assert!(session.texture_long(quality.media, quality.mono).is_ok(), "{}", quality.label());
    }
}

#[test]
fn same_seed_twice_is_bit_identical() {
    let session = Session::lock();
    let input = input_signal(longest_point_frames());
    let mut compared = 0usize;
    let sweep = subset::strided("texture self floor, same seed twice", points());
    for quality in Quality::all() {
        for point in &sweep {
            let a = render_point(&session, quality, point, &input).unwrap();
            let b = render_point(&session, quality, point, &input).unwrap();
            let r = residual_f32(&flat(&a), &flat(&b)).unwrap();
            assert!(r.is_exact(), "{} {}: {r}", quality.label(), point.label());
            compared += 1;
        }
    }
    for resolution in 0..4 {
        for point in synth_points() {
            let a = render_synth_point(&session, resolution, &point).unwrap();
            let b = render_synth_point(&session, resolution, &point).unwrap();
            let r = residual_f32(&flat(&a), &flat(&b)).unwrap();
            assert!(r.is_exact(), "synth {resolution} {}: {r}", point.label());
            compared += 1;
        }
    }
    for (rate, mode) in INPUT_RUNS {
        let a = render_input_run(&session, rate, mode).unwrap();
        let b = render_input_run(&session, rate, mode).unwrap();
        assert!(residual_f32(&flat(&a), &flat(&b)).unwrap().is_exact(), "input {rate} {mode}");
        compared += 1;
    }
    assert_eq!(compared, Quality::all().len() * sweep.len() + 4 * synth_points().len() + INPUT_RUNS.len());
}

#[test]
fn a_second_process_renders_the_same_bits() {
    let here = {
        let session = Session::lock();
        texture_sweep::digest_lines(&session).unwrap()
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
    for line in texture_sweep::digest_lines(&session).unwrap() {
        println!("{LINE_PREFIX}{line}");
    }
}

/// The grains' seeding, panning and randomness draw from the shared random state, so the seed must
/// reach the output.
#[test]
fn the_seed_reaches_the_grains() {
    let session = Session::lock();
    let input = input_signal(24_000);
    let render = |seed: u32| {
        session.seed(seed);
        let mut chain = session.texture(Media::Digital, false).unwrap();
        let mut controls = Controls { density: 0.9, ..center() };
        controls.time.base = 0.0;
        let mut out = vec![[0.0_f32; 2]; input.len()];
        for (i, o) in input.chunks(12).zip(out.chunks_mut(12)) {
            chain.process(&controls, &[false; 12], None, i, o).unwrap();
        }
        flat(&out)
    };
    let a = render(7);
    let b = render(7);
    let c = render(8);
    assert!(residual_f32(&a, &b).unwrap().is_exact());
    assert!(!residual_f32(&a, &c).unwrap().is_exact(), "a different seed must change the grains");
}

/// Every quality sounds at both centers in every pattern (prints the peak level).
#[test]
fn every_quality_sounds() {
    let session = Session::lock();
    let input = input_signal(longest_point_frames());
    for quality in Quality::all() {
        for point in points().into_iter().filter(|p| p.base == center() || p.base == delay_center()) {
            let out = render_point(&session, quality, &point, &input).unwrap();
            // The delay points from 2.25 s on: the crossing into the delay mode is complete.
            let from = if point.delay() { out.len() * 9 / 16 } else { out.len() / 8 };
            let peak = out[from..].iter().flatten().fold(0.0_f32, |m, s| m.max(s.abs()));
            println!("{} {}: peak {:.1} dBFS", quality.label(), point.label(), 20.0 * f64::from(peak).log10());
            assert!(peak > 0.01, "{} {}: peak {peak}", quality.label(), point.label());
        }
    }
    for resolution in 0..4 {
        for point in synth_points().into_iter().filter(|p| p.base == synth_center()) {
            let out = render_synth_point(&session, resolution, &point).unwrap();
            let peak = out[out.len() / 8..].iter().flatten().fold(0.0_f32, |m, s| m.max(s.abs()));
            println!("synth {resolution} {}: peak {:.1} dBFS", point.label(), 20.0 * f64::from(peak).log10());
            assert!(peak > 0.01, "synth {resolution} {}: peak {peak}", point.label());
        }
    }
    for (rate, mode) in INPUT_RUNS {
        let out = render_input_run(&session, rate, mode).unwrap();
        let peak = out.iter().flatten().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1, "input {rate} {mode}: peak {peak}");
    }
}

/// Inputs the C++ would misuse (a table index out of bounds, a float-to-int cast out of range) are
/// refused before the call (FFI invariant 4), and so are blocks the chain cannot take. The module's
/// own extremes are accepted.
#[test]
fn out_of_range_inputs_are_refused_before_the_cxx() {
    let session = Session::lock();
    let mut chain = session.texture(Media::Digital, false).unwrap();
    let input = [[0.0_f32; 2]; TEXTURE_MAX_BLOCK_SIZE];
    let mut out = [[0.0_f32; 2]; TEXTURE_MAX_BLOCK_SIZE];
    let freeze = [false; TEXTURE_MAX_BLOCK_SIZE];
    let ok = center();
    assert_eq!(chain.process(&ok, &freeze, None, &input, &mut out), Ok(()));
    let mut top = ok;
    for p in [&mut top.time, &mut top.size, &mut top.shape] {
        p.base = 1.0;
        p.attenuverter = 1.0;
        p.cv = 1.6;
        p.has_cv = true;
    }
    top.pitch.base = 24.0;
    top.pitch.attenuverter = -1.0;
    top.pitch.cv = 60.0;
    top.pitch.has_cv = true;
    top.density = 1.0;
    top.density_cv = -1.0;
    top.feedback = 1.0;
    top.dry_wet = 1.0;
    top.reverb = 1.0;
    let loud = [[INPUT_LIMIT, -INPUT_LIMIT]; TEXTURE_MAX_BLOCK_SIZE];
    assert_eq!(chain.process(&top, &[true; TEXTURE_MAX_BLOCK_SIZE], Some(&[3; TEXTURE_MAX_BLOCK_SIZE]), &loud, &mut out), Ok(()));
    let mut bad = Vec::new();
    for f in [
        |c: &mut Controls| c.pitch.base = 24.5,
        |c: &mut Controls| c.pitch.cv = -61.0,
        |c: &mut Controls| c.time.base = 1.01,
        |c: &mut Controls| c.size.attenuverter = -1.5,
        |c: &mut Controls| c.shape.cv = 2.0,
        |c: &mut Controls| c.density = f32::NAN,
        |c: &mut Controls| c.density_cv = 1.5,
        |c: &mut Controls| c.feedback = f32::INFINITY,
        |c: &mut Controls| c.dry_wet = -0.1,
        |c: &mut Controls| c.reverb = 2.0,
    ] {
        let mut c = ok;
        f(&mut c);
        bad.push(c);
    }
    for c in bad {
        assert_eq!(chain.process(&c, &freeze, None, &input, &mut out), Err(Error::BadParameter), "{c:?}");
    }
    let hot = [[INPUT_LIMIT * 1.01, 0.0]; TEXTURE_MAX_BLOCK_SIZE];
    assert_eq!(chain.process(&ok, &freeze, None, &hot, &mut out), Err(Error::BadParameter));
    let nan = [[f32::NAN, 0.0]; TEXTURE_MAX_BLOCK_SIZE];
    assert_eq!(chain.process(&ok, &freeze, None, &nan, &mut out), Err(Error::BadParameter));
    assert_eq!(chain.process(&ok, &freeze[..0], None, &input[..0], &mut out[..0]), Err(Error::BadBlock));
    assert_eq!(chain.process(&ok, &freeze[..3], None, &input[..3], &mut out[..4]), Err(Error::BadBlock));
    assert_eq!(chain.process(&ok, &freeze[..3], Some(&[0; 2]), &input[..3], &mut out[..3]), Err(Error::BadBlock));
    assert_eq!(chain.process(&ok, &freeze[..3], None, &input[..3], &mut out[..3]), Ok(()));
    chain.configure(Media::CleanTape, false);
    assert_eq!(chain.process(&ok, &freeze[..3], None, &input[..3], &mut out[..3]), Err(Error::BadBlock), "odd blocks at the clean-tape quality");
    assert_eq!(chain.process(&ok, &freeze[..4], None, &input[..4], &mut out[..4]), Ok(()));
}

/// The sweep's patterns all reach the chain: a quick check that the points render (the full sweep is
/// the floor above).
#[test]
fn a_seed_point_renders() {
    let session = Session::lock();
    let input = input_signal(longest_point_frames());
    let point = Point { base: delay_center(), gate: Gate::Seed };
    let quality = Quality { media: Media::DirtyTape, mono: true };
    let out = render_point(&session, quality, &point, &input).unwrap();
    assert_eq!(out.len(), point.frames(quality));
}
