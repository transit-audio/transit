//! User gestures for printing and recovering hidden tracks, with no audio device.
use crate::bounce_in_place::Source;
use crate::{Dialog, Services, StudioApp};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use transit_model::TrackId;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

struct Count {
    inner: Box<dyn transit_dsp::Plugin>,
    calls: Arc<AtomicUsize>,
}

impl transit_dsp::Plugin for Count {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, block: usize, ch: usize) {
        self.inner.prepare(sr, block, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.process(io, frames);
    }
}

/// Count the source's first insert through a live plan, with no device or worker scheduling.
fn process_calls(s: &transit_model::Session, source: TrackId) -> usize {
    use transit_mix::{DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanLimits, RenderTarget, SlotGen};
    MixEngine::force_serial(true);
    let calls = Arc::new(AtomicUsize::new(0));
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: 256, out_channels: 0, in_channels: 0 };
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| {
        let mut instance = transit_mix::create_instance(&spec.id, spec.state.as_deref(), 48_000.0, 256, spec.ch)?;
        if spec.track == source && spec.slot == 0 {
            instance.plugin = Box::new(Count { inner: instance.plugin, calls: calls.clone() });
        }
        Some(instance)
    };
    let mut env = comp.compile(Arc::new(s.clone()), comp.active(), comp.active().checked_next().unwrap(), &mut make).unwrap();
    let mut mix = MixEngine::realtime(target);
    assert_eq!(mix.adopt_plan(&mut env), transit_mix::Adoption::Adopted);
    comp.reclaim(env);
    mix.render_active(1200, 256, &mut vec![vec![0.0; 256]; 2]);
    drop(mix.into_retired());
    calls.load(Ordering::Relaxed)
}

impl T {
    fn new() -> Self {
        let mut t =
            Self { app: StudioApp::new(transit_engine::Engine::default(), None, Services::default()), ctx: egui::Context::default(), time: 0.0 };
        t.frames();
        t
    }
    fn frames(&mut self) {
        for _ in 0..5 {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let mut output = self.ctx.run_ui(raw, |ui| {
                self.app.logic(ui.ctx());
                self.app.ui(ui);
            });
            output.textures_delta.clear();
        }
    }
    fn input(&mut self, method: &str, params: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, params);
        let _ = crate::control::handle_for_test(&mut self.app, &self.ctx, &req);
        // Drain queued press and release frames, as the existing gesture harness does.
        for _ in 0..6 {
            self.frames();
        }
    }
    fn key(&mut self, key: &str) {
        self.input("ui.key", json!({"key": key}));
    }
    fn click(&mut self, r: [f32; 4]) {
        self.input("ui.click", json!({"x": (r[0]+r[2])/2.0, "y": (r[1]+r[3])/2.0}));
    }
    fn right_click(&mut self, r: [f32; 4]) {
        self.input("ui.click", json!({"x": (r[0]+r[2])/2.0, "y": (r[1]+r[3])/2.0, "button": "right"}));
    }
    fn menu(&mut self, name: &str) {
        let rect = self.app.arrange.bar_menu.iter().find(|m| m.label == name).unwrap().rect;
        self.click(rect);
    }
    fn track(&mut self, name: &str, kind: &str) -> TrackId {
        let id = TrackId(self.app.run("track.new", json!({"kind": kind, "name": name})).unwrap()["tracks"][0].as_u64().unwrap());
        self.frames();
        id
    }
    fn source(&mut self) -> TrackId {
        let id = self.track("Keys", "instrument");
        self.app.run("midi.new_clip", json!({"track": id.0, "start": 1200, "length": 4800})).unwrap();
        self.app.focus_header(id, false);
        self.frames();
        id
    }
}

