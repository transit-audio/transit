#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MIDI Learn for plug-in parameters (issue #97 slice 2, ADR 0028): the knob's range onto the
//! parameter's normalized travel, learning from a built-in editor's controls, "learn next touched
//! parameter" for a hosted editor, refusal of parameters the host does not list, and the
//! automation write modes a mapped move shares with a mouse drag. No device, no real plug-in: the
//! worker's output is fed in as the `MapMsg`s it would publish.
use crate::midi_learn::{check_param, plugin_value};
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use soundcraft_dsp::{HeaderAction, plugin_info};
use soundcraft_model::{AutoParam, AutomationMode, Insert, SourceKind, TrackId};
use soundcraft_playback::MapMsg;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

impl T {
    fn new() -> T {
        let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0 };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn send(&mut self, method: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, p);
        let _ = crate::control::handle_for_test(&mut self.app, &egui::Context::default(), &req);
        self.frames(8);
    }

    fn click(&mut self, at: (f32, f32), button: &str) {
        self.send("ui.click", json!({"x": at.0, "y": at.1, "button": button}));
    }

    fn track(&mut self, kind: &str) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": kind})).unwrap();
        let t = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.focus(t);
        self.frames(4);
        t
    }

    /// An audio track with `plugin` in slot A, its editor window open, in automation mode `mode`.
    fn insert(&mut self, plugin: &str, mode: AutomationMode) -> TrackId {
        let t = self.track("audio");
        self.app.add_device(t, plugin).unwrap();
        self.app.set_automation_mode(t, mode).unwrap();
        self.app.ui.plugin_windows.push((t, 0));
        self.frames(4);
        t
    }

    fn control(&self, t: TrackId, param: &str) -> (f32, f32) {
        let w = self.app.builtin_editor.windows.iter().find(|w| w.key.0 == t).expect("an editor window is open");
        mid(w.rows.iter().find(|r| r.param == param).unwrap_or_else(|| panic!("a control for {param}")).control)
    }

    fn menu(&self) -> Vec<String> {
        self.app.midi_learn.menu.iter().map(|m| m.label.clone()).collect()
    }

    fn choose(&mut self, label: &str) {
        let r = self.app.midi_learn.menu.iter().find(|m| m.label == label).unwrap_or_else(|| panic!("no `{label}` in {:?}", self.menu())).rect;
        self.click(mid(r), "left");
    }

    fn candidate(&mut self, cc: u8) {
        let msgs = vec![MapMsg::Candidate { port: "KeyStep".into(), channel: 0, kind: SourceKind::Cc, number: cc }];
        self.app.midi_learn_apply((0, msgs), self.time);
        self.frames(2);
    }

    fn map(&mut self, t: TrackId, slot: &str, param: &str, cc: u8) {
        let target = format!("track:{}:plugin:{slot}:{param}", t.0);
        self.app
            .run("midi.map", json!({"source": {"port": "KeyStep", "channel": 1, "kind": "cc", "number": cc}, "target": target, "mode": "jump"}))
            .unwrap();
        self.frames(3);
    }

    /// The knob moves to `v` (0..1), one frame later.
    fn knob(&mut self, index: u32, v: f32, seq: u32) {
        self.app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index, value: v, seq }]), self.time);
        self.frames(1);
    }

    fn play(&mut self) {
        self.send("ui.key", json!({"key": "Space"}));
    }

    fn value(&self, t: TrackId, param: &str) -> f32 {
        self.app.engine.session().track(t).unwrap().mixer.inserts[0].as_ref().unwrap().params.get(param).copied().unwrap_or(0.0)
    }

    fn lane(&self, t: TrackId, param: &str) -> usize {
        let p = AutoParam::Plugin { slot: 0, param: param.into() };
        self.app.engine.session().track(t).unwrap().lane(&p).map_or(0, |l| l.points.len())
    }
}

const GAIN: &str = "mid_gain";

