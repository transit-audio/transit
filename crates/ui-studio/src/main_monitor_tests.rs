//! The main bus monitoring set on the strip (#366): M, Mono, Dim and Ref in the 2 by 2 grid, the
//! subtitle under the name, the output label as the path name, the loudness footer; the
//! `mix.monitor` command and its state in `ui.inspect`; the toggles reaching the Player and the
//! readout following its live loudness. Each check has a negative control.

use crate::tests::*;
use crate::widgets::toggle::{Role, colors};

fn master(app: &mut StudioApp) -> TrackId {
    new_track(app, json!({"kind": "master"}))
}

fn main_strip(app: &StudioApp) -> crate::mixer::StripLayout {
    app.mixer.layout.master.clone().expect("the main bus")
}

fn toggle<'a>(s: &'a crate::mixer::StripLayout, text: &str) -> &'a crate::widgets::toggle::ToggleLayout {
    s.toggles.iter().find(|t| t.text == text).unwrap_or_else(|| panic!("no {text} toggle"))
}

fn alt_click(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "alt": true}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

#[test]
fn the_main_bus_has_m_mono_dim_ref_the_subtitle_the_path_as_output_and_the_loudness_footer() {
    let (mut app, ctx, mut time, a) = mixer_app();
    master(&mut app);
    for full in [true, false] {
        if full {
            full_mixer(&mut app, &ctx, &mut time);
        } else {
            app.ui.mixer_full = false;
            frames(&mut app, &ctx, 4, &mut time);
        }
        let m = main_strip(&app);
        assert_eq!(m.toggles.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["M", "Mono", "Dim", "Ref"], "the board's grid");
        // The grid: M top left, Mono top right, Dim bottom left, Ref bottom right.
        assert!(m.mono[1] == m.mute[1] && m.mono[0] > m.mute[0], "Mono beside M");
        assert!(m.dim[0] == m.mute[0] && m.dim[1] > m.mute[1], "Dim under M");
        assert!(m.reference[0] == m.mono[0] && m.reference[1] == m.dim[1], "Ref under Mono");
        assert_eq!(m.subtitle, "Out 1-2 · no device", "the path Main plays on; no device in a test");
        assert_eq!(m.subtitle_rect, m.input, "in the input row's place");
        assert_eq!(m.output_label, "Out 1-2", "the output is the path name, not \"Main\"");
        assert_eq!(m.lufs_text, "—", "nothing measured");
        assert!(m.lufs_rect[1] >= m.readout_rect[3] && m.lufs_rect[3] <= m.rect[3], "the footer under the readouts, inside the strip");
        assert!(m.fader[3] < m.readout_rect[1], "the fader keeps clear of the readouts");
        // Negative control: a channel has none of it.
        let s = strip(&app, a);
        assert_eq!((s.mono, s.dim, s.reference, s.lufs_rect, s.subtitle_rect), ([0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4]));
        assert!(s.subtitle.is_empty() && s.lufs_text.is_empty());
        assert_eq!(s.toggles.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["M", "S", "R", "I"]);
    }
}

#[test]
fn the_toggles_run_mix_monitor_without_an_undo_step_and_alt_click_stores_the_reference() {
    let (mut app, ctx, mut time, _) = mixer_app();
    master(&mut app);
    full_mixer(&mut app, &ctx, &mut time);
    let undo = undo_len(&mut app);
    let clean = !app.engine.is_dirty();
    let m = main_strip(&app);
    assert!(m.toggles.iter().all(|t| !t.on), "all off at first");
    for (name, role) in [("Mono", Role::Monitor), ("Dim", Role::Mute)] {
        let r = match name {
            "Mono" => m.mono,
            _ => m.dim,
        };
        click(&mut app, mid(r));
        frames(&mut app, &ctx, 5, &mut time);
        let lay = main_strip(&app);
        let t = toggle(&lay, name);
        assert!(t.on, "{name} latches");
        assert_eq!(t.fill, colors(role, true, true).0, "{name}: the board's role");
    }
    assert!(app.ui.monitor.mono && app.ui.monitor.dim);
    // Alt+click on Ref stores the monitor level as the reference and leaves Ref off.
    app.run("mix.monitor", json!({"level_db": -6.0})).unwrap();
    alt_click(&mut app, mid(m.reference));
    frames(&mut app, &ctx, 5, &mut time);
    assert_eq!((app.ui.monitor.reference_db, app.ui.monitor.reference), (-6.0, false), "stored, not switched");
    // Control: a plain click switches Ref and stores nothing.
    app.run("mix.monitor", json!({"level_db": -3.0})).unwrap();
    click(&mut app, mid(m.reference));
    frames(&mut app, &ctx, 5, &mut time);
    assert_eq!((app.ui.monitor.reference_db, app.ui.monitor.reference), (-6.0, true));
    assert_eq!(undo_len(&mut app), undo, "monitor state is never an undo step");
    assert_eq!(!app.engine.is_dirty(), clean, "nor a session edit");
    // A second click releases a toggle.
    click(&mut app, mid(m.dim));
    frames(&mut app, &ctx, 5, &mut time);
    assert!(!app.ui.monitor.dim && !toggle(&main_strip(&app), "Dim").on);
    // Persisted per machine with the preferences.
    let saved = serde_json::to_value(&app.ui).unwrap();
    assert_eq!(saved["monitor"], json!({"level_db": -3.0, "reference_db": -6.0, "mono": true, "dim": false, "reference": true}));
}