#[test]
fn menu_opens_defaults_enter_prints_and_focuses_the_named_destination() {
    let mut t = T::new();
    let source = t.source();
    t.app.run("mix.insert", json!({"track": source.0, "slot": 0, "plugin": "gain"})).unwrap();
    assert!(process_calls(t.app.engine.session(), source) > 0);
    let steps = t.app.engine.undo_history().len();
    t.menu("Track");
    t.menu("Bounce in Place");
    assert!(matches!(t.app.dialog, Some(Dialog::BounceInPlace)));
    assert!(!t.app.bounce_place.existing);
    assert_eq!(t.app.bounce_place.source, Source::Mute);
    assert_eq!(t.app.engine.undo_history().len(), steps);
    t.key("Enter");
    assert!(t.app.dialog.is_none());
    let s = t.app.engine.session();
    let dest = t.app.focus_track().unwrap();
    assert_ne!(dest, source);
    assert_eq!(s.track(dest).unwrap().name, "Keys bounce");
    assert!(s.track(source).unwrap().mixer.mute && s.track(source).unwrap().inactive);
    assert!(!s.track(source).unwrap().hidden);
    assert!(t.app.arrange.layout.tracks.iter().any(|row| row.id == source.0), "the inactive source stays in the arrangement");
    assert_eq!(t.app.status, "Bounced Keys to Keys bounce (source muted)");
    assert_eq!(t.app.engine.undo_history().len(), steps + 1);
    t.app.ui.show_mixer = true;
    t.frames();
    assert!(t.app.mixer.layout.strips.iter().any(|strip| strip.track == source.0), "the inactive source stays in the mixer");
    assert_eq!(process_calls(t.app.engine.session(), source), 0);
    t.app.focus_header(source, false);
    t.frames();
    t.menu("Track");
    t.menu("Make Active");
    let source_track = t.app.engine.session().track(source).unwrap();
    assert!(!source_track.inactive);
    assert!(source_track.mixer.mute, "activation leaves mute alone");
    let mute = t.app.arrange.layout.tracks.iter().find(|row| row.id == source.0).unwrap().mute;
    t.click(mute);
    let source_track = t.app.engine.session().track(source).unwrap();
    assert!(!source_track.inactive && !source_track.mixer.mute);
    assert!(process_calls(t.app.engine.session(), source) > 0, "the source insert processes again after the UI recovery");
}

#[test]
fn header_menu_toggles_only_its_track_and_inactive_tags_follow_the_state() {
    let mut t = T::new();
    t.app.help.open = true;
    let source = t.track("Source", "audio");
    let other = t.track("Other", "audio");
    t.app.focus_header(other, false);
    t.app.ui.show_mixer = true;
    t.frames();
    for (label, inactive, help) in [("Make Inactive", true, "track_menu.make_inactive"), ("Make Active", false, "track_menu.make_active")] {
        let row = t.app.arrange.layout.tracks.iter().find(|row| row.id == source.0).unwrap();
        assert_eq!(row.inactive_tag != [0.0; 4], !inactive);
        t.right_click(row.name_rect);
        assert!(t.app.help.layout.targets.contains_key(help));
        let rect = t.app.arrange.menu.iter().find(|item| item.label == label).unwrap().rect;
        t.click(rect);
        assert_eq!(t.app.engine.session().track(source).unwrap().inactive, inactive);
        assert!(!t.app.engine.session().track(other).unwrap().inactive);
        let row = t.app.arrange.layout.tracks.iter().find(|row| row.id == source.0).unwrap();
        let strip = t.app.mixer.layout.strips.iter().find(|strip| strip.track == source.0).unwrap();
        assert_eq!(row.inactive_tag != [0.0; 4], inactive);
        assert_eq!(strip.inactive_tag != [0.0; 4], inactive);
    }
}