#[test]
fn the_knob_range_maps_onto_the_parameters_normalized_travel() {
    let eq = plugin_info("eq_7band").unwrap().param(GAIN).unwrap();
    assert_eq!((plugin_value(eq, 0.0), plugin_value(eq, 1.0)), (eq.min, eq.max), "the ends are the parameter's own, not 0 and 127");
    assert!((plugin_value(eq, 0.5) - (eq.min + eq.max) / 2.0).abs() < 1e-4);
    // A logarithmic parameter takes its taper: the middle of the knob is the geometric middle.
    let synth = plugin_info("subtractive_synth").unwrap();
    let cutoff = synth.param("cutoff").unwrap();
    assert_eq!(cutoff.taper, soundcraft_dsp::Taper::Log);
    let half = plugin_value(cutoff, 0.5);
    assert!((half / (cutoff.min * cutoff.max).sqrt() - 1.0).abs() < 0.01, "{half}");
    // A choice lands on a whole step inside its range.
    let choice = synth.params.iter().find(|p| p.unit == soundcraft_dsp::Unit::Choice).unwrap();
    for i in 0..=20 {
        let v = plugin_value(choice, i as f32 / 20.0);
        assert_eq!(v, v.round());
        assert!((choice.min..=choice.max).contains(&v));
    }
    assert_eq!(plugin_value(eq, 7.0), eq.max, "a stray value cannot leave the range");
}

#[test]
fn a_mapped_move_sets_the_session_parameter_in_its_own_units_as_one_undo_step() {
    let mut t = T::new();
    let tr = t.insert("eq_7band", AutomationMode::Off);
    t.map(tr, "a", GAIN, 74);
    let depth = t.app.engine.undo_history().len();
    t.knob(0, 1.0, 1);
    assert_eq!(t.value(tr, GAIN), 24.0, "knob full up is the parameter's maximum, not 1 and not 127");
    t.knob(0, 0.25, 2);
    assert!((t.value(tr, GAIN) + 12.0).abs() < 1e-4);
    t.knob(0, 0.0, 3);
    assert_eq!(t.value(tr, GAIN), -24.0);
    assert_eq!(t.app.engine.undo_history().len(), depth + 1, "a sweep is one step");
    assert!(t.app.engine.undo());
    assert_eq!(t.value(tr, GAIN), 0.0, "undo returns the parameter to where the sweep began");
}

#[test]
fn right_click_learn_forget_and_markers_work_on_panel_knobs_and_generic_rows() {
    for (plugin, param) in [("eq_7band", GAIN), ("chorus", "rate")] {
        let mut t = T::new();
        let tr = t.insert(plugin, AutomationMode::Off);
        assert!(t.app.builtin_editor.windows[0].rows.iter().any(|r| r.param == param), "{plugin} has {param}");
        t.click(t.control(tr, param), "right");
        assert_eq!(t.menu(), ["MIDI Learn", "MIDI Learn for this controller (all sessions)"], "{plugin}");
        t.choose("MIDI Learn");
        let want = format!("track:{}:plugin:a:{param}", tr.0);
        assert_eq!(t.app.engine.learn().expect("armed").target.to_string(), want);
        let markers = t.app.inspect(&t.ctx)["midi_learn"]["markers"].clone();
        assert!(markers.as_array().unwrap().iter().any(|m| m["learning"] == true && m["control"].as_str().unwrap().contains(param)), "{markers}");
        t.candidate(74);
        assert!(t.app.engine.learn().is_none());
        assert_eq!(t.app.engine.session().midi_mappings.iter().next().unwrap().target.to_string(), want);
        assert!(t.app.status.starts_with("Mapped Insert A "), "{}", t.app.status);
        let markers = t.app.inspect(&t.ctx)["midi_learn"]["markers"].clone();
        assert!(
            markers
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["mapped"] == true && m["learning"] == false && m["control"].as_str().unwrap().contains(param))
        );
        t.click(t.control(tr, param), "right");
        assert_eq!(t.menu(), ["Mapped: KeyStep ch 1 CC 74", "MIDI Learn", "Forget MIDI Mapping"], "{plugin}");
        t.choose("Forget MIDI Mapping");
        assert!(t.app.engine.session().midi_mappings.is_empty(), "{plugin}: forgotten");
        assert!(t.app.engine.undo(), "and the forgetting is an undo step");
        assert_eq!(t.app.engine.session().midi_mappings.len(), 1);
    }
}

