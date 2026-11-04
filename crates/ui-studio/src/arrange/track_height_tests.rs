use super::*;
use egui::{Event, Modifiers, MouseWheelUnit, PointerButton};

struct Rig {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    ids: Vec<TrackId>,
}

impl Rig {
    fn new(heights: &[TrackHeight]) -> Self {
        let mut app = crate::tests::app();
        let mut ids = Vec::new();
        for height in heights {
            let v = app.run("track.new", json!({"kind": "audio"})).unwrap();
            let id = TrackId(v["tracks"][0].as_u64().unwrap());
            app.engine.session_mut().track_mut(id).unwrap().height = *height;
            ids.push(id);
        }
        app.ui.show_mixer = false;
        app.ui.show_devices = false;
        app.engine.mark_clean();
        let mut r = Self { app, ctx: egui::Context::default(), time: 0.0, ids };
        r.frame(vec![], Modifiers::NONE, true);
        r.frame(vec![], Modifiers::NONE, true);
        r
    }

    fn frame(&mut self, mut events: Vec<Event>, modifiers: Modifiers, focused: bool) {
        self.time += 0.02;
        events.insert(0, Event::ModifiersChanged(modifiers));
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
            time: Some(self.time),
            events,
            focused,
            ..Default::default()
        };
        self.app.raw_input_hook(&mut raw);
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        output.textures_delta.clear();
    }

    fn row(&self, i: usize) -> &TrackRow {
        self.app.arrange.layout.tracks.iter().find(|r| r.id == self.ids[i].0).unwrap()
    }

    fn press(&mut self, i: usize, modifiers: Modifiers) -> Pos2 {
        let h = self.row(i).height_handle.unwrap();
        let p = pos2((h[0] + h[2]) * 0.5, (h[1] + h[3]) * 0.5);
        self.frame(
            vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers }],
            modifiers,
            true,
        );
        p
    }

    fn release(&mut self, p: Pos2, modifiers: Modifiers) {
        self.frame(vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers }], modifiers, true);
    }

    fn wheel(&mut self, i: usize, unit: MouseWheelUnit, dy: f32) {
        let a = self.row(i).lane;
        let p = pos2(a[0] + 30.0, a[1] + 5.0);
        self.frame(
            vec![Event::PointerMoved(p), Event::MouseWheel { unit, delta: vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers: Modifiers::ALT }],
            Modifiers::ALT,
            true,
        );
    }

    fn commits(&self) -> usize {
        self.app.engine.journal.iter().filter(|(id, _)| id == "track.height").count()
    }
    fn stored(&self, i: usize) -> TrackHeight {
        self.app.engine.session().track(self.ids[i]).unwrap().height
    }
}


fn key(key: egui::Key, pressed: bool, repeat: bool, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed, repeat, modifiers }
}

impl Rig {
    fn click_rect(&mut self, rect: R4) {
        let p = pos2((rect[0] + rect[2]) * 0.5, (rect[1] + rect[3]) * 0.5);
        self.frame(
            vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }],
            Modifiers::NONE,
            true,
        );
        self.release(p, Modifiers::NONE);
        self.frame(vec![], Modifiers::NONE, true);
    }
    fn menu_item(&mut self, label: &str) {
        let rect = self.app.arrange.bar_menu.iter().find(|m| m.label == label).unwrap().rect;
        self.click_rect(rect);
    }
}

#[test]
fn wheel_on_unselected_tall_row_commits_when_pointer_leaves_the_viewport() {
    let mut r = Rig::new(&[TrackHeight::Custom(440), TrackHeight::Custom(430)]);
    r.app.run("edit.select", json!({"tracks": [r.ids[0].0]})).unwrap();
    r.wheel(1, MouseWheelUnit::Line, 1.0);
    assert_eq!(r.row(1).height, 438.0);
    assert_eq!(r.app.engine.session().edit.selected_tracks, vec![r.ids[0]]);
    assert_eq!(r.stored(0), TrackHeight::Custom(440));
    assert_eq!(r.stored(1), TrackHeight::Custom(430));
    assert_eq!(r.commits(), 0);
    let timeline = r.app.arrange.layout.timeline;
    let outside = pos2(timeline[0] + 20.0, timeline[3] + 2.0);
    assert!(crate::finish::rect(r.row(1).lane).contains(outside));
    r.frame(vec![Event::PointerMoved(outside)], Modifiers::ALT, true);
    assert_eq!(r.commits(), 1);
    assert_eq!(r.stored(1), TrackHeight::Custom(438));
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.stored(1), TrackHeight::Custom(430));
}

