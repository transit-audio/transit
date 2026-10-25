//! Issue #104 (escape E56): a panic on the UI thread with a plug-in loaded ends the process within
//! a bound, through the app's quit order ([`StudioApp::shut_down`]) and the bounded emergency
//! path ([`soundcraft_engine::emergency`]), instead of hanging in a plug-in's unload code.
//!
//! The plug-in is the VST3 test fixture in its unload mode (`VST3_FIXTURE_UNLOAD_WAITS_ON_WORKER`):
//! its module entry starts a worker thread, its module exit stops it, and its static destructor
//! waits, with no time limit, until the worker has stopped. On Windows the process exit kills the
//! worker before that destructor runs, so a process that exits with the module never exited hangs
//! for good, as the real instrument did. Each case runs in a child process (the plug-in registry
//! and the host thread are process-global); the parent asserts how and how fast it ends.
//!
//! Where the default host runs inline (macOS, ADR 0018 section 7) the tests return at once.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Services, StudioApp};
use soundcraft_engine::emergency::{EXIT_AFTER_SHUTDOWN, EXIT_TERMINATED, emergency_shutdown};
use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const CHILD: &str = "TRANSIT_EMERGENCY_CHILD";
const FIXTURE: &str = "TRANSIT_EMERGENCY_FIXTURE";
/// The child prints this once the plug-in is loaded, right before the simulated panic.
const READY: &str = "emergency child: plug-in loaded, panicking";

/// How long the child may take to end after its panic. A clean shutdown takes well under a
/// second, an unclean one at most the host thread's 5 s wait; a hang never ends. Above the
/// watchdog's own bound (`SHUTDOWN_BOUND`, 15 s), so an overrun shows as the watchdog's exit code.
// timing: stall-bound a ceiling for a starved runner; the property is "ends at all"
const EXIT_BOUND: Duration = Duration::from_secs(60);
/// How long the child may take to build its app and load the plug-in (not under test).
// timing: stall-bound loading only, generous for a starved runner
const LOAD_BOUND: Duration = Duration::from_secs(120);

/// Builds the VST3 fixture once per test process (not registered here: the fixture must not load
/// into this process). Returns the built binary.
fn built_fixture() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vst3-host/tests/fixture/Cargo.toml");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .join("tmp/studio-vst3-fixture-target");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let st = Command::new(cargo)
            .args(["build", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .expect("run cargo");
        assert!(st.success(), "building the VST3 fixture failed");
        let lib = if cfg!(windows) { "vst3_fixture.dll" } else { "libvst3_fixture.so" };
        target.join("debug").join(lib)
    })
}

/// In the child: installs the built fixture as a bundle in a folder of its own, adds it to the
/// search path and returns the synth's id.
fn install_fixture(built: &Path) -> String {
    soundcraft_vst3_host::set_scan_extra_dirs_only(true);
    let (folder, exe) = if cfg!(windows) {
        ("x86_64-win", "Fixture.vst3")
    } else {
        (if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "Fixture.so")
    };
    let dir = std::env::temp_dir().join(format!("studio-vst3-emergency-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("Fixture.vst3/Contents").join(folder);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(built, bin.join(exe)).unwrap();
    assert_eq!(soundcraft_vst3_host::add_search_dir(&dir), 3, "the fixture's plug-ins");
    soundcraft_vst3_host::scan_paths(&[dir]).into_iter().find(|d| d.name == "Fixture Synth").expect("the fixture's synth").id
}

fn frame(app: &mut StudioApp, ctx: &egui::Context) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.logic(ui.ctx()));
    out.textures_delta.clear();
}

/// The child: an app with the fixture synth loaded on the plug-in host thread, then a panic on
/// this thread, then `case`.
fn child(case: &str) -> ! {
    let synth = install_fixture(Path::new(&std::env::var_os(FIXTURE).expect("the built fixture")));
    let mut engine = soundcraft_engine::Engine::default();
    let track = engine.session_mut().add_track(soundcraft_model::TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, Some("Synth"));
    let player = soundcraft_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    let ctx = egui::Context::default();
    let slot = soundcraft_mix::INSTRUMENT_SLOT;
    app.set_instrument(track, &synth).unwrap();
    let started = Instant::now();
    while !app.player.as_ref().unwrap().has_editor(track, slot) {
        assert!(started.elapsed() < LOAD_BOUND, "the synth never loaded: status {:?}", app.status);
        app.player.as_ref().unwrap().step_device_free();
        frame(&mut app, &ctx);
        // timing: sleep-ok paces the frame loop while the plug-in loads; nothing is measured
        std::thread::sleep(Duration::from_millis(5));
    }
    if case == "leaked" {
        // A second instance nobody releases: its binary can never have its module exit.
        std::mem::forget(soundcraft_vst3_host::instantiate_plugin(&synth).expect("a second instance"));
    }
    println!("{READY}");
    std::io::stdout().flush().unwrap();
    // The UI thread's panic (in the app, inside the renderer), caught where `main` catches it.
    let caught = std::panic::catch_unwind(|| std::panic::panic_any("simulated: the graphics device was lost"));
    assert!(caught.is_err());
    match case {
        "shutdown" | "leaked" => emergency_shutdown("test: a panic on the UI thread", || app.shut_down()),
        // What happened before the fix: the C runtime's exit with the plug-in still loaded.
        _ => std::process::exit(101),
    }
}

