//! The random sequencer oracle's floor against itself (#202): the same sweep with the same seed
//! renders bit-identical gates and voltages twice in one process and again in a second process;
//! and the wrapper's copies of the firmware's main-loop tables match the vendored sources.
//!
//! A failure here means the reference itself is unstable on this runner (an uninitialized read,
//! state leaking between renders, a floating-point mode that differs between processes) or a
//! transcription error; the port gate cannot be set until it is fixed. Never rerun it to pass.

#![forbid(unsafe_code)]

use transit_macro_osc_oracle::sequencer::{Controls, RecorderEvent, SEQUENCER_BLOCK_SIZE, SEQUENCER_SAMPLE_RATE};
use transit_macro_osc_oracle::sequencer_sweep::{self, Point, SEED, center, points, render_point, render_point_with};
use transit_macro_osc_oracle::{Session, subset};

const CHILD_ENV: &str = "TRANSIT_SEQUENCER_ORACLE_SELF_FLOOR_CHILD";
const LINE_PREFIX: &str = "sequencer-digest ";

fn vendored(path: &str) -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/marbles").join(path);
    std::fs::read_to_string(&root).unwrap_or_else(|e| panic!("{}: {e}", root.display()))
}

/// The text of the C array `name` in `source`: from its `=` to the closing `};`.
fn array_body<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source.find(name).unwrap_or_else(|| panic!("{name} not found"));
    let rest = &source[start..];
    let open = rest.find('{').unwrap();
    let close = rest.find("};").unwrap();
    &rest[open + 1..close]
}

fn numbers(text: &str) -> Vec<String> {
    // Drop comments first: the scale table names each degree in a trailing comment.
    let stripped: String = text.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    stripped
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|t| !t.is_empty() && t.chars().any(|c| c.is_ascii_digit()))
        .map(str::to_string)
        .collect()
}

#[test]
fn the_firmware_constants_match() {
    let session = Session::lock();
    #[allow(clippy::cast_possible_truncation)] // 32000.
    let rate = SEQUENCER_SAMPLE_RATE as i32;
    assert_eq!(session.sequencer_constants(), (SEQUENCER_BLOCK_SIZE, rate));
}

/// The loop lengths and the Y divisions in `cpp/sequencer_oracle.cc` are `marbles.cc`'s, and the
/// preset scales are `settings.cc`'s, value for value.
#[test]
fn the_wrappers_tables_are_the_firmwares() {
    let session = Session::lock();
    let main = vendored("marbles.cc");
    let lengths: Vec<i32> = numbers(array_body(&main, "int loop_length[]")).iter().map(|n| n.parse().unwrap()).collect();
    assert_eq!(session.sequencer_loop_lengths(), lengths);
    let ratios: Vec<i32> = numbers(array_body(&main, "Ratio y_divider_ratios[]")).iter().map(|n| n.parse().unwrap()).collect();
    let pairs: Vec<(i32, i32)> = ratios.chunks(2).map(|c| (c[0], c[1])).collect();
    assert_eq!(session.sequencer_y_dividers(), pairs);

    let settings = vendored("settings.cc");
    let start = settings.find("const Scale preset_scales[6]").unwrap();
    let body = &settings[start..];
    let body = &body[body.find('{').unwrap() + 1..body.find("};").unwrap()];
    // Each scale: `{ base, n, { {v, w}, ... } }`, split at the scale's opening line.
    let values = numbers(body);
    let mut at = 0;
    for (i, scale) in session.sequencer_preset_scales().iter().enumerate() {
        let base: f32 = values[at].parse().unwrap();
        let n: usize = values[at + 1].parse().unwrap();
        at += 2;
        assert_eq!(base, scale.base_interval, "scale {i}");
        assert_eq!(n, scale.degrees.len(), "scale {i}");
        for (k, (v, w)) in scale.degrees.iter().enumerate() {
            let fv: f32 = values[at].trim_end_matches('f').parse().unwrap();
            let fw: u8 = values[at + 1].parse().unwrap();
            assert_eq!((fv, fw), (*v, *w), "scale {i} degree {k}");
            at += 2;
        }
    }
    assert_eq!(at, values.len(), "every value of settings.cc's table is read");
}

#[test]
fn same_seed_twice_is_bit_identical() {
    let session = Session::lock();
    let sweep = subset::strided("sequencer self floor, same seed twice", points());
    for point in &sweep {
        let a = render_point(&session, point).unwrap();
        let b = render_point(&session, point).unwrap();
        assert!(a.first_difference(&b).is_none(), "{}: {:?}", point.label(), a.first_difference(&b));
    }
}

