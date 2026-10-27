//! ADR 0018 (issue #36, K10): the app's frame loop keeps running while a slow plug-in is made on
//! the plug-in host thread, the slot reads "Loading…" until it is ready, and the instance then
//! plays. The plug-in is the VST3 test fixture's synth built with `slow-create`, set to 1 s in
//! `createInstance` (`VST3_FIXTURE_CREATE_MS`), loaded through the real Player and plug-in host.
//!
//! Where the default host runs inline (macOS, ADR 0018 section 7) creation stays on the UI
//! thread, so the test returns at once there (`transit_plugin_host::INLINE_BY_DEFAULT`, the
//! runtime's own predicate).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Services, StudioApp};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Builds the VST3 fixture with `slow-create`, installs it as a bundle and adds its folder to the
/// VST3 search path. Returns the synth's id.
fn slow_synth() -> String {
    transit_vst3_host::set_scan_extra_dirs_only(true);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vst3-host/tests/fixture/Cargo.toml");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .join("tmp/studio-vst3-slow-fixture-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .args(["--features", "slow-create"])
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("run cargo");
    assert!(st.success(), "building the VST3 fixture failed");
    let (lib, folder, exe) = if cfg!(windows) {
        ("vst3_fixture.dll", "x86_64-win", "Fixture.vst3")
    } else {
        ("libvst3_fixture.so", if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "Fixture.so")
    };
    let dir = std::env::temp_dir().join(format!("studio-vst3-slow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("Fixture.vst3/Contents").join(folder);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(target.join("debug").join(lib), bin.join(exe)).unwrap();
    assert_eq!(transit_vst3_host::add_search_dir(&dir), 3, "the fixture's plug-ins");
    transit_vst3_host::scan_paths(&[dir]).into_iter().find(|d| d.name == "Fixture Synth").expect("the fixture's synth").id
}

/// The fixture synth's creation time in the child. A frame that waited for it takes all of it,
/// against a 400 ms frame budget: a 2.5:1 margin for a starved runner (issue #56).
const SLOW: Duration = Duration::from_secs(1);

/// One frame of the app's per-frame logic.
fn frame(app: &mut StudioApp, ctx: &egui::Context) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.logic(ui.ctx()));
    out.textures_delta.clear();
}