/// How a child ended: its exit code, or `None` if it was still running `EXIT_BOUND` (or `wait`)
/// after it said it was ready (then it is killed), and its output.
struct Ended {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_child(test: &str, case: &str, wait: Duration) -> Ended {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(CHILD, case)
        .env(FIXTURE, built_fixture())
        .env("VST3_FIXTURE_UNLOAD_WAITS_ON_WORKER", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let out = child.stdout.take().unwrap();
    let stdout = std::thread::spawn(move || {
        let mut all = String::new();
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if line.contains(READY) {
                let _ = tx.send(());
            }
            all.push_str(&line);
            all.push('\n');
        }
        all
    });
    let mut err = child.stderr.take().unwrap();
    let stderr = std::thread::spawn(move || {
        let mut all = String::new();
        let _ = std::io::Read::read_to_string(&mut err, &mut all);
        all
    });
    let ready = rx.recv_timeout(LOAD_BOUND).is_ok();
    let deadline = Instant::now() + wait;
    let code = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code();
        }
        if !ready || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        // timing: sleep-ok polls the child's exit; the bound is the deadline above
        std::thread::sleep(Duration::from_millis(20));
    };
    let ended = Ended { code, stdout: stdout.join().unwrap(), stderr: stderr.join().unwrap() };
    assert!(ready, "the child never loaded its plug-in:\n{}\n{}", ended.stdout, ended.stderr);
    ended
}

#[test]
fn a_ui_thread_panic_with_a_plug_in_loaded_shuts_down_and_exits_within_the_bound() {
    if transit_plugin_host::INLINE_BY_DEFAULT {
        return;
    }
    let name = "emergency_tests::a_ui_thread_panic_with_a_plug_in_loaded_shuts_down_and_exits_within_the_bound";
    if let Some(case) = std::env::var_os(CHILD) {
        child(&case.to_string_lossy());
    }
    let ended = run_child(name, "shutdown", EXIT_BOUND);
    assert_eq!(ended.code, Some(EXIT_AFTER_SHUTDOWN), "the shutdown was not clean, or the exit hung (None):\n{}\n{}", ended.stdout, ended.stderr);
}

#[test]
fn a_binary_that_cannot_be_exited_is_terminated_instead_of_hanging_the_exit() {
    if transit_plugin_host::INLINE_BY_DEFAULT {
        return;
    }
    let name = "emergency_tests::a_binary_that_cannot_be_exited_is_terminated_instead_of_hanging_the_exit";
    if let Some(case) = std::env::var_os(CHILD) {
        child(&case.to_string_lossy());
    }
    let ended = run_child(name, "leaked", EXIT_BOUND);
    assert_eq!(
        ended.code,
        Some(EXIT_TERMINATED),
        "a live instance must end in a termination, not a hang (None):\n{}\n{}",
        ended.stdout,
        ended.stderr
    );
}

/// The fixture reproduces the defect: without the shutdown, the C runtime's exit hangs in the
/// fixture's unload code (Windows, where the exit kills the worker first). Without this, the two
/// tests above would pass on a fixture that never hangs.
#[cfg(windows)]
#[test]
fn without_the_shutdown_the_exit_hangs_in_the_plug_ins_unload() {
    if transit_plugin_host::INLINE_BY_DEFAULT {
        return;
    }
    let name = "emergency_tests::without_the_shutdown_the_exit_hangs_in_the_plug_ins_unload";
    if let Some(case) = std::env::var_os(CHILD) {
        child(&case.to_string_lossy());
    }
    // A hang never ends; a slow exit could only make this pass wrongly, never fail it.
    // timing: stall-bound the child is expected not to end at all
    let ended = run_child(name, "no-shutdown", Duration::from_secs(5));
    assert_eq!(ended.code, None, "the exit did not hang, so the fixture no longer reproduces #104:\n{}\n{}", ended.stdout, ended.stderr);
}