#[test]
fn batched_drag_uses_press_position_and_modifiers_even_with_release_in_the_frame() {
    for released in [false, true] {
        let mut r = Rig::new(&[TrackHeight::Small, TrackHeight::Medium]);
        let h = r.row(0).height_handle.unwrap();
        let p = pos2((h[0] + h[2]) * 0.5, (h[1] + h[3]) * 0.5);
        let end = p + vec2(0.0, 24.0);
        let mut events = vec![
            Event::PointerMoved(p),
            Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE },
            Event::PointerMoved(end),
            Event::ModifiersChanged(Modifiers::ALT),
        ];
        if released {
            events.push(Event::PointerButton { pos: end, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::ALT });
        }
        r.frame(events, Modifiers::NONE, true);
        assert_eq!(r.row(0).height, 70.0);
        assert_eq!(r.row(1).height, 88.0);
        assert_eq!(r.commits(), usize::from(released));
        if !released {
            assert_eq!(r.stored(0), TrackHeight::Small);
            r.release(end, Modifiers::ALT);
        }
        assert_eq!(r.commits(), 1);
        assert_eq!(r.stored(0), TrackHeight::Custom(70));
        assert_eq!(r.stored(1), TrackHeight::Medium);
    }
}

#[test]
fn drag_is_local_across_frames_then_one_command_and_one_undo() {
    let mut r = Rig::new(&[TrackHeight::Small, TrackHeight::Custom(73), TrackHeight::Fit(120)]);
    let toggle = transit_model::ZoomToggle { samples_per_px: 90.0, scroll: 123, heights: vec![(r.ids[1], TrackHeight::Medium)] };
    r.app.engine.session_mut().edit.zoom_toggle = Some(toggle.clone());
    r.app.engine.mark_clean();
    assert_eq!(r.row(1).height, 73.0); // base: no height field; header remained 46.
    let before = serde_json::to_value(r.app.engine.session()).unwrap();
    let undo = r.app.engine.undo_history();
    let p = r.press(1, Modifiers::NONE);
    for dy in [1.2, 10.4, 21.6, 35.0] {
        r.frame(vec![Event::PointerMoved(p + vec2(0.0, dy))], Modifiers::NONE, true);
        assert_eq!(r.row(1).height, (73.0_f32 + dy).round());
        assert_eq!(r.app.inspect(&r.ctx)["layout"]["tracks"][1]["height"], json!((73.0_f32 + dy).round()));
        assert_eq!(serde_json::to_value(r.app.engine.session()).unwrap(), before);
        assert!(!r.app.engine.is_dirty());
        assert_eq!(r.app.engine.undo_history(), undo);
        assert_eq!(r.commits(), 0);
        assert_eq!(r.app.run("query.track_heights", json!({})).unwrap()["tracks"][1]["points"], 73);
    }
    r.release(p + vec2(0.0, 35.0), Modifiers::NONE);
    assert_eq!(r.commits(), 1);
    assert_eq!(r.stored(1).points(), 108.0);
    assert_eq!(r.app.engine.undo_history().len(), undo.len() + 1);
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(r.stored(1), TrackHeight::Custom(73));
    assert_eq!(r.app.engine.session().edit.zoom_toggle, Some(toggle));
    r.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(r.stored(1).points(), 108.0);
}

