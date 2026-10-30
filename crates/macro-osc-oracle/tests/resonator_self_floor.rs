//! The resonator oracle's residual floor against itself (#198): the same sweep with the same seed
//! renders bit-identical output twice in one process and again in a second process.
//!
//! A failure here means the reference itself is unstable on this runner (an uninitialized read,
//! state leaking between renders, a floating-point mode that differs between processes); the port
//! gate cannot be set until it is fixed. Never rerun it to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::Session;
use transit_macro_osc_oracle::residual::residual_f32;
use transit_macro_osc_oracle::resonator::{MAX_POLYPHONY, PerformanceState, RESONATOR_BLOCK_SIZE, ResonatorModel, ResonatorPatch};
use transit_macro_osc_oracle::resonator_sweep::{self, POLYPHONIES, points, render_point};

const CHILD_ENV: &str = "TRANSIT_RESONATOR_ORACLE_SELF_FLOOR_CHILD";
const LINE_PREFIX: &str = "resonator-digest ";

#[test]
fn the_firmware_constants_match() {
    let session = Session::lock();
    assert_eq!(session.resonator_max_block_size(), RESONATOR_BLOCK_SIZE);
    assert_eq!(session.resonator_max_polyphony(), MAX_POLYPHONY);
    assert_eq!(session.resonator_model_count(), ResonatorModel::ALL.len());
}

#[test]
fn same_seed_twice_is_bit_identical() {
    let session = Session::lock();
    let mut compared = 0usize;
    for model in ResonatorModel::ALL {
        for polyphony in POLYPHONIES {
            for point in points() {
                let a = render_point(&session, model, polyphony, &point).unwrap();
                let b = render_point(&session, model, polyphony, &point).unwrap();
                for (which, x, y) in [("out", &a.out, &b.out), ("aux", &a.aux, &b.aux)] {
                    let r = residual_f32(x, y).unwrap();
                    assert!(r.is_exact(), "{} p{polyphony} {} {which}: {r}", model.label(), point.label());
                }
                compared += 1;
            }
        }
    }
    assert_eq!(compared, ResonatorModel::ALL.len() * POLYPHONIES.len() * points().len());
}

