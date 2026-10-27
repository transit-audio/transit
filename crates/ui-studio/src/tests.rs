pub(crate) use crate::mixer::RoutingChoice;
pub(crate) use crate::{Services, StudioApp};
pub(crate) use serde_json::{Value, json};
pub(crate) use transit_model::{ClipId, TrackId};

pub(crate) fn app() -> StudioApp {
    StudioApp::new(transit_engine::Engine::default(), None, Services::default())
}

/// An instrument track with a one-bar MIDI clip at bar 1; returns (track, clip, bar length).
pub(crate) fn track_with_clip(app: &mut StudioApp) -> (TrackId, ClipId, i64) {
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    let c = app.run("midi.new_clip", json!({"tracks": [t.0], "start": 0, "end": 0})).unwrap();
    let bar = c["end"].as_i64().unwrap() - c["start"].as_i64().unwrap();
    (t, ClipId(c["clip"].as_u64().unwrap()), bar)
}

pub(crate) fn clips_on(app: &StudioApp, t: TrackId) -> Vec<(u64, i64, i64)> {
    app.engine.session().track(t).unwrap().clips().iter().map(|c| (c.id.0, c.start, c.end())).collect()
}

#[test]
fn play_starts_at_the_cursor_and_stop_returns_there() {
    let mut app = app();
    app.set_cursor(48_000, None);
    app.run("transport.play", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(app.is_playing());
    assert_eq!(app.position(), 48_000 + 12_000);
    app.run("transport.toggle", json!({})).unwrap();
    app.handle_transport(0.0);
    assert!(!app.is_playing());
    assert_eq!(app.position(), 48_000);
    app.run("transport.rtz", json!({})).unwrap();
    app.handle_transport(0.0);
    assert_eq!(app.cursor(), 0);
}

#[test]
fn new_session_stops_playback() {
    let mut app = StudioApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    app.run("transport.play", json!({"from": 48_000})).unwrap();
    app.handle_transport(0.25);
    assert!(app.is_playing());
    app.run("session.new", json!({})).unwrap();
    app.handle_transport(0.25);
    assert!(!app.is_playing());
    app.handle_transport(0.25);
    assert_eq!(app.engine.transport.position, app.cursor(), "the old clock stays stopped");
}

#[test]
fn ui_set_lists_editors_without_a_player() {
    let mut app = app();
    let v = app.run("ui.set", json!({"plugin_windows": [[3, usize::MAX]]})).unwrap();
    assert_eq!(v["ui"]["plugin_windows"][0][0], json!(3));
    assert!(app.run("ui.set", json!({"plugin_dirs": {"vst3": ["x"]}})).is_err());
}

#[test]
fn inspect_reports_the_plugin_scan_without_starting_one() {
    // Tests never scan: nothing starts one, so the state is "idle" with no counts.
    let scan = crate::plugin_scan::inspect();
    assert_eq!(scan["state"], json!("idle"));
    assert!(scan["vst3"].is_null() && scan["clap"].is_null());
}

/// Guard: the host registries are process-global, so a test that registers plug-in folders or
/// runs a scan in this process leaks into every other test (and a scan reads the machine's
/// installed plug-ins). Such tests must run in a child process (see `plugin_requests_tests`).
/// With no scan started and no folders registered, no test here can see a real plug-in library.
#[test]
fn the_shared_host_registries_hold_no_scan_folders_or_results() {
    assert!(transit_clap_host::cached().is_none() && transit_vst3_host::cached().is_none(), "a test scanned in the shared process");
    assert!(transit_clap_host::search_dirs().1.is_empty(), "a test registered a CLAP folder in the shared process");
    assert!(transit_vst3_host::search_dirs().1.is_empty(), "a test registered a VST3 folder in the shared process");
}

/// Run frames of the real UI, feeding `app.synthetic` like the control channel does.
pub(crate) fn frames(app: &mut StudioApp, ctx: &egui::Context, n: usize, t0: &mut f64) {
    for _ in 0..n {
        *t0 += 1.0 / 60.0;
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
            time: Some(*t0),
            ..Default::default()
        };
        app.raw_input_hook(&mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
}

pub(crate) fn drag(app: &mut StudioApp, from: (f32, f32), to: (f32, f32), mods: Value) {
    let mut p = json!({"x": from.0, "y": from.1, "to_x": to.0, "to_y": to.1});
    if let (Some(o), Some(m)) = (p.as_object_mut(), mods.as_object()) {
        o.extend(m.clone());
    }
    let (req, _rx) = crate::ControlRequest::new("ui.drag", p);
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

/// A bar-long clip with a second track (at the default zoom the visible grid is a beat); returns what a drag needs.
pub(crate) struct CopyRig {
    pub(crate) app: StudioApp,
    pub(crate) ctx: egui::Context,
    pub(crate) time: f64,
    pub(crate) t: TrackId,
    pub(crate) t2: TrackId,
    pub(crate) grab: (f32, f32),
    pub(crate) lane2_y: f32,
    pub(crate) px_per_beat: f32,
    pub(crate) beat: i64,
}

pub(crate) fn copy_rig() -> CopyRig {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let layout = app.arrange.layout.clone();
    let rect = layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    let lane2 = layout.tracks.iter().find(|r| r.id == t2.0).unwrap().lane;
    let beat = bar / 4;
    let s = app.engine.session();
    assert_eq!(
        crate::automation::grid_ticks(s, layout.samples_per_px, &app.arrange.grid),
        transit_time::TICKS_PER_QUARTER,
        "the visible grid is a beat"
    );
    CopyRig {
        app,
        ctx,
        time,
        t,
        t2,
        grab: (rect[0] + 20.0, (rect[1] + rect[3]) / 2.0),
        lane2_y: (lane2[1] + lane2[3]) / 2.0,
        px_per_beat: (beat as f64 / layout.samples_per_px) as f32,
        beat,
    }
}

// ---- Slice 2: mixer and device chain ----

pub(crate) fn undo_len(app: &mut StudioApp) -> usize {
    app.engine.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len()
}

pub(crate) fn click(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

pub(crate) fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// An app with the mixer pane open and a stereo audio track; returns (app, ctx, time, track).
pub(crate) fn mixer_app() -> (StudioApp, egui::Context, f64, TrackId) {
    let mut app = app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Gtr"})).unwrap();
    let t = TrackId(r["tracks"][0].as_u64().unwrap());
    app.ui.show_mixer = true;
    app.ui.show_devices = true;
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time, t)
}

pub(crate) fn strip(app: &StudioApp, t: TrackId) -> crate::mixer::StripLayout {
    app.mixer.layout.strips.iter().find(|s| s.track == t.0).cloned().unwrap()
}

// --- Slice 2b: instrument menu, sends, routing loops, review follow-ups ---

pub(crate) fn new_track(app: &mut StudioApp, p: Value) -> TrackId {
    TrackId(app.run("track.new", p).unwrap()["tracks"][0].as_u64().unwrap())
}

pub(crate) fn right_click(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

pub(crate) fn click_right(app: &mut StudioApp, at: (f32, f32)) {
    let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

pub(crate) fn send_control(app: &mut StudioApp, id: &str, p: Value) {
    let (req, _rx) = crate::ControlRequest::new(id, p);
    let ctx = egui::Context::default();
    let _ = crate::control::handle_for_test(app, &ctx, &req);
}

pub(crate) fn click_with(app: &mut StudioApp, at: (f32, f32), extra: Value) {
    let mut p = json!({"x": at.0, "y": at.1});
    if let (Some(o), Some(m)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(m.clone());
    }
    send_control(app, "ui.click", p);
}

pub(crate) fn key(app: &mut StudioApp, name: &str) {
    send_control(app, "ui.key", json!({"key": name}));
}

pub(crate) fn typed(app: &mut StudioApp, text: &str) {
    send_control(app, "ui.text", json!({"text": text}));
}

pub(crate) fn input_label(app: &StudioApp, t: TrackId) -> String {
    let s = app.engine.session();
    let tr = s.track(t).unwrap();
    crate::mixer::route_label(s, &tr.mixer.input, true, tr.channels())
}

pub(crate) fn new_audio(app: &mut StudioApp, r: Result<Value, String>) -> TrackId {
    let t = TrackId(r.unwrap()["tracks"][0].as_u64().unwrap());
    assert_eq!(app.cursor_track(), Some(t), "the new track is the cursor's track");
    t
}

/// One beat at the default tempo and rate; the grid at the default zoom is a beat.
pub(crate) fn beat(app: &StudioApp) -> i64 {
    let s = app.engine.session();
    s.tempo.tick_to_samples(transit_time::TICKS_PER_QUARTER, s.sample_rate)
}

pub(crate) fn spans(app: &StudioApp, t: TrackId) -> Vec<(i64, i64)> {
    clips_on(app, t).iter().map(|c| (c.1, c.2)).collect()
}

pub(crate) fn the_clip(app: &StudioApp, id: ClipId) -> transit_model::Clip {
    app.engine.session().find_clip(id).unwrap().1.clone()
}

#[test]
fn a_refused_tempo_change_says_why_in_the_status_line() {
    use transit_model::{ChannelFormat, Clip, ClipLoop, TrackKind};
    let mut app = app();
    let s = app.engine.session_mut();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "tiny", 0, 1000, Default::default());
    c.looping = Some(ClipLoop { start: 0, end: 100 });
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
    assert!(!app.set_tempo_at(0, 240.0, None));
    assert!(app.status.contains("tiny") && app.status.contains("shorten the loop or clear it before changing tempo"), "{}", app.status);
    assert_eq!(app.engine.session().tempo.tempos()[0].bpm, 120.0);
}
