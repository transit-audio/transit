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