#[test]
fn a_controller_wide_learn_names_the_strip_by_position() {
    let mut t = T::new();
    let tr = t.insert("eq_7band", AutomationMode::Off);
    t.click(t.control(tr, GAIN), "right");
    t.choose("MIDI Learn for this controller (all sessions)");
    assert_eq!(t.app.engine.learn().unwrap().target.to_string(), "strip:1:plugin:a:mid_gain");
}

#[test]
fn a_mapping_whose_device_is_gone_is_stored_but_idle() {
    let mut t = T::new();
    let tr = t.insert("eq_7band", AutomationMode::Off);
    t.map(tr, "a", GAIN, 74);
    assert_eq!(t.app.midi_learn.resolved_len(), 1);
    t.app.run("mix.insert_remove", json!({"track": tr.0, "slot": 0})).unwrap();
    t.frames(3);
    assert_eq!(t.app.engine.session().midi_mappings.len(), 1, "still stored");
    assert_eq!(t.app.midi_learn.resolved_len(), 0, "not matched");
    assert!(t.app.engine.undo());
    t.frames(3);
    assert_eq!(t.app.midi_learn.resolved_len(), 1, "the device is back, so is the mapping");
}

#[test]
fn only_parameters_the_host_lists_can_be_mapped() {
    let table = plugin_info("eq_7band");
    assert!(check_param(table, false, GAIN).is_ok());
    let why = check_param(table, true, "meter_level").unwrap_err();
    assert!(why.contains("read-only or cannot be automated"), "{why}");
    assert!(check_param(None, true, "7").is_ok(), "an uncached table accepts what the plug-in reports");
    assert!(check_param(None, false, "7").unwrap_err().contains("not loaded"));
}

fn hosted(t: &mut T) -> (TrackId, (TrackId, usize)) {
    let tr = t.track("audio");
    // No panel shows the hosted plug-in's name: that would start a scan, and the host registries
    // are process-global (see `the_shared_host_registries_hold_no_scan_folders_or_results`).
    t.app.ui.show_mixer = false;
    t.app.ui.show_devices = false;
    t.app.engine.session_mut().track_mut(tr).unwrap().mixer.inserts[1] = Some(Insert::new("clap:transit.test.learn97"));
    // no frames
    (tr, (tr, 1))
}

#[test]
fn learn_next_touched_on_a_hosted_header_takes_the_reported_parameter_then_the_next_controller_message() {
    // No frames and no edits through the engine: a hosted insert's name or parameter lookup would
    // start a plug-in scan, and the host registries are process-global.
    let mut t = T::new();
    let (tr, key) = hosted(&mut t);
    assert!(t.app.midi_learn.next.is_none());
    t.app.header_action(tr, 1, HeaderAction::LearnNext);
    assert_eq!(t.app.midi_learn.next, Some(key));
    assert!(t.app.engine.learn().is_none(), "no target yet");
    // An edit in another plug-in's editor is not this one's.
    t.app.learn_touched(tr, 2, "Other");
    assert_eq!(t.app.midi_learn.next, Some(key));
    // The parameter this editor reports becomes the target (the id the host gave, not a position)...
    t.app.learn_touched(tr, 1, "Gain");
    assert!(t.app.midi_learn.next.is_none());
    let want = format!("track:{}:plugin:b:Gain", tr.0);
    assert_eq!(t.app.engine.learn().expect("armed for a controller message").target.to_string(), want);
    t.app.learn_touched(tr, 1, "Later");
    assert_eq!(t.app.engine.learn().unwrap().target.to_string(), want, "only the first");
    // ...and the next CC is its source.
    let msgs = vec![MapMsg::Candidate { port: "KeyStep".into(), channel: 0, kind: SourceKind::Cc, number: 21 }];
    t.app.midi_learn_apply((0, msgs), t.time);
    let m = t.app.engine.session().midi_mappings.iter().next().unwrap().clone();
    assert_eq!((m.target.to_string(), m.source.number), (want, 21));
    // A move of a plug-in whose parameter table is not cached is ignored, not guessed at.
    let revision = t.app.engine.revision;
    t.app.midi_learn_apply((0, vec![MapMsg::Move { epoch: 0, index: 0, value: 1.0, seq: 1 }]), t.time);
    assert_eq!(t.app.engine.revision, revision);
}