#[test]
fn bulk_drag_captures_hidden_collapsed_frozen_and_master_and_plain_folder_is_local() {
    let original = [TrackHeight::Small, TrackHeight::Medium, TrackHeight::Fit(140), TrackHeight::Custom(99), TrackHeight::Mini];
    let mut r = Rig::new(&original);
    // This fixture has no Player or render engine; none of its five strips renders audio.
    let ids = r.ids.clone();
    let s = r.app.engine.session_mut();
    s.track_mut(ids[0]).unwrap().kind = TrackKind::Folder;
    s.track_mut(ids[1]).unwrap().folder = Some(ids[0]);
    s.track_mut(ids[1]).unwrap().hidden = true;
    s.track_mut(ids[2]).unwrap().folder = Some(ids[0]);
    s.track_mut(ids[3]).unwrap().freeze =
        Some(transit_model::Frozen { source: transit_model::SourceId(99), start: 0, length: 1, hash: 0, revision: 0, latency: 0 });
    s.track_mut(ids[4]).unwrap().kind = TrackKind::Master;
    s.edit.selected_tracks = vec![ids[3]];
    r.app.ui.collapsed.push(ids[0]);
    r.frame(vec![], Modifiers::NONE, true);
    let p = r.press(0, Modifiers::ALT);
    r.frame(vec![Event::PointerMoved(p + vec2(0.0, 18.0))], Modifiers::NONE, true);
    assert_eq!(r.row(0).height, 64.0);
    assert_eq!(r.stored(1), original[1]);
    r.release(p + vec2(0.0, 18.0), Modifiers::NONE);
    assert_eq!(r.commits(), 1);
    for i in 0..5 {
        assert_eq!(r.stored(i).points(), 64.0);
    }
    assert_eq!(r.app.engine.journal.iter().find(|(id, _)| id == "track.height").unwrap().1["tracks"].as_array().unwrap().len(), 5);
    r.app.run("edit.undo", json!({})).unwrap();
    for (i, h) in original.into_iter().enumerate() {
        assert_eq!(r.stored(i), h);
    }
    r.frame(vec![], Modifiers::NONE, true);
    let p = r.press(0, Modifiers::NONE);
    r.frame(vec![Event::PointerMoved(p + vec2(0.0, 10.0))], Modifiers::NONE, true);
    r.release(p + vec2(0.0, 10.0), Modifiers::NONE);
    assert_eq!(r.stored(0).points(), 56.0);
    assert_eq!(r.stored(1), original[1]);
    r.app.ui.collapsed.clear();
    r.frame(vec![], Modifiers::NONE, true);
    assert_eq!(r.row(2).height, 140.0);
}

#[test]
fn height_keys_are_exact_one_step_per_press_and_exclude_piano_and_text() {
    for plus in [egui::Key::Equals, egui::Key::Plus] {
        let mut r = Rig::new(&[TrackHeight::Small, TrackHeight::Custom(100)]);
        r.frame(vec![key(plus, true, false, Modifiers::ALT)], Modifiers::ALT, true);
        assert_eq!(r.stored(0), TrackHeight::Medium);
        assert_eq!(r.stored(1), TrackHeight::Large);
        r.frame(vec![key(plus, true, true, Modifiers::ALT)], Modifiers::ALT, true);
        assert_eq!(r.stored(0), TrackHeight::Medium);
        r.frame(vec![key(plus, false, false, Modifiers::ALT)], Modifiers::ALT, true);
        r.frame(vec![key(plus, true, false, Modifiers::ALT)], Modifiers::ALT, true);
        assert_eq!(r.stored(0), TrackHeight::Large);
        r.frame(vec![key(plus, false, false, Modifiers::ALT), key(egui::Key::Minus, true, false, Modifiers::ALT)], Modifiers::ALT, true);
        assert_eq!(r.stored(0), TrackHeight::Medium);
    }
    for m in [Modifiers::ALT | Modifiers::SHIFT, Modifiers::ALT | Modifiers::CTRL, Modifiers::ALT | Modifiers::COMMAND] {
        let mut r = Rig::new(&[TrackHeight::Small]);
        r.frame(vec![key(egui::Key::Equals, true, false, m)], m, true);
        assert_eq!(r.stored(0), TrackHeight::Small);
    }
    let mut r = Rig::new(&[TrackHeight::Small]);
    let (_, clip, _) = crate::tests::track_with_clip(&mut r.app);
    r.app.open_keys(clip);
    r.app.keys.active = true;
    r.frame(vec![key(egui::Key::Equals, true, false, Modifiers::ALT)], Modifiers::ALT, true);
    assert_eq!(r.stored(0), TrackHeight::Small);
    r.app.close_keys();
    r.frame(vec![key(egui::Key::Equals, false, false, Modifiers::ALT)], Modifiers::ALT, true);
    let text_id = egui::Id::new("height-text");
    r.ctx.memory_mut(|m| m.request_focus(text_id));
    r.time += 0.02;
    let raw = egui::RawInput {
        time: Some(r.time),
        events: vec![Event::ModifiersChanged(Modifiers::ALT), key(egui::Key::Equals, true, false, Modifiers::ALT)],
        ..Default::default()
    };
    let mut text = String::new();
    let mut output = r.ctx.run_ui(raw, |ui| {
        ui.add(egui::TextEdit::singleline(&mut text).id(text_id));
        crate::menus::shortcuts(&mut r.app, ui.ctx());
    });
    output.textures_delta.clear();
    assert_eq!(r.stored(0), TrackHeight::Small);
}

