//! Gate G3 (ADR 0012, audit F02): parameter changes reach a VST3 processor without any
//! edit-controller call on the render thread, already normalized, and follow a curve the plug-in
//! changes through `restartComponent`.
//!
//! The fixture's probe mode is per process (`VST3_FIXTURE_PROBES`), so `main` reruns this binary
//! with it set (`harness = false`, which also makes `main` the process main thread, where editor
//! calls are allowed). In probe mode the fixture ends the process with status 3 as soon as its
//! edit controller is called on a thread that has run `process`, and writes the last Freq value
//! its processor received to its second output channel.

mod common;

use transit_dsp::Plugin;

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const PROBES: &str = "VST3_FIXTURE_PROBES";
const MANY_VAR: &str = "VST3_FIXTURE_PARAMS";
const BLOCK: usize = 64;

/// Freq's normalized value for `plain`, as the fixture's controller computes it (`20 · 1000ⁿ`).
fn freq_log(plain: f64) -> f64 {
    (plain / 20.0).ln() / 1000f64.ln()
}

/// The same after the fixture's Mode C reshape (`20 + 19980 n²`).
fn freq_square(plain: f64) -> f64 {
    ((plain - 20.0) / 19_980.0).sqrt()
}

fn gain_id() -> String {
    let found = transit_vst3_host::scan_paths(&[common::fixture_dir().to_path_buf()]);
    found.into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id
}

/// Runs one block of 0.5 in, returns (channel 0, channel 1) at the first frame.
fn block(p: &mut Box<dyn transit_dsp::Plugin>, io: &mut [Vec<f32>]) -> (f32, f32) {
    for ch in io.iter_mut() {
        ch.fill(0.5);
    }
    p.process(io, BLOCK);
    (io[0][0], io[1][0])
}

/// (1) and (2): automation changes Gain and Freq every block on the render thread while the main
/// thread services the editor handle, pushes editor-side values and saves state. No controller
/// call may happen on the render thread, and each block's Freq point must match the controller's
/// own conversion within 1e-4 (the last of several changes in a block wins).
fn automation_never_calls_the_controller_on_the_render_thread() {
    let id = gain_id();
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let _ = ed.idle(); // samples the curves (lazily, on the UI thread) before the render thread runs
    let mut reader = p.state_reader().expect("state reader");
    let done = Arc::new(AtomicBool::new(false));
    let render_done = Arc::clone(&done);
    let render = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        // The first block marks this thread as the render thread in the fixture.
        block(&mut p, &mut io);
        let mut worst = 0.0f64;
        for k in 0..2_000u32 {
            let plain = 20.0 * 1000f32.powf(k as f32 / 1_999.0);
            let gain = (k % 200) as f32 / 100.0;
            assert!(p.set_param("12", 1_000.0));
            assert!(p.set_param("12", plain), "latest value in a block wins");
            assert!(p.set_param("7", gain));
            let (out, freq) = block(&mut p, &mut io);
            worst = worst.max((f64::from(freq) - freq_log(f64::from(plain))).abs());
            assert!((out - 0.5 * gain).abs() < 1e-4, "block {k}: gain {gain} gave {out}");
        }
        render_done.store(true, Ordering::SeqCst);
        (p, worst)
    });
    let mut saves = 0;
    while !done.load(Ordering::SeqCst) && !render.is_finished() {
        let _ = ed.idle();
        ed.set_param("7", 1.0);
        assert!(reader.save_state().is_some());
        saves += 1;
    }
    let (p, worst) = render.join().expect("render thread");
    assert!(worst < 1e-4, "Freq points off the controller's curve by {worst}");
    println!("  render thread: 2000 automated blocks, {saves} state saves alongside, worst Freq error {worst:.2e}");
    drop(ed);
    drop(p);
}

/// The plug-in reshapes Freq (Mode C) and signals `kParamTitlesChanged` from the UI thread; the
/// editor handle's idle re-samples the curve there. The value the host set before the change is
/// re-sent on the new curve without another `set_param`, and new values follow the new curve.
fn a_restart_re_samples_the_curve_on_the_ui_thread() {
    let id = gain_id();
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let _ = ed.idle(); // samples the curves (lazily, on the UI thread) before the render thread runs
    let mut p = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        block(&mut p, &mut io);
        assert!(p.set_param("12", 5_000.0));
        let (_, freq) = block(&mut p, &mut io);
        assert!((f64::from(freq) - freq_log(5_000.0)).abs() < 1e-4, "before the restart: {freq}");
        p
    })
    .join()
    .expect("render thread");
    // Main (UI) thread: the mode change makes the fixture call restartComponent; idle re-samples.
    ed.set_param("3", 2.0);
    let _ = ed.idle();
    let p = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let (_, freq) = block(&mut p, &mut io);
        assert!((f64::from(freq) - freq_square(5_000.0)).abs() < 1e-4, "re-sent on the new curve: {freq}");
        for plain in [200.0f32, 1_000.0, 12_345.0, 20_000.0] {
            assert!(p.set_param("12", plain));
            let (_, freq) = block(&mut p, &mut io);
            let want = freq_square(f64::from(plain));
            assert!((f64::from(freq) - want).abs() < 1e-4, "{plain}: {freq} vs {want}");
        }
        p
    })
    .join()
    .expect("render thread");
    ed.set_param("3", 0.0);
    let _ = ed.idle();
    drop(ed);
    drop(p);
}