#[test]
fn learn_next_touched_skips_echoes_and_takes_the_first_changed_edit_an_editor_reports() {
    let mut t = T::new();
    let tr = t.insert("eq_7band", AutomationMode::Off);
    let key = (tr, 0);
    t.app.learn_next_touched(key);
    assert_eq!(t.app.midi_learn.next, Some(key));
    // The plug-in echoing the value it already has is not a touch.
    t.app.apply_edits(vec![(tr, 0, GAIN.into(), 0.0)]);
    assert_eq!(t.app.midi_learn.next, Some(key));
    t.app.apply_edits(vec![(tr, 0, GAIN.into(), 6.0), (tr, 0, "high_shelf_gain".into(), 3.0)]);
    assert!(t.app.midi_learn.next.is_none());
    assert_eq!(t.app.engine.learn().unwrap().target.to_string(), format!("track:{}:plugin:a:{GAIN}", tr.0));
    t.candidate(21);
    assert_eq!(t.app.engine.session().midi_mappings.len(), 1);
}

#[test]
fn learn_next_touched_toggles_off_and_escape_cancels_it() {
    let mut t = T::new();
    let tr = t.insert("eq_7band", AutomationMode::Off);
    let key = (tr, 0);
    t.app.learn_next_touched(key);
    t.app.learn_next_touched(key);
    assert!(t.app.midi_learn.next.is_none(), "the same item again cancels");
    t.app.learn_next_touched(key);
    assert_eq!(t.app.midi_learn.next, Some(key));
    t.send("ui.key", json!({"key": "Escape"}));
    assert!(t.app.midi_learn.next.is_none(), "Escape cancels");
    t.app.apply_edits(vec![(tr, 0, GAIN.into(), 6.0)]);
    assert!(t.app.engine.learn().is_none(), "a cancelled wait learns nothing");
}

#[test]
fn a_mapped_move_writes_automation_in_write_latch_and_touch_and_not_in_read() {
    for (mode, writes) in [(AutomationMode::Write, true), (AutomationMode::Latch, true), (AutomationMode::Touch, true), (AutomationMode::Read, false)]
    {
        let mut t = T::new();
        let tr = t.insert("eq_7band", mode);
        t.map(tr, "a", GAIN, 74);
        // Stopped: the value moves and nothing is written, as with the mouse.
        t.knob(0, 0.75, 1);
        assert!(t.value(tr, GAIN) > 0.0, "{mode:?}: applied live");
        assert_eq!(t.lane(tr, GAIN), 0, "{mode:?}: nothing is written while stopped");
        t.frames(20);
        t.play();
        assert!(t.app.is_playing());
        t.frames(10);
        assert_eq!(t.lane(tr, GAIN), 0, "{mode:?}: an untouched parameter writes nothing");
        for i in 0..30u32 {
            t.knob(0, 0.2 + 0.02 * i as f32, 2 + i);
        }
        let written = t.lane(tr, GAIN);
        let end = 24.0 * (2.0 * (0.2 + 0.02 * 29.0) - 1.0);
        assert!((t.value(tr, GAIN) - end).abs() < 1e-3, "{mode:?}: the value follows the knob: {}", t.value(tr, GAIN));
        if writes {
            assert!(written >= 5, "{mode:?}: wrote {written} points while playing");
        } else {
            assert_eq!(written, 0, "{mode:?}: Read applies the value live and writes nothing");
        }
        t.play();
    }
}

#[test]
fn touch_stops_writing_when_the_controller_goes_still() {
    let mut t = T::new();
    let tr = t.insert("eq_7band", AutomationMode::Touch);
    t.map(tr, "a", GAIN, 74);
    t.play();
    for i in 0..20u32 {
        t.knob(0, 0.3 + 0.02 * i as f32, 1 + i);
    }
    t.frames(14);
    let n = t.lane(tr, GAIN);
    assert!(n >= 5);
    t.frames(40);
    assert_eq!(t.lane(tr, GAIN), n, "released (held 150 ms past the last value): Touch stops writing");
    t.play();
}