#[test]
fn mix_monitor_checks_its_fields_and_ui_inspect_shows_the_state() {
    let (mut app, ctx, mut time, _) = mixer_app();
    let r = app.run("mix.monitor", json!({"mono": true, "level_db": "-inf", "store_ref": true})).unwrap();
    assert_eq!((r["mono"].as_bool(), r["level_db"].as_f64(), r["ref_db"].as_f64()), (Some(true), Some(-144.0), Some(-144.0)));
    assert_eq!(r["lufs"], Value::Null);
    assert_eq!(r["lufs_text"], "—");
    for bad in [json!({"mono": 1}), json!({"level_db": 3.0}), json!({"level_db": "loud"}), json!({"store_ref": "yes"})] {
        let before = app.ui.monitor;
        assert!(app.run("mix.monitor", bad.clone()).is_err(), "{bad} is refused");
        assert_eq!(app.ui.monitor, before, "{bad} changes nothing");
    }
    frames(&mut app, &ctx, 5, &mut time);
    let v = app.inspect(&ctx);
    assert_eq!(v["monitor"]["mono"], true);
    assert_eq!(v["monitor"]["subtitle"], "Out 1-2 · no device");
    // The command is listed for agents and the palette.
    assert!(transit_engine::cmd::command_specs().iter().any(|c| c.id == "mix.monitor"));
}

#[test]
fn the_main_out_strip_without_a_master_track_has_the_monitoring_set_too() {
    let (mut app, ctx, mut time, _) = mixer_app();
    full_mixer(&mut app, &ctx, &mut time);
    let m = main_strip(&app);
    assert_eq!(m.track, 0, "no master track");
    assert_eq!(
        m.toggles.iter().map(|t| (t.text.as_str(), t.available)).collect::<Vec<_>>(),
        [("M", false), ("Mono", true), ("Dim", true), ("Ref", true)]
    );
    assert_eq!(m.subtitle, "Out 1-2 · no device");
    assert_eq!(m.lufs_text, "—");
    click(&mut app, mid(m.mono));
    frames(&mut app, &ctx, 5, &mut time);
    assert!(app.ui.monitor.mono, "Mono works without a master track");
    // Control: the ghosted M does nothing.
    click(&mut app, mid(m.mute));
    frames(&mut app, &ctx, 5, &mut time);
    assert!(app.engine.session().tracks.iter().all(|t| t.kind != transit_model::TrackKind::Master));
}

/// A session with a stereo −20 dBFS 997 Hz tone for `seconds`, and a master track.
fn tone_app(seconds: usize) -> (StudioApp, egui::Context, f64) {
    use transit_model::{Clip, SourceAudio, SourceId};
    let (mut app, ctx, mut time, t) = mixer_app();
    master(&mut app);
    {
        let s = app.engine.session_mut();
        let len = 48_000 * seconds;
        let amp = 10f32.powf(-1.0);
        let x: Vec<f32> = (0..len).map(|i| amp * (std::f32::consts::TAU * 997.0 * i as f32 / 48_000.0).sin()).collect();
        // The audio buffer type is not a dependency here: take one from the demo and refill it.
        let demo = transit_engine::demo::demo_session();
        let any = demo.pool.ids().next().unwrap();
        let mut buffer = demo.pool.get(any).unwrap().buffer.clone();
        buffer.sample_rate = 48_000;
        buffer.channels = vec![x.clone(), x];
        s.pool.insert(SourceId(77_001), std::sync::Arc::new(SourceAudio::new(buffer)));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "tone", SourceId(77_001), 0, 0, len as i64));
    }
    app.player = Some(transit_playback::Player::prepare_device_free(app.engine.session_arc()).unwrap());
    full_mixer(&mut app, &ctx, &mut time);
    (app, ctx, time)
}

fn step(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, blocks: usize) {
    for _ in 0..blocks {
        app.player.as_ref().unwrap().step_device_free();
    }
    frames(app, ctx, 1, time);
}

#[test]
fn the_toggles_reach_the_player_and_the_footer_follows_the_live_loudness() {
    let (mut app, ctx, mut time) = tone_app(4);
    let gain = |app: &StudioApp| app.player.as_ref().unwrap().monitor().target_gain();
    assert_eq!(gain(&app), 1.0, "unity at first");
    let m = main_strip(&app);
    click(&mut app, mid(m.dim));
    frames(&mut app, &ctx, 5, &mut time);
    assert!((gain(&app) - 0.1).abs() < 1e-6, "Dim reached the player: {}", gain(&app));
    click(&mut app, mid(m.dim));
    frames(&mut app, &ctx, 5, &mut time);
    assert_eq!(gain(&app), 1.0, "and its release");
    // Play two seconds of the tone: the footer reads the mix's loudness.
    app.run("transport.play", json!({})).unwrap();
    for _ in 0..20 {
        step(&mut app, &ctx, &mut time, 1);
        if app.is_playing() {
            break;
        }
    }
    assert!(app.is_playing(), "{}", app.status);
    step(&mut app, &ctx, &mut time, 190);
    let text = main_strip(&app).lufs_text;
    assert_eq!(text, "\u{2212}20.0 LUFS", "a -20 dBFS sine through a unity master");
    // A click on the readout resets it.
    let readout = main_strip(&app).lufs_rect;
    click(&mut app, mid(readout));
    frames(&mut app, &ctx, 5, &mut time);
    assert_eq!(main_strip(&app).lufs_text, "—", "reset");
    // Control: the measurement resumes.
    step(&mut app, &ctx, &mut time, 60);
    assert_ne!(main_strip(&app).lufs_text, "—");
}