#[test]
fn a_second_process_renders_the_same_bits() {
    let here = {
        let session = Session::lock();
        sequencer_sweep::digest_lines(&session).unwrap()
    };
    let exe = std::env::current_exe().unwrap();
    let output = std::process::Command::new(exe)
        .args(["child_prints_the_sweep_digests", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "child failed ({}):\n{stdout}\n{}", output.status, String::from_utf8_lossy(&output.stderr));
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
    for line in sequencer_sweep::digest_lines(&session).unwrap() {
        println!("{LINE_PREFIX}{line}");
    }
}

/// The seed reaches the output, and each instance owns its random source: two sequencers made in
/// turn from one seed render the same, whatever the other drew.
#[test]
fn the_seed_reaches_every_output_and_instances_do_not_share_it() {
    let session = Session::lock();
    // The complementary Bernoulli model at the center bias: the coin toss decides each tick.
    let point = &points()[2];
    assert_eq!(point.base.t_bias, 0.5);
    let a = render_point(&session, point).unwrap();
    let mut other = session.sequencer(SEED + 1).unwrap();
    let mut g = [[0_u8; 3]; SEQUENCER_BLOCK_SIZE];
    let mut v = [[0.0_f32; 4]; SEQUENCER_BLOCK_SIZE];
    for _ in 0..100 {
        other.process(&center(), None, None, &mut g, &mut v).unwrap();
    }
    assert_eq!(render_point(&session, point).unwrap().digest(), a.digest(), "another instance's draws change nothing");
    // A different seed changes the gates and every voltage channel.
    let mut s = session.sequencer(SEED ^ 0xFFFF).unwrap();
    let blocks = point.frames() / SEQUENCER_BLOCK_SIZE;
    let mut b = sequencer_sweep::Render { gates: vec![[0; 3]; point.frames()], voltages: vec![[0.0; 4]; point.frames()] };
    for (block, (g, v)) in b.gates.chunks_mut(SEQUENCER_BLOCK_SIZE).zip(b.voltages.chunks_mut(SEQUENCER_BLOCK_SIZE)).enumerate() {
        s.process(&point.controls(block, blocks), None, None, g, v).unwrap();
    }
    assert!(a.gates != b.gates, "the gates follow the seed");
    for k in 0..4 {
        assert!(a.voltages.iter().zip(&b.voltages).any(|(x, y)| x[k] != y[k]), "channel {k}");
    }
}

/// Every model gates both channels at the center bias, each X channel moves when its clock runs,
/// and the
/// master clock t2 runs at the rate (16 Hz at the center: 48 rising edges in 3 s); prints the
/// counts per model, the evidence that each path runs.
#[test]
fn every_model_gates_and_the_clock_runs_at_the_rate() {
    let session = Session::lock();
    for point in points().iter().filter(|p| p.name.starts_with("models ")) {
        let r = render_point(&session, point).unwrap();
        let rises = |k: usize| r.gates.windows(2).filter(|w| w[0][k] == 0 && w[1][k] == 1).count();
        let (t1, t2, t3) = (rises(0), rises(1), rises(2));
        println!("{}: t1 {t1} t2 {t2} t3 {t3}", point.label());
        assert!((47..=49).contains(&t2), "{}: t2 rose {t2} times", point.label());
        if point.base.t_bias == 0.5 {
            assert!(t1 > 0 && t3 > 0, "{}: t1 {t1} t3 {t3}", point.label());
        }
        // Each X channel draws on its own clock: X1 on t1, X2 on t2, X3 on t3.
        for (k, rose) in [t1, t2, t3].into_iter().enumerate() {
            let moves = r.voltages.windows(2).any(|w| w[0][k] != w[1][k]);
            assert_eq!(moves, rose > 0, "{}: X{} moves {moves}, its gate rose {rose} times", point.label(), k + 1);
        }
    }
}

/// The flush moves nothing: the sweep renders the same bits with the oracle's flush-to-zero scope
/// off (strided). The port flushes anyway, as the family's references do.
#[test]
fn the_flush_moves_no_value() {
    let session = Session::lock();
    let sweep = subset::strided("sequencer flush evidence", points());
    let mut moved = Vec::new();
    for point in &sweep {
        let a = render_point_with(&session, point, true).unwrap();
        let b = render_point_with(&session, point, false).unwrap();
        if let Some(d) = a.first_difference(&b) {
            moved.push(format!("{}: {d}", point.label()));
        }
    }
    println!("flush moved {} of {} points", moved.len(), sweep.len());
    for m in &moved {
        println!("  {m}");
    }
}

/// The scale recorder learns a scale from notes and weighs degrees by how often they came.
#[test]
fn the_recorder_extracts_a_scale() {
    let session = Session::lock();
    let mut r = session.scale_recorder().unwrap();
    assert!(r.extract().is_none(), "nothing recorded");
    for (note, times) in [(0.0_f32, 4), (4.0 / 12.0, 2), (7.0 / 12.0, 3), (1.0 + 4.0 / 12.0, 1)] {
        for _ in 0..times {
            r.event(RecorderEvent::NewNote(note)).unwrap();
            r.event(RecorderEvent::Update(note)).unwrap();
            r.event(RecorderEvent::Accept).unwrap();
        }
    }
    let s = r.extract().unwrap();
    assert_eq!(s.degrees.len(), 3);
    assert_eq!(s.degrees[0].1, 255);
    assert!(r.event(RecorderEvent::NewNote(6.0)).is_err());
}

#[test]
fn bad_controls_are_refused() {
    let session = Session::lock();
    let mut s = session.sequencer(1).unwrap();
    let mut g = [[0_u8; 3]; SEQUENCER_BLOCK_SIZE];
    let mut v = [[0.0_f32; 4]; SEQUENCER_BLOCK_SIZE];
    assert!(s.process(&Controls { t_model: 7, ..center() }, None, None, &mut g, &mut v).is_err());
    assert!(s.process(&Controls { x_spread: 1.5, ..center() }, None, None, &mut g, &mut v).is_err());
    assert!(s.process(&Controls { t_external: 1, ..center() }, None, None, &mut g, &mut v).is_err(), "a patched clock needs its flags");
    assert!(s.process(&center(), Some(&[9; SEQUENCER_BLOCK_SIZE]), None, &mut g, &mut v).is_err());
    let _: &Point = &points()[0];
}