#[test]
fn mixer_track_menu_offers_both_activity_actions() {
    let mut t = T::new();
    let audio = t.track("Audio", "audio");
    let instrument = t.source();
    t.app.ui.show_mixer = true;
    t.frames();
    for id in [audio, instrument] {
        for (label, inactive) in [("Make Inactive", true), ("Make Active", false)] {
            let strip = t.app.mixer.layout.strips.iter().find(|strip| strip.track == id.0).unwrap();
            t.right_click(strip.name_rect);
            let rect = t.app.mixer.layout.menu.iter().find(|item| item.label == label).unwrap().rect;
            t.click(rect);
            assert_eq!(t.app.engine.session().track(id).unwrap().inactive, inactive);
        }
    }
}

#[test]
fn track_menu_changes_the_selection_and_recovers_a_mixed_selection() {
    let mut t = T::new();
    let a = t.track("A", "audio");
    let b = t.track("B", "audio");
    let other = t.track("Other", "audio");
    t.app.focus_header(a, false);
    t.app.focus_header(b, true);
    t.frames();
    t.menu("Track");
    t.menu("Make Inactive");
    for id in [a, b] {
        assert!(t.app.engine.session().track(id).unwrap().inactive);
    }
    assert!(!t.app.engine.session().track(other).unwrap().inactive);
    t.app.run("track.make_inactive", json!({"tracks": [b.0], "inactive": false})).unwrap();
    t.frames();
    t.menu("Track");
    t.menu("Make Active");
    for id in [a, b, other] {
        assert!(!t.app.engine.session().track(id).unwrap().inactive);
    }
}

#[test]
fn commit_shortcut_opens_escape_cancels_and_choices_last_only_for_the_session() {
    let mut t = T::new();
    t.source();
    let before = t.app.engine.session().to_json().unwrap();
    t.input("ui.key", json!({"key": "C", "alt": true, "shift": true}));
    assert!(matches!(t.app.dialog, Some(Dialog::BounceInPlace)));
    t.click(t.app.bounce_place.layout.sources[0]);
    t.key("Escape");
    assert!(t.app.dialog.is_none());
    assert_eq!(t.app.engine.session().to_json().unwrap(), before);
    t.app.open_bounce_in_place();
    t.frames();
    assert_eq!(t.app.bounce_place.source, Source::Leave);
    t.key("Escape");
    t.app.run("session.new", json!({})).unwrap();
    assert_eq!(t.app.bounce_place.source, Source::Mute);
    assert!(!t.app.bounce_place.existing);
}

#[test]
fn cancel_button_preserves_the_document_and_source_choices_report_the_result() {
    let mut t = T::new();
    t.app.help.open = true;
    let source = t.source();
    let before = t.app.engine.session().to_json().unwrap();
    t.app.open_bounce_in_place();
    t.frames();
    for id in ["bounce_in_place.destination", "bounce_in_place.source", "bounce_in_place.apply", "bounce_in_place.cancel"] {
        assert!(t.app.help.layout.targets.contains_key(id), "missing help target {id}");
    }
    t.click(t.app.bounce_place.layout.cancel);
    assert!(t.app.dialog.is_none());
    assert_eq!(t.app.engine.session().to_json().unwrap(), before);
    for (index, text, hidden) in [(0, "left unchanged", false), (2, "hidden", true)] {
        t.app.focus_header(source, false);
        t.app.open_bounce_in_place();
        t.frames();
        t.click(t.app.bounce_place.layout.sources[index]);
        t.click(t.app.bounce_place.layout.bounce);
        assert_eq!(t.app.status, format!("Bounced Keys to Keys bounce (source {text})"));
        assert_eq!(t.app.engine.session().track(source).unwrap().hidden, hidden);
        t.app.run("edit.undo", json!({})).unwrap();
    }
}