#[test]
fn frames_advance_while_a_slow_plug_in_loads_and_the_slot_shows_loading_then_ready() {
    if transit_plugin_host::INLINE_BY_DEFAULT {
        return;
    }
    // The host registry is process-global: run in a child so no other test sees the fixture.
    if std::env::var_os("TRANSIT_PLUGIN_HOST_FIXTURE_CHILD").is_none() {
        let name = "plugin_host_tests::frames_advance_while_a_slow_plug_in_loads_and_the_slot_shows_loading_then_ready";
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("TRANSIT_PLUGIN_HOST_FIXTURE_CHILD", "1")
            .env("VST3_FIXTURE_CREATE_MS", SLOW.as_millis().to_string())
            .output()
            .unwrap();
        let (out, err) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{out}{err}");
        assert!(out.contains("1 passed"), "the child ran no test: {out}");
        return;
    }
    let synth = slow_synth();
    let mut engine = transit_engine::Engine::default();
    let track = engine.session_mut().add_track(transit_model::TrackKind::Instrument, transit_model::ChannelFormat::Stereo, Some("Slow"));
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    let ctx = egui::Context::default();
    for _ in 0..3 {
        app.player.as_ref().unwrap().step_device_free();
        frame(&mut app, &ctx);
    }
    let slot = transit_mix::INSTRUMENT_SLOT;

    // Add the slow instrument the way the UI does, then keep running frames.
    app.set_instrument(track, &synth).unwrap();
    let started = Instant::now();
    let (mut frames, mut loading_frames, mut worst) = (0u32, 0u32, Duration::ZERO);
    let mut seen_loading = false;
    loop {
        // The device-free player's "audio callback": it adopts published plans.
        app.player.as_ref().unwrap().step_device_free();
        let t = Instant::now();
        frame(&mut app, &ctx);
        worst = worst.max(t.elapsed());
        frames += 1;
        let p = app.player.as_ref().unwrap();
        if p.loading(track, slot) {
            seen_loading = true;
            loading_frames += 1;
        } else if seen_loading {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the plug-in never finished loading: seen loading {seen_loading}, any loading {}, stuck {}, status {:?}",
            p.any_loading(),
            p.stuck(track, slot),
            app.status
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let took = started.elapsed();
    let p = app.player.as_ref().unwrap();
    assert!(seen_loading, "the slot read as loading");
    assert!(!p.stuck(track, slot), "a 1 s creation is not stuck");
    assert!(took >= SLOW, "the fixture's synth is slow to make ({took:?})");
    // Load-tolerant (#87, E49): not a frame-rate floor. A frame that waited for the plug-in takes
    // the fixture's full second and sees one poll, so more than one frame plus a stall bound on the
    // longest frame (400 ms, below the 1 s creation) is the property.
    assert!(loading_frames >= 2, "only {loading_frames} frames ran while the synth loaded ({took:?})");
    assert!(worst < Duration::from_millis(400), "a frame waited for the plug-in: {worst:?}");
    // Ready: its plan is adopted on the next blocks and it has an editor handle.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.player.as_ref().unwrap().has_editor(track, slot) && Instant::now() < deadline {
        app.player.as_ref().unwrap().step_device_free();
        frame(&mut app, &ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.player.as_ref().unwrap().has_editor(track, slot), "the instance is bound once ready");
    eprintln!("plugin_host_tests: {frames} frames, {loading_frames} while loading, worst {worst:?}, loaded in {took:?}");
    // The app's quit order: the player (and its instances) first, then plug-in hosting, which
    // runs the module exit on the host thread that entered the module (the fixture checks).
    app.player = None;
    transit_engine::shutdown_plugin_hosts();
}

/// The heavy-instrument fixture's creation time for the late-adoption test (issue #82, E48).
const HEAVY: Duration = Duration::from_millis(1500);
/// The display budget that test sets: the creation outlives it (and the request stops waiting).
const HEAVY_SLOW_AFTER: Duration = Duration::from_millis(300);

/// Issue #82 (escape E48), with the real VST3 fixture through the real Player and plug-in host:
/// a creation that outlives the display budget reads "Loading… (slow)", is never "Not
/// responding", and when it lands it is adopted (bound, with its editor handle), not discarded.
#[test]
fn a_heavy_instrument_slower_than_the_display_budget_is_adopted() {
    if transit_plugin_host::INLINE_BY_DEFAULT {
        return;
    }
    if std::env::var_os("TRANSIT_PLUGIN_HOST_FIXTURE_CHILD").is_none() {
        let name = "plugin_host_tests::a_heavy_instrument_slower_than_the_display_budget_is_adopted";
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("TRANSIT_PLUGIN_HOST_FIXTURE_CHILD", "1")
            .env("VST3_FIXTURE_CREATE_MS", HEAVY.as_millis().to_string())
            .output()
            .unwrap();
        let (out, err) = (String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{out}{err}");
        assert!(out.contains("1 passed"), "the child ran no test: {out}");
        return;
    }
    let synth = slow_synth();
    let mut engine = transit_engine::Engine::default();
    let track = engine.session_mut().add_track(transit_model::TrackKind::Instrument, transit_model::ChannelFormat::Stereo, Some("Heavy"));
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    player.set_slow_after(HEAVY_SLOW_AFTER);
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    let ctx = egui::Context::default();
    for _ in 0..3 {
        app.player.as_ref().unwrap().step_device_free();
        frame(&mut app, &ctx);
    }
    let slot = transit_mix::INSTRUMENT_SLOT;
    app.set_instrument(track, &synth).unwrap();
    let started = Instant::now();
    let (mut seen_slow, mut seen_stuck) = (false, false);
    while !app.player.as_ref().unwrap().has_editor(track, slot) {
        assert!(started.elapsed() < Duration::from_secs(30), "the heavy instrument was never adopted: status {:?}", app.status);
        app.player.as_ref().unwrap().step_device_free();
        frame(&mut app, &ctx);
        let p = app.player.as_ref().unwrap();
        seen_slow |= matches!(p.slot_load(track, slot), Some(transit_playback::SlotLoad::Slow { .. }));
        seen_stuck |= p.stuck(track, slot);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(started.elapsed() >= HEAVY, "the fixture's synth is slow to make ({:?})", started.elapsed());
    assert!(seen_slow, "it read Loading… (slow) past the display budget");
    assert!(!seen_stuck, "it never read Not responding");
    assert!(!app.player.as_ref().unwrap().any_loading());
    app.player = None;
    transit_engine::shutdown_plugin_hosts();
}