/// Lazy sampling: a curve the render thread needs before the UI thread sampled it is
/// converted linearly for that block; once the UI thread has sampled it (priority, no
/// `set_param` again) the value is re-sent on the real curve.
fn an_unsampled_curve_is_linear_until_sampled_then_re_sent() {
    let id = gain_id();
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let mut ed = p.editor().expect("editor handle");
    let mut p = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        block(&mut p, &mut io);
        assert!(p.set_param("12", 5_000.0));
        let (_, freq) = block(&mut p, &mut io);
        let linear = (5_000.0 - 20.0) / 19_980.0;
        assert!((f64::from(freq) - linear).abs() < 1e-4, "not sampled yet: linear fallback, got {freq}");
        p
    })
    .join()
    .expect("render thread");
    let _ = ed.idle();
    let p = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        let (_, freq) = block(&mut p, &mut io);
        assert!((f64::from(freq) - freq_log(5_000.0)).abs() < 1e-4, "re-sent on the sampled curve: {freq}");
        p
    })
    .join()
    .expect("render thread");
    drop(ed);
    drop(p);
}

/// An offline render (bounce, freeze, CLI) runs no editor `idle`. Instances created inside an
/// `offline_scope` have every curve sampled at creation, so automating the log-curve Freq
/// during the render converts through the controller's curve, not linearly; `sample_all_curves_now`
/// does the same for an instance made outside the scope.
fn an_offline_render_converts_through_the_curve_without_idle() {
    let id = gain_id();
    let render = |p: transit_vst3_host::Vst3Plugin| {
        std::thread::spawn(move || {
            let mut p: Box<dyn transit_dsp::Plugin> = Box::new(p);
            let mut io = vec![vec![0.0f32; BLOCK]; 2];
            block(&mut p, &mut io);
            let mut worst = 0.0f64;
            for plain in [20.0f32, 100.0, 1_000.0, 5_000.0, 20_000.0] {
                assert!(p.set_param("12", plain));
                let (_, freq) = block(&mut p, &mut io);
                worst = worst.max((f64::from(freq) - freq_log(f64::from(plain))).abs());
            }
            // Dropped on the calling thread: tearing down touches the controller.
            (worst, p)
        })
        .join()
        .expect("render thread")
    };
    // Created in the scope, as the mix engine does for a bounce; no editor, no idle.
    let scope = transit_vst3_host::offline_scope();
    let p = transit_vst3_host::instantiate_plugin(&id).expect("instantiate");
    drop(scope);
    assert_eq!(p.curves_sampled(), p.curves_total(), "sampled at creation");
    let (worst, p) = render(p);
    drop(p);
    assert!(worst < 1e-4, "offline render off the controller's curve by {worst}");
    // Outside the scope nothing is sampled until asked.
    let p = transit_vst3_host::instantiate_plugin(&id).expect("instantiate");
    assert_eq!(p.curves_sampled(), 0);
    p.sample_all_curves_now();
    assert_eq!(p.curves_sampled(), p.curves_total());
    let (worst, p) = render(p);
    drop(p);
    assert!(worst < 1e-4, "after sample_all_curves_now: off by {worst}");
}

/// A large plug-in (`VST3_FIXTURE_PARAMS`): how many extra continuous parameters it exposes.
const MANY: usize = 2_500;
/// Most curves one editor idle may sample.
const BUDGET: usize = 64;
const CURVE_POINTS: usize = 257;

/// (3) A parameter with 300 steps converts exactly in both directions: every step the host sets
/// reaches the controller as `step / 300` and comes back as the same step.
fn a_300_step_parameter_round_trips_every_step() {
    let id = gain_id();
    let mut p = transit_vst3_host::create(&id).expect("instantiate");
    let fine = p.info().param("2000").expect("Fine is exposed");
    assert_eq!((fine.min, fine.max), (0.0, 300.0), "the step index is the value");
    assert!(fine.choices.is_empty(), "too many steps for a menu");
    let mut reader = p.state_reader().expect("state reader");
    for k in 0..=300u32 {
        assert!(p.set_param("2000", k as f32));
        let blob = reader.save_state().expect("save");
        assert!(p.load_state(&blob), "load");
        assert_eq!(p.param("2000"), Some(k as f32), "step {k} did not survive the controller");
    }
    println!("  Fine: all 301 steps of 300 round-trip exactly");
}

enum Cmd {
    /// Run one block after setting these parameters.
    Block(Vec<(String, f32)>),
    Stop,
}