#[test]
fn a_second_process_renders_the_same_bits() {
    let here = {
        let session = Session::lock();
        resonator_sweep::digest_lines(&session).unwrap()
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
    for line in resonator_sweep::digest_lines(&session).unwrap() {
        println!("{LINE_PREFIX}{line}");
    }
}

/// The internal exciter's noise burst draws from the shared random state, so the seed must reach
/// the strings' output.
#[test]
fn the_seed_reaches_the_noise_burst() {
    let session = Session::lock();
    let patch = ResonatorPatch { structure: 0.5, brightness: 0.5, damping: 0.5, position: 0.5 };
    let render = |seed: u32| {
        session.seed(seed);
        let mut part = session.resonator().unwrap();
        part.set_model(ResonatorModel::InharmonicString);
        let input = [0.0_f32; RESONATOR_BLOCK_SIZE];
        let mut out = vec![0.0_f32; 2400];
        let mut aux = vec![0.0_f32; 2400];
        for (block, (o, a)) in
            out.as_chunks_mut::<RESONATOR_BLOCK_SIZE>().0.iter_mut().zip(aux.as_chunks_mut::<RESONATOR_BLOCK_SIZE>().0.iter_mut()).enumerate()
        {
            let performance = PerformanceState { strum: block == 0, internal_exciter: true, tonic: 36.0, note: 24.0, ..PerformanceState::default() };
            part.process(&performance, &patch, &input, o, a).unwrap();
        }
        out
    };
    let a = render(7);
    let b = render(7);
    let c = render(8);
    assert!(residual_f32(&a, &b).unwrap().is_exact());
    assert!(!residual_f32(&a, &c).unwrap().is_exact(), "a different seed must change the string's noise burst");
}

/// Every model sounds at every polyphony (prints the peak level per model and polyphony).
#[test]
fn every_model_sounds() {
    let session = Session::lock();
    let center = points().into_iter().find(|p| p.label() == "n60 s0.50 b0.50 d0.50 p0.50 int fm0").unwrap();
    for model in ResonatorModel::ALL {
        for polyphony in POLYPHONIES {
            let r = render_point(&session, model, polyphony, &center).unwrap();
            let peak = r.out.iter().chain(&r.aux).fold(0.0_f32, |m, s| m.max(s.abs()));
            println!("{} p{polyphony}: peak {:.1} dBFS", model.label(), 20.0 * peak.log10());
            assert!(peak > 1e-4 && peak.is_finite(), "{} p{polyphony}: peak {peak}", model.label());
        }
    }
}

/// Inputs the C++ would index a table with out of bounds are refused before the call (FFI invariant
/// 4): macros outside 0 to 1, a tonic, note or FM outside their ranges, NaN or infinite values, and a
/// chord outside the table. The sweep's own extremes are accepted.
#[test]
fn out_of_range_inputs_are_refused_before_the_cxx() {
    use transit_macro_osc_oracle::Error;
    use transit_macro_osc_oracle::resonator::{FM_RANGE, MAX_DAMPING, NOTE_RANGE, STRUCTURE_RANGE, TONIC_RANGE};
    let session = Session::lock();
    let mut part = session.resonator().unwrap();
    let ok_patch = ResonatorPatch { structure: 0.5, brightness: 0.5, damping: 0.5, position: 0.5 };
    let ok = PerformanceState { tonic: 36.0, note: 24.0, ..PerformanceState::default() };
    let input = [0.0_f32; RESONATOR_BLOCK_SIZE];
    let (mut out, mut aux) = ([0.0_f32; RESONATOR_BLOCK_SIZE], [0.0_f32; RESONATOR_BLOCK_SIZE]);
    let mut run = |p: &PerformanceState, m: &ResonatorPatch| part.process(p, m, &input, &mut out, &mut aux);

    assert_eq!(run(&ok, &ok_patch), Ok(()));
    let edges = PerformanceState { tonic: *TONIC_RANGE.end(), note: *NOTE_RANGE.end(), fm: *FM_RANGE.end(), chord: 10, ..ok };
    assert_eq!(run(&edges, &ResonatorPatch { structure: *STRUCTURE_RANGE.end(), brightness: 1.0, damping: MAX_DAMPING, position: 1.0 }), Ok(()));
    let low = PerformanceState { tonic: *TONIC_RANGE.start(), note: *NOTE_RANGE.start(), fm: *FM_RANGE.start(), ..ok };
    assert_eq!(run(&low, &ResonatorPatch { structure: 0.0, brightness: 0.0, damping: 0.0, position: 0.0 }), Ok(()));

    let bad_performances = [
        PerformanceState { tonic: 72.5, ..ok },
        PerformanceState { tonic: 11.0, ..ok },
        PerformanceState { note: 84.5, ..ok },
        PerformanceState { note: -44.0, ..ok },
        PerformanceState { fm: 15.5, ..ok },
        PerformanceState { fm: -16.0, ..ok },
        PerformanceState { note: f32::NAN, ..ok },
        PerformanceState { tonic: f32::INFINITY, ..ok },
        PerformanceState { fm: f32::NEG_INFINITY, ..ok },
        PerformanceState { chord: 11, ..ok },
        PerformanceState { chord: -1, ..ok },
    ];
    for p in bad_performances {
        assert_eq!(run(&p, &ok_patch), Err(Error::BadParameter), "{p:?}");
    }
    let bad_patches = [
        ResonatorPatch { structure: 1.01, ..ok_patch },
        // Exactly 1.0 would make `Interpolate` read one past `lut_stiffness` (modal) or
        // `lut_fm_frequency_quantizer` (FM voice), and damping 1.0 one past `lut_4_decades`.
        ResonatorPatch { structure: 1.0, ..ok_patch },
        ResonatorPatch { structure: 0.9996, ..ok_patch },
        ResonatorPatch { damping: 1.0, ..ok_patch },
        ResonatorPatch { brightness: -0.01, ..ok_patch },
        ResonatorPatch { damping: f32::NAN, ..ok_patch },
        ResonatorPatch { position: f32::INFINITY, ..ok_patch },
    ];
    for m in bad_patches {
        assert_eq!(run(&ok, &m), Err(Error::BadParameter), "{m:?}");
    }
    // Audio input samples must be finite and within the codec's -1 to 1.
    for bad in [f32::NAN, f32::INFINITY, 1.5, -1.01] {
        let mut samples = [0.0_f32; RESONATOR_BLOCK_SIZE];
        samples[5] = bad;
        let (mut o, mut a) = ([0.0_f32; RESONATOR_BLOCK_SIZE], [0.0_f32; RESONATOR_BLOCK_SIZE]);
        assert_eq!(part.process(&ok, &ok_patch, &samples, &mut o, &mut a), Err(Error::BadParameter), "input {bad}");
    }
}
