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
    // Mute means mute (#333); the choice that deactivates is its own radio button, third in the row.
    t.click(t.app.bounce_place.layout.sources[2]);
    assert_eq!(t.app.bounce_place.source, Source::MuteDeactivate);
    t.key("Enter");
    assert!(t.app.dialog.is_none());
    let s = t.app.engine.session();
    let dest = t.app.focus_track().unwrap();
    assert_ne!(dest, source);
    assert_eq!(s.track(dest).unwrap().name, "Keys bounce");
    assert!(s.track(source).unwrap().mixer.mute && s.track(source).unwrap().inactive);
    assert!(!s.track(source).unwrap().hidden);
    assert!(t.app.arrange.layout.tracks.iter().any(|row| row.id == source.0), "the inactive source stays in the arrangement");
    assert_eq!(t.app.status, "Bounced Keys to Keys bounce (source muted and deactivated)");
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
    for (index, text, hidden) in [(0, "left unchanged", false), (2, "muted and deactivated", false), (3, "hidden", true)] {
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

#[test]
fn an_inactive_source_refuses_the_form_with_the_menu_path_and_changes_nothing() {
    let mut t = T::new();
    let source = t.source();
    t.app.run("track.make_inactive", json!({"tracks": [source.0], "inactive": true})).unwrap();
    t.app.open_bounce_in_place();
    t.frames();
    let before = t.app.engine.session().to_json().unwrap();
    let steps = t.app.engine.undo_history().len();
    t.key("Enter");
    assert_eq!(t.app.status, "Keys is inactive: Track > Make Active first");
    assert!(matches!(t.app.dialog, Some(Dialog::BounceInPlace)), "the form stays open");
    assert_eq!(t.app.engine.session().to_json().unwrap(), before);
    assert_eq!(t.app.engine.undo_history().len(), steps);
    let (req, _rx) = crate::ControlRequest::new("engine.execute", json!({"command": "track.bounce", "params": {"tracks": [source.0]}}));
    let reply = format!("{:?}", crate::control::handle_for_test(&mut t.app, &t.ctx, &req));
    assert!(reply.contains("Keys is inactive: Track > Make Active first"), "{reply}");
    assert_eq!(t.app.engine.session().to_json().unwrap(), before);
}

#[test]
fn the_default_choice_mutes_only_and_the_inserts_keep_running() {
    let mut t = T::new();
    let source = t.source();
    t.app.run("mix.insert", json!({"track": source.0, "slot": 0, "plugin": "gain"})).unwrap();
    t.app.open_bounce_in_place();
    t.frames();
    t.key("Enter");
    let track = t.app.engine.session().track(source).unwrap();
    assert!(track.mixer.mute && !track.inactive && !track.hidden);
    assert_eq!(t.app.status, "Bounced Keys to Keys bounce (source muted)");
    assert!(process_calls(t.app.engine.session(), source) > 0, "the plug-ins of a muted source keep running");
}

#[test]
fn hide_still_deactivates_and_hides_and_mute_deactivate_is_a_control_channel_value() {
    let mut t = T::new();
    let source = t.source();
    t.app.run("track.bounce", json!({"tracks": [source.0], "source": "mute_deactivate"})).unwrap();
    let track = t.app.engine.session().track(source).unwrap();
    assert!(track.mixer.mute && track.inactive && !track.hidden);
    let hidden = t.track("Pad", "instrument");
    t.app.run("midi.new_clip", json!({"track": hidden.0, "start": 0, "length": 4800})).unwrap();
    t.app.run("track.bounce", json!({"tracks": [hidden.0], "source": "hide"})).unwrap();
    let track = t.app.engine.session().track(hidden).unwrap();
    assert!(track.inactive && track.hidden);
    assert!(t.app.run("track.bounce", json!({"tracks": [hidden.0], "source": "sideways"})).is_err());
}

#[test]
fn clicking_the_inactive_tag_makes_that_track_active_in_one_undo_step() {
    let mut t = T::new();
    t.app.help.open = true;
    let source = t.track("Source", "audio");
    let other = t.track("Other", "audio");
    for id in [source, other] {
        t.app.run("track.make_inactive", json!({"tracks": [id.0], "inactive": true})).unwrap();
    }
    t.app.ui.show_mixer = true;
    t.frames();
    // A right-click on the tag still opens the track menu.
    let row = t.app.arrange.layout.tracks.iter().find(|row| row.id == source.0).unwrap();
    t.right_click(row.inactive_tag);
    let item = t.app.arrange.menu.iter().find(|item| item.label == "Make Active").unwrap().rect;
    t.click(item);
    assert!(!t.app.engine.session().track(source).unwrap().inactive);
    t.app.run("track.make_inactive", json!({"tracks": [source.0], "inactive": true})).unwrap();
    t.frames();
    let steps = t.app.engine.undo_history().len();
    let row = t.app.arrange.layout.tracks.iter().find(|row| row.id == source.0).unwrap();
    assert!(t.app.help.layout.targets.contains_key("arrange_header.inactive"));
    t.click(row.inactive_tag);
    assert!(!t.app.engine.session().track(source).unwrap().inactive);
    assert!(t.app.engine.session().track(other).unwrap().inactive, "only the clicked track changes");
    assert_eq!(t.app.engine.undo_history().len(), steps + 1);
    assert!(t.app.engine.undo());
    assert!(t.app.engine.session().track(source).unwrap().inactive);
    // The mixer strip's tag does the same.
    t.frames();
    assert!(t.app.help.layout.targets.contains_key("mixer_strip.inactive"));
    let strip = t.app.mixer.layout.strips.iter().find(|strip| strip.track == other.0).unwrap();
    t.click(strip.inactive_tag);
    assert!(!t.app.engine.session().track(other).unwrap().inactive);
    assert!(t.app.engine.session().track(source).unwrap().inactive);
}

/// An instrument track named `name` (it prints through its instrument, as a track bounce does)
/// with a MIDI clip per `(start, end)`; returns the clip ids.
fn audio_clips(t: &mut T, name: &str, spans: &[(i64, i64)]) -> (TrackId, Vec<u64>) {
    let track = t.track(name, "instrument");
    for (a, b) in spans {
        t.app.run("midi.new_clip", json!({"track": track.0, "start": a, "length": b - a})).unwrap();
    }
    let ids = t.app.engine.session().track(track).unwrap().clips().iter().map(|c| c.id.0).collect();
    t.frames();
    (track, ids)
}

fn select(t: &mut T, ids: &[u64]) {
    t.app.engine.session_mut().edit.selected_clips = ids.iter().map(|i| transit_model::ClipId(*i)).collect();
    t.frames();
}

#[test]
fn the_context_menu_item_bounces_the_selected_clips_with_the_chosen_source() {
    let mut t = T::new();
    t.app.help.open = true;
    let (track, ids) = audio_clips(&mut t, "Keys", &[(10_000, 20_000), (30_000, 40_000)]);
    select(&mut t, &ids[..1]);
    let rect = t.app.arrange.layout.clips.iter().find(|c| c.id == ids[0]).unwrap().rect;
    t.right_click(rect);
    assert!(t.app.help.layout.targets.contains_key("bounce_in_place.clip_menu"));
    let item = t.app.arrange.menu.iter().find(|m| m.label == "Bounce in Place").unwrap().rect;
    let steps = t.app.engine.undo_history().len();
    t.click(item);
    assert!(matches!(t.app.dialog, Some(Dialog::BounceInPlace)));
    assert_eq!(t.app.bounce_place.clips, vec![transit_model::ClipId(ids[0])]);
    assert_eq!(t.app.engine.undo_history().len(), steps, "opening the form prints nothing");
    assert!(t.app.bounce_place.layout.span != [0.0; 4], "the span line is shown");
    assert!(t.app.help.layout.targets.contains_key("bounce_in_place.span"));
    assert!(t.app.bounce_span_line().starts_with("1 clip on 1 track, "), "{}", t.app.bounce_span_line());
    t.click(t.app.bounce_place.layout.sources[0]);
    t.key("Enter");
    assert!(t.app.dialog.is_none(), "{}", t.app.status);
    let s = t.app.engine.session();
    let dest = t.app.focus_track().unwrap();
    let print = &s.track(dest).unwrap().clips()[0];
    assert_eq!((print.start, print.length), (10_000, 10_000 + 96_000), "only the clicked clip's span, plus the tail");
    assert!(!s.track(track).unwrap().mixer.mute, "Leave keeps the source as it was");
    assert_eq!(t.app.engine.undo_history().len(), steps + 1);
}

#[test]
fn the_edit_menu_item_needs_a_selected_clip_and_the_form_hides_existing_for_several_tracks() {
    let mut t = T::new();
    let (_, a) = audio_clips(&mut t, "A", &[(10_000, 20_000)]);
    let (_, b) = audio_clips(&mut t, "B", &[(10_000, 20_000)]);
    select(&mut t, &[]);
    t.menu("Edit");
    t.menu("Bounce in Place");
    assert!(t.app.dialog.is_none(), "no clip selected: the item is disabled");
    select(&mut t, &[a[0], b[0]]);
    t.menu("Edit");
    t.menu("Bounce in Place");
    assert!(matches!(t.app.dialog, Some(Dialog::BounceInPlace)));
    assert!(t.app.bounce_span_line().starts_with("2 clips on 2 tracks, "), "{}", t.app.bounce_span_line());
    let steps = t.app.engine.undo_history().len();
    t.click(t.app.bounce_place.layout.existing_track);
    assert!(!t.app.bounce_place.existing, "several source tracks: one new track each");
    t.key("Enter");
    assert!(t.app.dialog.is_none(), "{}", t.app.status);
    assert_eq!(t.app.engine.undo_history().len(), steps + 1, "both prints are one undo step");
    assert_eq!(t.app.engine.session().tracks.len(), 4);
}

#[test]
fn the_track_form_still_runs_track_bounce_without_a_range() {
    let mut t = T::new();
    let (track, ids) = audio_clips(&mut t, "Keys", &[(10_000, 20_000)]);
    select(&mut t, &ids);
    t.app.focus_header(track, false);
    t.frames();
    t.menu("Track");
    t.menu("Bounce in Place");
    assert!(t.app.bounce_place.clips.is_empty());
    assert_eq!(t.app.bounce_span_line(), "");
    t.key("Enter");
    let dest = t.app.focus_track().unwrap();
    let print = &t.app.engine.session().track(dest).unwrap().clips()[0];
    assert_eq!((print.start, print.length), (0, 20_000 + 96_000), "the whole track from the session start");
}