/// (2) Creation samples nothing; the editor's idle samples at most `BUDGET` curves a frame until
/// all are done; the controller is never called on the render thread meanwhile.
fn a_large_plugin_is_sampled_lazily_within_the_budget() {
    let id = gain_id();
    let mut p = transit_vst3_host::instantiate_plugin(&id).expect("instantiate");
    p.prepare(48_000.0, BLOCK, 2);
    let total = p.curves_total();
    assert!(total >= MANY, "{total} curves");
    assert_eq!(p.curves_sampled(), 0, "creation samples nothing");
    let mut ed = p.editor().expect("editor handle");
    let (tx, cmds) = std::sync::mpsc::channel::<Cmd>();
    let (reply, replies) = std::sync::mpsc::channel::<(u64, usize)>();
    let render = std::thread::spawn(move || {
        let mut io = vec![vec![0.0f32; BLOCK]; 2];
        while let Ok(Cmd::Block(sets)) = cmds.recv() {
            for (id, v) in sets {
                assert!(p.set_param(&id, v));
            }
            for ch in io.iter_mut() {
                ch.fill(0.5);
            }
            p.process(&mut io, BLOCK);
            // The fixture reports its controller-call count in channel 1, first sample.
            reply.send((io[1][0] as u64, p.curves_sampled())).expect("reply");
        }
        p
    });
    let ask = |sets: Vec<(String, f32)>| {
        tx.send(Cmd::Block(sets)).expect("render alive");
        replies.recv().expect("render reply")
    };
    // The render thread touches the very last parameter before anything is sampled.
    let last = format!("{}", 1000 + MANY - 1);
    let (at_creation, sampled) = ask(vec![(last, 5.0)]);
    assert_eq!(sampled, 0);
    println!("  large plug-in ({total} curves): {at_creation} controller calls at creation, none sampled");
    let mut prev = at_creation;
    let mut frames = 0usize;
    loop {
        let _ = ed.idle();
        let (calls, sampled) = ask(vec![]);
        let delta = calls - prev;
        // A few calls besides the sampling itself (idle bookkeeping) are fine.
        assert!(delta <= (BUDGET * CURVE_POINTS) as u64 + 64, "frame {frames}: {delta} controller calls, budget {BUDGET} curves");
        prev = calls;
        frames += 1;
        if sampled == total {
            break;
        }
        assert!(frames < 10 * total, "sampling stalled at {sampled}/{total}");
    }
    assert!(prev - at_creation >= (total * CURVE_POINTS) as u64, "every curve was sampled in full");
    assert!(frames >= total / BUDGET, "{frames} frames for {total} curves: the budget was ignored");
    let _ = ed.idle();
    let (after, _) = ask(vec![]);
    assert_eq!(after, prev, "nothing is sampled twice");
    println!("  sampled all {total} curves over {frames} idle frames, {} controller calls, none on the render thread", prev - at_creation);
    tx.send(Cmd::Stop).expect("stop");
    let p = render.join().expect("render thread");
    drop(ed);
    drop(p);
}

/// How long a probe child may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Reruns this binary with `envs`, killing it (no zombie) and failing when it outlives `TIMEOUT`.
fn run_child(envs: &[(&str, &str)]) {
    let exe = std::env::current_exe().expect("test binary path");
    let mut child = Command::new(exe)
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rerun with fixture probes");
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = pipe.read_to_string(&mut s);
            s
        })
    };
    let out = drain(Box::new(child.stdout.take().expect("piped")));
    let err = drain(Box::new(child.stderr.take().expect("piped")));
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().expect("wait") {
            break Some(st);
        }
        if start.elapsed() >= TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    print!("{}", out.join().unwrap_or_default());
    eprint!("{}", err.join().unwrap_or_default());
    match status {
        Some(st) => assert!(st.success(), "probe run {envs:?} failed: {st:?}"),
        None => panic!("probe run {envs:?} timed out after {TIMEOUT:?} and was killed"),
    }
}

fn main() {
    if std::env::var_os(PROBES).is_none() {
        run_child(&[(PROBES, "1")]);
        run_child(&[(PROBES, "1"), (MANY_VAR, "5")]);
        run_child(&[(PROBES, "1"), (MANY_VAR, &MANY.to_string())]);
        return;
    }
    match std::env::var(MANY_VAR).ok().and_then(|v| v.parse::<usize>().ok()) {
        Some(MANY) => {
            a_large_plugin_is_sampled_lazily_within_the_budget();
            println!("a_large_plugin_is_sampled_lazily_within_the_budget: ok");
        }
        Some(_) => {
            a_300_step_parameter_round_trips_every_step();
            println!("a_300_step_parameter_round_trips_every_step: ok");
        }
        None => {
            automation_never_calls_the_controller_on_the_render_thread();
            println!("automation_never_calls_the_controller_on_the_render_thread: ok");
            a_restart_re_samples_the_curve_on_the_ui_thread();
            println!("a_restart_re_samples_the_curve_on_the_ui_thread: ok");
            an_unsampled_curve_is_linear_until_sampled_then_re_sent();
            println!("an_unsampled_curve_is_linear_until_sampled_then_re_sent: ok");
            an_offline_render_converts_through_the_curve_without_idle();
            println!("an_offline_render_converts_through_the_curve_without_idle: ok");
        }
    }
}