#[test]
fn menu_entries_dispatch_directions_and_reset_keeps_bases_and_horizontal_zoom() {
    let mut r = Rig::new(&[TrackHeight::Small, TrackHeight::Medium]);
    r.app.toggle_automation(r.ids[0]);
    r.app.ui.auto_lanes[0].height = 97.0;
    let zoom = r.app.engine.session().edit.zoom;
    for (label, expected) in [("Increase All", [88.0, 132.0]), ("Decrease All", [46.0, 88.0]), ("Reset Track Heights", [46.0, 46.0])] {
        r.menu_item("View");
        r.menu_item("Track Height");
        r.menu_item(label);
        assert_eq!([r.stored(0).points(), r.stored(1).points()], expected);
        assert_eq!(r.app.ui.auto_lanes[0].height, 97.0);
        assert_eq!(r.app.engine.session().edit.zoom, zoom);
    }
    let commands: Vec<_> = r.app.engine.journal.iter().filter(|(id, _)| id.starts_with("track.height_")).collect();
    assert_eq!(commands.len(), 3);
    assert_eq!(commands[0].1["direction"], "in");
    assert_eq!(commands[1].1["direction"], "out");
    assert_eq!(commands[2].0, "track.height_reset");
}

#[test]
fn reopen_and_revert_drop_previews_reset_lane_bases_and_ignore_old_ui_height() {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    for wheel in [false, true] {
        for reopen in ["session.open", "session.revert"] {
            let mut r = Rig::new(&[TrackHeight::Medium, TrackHeight::Custom(73), TrackHeight::Fit(120)]);
            let dir = std::env::temp_dir().join(format!("height-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("heights.transit");
            r.app.run("session.save_as", json!({"path":file})).unwrap();
            r.app.toggle_automation(r.ids[0]);
            r.app.ui.auto_lanes[0].height = 123.0;
            if wheel {
                r.wheel(0, MouseWheelUnit::Line, 1.0);
            } else {
                let p = r.press(0, Modifiers::NONE);
                r.frame(vec![Event::PointerMoved(p + vec2(0.0, 8.0))], Modifiers::NONE, true);
            }
            assert_eq!(r.row(0).height, 96.0);
            r.app.run(reopen, json!({"path":file})).unwrap();
            assert_eq!(r.commits(), 0);
            assert!(r.app.ui.auto_lanes.is_empty());
            r.app.toggle_automation(r.ids[0]);
            assert_eq!(r.app.ui.auto_lanes[0].height, 56.0);
            r.frame(vec![], Modifiers::NONE, true);
            assert_eq!(r.row(0).height, 88.0);
            assert!(!r.app.engine.is_dirty());
            let bytes = std::fs::read(&file).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("height_gesture"));
            let (bundle, _) = transit_engine::io::session_bundle(r.app.engine.session()).unwrap();
            for payload in [&bytes, &bundle] {
                let mut e = transit_engine::Engine::default();
                transit_engine::io::open_session_bytes(&mut e, "heights.transit", payload).unwrap();
                assert_eq!(
                    e.session().tracks.iter().map(|t| t.height).collect::<Vec<_>>(),
                    vec![TrackHeight::Medium, TrackHeight::Custom(73), TrackHeight::Fit(120)]
                );
            }
            let before = r.app.arrange.layout.tracks.iter().map(|t| (t.height, t.header, t.stack_height)).collect::<Vec<_>>();
            for old in [json!({}), json!({"track_height":200})] {
                r.app.run("ui.set", old).unwrap();
                r.frame(vec![], Modifiers::NONE, true);
                assert_eq!(r.app.arrange.layout.tracks.iter().map(|t| (t.height, t.header, t.stack_height)).collect::<Vec<_>>(), before);
                assert!(serde_json::to_value(&r.app.ui).unwrap().get("track_height").is_none());
            }
            r.app.run("session.close", json!({})).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}

#[test]
fn stationary_and_returning_bulk_drag_preserve_unequal_variants() {
    for travel in [false, true] {
        let mut r = Rig::new(&[TrackHeight::Small, TrackHeight::Fit(92), TrackHeight::Custom(132)]);
        let before = serde_json::to_value(r.app.engine.session()).unwrap();
        let p = r.press(0, Modifiers::ALT);
        if travel {
            r.frame(vec![Event::PointerMoved(p + vec2(0.0, 20.0))], Modifiers::ALT, true);
            assert!(r.app.arrange.layout.tracks.iter().all(|t| t.height == 66.0));
            r.frame(vec![Event::PointerMoved(p)], Modifiers::ALT, true);
        }
        assert_eq!(r.row(1).height, 92.0);
        r.release(p, Modifiers::ALT);
        assert_eq!(r.commits(), 0);
        assert_eq!(serde_json::to_value(r.app.engine.session()).unwrap(), before);
    }
}

#[test]
fn release_outside_and_focus_loss_commit_last_preview_once() {
    for focus in [false, true] {
        let mut r = Rig::new(&[TrackHeight::Small]);
        let p = r.press(0, Modifiers::NONE);
        r.frame(vec![Event::PointerMoved(p + vec2(0.0, 54.0))], Modifiers::NONE, true);
        assert_eq!(r.row(0).height, 100.0);
        if focus {
            r.frame(vec![], Modifiers::NONE, false);
        } else {
            r.release(pos2(1190.0, p.y + 54.0), Modifiers::NONE);
        }
        r.frame(vec![], Modifiers::NONE, true);
        assert_eq!(r.commits(), 1);
        assert_eq!(r.stored(0).points(), 100.0);
    }
}

#[test]
fn wheel_units_remainders_reversals_clamps_and_zero_sum() {
    let mut r = Rig::new(&[TrackHeight::Small, TrackHeight::Fit(88)]);
    r.wheel(1, MouseWheelUnit::Line, 2.0);
    assert_eq!(r.row(1).height, 104.0);
    assert_eq!(r.stored(1), TrackHeight::Fit(88));
    assert_eq!(r.stored(0), TrackHeight::Small);
    r.wheel(1, MouseWheelUnit::Point, 39.0);
    assert_eq!(r.row(1).height, 104.0);
    r.wheel(1, MouseWheelUnit::Point, 1.0);
    assert_eq!(r.row(1).height, 112.0);
    r.wheel(1, MouseWheelUnit::Page, -3.0);
    assert_eq!(r.row(1).height, 88.0);
    r.frame(vec![], Modifiers::NONE, true);
    assert_eq!(r.commits(), 0);
    r.wheel(1, MouseWheelUnit::Line, f32::MAX);
    assert_eq!(r.row(1).height, 440.0);
    r.wheel(1, MouseWheelUnit::Line, -1.0);
    assert_eq!(r.row(1).height, 432.0);
    r.wheel(1, MouseWheelUnit::Line, -f32::MAX);
    assert_eq!(r.row(1).height, 16.0);
    r.wheel(1, MouseWheelUnit::Line, 1.0);
    assert_eq!(r.row(1).height, 24.0);
    r.frame(vec![], Modifiers::NONE, true);
    assert_eq!(r.commits(), 1);
}

#[test]
fn every_wheel_boundary_commits_once_and_next_notch_starts_fresh() {
    for boundary in 0..5 {
        let mut r = Rig::new(&[TrackHeight::Small]);
        r.wheel(0, MouseWheelUnit::Line, 1.0);
        for _ in 0..4 {
            r.frame(vec![], Modifiers::ALT, true);
        }
        r.wheel(0, MouseWheelUnit::Line, 2.0);
        assert_eq!(r.row(0).height, 70.0);
        assert_eq!(r.commits(), 0);
        match boundary {
            0 => r.frame(vec![Event::PointerMoved(pos2(1100.0, 750.0))], Modifiers::ALT, true),
            1 => r.frame(vec![], Modifiers::NONE, true),
            2 => r.frame(vec![], Modifiers::ALT, false),
            3 => {
                r.app.dialog = Some(crate::Dialog::HiddenTracks);
                r.frame(vec![], Modifiers::ALT, true);
                r.app.dialog = None;
            }
            _ => {
                r.app.run("edit.select", json!({"tracks": [r.ids[0].0]})).unwrap();
            }
        }
        assert_eq!(r.commits(), 1);
        r.frame(vec![], Modifiers::NONE, true);
        r.wheel(0, MouseWheelUnit::Line, 1.0);
        assert_eq!(r.row(0).height, 78.0);
        r.frame(vec![], Modifiers::NONE, true);
        assert_eq!(r.commits(), 2);
    }
}

#[test]
fn session_replacement_drops_drag_and_wheel_before_reusing_ids() {
    for wheel in [false, true] {
        for command in ["session.new", "session.close"] {
            let mut r = Rig::new(&[TrackHeight::Small]);
            if wheel {
                r.wheel(0, MouseWheelUnit::Line, 3.0);
            } else {
                let p = r.press(0, Modifiers::NONE);
                r.frame(vec![Event::PointerMoved(p + vec2(0.0, 24.0))], Modifiers::NONE, true);
            }
            assert_eq!(r.row(0).height, 70.0);
            r.app.run(command, json!({})).unwrap();
            assert!(r.app.arrange.height_gesture.is_none());
            assert_eq!(r.commits(), 0);
            r.app.run("track.new", json!({"kind": "audio"})).unwrap();
            r.frame(vec![], Modifiers::NONE, true);
            assert!(r.app.engine.session().tracks.iter().all(|t| t.height == TrackHeight::Small));
        }
    }
}

#[test]
fn ordinary_and_control_commands_flush_but_queries_do_not_and_refusal_does_not_retry() {
    for control in [false, true] {
        let mut r = Rig::new(&[TrackHeight::Small]);
        r.wheel(0, MouseWheelUnit::Line, 2.0);
        assert_eq!(r.row(0).height, 62.0);
        r.app.run("query.track_heights", json!({})).unwrap();
        assert_eq!(r.commits(), 0);
        let params = json!({"tracks": [r.ids[0].0], "value": true});
        if control {
            let (tx, rx) = std::sync::mpsc::channel();
            r.app.control_rx = Some(rx);
            let (request, reply) = crate::ControlRequest::new("engine.execute", json!({"command": "mix.mute", "params": params}));
            tx.send(request).unwrap();
            r.frame(vec![], Modifiers::ALT, true);
            assert_eq!(reply.try_recv().unwrap()["ok"], true);
        } else {
            r.app.run("mix.mute", params).unwrap();
        }
        assert_eq!(r.commits(), 1);
        let journal = &r.app.engine.journal;
        let i = journal.iter().position(|(id, _)| id == "track.height").unwrap();
        assert_eq!(journal[i + 1].0, "mix.mute");
    }
    let mut r = Rig::new(&[TrackHeight::Small]);
    r.wheel(0, MouseWheelUnit::Line, 1.0);
    r.app.engine.session_mut().tracks.clear(); // Inject refusal: a captured ID no longer exists.
    r.app.commit_height_preview();
    assert!(r.app.arrange.height_gesture.is_none());
    assert!(r.app.status.contains("track"));
    let journal = r.app.engine.journal.len();
    r.frame(vec![], Modifiers::NONE, true);
    assert_eq!(r.app.engine.journal.len(), journal);
}
