#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MIDI Learn onto a hosted VST3 instrument's parameter (issue #97 slice 2, ADR 0028), with a
//! real plug-in: the vst3-host fixture's events synth, whose processor writes every parameter
//! point it receives into its output and whose controller (its editor's side) reports the value
//! the host last gave it. Learn next touched parameter, a controller move, and then the plug-in
//! must get the value with the parameter's own id, normalized, and its controller must hold it:
//! the move reaches the plug-in, not only the session (before the fix `mix.instrument_param`
//! refused every hosted instrument, so a mapped move changed nothing).
//!
//! The host registries are process-global, so this runs in a child process (see
//! `the_shared_host_registries_hold_no_scan_folders_or_results`). Its table is cached by making
//! one instance before the app looks it up, so nothing waits on a scan.
use crate::{Services, StudioApp};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use transit_dsp::HeaderAction;
use transit_mix::INSTRUMENT_SLOT;
use transit_model::{SourceKind, TrackId};
use transit_playback::MapMsg;
use transit_time::Range;

/// The events synth's one parameter (VST3 ParamID 5, normalized 0..1).
const PROBE: &str = "5";

/// Builds the vst3-host fixture, installs it as a bundle in a folder of its own and adds that
/// folder (only) to the VST3 search path. The id of "Fixture Events".
fn events_synth() -> String {
    transit_vst3_host::set_scan_extra_dirs_only(true);
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vst3-host/tests/fixture/Cargo.toml");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
        .join("ui-learn-vst3-fixture");
    let st = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .unwrap();
    assert!(st.success(), "building the VST3 fixture failed");
    let (lib, folder, exe) = if cfg!(target_os = "macos") {
        ("libvst3_fixture.dylib", "MacOS", "Fixture")
    } else if cfg!(windows) {
        ("vst3_fixture.dll", "x86_64-win", "Fixture.vst3")
    } else {
        ("libvst3_fixture.so", if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, "Fixture.so")
    };
    let dir = target.join(format!("plugins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let bin = dir.join("Fixture.vst3/Contents").join(folder);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(target.join("debug").join(lib), bin.join(exe)).unwrap();
    assert_eq!(transit_vst3_host::add_search_dir(&dir), 3, "the fixture's plug-ins");
    transit_vst3_host::scan_paths(&[dir]).into_iter().find(|d| d.name == "Fixture Events").expect("the events synth").id
}

/// One frame of the host loop's parts this test needs: the player's idle (which services the
/// plug-ins and compiles the requested plan), its notices, and the session handed over.
fn step(app: &mut StudioApp) {
    app.player.as_ref().unwrap().step_device_free();
    app.apply_editor_edits();
    app.poll_player();
    app.sync_player_session();
    std::thread::sleep(Duration::from_millis(5));
}

/// The events synth's saved state is its report; `probe_ctrl` is its controller's probe value in
/// millionths.
fn controller_probe(app: &StudioApp, track: TrackId) -> Option<i64> {
    let p = app.player.as_ref()?;
    let states = p.capture_states(&app.engine.session_arc()).ok()?;
    let (_, _, blob) = states.into_iter().find(|(t, slot, _)| *t == track && *slot == INSTRUMENT_SLOT)?;
    let text = String::from_utf8_lossy(&blob).into_owned();
    let at = text.find("probe_ctrl=")? + "probe_ctrl=".len();
    text[at..].chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect::<String>().parse().ok()
}

/// The parameter points the events synth received in the first block of `out`: (offset, id,
/// value in millionths).
fn points(out: &[Vec<f32>]) -> Vec<(u32, u32, i64)> {
    out[0].chunks(4).take_while(|r| r[0] != 0.0).filter(|r| r[0] == 4.0).map(|r| (r[1] as u32, r[2] as u32, r[3] as i64)).collect()
}

#[test]
fn a_learned_controller_moves_a_hosted_instruments_parameter_in_the_plug_in_and_its_editor() {
    if std::env::var_os("TRANSIT_LEARN_VST3_CHILD").is_none() {
        let name = "midi_learn_hosted_tests::a_learned_controller_moves_a_hosted_instruments_parameter_in_the_plug_in_and_its_editor";
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("TRANSIT_LEARN_VST3_CHILD", "1")
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{text}");
        // E46: the filter must name the test exactly, or the child runs nothing and passes.
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"), "the child ran no test:\n{text}");
        return;
    }
    let id = events_synth();
    // Seed the parameter table: one instance of the type caches it, as a plug-in loaded once this
    // run has (K81).
    let info = transit_vst3_host::plugin_info(&id).expect("the events synth's table");
    assert!(info.param(PROBE).is_some());
    let engine = transit_engine::Engine::default();
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let tr = TrackId(r["tracks"][0].as_u64().unwrap());
    app.run("mix.instrument", json!({"track": tr.0, "plugin": id})).unwrap();
    // Learn next touched parameter from the instrument's editor header; the editor reports the
    // probe (its VST3 ParamID as the host's id), then a controller message comes.
    app.ui.plugin_windows.push((tr, INSTRUMENT_SLOT));
    app.header_action(tr, INSTRUMENT_SLOT, HeaderAction::LearnNext);
    app.apply_edits(vec![(tr, INSTRUMENT_SLOT, PROBE.into(), 0.25)]);
    let want = format!("track:{}:plugin:instrument:{PROBE}", tr.0);
    assert_eq!(app.engine.learn().expect("armed").target.to_string(), want);
    let ins = |app: &StudioApp| app.engine.session().track(tr).unwrap().instrument.clone().unwrap();
    assert_eq!(ins(&app).params.get(PROBE), Some(&0.25), "a hosted instrument's own edit is stored, like an insert's (K83)");
    app.midi_learn_apply((0, vec![MapMsg::Candidate { port: "KeyStep".into(), channel: 0, kind: SourceKind::Cc, number: 74 }]), 0.0);
    assert_eq!(app.engine.session().midi_mappings.iter().next().unwrap().target.to_string(), want);
    assert!(app.status.starts_with("Mapped Instrument 5 "), "{}", app.status);
    // The mapping is in force (its device is live); the controller moves to 0.8.
    let _ = app.midi_learn_feed(1.0);
    assert_eq!(app.midi_learn.resolved_len(), 1);
    let epoch = app.player.as_ref().unwrap().midi_map_epoch();
    app.midi_learn_apply((epoch, vec![MapMsg::Move { epoch, index: 0, value: 0.8, seq: 1 }]), 1.0);
    assert_eq!(ins(&app).params.get(PROBE), Some(&0.8), "the move reaches the session (status: {})", app.status);
    // The plug-in's processor receives the value with its own id, normalized.
    let out = transit_mix::render_track_pre_fader(app.engine.session(), tr, Range::new(0, 512));
    assert!(points(&out).contains(&(0, 5, 800_000)), "the processor got ParamID 5 = 0.8: {:?}", points(&out));
    // The player's running instance gets it too: its controller (the editor's side) ends with
    // 0.8. (That the frame loop tells the controller between saves, not only a save's handover,
    // is vst3-host's `a_host_set_value_reaches_the_processor_and_then_the_editor`.)
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        step(&mut app);
        if controller_probe(&app, tr) == Some(800_000) {
            break;
        }
        assert!(Instant::now() < deadline, "the editor never saw 0.8: {:?} (status: {})", controller_probe(&app, tr), app.status);
    }
}
