//! The meta-modulator oracle's residual floor against itself (#200): the same sweep with the same
//! seed renders bit-identical output twice in one process and again in a second process.
//!
//! A failure here means the reference itself is unstable on this runner (an uninitialized read,
//! state leaking between renders, a floating-point mode that differs between processes); the port
//! gate cannot be set until it is fixed. Never rerun it to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::modulator::{CarrierShape, MODULATOR_BLOCK_SIZE, MODULATOR_OVERSAMPLING, Parameters};
use transit_macro_osc_oracle::modulator_sweep::{self, MOTION_FRAMES, Motion, Point, center, input_signal, points, render_point};
use transit_macro_osc_oracle::residual::residual_i16;
use transit_macro_osc_oracle::{Error, Session, subset};

const CHILD_ENV: &str = "TRANSIT_MODULATOR_ORACLE_SELF_FLOOR_CHILD";
const LINE_PREFIX: &str = "modulator-digest ";

fn flat(frames: &[[i16; 2]]) -> Vec<i16> {
    frames.iter().flatten().copied().collect()
}

fn render(session: &Session, parameters: &Parameters, input: &[[i16; 2]], seed: u32, flush: bool) -> Vec<[i16; 2]> {
    session.seed(seed);
    let mut m = session.modulator().unwrap();
    m.set_flush(flush);
    let mut out = vec![[0_i16; 2]; input.len()];
    for (i, o) in input.chunks(MODULATOR_BLOCK_SIZE).zip(out.chunks_mut(MODULATOR_BLOCK_SIZE)) {
        m.process(parameters, i, o).unwrap();
    }
    out
}

#[test]
fn the_firmware_constants_match() {
    let session = Session::lock();
    assert_eq!(session.modulator_constants(), (MODULATOR_BLOCK_SIZE, 96, MODULATOR_OVERSAMPLING));
}

#[test]
fn same_seed_twice_is_bit_identical() {
    let session = Session::lock();
    let input = input_signal(MOTION_FRAMES);
    let sweep = subset::strided("modulator self floor, same seed twice", points());
    for point in &sweep {
        let a = render_point(&session, point, &input).unwrap();
        let b = render_point(&session, point, &input).unwrap();
        let r = residual_i16(&flat(&a), &flat(&b)).unwrap();
        assert!(r.is_exact(), "{}: {r}", point.label());
    }
}

#[test]
fn a_second_process_renders_the_same_bits() {
    let here = {
        let session = Session::lock();
        modulator_sweep::digest_lines(&session).unwrap()
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
    for line in modulator_sweep::digest_lines(&session).unwrap() {
        println!("{LINE_PREFIX}{line}");
    }
}

/// The internal oscillator's noise (the vocoder's carrier for the third shape) draws from the
/// shared random state, so the seed must reach the output; nothing else draws from it.
#[test]
fn the_seed_reaches_the_noise_carrier_only() {
    let session = Session::lock();
    let input = input_signal(12_000);
    let noise = center(CarrierShape::Saw, 0.875);
    let a = flat(&render(&session, &noise, &input, 7, true));
    assert_eq!(a, flat(&render(&session, &noise, &input, 7, true)));
    assert_ne!(a, flat(&render(&session, &noise, &input, 8, true)), "a different seed changes the noise");
    let sine = center(CarrierShape::Sine, 0.875);
    assert_eq!(flat(&render(&session, &sine, &input, 7, true)), flat(&render(&session, &sine, &input, 8, true)), "no other path draws");
}

/// Every point of the grid sounds on the main output after the first block, except the points
/// silent by design (the vocoder frozen on a fresh modulator), which are exactly silent (prints the
/// peak per shape and algorithm, the evidence that each path is exercised).
#[test]
fn every_algorithm_sounds() {
    let session = Session::lock();
    let input = input_signal(MOTION_FRAMES);
    for carrier in CarrierShape::ALL {
        let mut peaks = Vec::new();
        for point in modulator_sweep::points_for(carrier).into_iter().take(modulator_sweep::ALGORITHM_LEVELS.len() * 3) {
            let out = render_point(&session, &point, &input).unwrap();
            let peak = out[MODULATOR_BLOCK_SIZE..].iter().map(|[l, _]| l.unsigned_abs()).max().unwrap_or(0);
            if point.silent_by_design() {
                assert_eq!(peak, 0, "{}: silent by design, main output peak {peak}", point.label());
            } else {
                assert!(peak > 100, "{}: main output peak {peak}", point.label());
            }
            peaks.push(peak);
        }
        println!("{}: main peaks {peaks:?}", carrier.label());
    }
}

/// The flush-to-zero choice: whether rendering without it moves any output sample over the sweep's
/// slowest decays (the vocoder freezing and releasing, the inputs going silent). Prints the count;
/// the output is int16, so a subnormal can move a sample only through a comparison or a
/// conversion.
#[test]
fn the_flush_choice_is_measured() {
    let session = Session::lock();
    let input = input_signal(MOTION_FRAMES);
    let mut moved = 0usize;
    let mut compared = 0usize;
    for point in [
        Point { base: center(CarrierShape::External, 1.0), motion: Motion::Static },
        Point { base: center(CarrierShape::External, 0.9375), motion: Motion::Static },
        Point { base: center(CarrierShape::Triangle, 0.875), motion: Motion::Static },
        Point { base: center(CarrierShape::External, 0.3125), motion: Motion::Static },
        Point { base: center(CarrierShape::Sine, 0.0), motion: Motion::Static },
    ] {
        let p = point.parameters(0, 1);
        let a = flat(&render(&session, &p, &input, modulator_sweep::SEED, true));
        let b = flat(&render(&session, &p, &input, modulator_sweep::SEED, false));
        moved += a.iter().zip(&b).filter(|(x, y)| x != y).count();
        compared += a.len();
    }
    println!("flush to zero off: {moved} of {compared} output samples differ");
}

#[test]
fn out_of_range_inputs_are_refused_before_the_cxx() {
    let session = Session::lock();
    let mut m = session.modulator().unwrap();
    let input = vec![[0_i16; 2]; 60];
    let mut out = vec![[0_i16; 2]; 60];
    let ok = center(CarrierShape::External, 0.5);
    assert!(m.process(&ok, &input, &mut out).is_ok());
    for bad in [
        Parameters { algorithm: 1.01, ..ok },
        Parameters { algorithm: -0.01, ..ok },
        Parameters { timbre: f32::NAN, ..ok },
        Parameters { drive: [1.5, 0.0], ..ok },
        Parameters { note: 128.0, ..ok },
        Parameters { note: f32::INFINITY, ..ok },
    ] {
        assert_eq!(m.process(&bad, &input, &mut out), Err(Error::BadParameter), "{bad:?}");
    }
    for n in [0, 6, 13, 59, 72] {
        let i = vec![[0_i16; 2]; n];
        let mut o = vec![[0_i16; 2]; n];
        assert_eq!(m.process(&ok, &i, &mut o), Err(Error::BadBlock), "{n} frames");
    }
    for n in [12, 24, 36, 48] {
        assert!(m.process(&ok, &input[..n], &mut out[..n]).is_ok(), "{n} frames");
    }
}
