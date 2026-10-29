//! The oracle's residual floor against itself (#194): the same sweep with the same seed renders
//! bit-identical output twice in one process and again in a second process.
//!
//! A failure here means the reference itself is unstable on this runner (an uninitialized read,
//! state leaking between renders, a floating-point mode that differs between processes); the
//! port gate (#195) cannot be set until it is fixed. Never rerun it to pass.

use transit_macro_osc_oracle::residual::{residual_f32, residual_i16};
use transit_macro_osc_oracle::sweep::{self, engine_points, render_engine_point, render_voice_point, voice_points};
use transit_macro_osc_oracle::{Model, Session, floor::RunnerFamily};

const CHILD_ENV: &str = "TRANSIT_ORACLE_SELF_FLOOR_CHILD";
const LINE_PREFIX: &str = "oracle-digest ";

#[test]
fn same_seed_twice_is_bit_identical() {
    let session = Session::lock();
    let mut compared = 0usize;
    for model in Model::ALL {
        for point in engine_points() {
            let a = render_engine_point(&session, model, &point).unwrap();
            let b = render_engine_point(&session, model, &point).unwrap();
            for (which, x, y) in [("out", &a.out, &b.out), ("aux", &a.aux, &b.aux)] {
                let r = residual_f32(x, y).unwrap();
                assert!(r.is_exact(), "{} engine {} {which}: {r}", model.label(), point.label());
            }
            compared += 1;
        }
        for point in voice_points() {
            let a = render_voice_point(&session, model, &point).unwrap();
            let b = render_voice_point(&session, model, &point).unwrap();
            let outs = |f: &[transit_macro_osc_oracle::Frame]| f.iter().flat_map(|f| [f.out, f.aux]).collect::<Vec<_>>();
            let r = residual_i16(&outs(&a), &outs(&b)).unwrap();
            assert!(r.is_exact(), "{} voice {}: {r}", model.label(), point.label());
            compared += 1;
        }
    }
    assert_eq!(compared, Model::ALL.len() * (engine_points().len() + voice_points().len()));
    // The documented floor for this runner family is the one just measured.
    if let Some(floor) = RunnerFamily::current().self_floor_dbfs() {
        assert_eq!(floor, f64::NEG_INFINITY);
    }
}

#[test]
fn a_second_process_renders_the_same_bits() {
    let here = {
        let session = Session::lock();
        sweep::digest_lines(&session).unwrap()
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
    for line in sweep::digest_lines(&session).unwrap() {
        println!("{LINE_PREFIX}{line}");
    }
}

#[test]
fn the_seed_reaches_the_random_state() {
    let session = Session::lock();
    session.seed(1);
    assert_eq!(session.rng_state(), 1);
    let point = sweep::EnginePoint { note: 60.0, harmonics: 0.5, timbre: 0.5, morph: 0.5, triggered: false };
    let render = |seed: u32| {
        session.seed(seed);
        let mut engine = session.engine(Model::FilteredNoise).unwrap();
        let mut out = vec![0.0_f32; 1200];
        let mut aux = vec![0.0_f32; 1200];
        for (block, (o, a)) in out.as_chunks_mut::<12>().0.iter_mut().zip(aux.as_chunks_mut::<12>().0.iter_mut()).enumerate() {
            engine.render(&point.parameters(block, 100), o, a).unwrap();
        }
        out
    };
    let a = render(7);
    let b = render(7);
    let c = render(8);
    assert!(residual_f32(&a, &b).unwrap().is_exact());
    assert!(!residual_f32(&a, &c).unwrap().is_exact(), "a different seed must change the noise engine's output");
}
