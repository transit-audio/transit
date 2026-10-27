use super::*;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

fn fixture() {
    static READY: OnceLock<()> = OnceLock::new();
    READY.get_or_init(|| {
        transit_clap_host::set_scan_extra_dirs_only(true);
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../clap-host/tests/fixture/Cargo.toml");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"))
            .join("ui-track-state-fixture");
        let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args(["build", "--quiet", "--manifest-path"])
            .arg(manifest)
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTFLAGS")
            .status()
            .unwrap();
        assert!(status.success());
        let dir = target.join(format!("plugins-{}", std::process::id()));
        let dest = dir.join("Fixture.clap");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::copy(target.join("debug/clap_fixture.dll"), dest).unwrap();
        assert_eq!(transit_clap_host::add_search_dir(&dir), 2);
    });
}
fn state(gain: f64) -> Vec<u8> {
    [gain.to_le_bytes(), 0.0f64.to_le_bytes()].concat()
}
/// Frames until the player's instances are made on the plug-in host thread (ADR 0018) and
/// its plan is confirmed for the app's document, then a few more.
fn settle(app: &mut StudioApp, track: transit_model::TrackId) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut extra = 4;
    while extra > 0 {
        app.player.as_ref().unwrap().step_device_free();
        app.apply_editor_edits();
        app.poll_player();
        app.sync_player_session();
        let p = app.player.as_ref().unwrap();
        if !p.any_loading() && p.has_editor(track, 0) && p.capture_states(&app.engine.session_arc()).is_ok() {
            extra -= 1;
        }
        assert!(std::time::Instant::now() < deadline, "the fixture never loaded");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
#[test]
fn audio_reopen_preserves_live_state_and_refuses_pending_document_without_teardown() {
    // The host registry is process-global. Keep this real CLAP fixture out
    // of the other UI tests' deliberately empty plugin catalog.
    if std::env::var_os("TRANSIT_REOPEN_FIXTURE_CHILD").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "host::player_tests::reopen_state::audio_reopen_preserves_live_state_and_refuses_pending_document_without_teardown",
                "--nocapture",
            ])
            .env("TRANSIT_REOPEN_FIXTURE_CHILD", "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        // The filter must name the test exactly, or the child runs nothing and passes.
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the child ran no test:
{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    fixture();
    let mut engine = transit_engine::Engine::default();
    let session = engine.session_mut();
    let track = session.add_track(transit_model::TrackKind::Audio, transit_model::ChannelFormat::Stereo, Some("State"));
    let mut insert = transit_model::Insert::new("clap:org.soundcraft.test.gain");
    insert.set_state_bytes(&state(1.0));
    session.track_mut(track).unwrap().mixer.inserts[0] = Some(insert);
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    settle(&mut app, track);
    let player = app.player.as_ref().unwrap();
    // The fixture's window-free GUI edits gain when it opens; the edit reaches the plug-in's
    // state with the next block. The open runs on the plug-in host thread (ADR 0018), so
    // render until the state read through that thread shows it.
    player.open_editor(track, 0).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let before = loop {
        let player = app.player.as_ref().unwrap();
        player.step_device_free();
        let now = player.capture_states(&app.engine.session_arc()).unwrap()[0].2.clone();
        if now == state(0.25) || std::time::Instant::now() > deadline {
            break now;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(before, state(0.25));
    assert_eq!(app.engine.session().track(track).unwrap().mixer.inserts[0].as_ref().unwrap().state_bytes().unwrap(), state(1.0));
    assert!(app.reconnect_audio_using(true, |session, _| transit_playback::Player::prepare_device_free(session).unwrap()).unwrap());
    settle(&mut app, track);
    let after = app.player.as_ref().unwrap().capture_states(&app.engine.session_arc()).unwrap()[0].2.clone();
    assert_eq!(after, before, "Apply audio discarded unsaved native plug-in state");
    assert!(app.reconnect_audio_using(true, |session, _| transit_playback::Player::prepare_device_free(session).unwrap()).unwrap());
    settle(&mut app, track);
    let confirmed = app.engine.session_arc();
    assert_eq!(app.player.as_ref().unwrap().capture_states(&confirmed).unwrap()[0].2, before);
    app.engine.session_mut().track_mut(track).unwrap().name = "Pending".into();
    let old_config = app.ui.audio.clone();
    let changed = transit_playback::AudioConfig { buffer_size: Some(128), ..old_config.clone() };
    assert!(app.apply_audio(changed).is_err());
    assert_eq!(app.ui.audio, old_config);
    assert!(app.reconnect_audio_using(true, |_, _| panic!("pending document must not tear down the player")).is_err());
    assert!(app.player.as_ref().unwrap().has_requested_session(&confirmed));
}