#[test]
fn existing_dropdown_lists_only_eligible_tracks_and_bounce_uses_the_selection() {
    let mut t = T::new();
    let source = t.track("Source", "audio");
    let dest = t.track("Print", "audio");
    let hidden = t.track("Hidden", "audio");
    t.app.run("track.hide", json!({"tracks": [hidden.0], "hidden": true})).unwrap();
    let midi = t.track("MIDI", "midi");
    let instrument = t.source();
    // A printed audio track supplies a real frozen destination to filter.
    let frozen = TrackId(t.app.run("track.bounce", json!({"tracks": [instrument.0], "source": "leave"})).unwrap()["tracks"][0].as_u64().unwrap());
    t.app.run("track.freeze", json!({"tracks": [frozen.0]})).unwrap();
    t.app.focus_header(source, false);
    t.app.open_bounce_in_place();
    t.frames();
    t.click(t.app.bounce_place.layout.existing_track);
    t.click(t.app.bounce_place.layout.dropdown);
    let rows = t.app.bounce_place.layout.destinations.clone();
    assert_eq!(rows.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [dest]);
    assert!(!rows.iter().any(|(id, _)| [source, hidden, midi, instrument, frozen].contains(id)));
    t.click(rows[0].1);
    // Print the instrument with the remembered destination.
    t.key("Escape");
    t.app.focus_header(instrument, false);
    t.app.open_bounce_in_place();
    t.frames();
    assert!(t.app.bounce_place.existing);
    assert_eq!(t.app.bounce_place.destination, Some(dest));
    t.click(t.app.bounce_place.layout.bounce);
    assert_eq!(t.app.focus_track(), Some(dest));
    assert_eq!(t.app.engine.session().track(dest).unwrap().clips()[0].start, 0);
    assert_eq!(t.app.status, "Bounced Keys to Print (source muted)");
}

#[test]
fn hidden_tracks_menu_lists_shows_individually_and_shows_all() {
    let mut t = T::new();
    t.app.help.open = true;
    let a = t.track("A", "audio");
    let b = t.track("B", "audio");
    t.app.run("track.hide", json!({"tracks": [a.0, b.0], "hidden": true})).unwrap();
    t.frames();
    t.menu("Track");
    t.menu("Show Hidden Tracks…");
    assert!(matches!(t.app.dialog, Some(Dialog::HiddenTracks)));
    for id in ["hidden_tracks.show", "hidden_tracks.show_all", "hidden_tracks.close"] {
        assert!(t.app.help.layout.targets.contains_key(id), "missing help target {id}");
    }
    let rows = t.app.bounce_place.layout.hidden.clone();
    assert_eq!(rows.iter().map(|(id, name, _)| (*id, name.as_str())).collect::<Vec<_>>(), [(a, "A"), (b, "B")]);
    t.click(rows[0].2);
    assert!(!t.app.engine.session().track(a).unwrap().hidden);
    assert!(t.app.engine.session().track(b).unwrap().hidden);
    assert!(t.app.arrange.layout.tracks.iter().any(|r| r.id == a.0));
    t.click(t.app.bounce_place.layout.show_all);
    assert!(!t.app.engine.session().track(b).unwrap().hidden);
    assert!(t.app.dialog.is_none());
    t.menu("Track");
    t.menu("Show Hidden Tracks…");
    assert!(t.app.dialog.is_none(), "the menu entry is disabled with no hidden tracks");
}

#[test]
fn bounce_modal_swallows_wheel_and_zoom_over_the_backdrop() {
    let mut t = T::new();
    t.source();
    let initial = t.app.engine.session().edit.zoom;
    t.input("ui.zoom", json!({"x": 350, "y": 180, "factor": 1.5}));
    assert_ne!(t.app.engine.session().edit.zoom, initial, "the same gesture reaches the arrangement without a modal");
    t.app.open_bounce_in_place();
    t.frames();
    let before = t.app.engine.session().edit.zoom;
    let scroll = t.app.arrange.vscroll;
    t.input("ui.scroll", json!({"x": 350, "y": 180, "dy": -1, "shift": true}));
    t.input("ui.zoom", json!({"x": 350, "y": 180, "factor": 1.5}));
    assert_eq!(t.app.engine.session().edit.zoom, before);
    assert_eq!(t.app.arrange.vscroll, scroll);
}
