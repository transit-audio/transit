use crate::tests::*;

/// Two empty audio tracks and one with a clip, in a fresh app with the arrangement laid out.
fn delete_app() -> (StudioApp, egui::Context, f64, [TrackId; 3]) {
    let (mut app, ctx, mut time, a) = mixer_app();
    let b = TrackId(app.run("track.new", json!({"kind": "audio", "name": "Bass"})).unwrap()["tracks"][0].as_u64().unwrap());
    let c = TrackId(app.run("track.new", json!({"kind": "instrument", "name": "Keys"})).unwrap()["tracks"][0].as_u64().unwrap());
    app.run("midi.new_clip", json!({"tracks": [c.0], "start": 0, "end": 0})).unwrap();
    app.run("edit.select_none", json!({})).unwrap();
    app.ui.show_mixer = false;
    frames(&mut app, &ctx, 4, &mut time);
    (app, ctx, time, [a, b, c])
}

fn header_mid(app: &StudioApp, t: TrackId) -> (f32, f32) {
    let hdr = app.arrange.layout.tracks.iter().find(|r| r.id == t.0).unwrap().header;
    (hdr[0] + 30.0, (hdr[1] + hdr[3]) / 2.0)
}

fn track_count(app: &StudioApp) -> usize {
    app.engine.session().tracks.iter().filter(|t| t.kind != transit_model::TrackKind::Master).count()
}

const NO_HEADER: &str = "Select a track header to delete the track";

#[test]
fn clip_click_then_delete_twice_keeps_the_track() {
    let (mut app, ctx, mut time, [_, _, keys]) = delete_app();
    let clip = app.engine.session().track(keys).unwrap().clips()[0].id;
    app.click_clip(clip, false);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().track(keys).unwrap().clips().is_empty(), "first Delete removes the clip");
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().track(keys).is_some(), "second Delete leaves the track");
    assert_eq!(app.status, NO_HEADER);
}

#[test]
fn lane_click_then_delete_keeps_the_track() {
    let (mut app, ctx, mut time, [a, ..]) = delete_app();
    app.click_empty(Some(a), 1000);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().track(a).is_some());
    assert_eq!(app.status, NO_HEADER);
}

#[test]
fn select_all_then_delete_keeps_every_track() {
    let (mut app, ctx, mut time, _) = delete_app();
    let n = track_count(&app);
    let undo = undo_len(&mut app);
    app.run("edit.select_all", json!({})).unwrap();
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(track_count(&app), n);
    assert_eq!(undo_len(&mut app), undo);
    assert_eq!(app.status, NO_HEADER);
}

#[test]
fn header_click_then_delete_removes_an_empty_track_in_one_undo_step() {
    let (mut app, ctx, mut time, [a, ..]) = delete_app();
    let at = header_mid(&app, a);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.tracks_selected_by_header);
    let undo = undo_len(&mut app);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.engine.session().track(a).is_none());
    assert!(app.dialog.is_none(), "an empty track needs no confirmation");
    assert_eq!(undo_len(&mut app), undo + 1);
    assert_eq!(app.status, "Deleted Gtr");
}

#[test]
fn deleting_a_track_with_clips_asks_first_escape_cancels_enter_deletes() {
    let (mut app, ctx, mut time, [_, _, keys]) = delete_app();
    let at = header_mid(&app, keys);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(matches!(app.dialog, Some(crate::Dialog::DeleteTracks { clips: true, .. })));
    assert!(app.engine.session().track(keys).is_some());
    key(&mut app, "Escape");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.dialog.is_none());
    assert!(app.engine.session().track(keys).is_some(), "Escape cancels");
    let undo = undo_len(&mut app);
    // Wait out the double-click time: a second click on the name soon after renames (slice 6).
    frames(&mut app, &ctx, 30, &mut time);
    let at = header_mid(&app, keys);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.dialog.is_some());
    key(&mut app, "Enter");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(app.dialog.is_none());
    assert!(app.engine.session().track(keys).is_none(), "Enter deletes");
    assert_eq!(undo_len(&mut app), undo + 1);
}

#[test]
fn shift_clicking_two_headers_then_delete_confirms_and_is_one_undo_step() {
    let (mut app, ctx, mut time, [a, b, _]) = delete_app();
    let at = header_mid(&app, a);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    let at = header_mid(&app, b);
    click_with(&mut app, at, json!({"shift": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(app.engine.session().edit.selected_tracks.len(), 2);
    let undo = undo_len(&mut app);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 4, &mut time);
    assert!(matches!(app.dialog, Some(crate::Dialog::DeleteTracks { clips: false, .. })));
    assert_eq!(track_count(&app), 3, "nothing deleted before the answer");
    key(&mut app, "Enter");
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(track_count(&app), 1);
    assert_eq!(undo_len(&mut app), undo + 1, "one undo entry");
    assert_eq!(app.status, "Deleted 2 tracks");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(track_count(&app), 3);
}

#[test]
fn the_header_menu_delete_asks_when_the_track_has_clips() {
    let (mut app, ctx, mut time, [_, _, keys]) = delete_app();
    let at = header_mid(&app, keys);
    right_click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.arrange.menu.iter().find(|m| m.label == "Delete Track").cloned().expect("Delete Track");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.dialog.is_some());
    assert!(app.engine.session().track(keys).is_some());
}

#[test]
fn the_master_is_never_deleted() {
    let (mut app, ..) = delete_app();
    let master = app.engine.session().tracks.iter().find(|t| t.kind == transit_model::TrackKind::Master).map(|t| t.id);
    if let Some(m) = master {
        assert!(app.request_delete_tracks(vec![m]).is_err());
        assert!(app.engine.session().track(m).is_some());
    }
}

#[test]
fn a_bare_audio_track_is_mono_on_input_one() {
    let mut app = app();
    let r = app.add_track("audio");
    let t = new_audio(&mut app, r);
    assert_eq!(app.engine.session().track(t).unwrap().channels(), 1);
    assert_eq!(app.engine.session().track(t).unwrap().format, transit_model::ChannelFormat::Mono);
    assert_eq!(input_label(&app, t), "In 1");
}

#[test]
fn add_track_midi_makes_a_focused_midi_track() {
    let mut app = app();
    let r = app.add_track("midi");
    let t = new_audio(&mut app, r);
    assert_eq!(app.engine.session().track(t).unwrap().kind, transit_model::TrackKind::Midi);
    assert_eq!(app.ui.focus_track, Some(t));
}

#[test]
fn the_menu_makes_mono_and_stereo_audio_tracks() {
    let mut app = app();
    let r = app.add_audio_track("mono");
    let mono = new_audio(&mut app, r);
    let r = app.add_audio_track("stereo");
    let stereo = new_audio(&mut app, r);
    assert_eq!(app.engine.session().track(mono).unwrap().format, transit_model::ChannelFormat::Mono);
    assert_eq!(input_label(&app, mono), "In 1");
    assert_eq!(app.engine.session().track(stereo).unwrap().format, transit_model::ChannelFormat::Stereo);
    assert_eq!(input_label(&app, stereo), "In 1-2");
}

#[test]
fn a_mono_track_can_take_input_two() {
    let mut app = app();
    let r = app.add_audio_track("mono");
    let t = new_audio(&mut app, r);
    let s = app.engine.session_arc();
    let labels: Vec<String> = crate::mixer::input_choices(&s, s.track(t).unwrap(), 2, &Default::default())
        .into_iter()
        .map(|choice| match choice {
            RoutingChoice::Entry { label, .. } => label,
            RoutingChoice::Separator => String::new(),
        })
        .collect();
    assert_eq!(labels, ["In 1", "In 2"]);
    app.set_input(t, "In 2").unwrap();
    assert_eq!(input_label(&app, t), "In 2");
}

#[test]
fn change_width_is_one_undo_step() {
    let mut app = app();
    let r = app.add_audio_track("mono");
    let t = new_audio(&mut app, r);
    let steps = app.engine.undo_history().len();
    app.run("track.change_width", json!({"tracks": [t.0], "format": "stereo"})).unwrap();
    assert_eq!(app.engine.session().track(t).unwrap().channels(), 2);
    assert_eq!(app.engine.undo_history().len(), steps + 1);
    assert_eq!(input_label(&app, t), "In 1-2");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().track(t).unwrap().channels(), 1);
    assert_eq!(app.engine.undo_history().len(), steps);
}

#[test]
fn change_width_remaps_hardware_inputs_and_undo_restores_both() {
    use transit_model::{ChannelFormat, Route};
    for (from, input, to, mapped, channels) in [
        ("mono", "In 2", "stereo", "In 1-2", vec![0, 1]),
        ("mono", "In 3", "stereo", "In 3-4", vec![2, 3]),
        ("stereo", "In 1-2", "mono", "In 1", vec![0]),
        ("stereo", "In 3-4", "mono", "In 3", vec![2]),
    ] {
        let mut app = app();
        let r = app.add_audio_track(from);
        let t = new_audio(&mut app, r);
        // Use the command to make even default input labels explicit hardware routes.
        app.run("track.input", json!({"tracks": [t.0], "input": input})).unwrap();
        app.run("mix.record_arm", json!({"tracks": [t.0], "value": true})).unwrap();
        let before = app.engine.session().track(t).unwrap().clone();
        let steps = app.engine.undo_history().len();
        app.run("track.change_width", json!({"tracks": [t.0], "format": to})).unwrap();
        let tr = app.engine.session().track(t).unwrap();
        assert_eq!(tr.format, ChannelFormat::from_id(to).unwrap());
        assert_eq!(tr.mixer.input, Route::Hardware(mapped.into()));
        assert_eq!(input_label(&app, t), mapped);
        assert!(tr.mixer.record_arm);
        assert_eq!(app.engine.undo_history().len(), steps + 1);
        // Pure planning only: no recorder or hardware device is opened.
        let takes = transit_engine::io::plan_takes(&app.engine, std::path::Path::new(env!("CARGO_MANIFEST_DIR")), 4);
        assert_eq!(takes[0].1, channels);
        let target = transit_mix::RenderTarget {
            device: transit_mix::DeviceGeneration(1),
            sample_rate: 48_000,
            max_block: 64,
            out_channels: 0,
            in_channels: 0,
        };
        let mut compiler = transit_mix::PlanCompiler::new(target, transit_mix::PlanLimits::default());
        let mut make = |spec: &transit_mix::InstanceSpec, _: transit_mix::SlotGen| {
            transit_mix::create_instance(&spec.id, spec.state.as_deref(), 48_000.0, 64, spec.ch)
        };
        let session = std::sync::Arc::new(app.engine.session().clone());
        let mut plan = compiler.compile(session, compiler.active(), transit_mix::PlanGeneration::from_raw(1), &mut make).unwrap();
        let mut mix = transit_mix::MixEngine::realtime(target);
        assert_eq!(mix.adopt_plan(&mut plan), transit_mix::Adoption::Adopted);
        compiler.reclaim(plan);
        mix.monitor_only = true;
        mix.input = vec![vec![0.2; 64], vec![0.4; 64], vec![0.6; 64], vec![0.8; 64]];
        let mut out = vec![vec![0.0; 64]; 2];
        mix.render_active(0, 64, &mut out);
        for (c, output) in out.iter().enumerate() {
            let expected =
                if channels.len() == 1 { mix.input[channels[0]][32] * std::f32::consts::FRAC_1_SQRT_2 } else { mix.input[channels[c]][32] };
            assert!((output[32] - expected).abs() < 1e-3, "{from} {input} -> {to}, channel {c}: {} vs {expected}", output[32]);
        }
        app.run("edit.undo", json!({})).unwrap();
        let restored = app.engine.session().track(t).unwrap();
        assert_eq!(restored.format, before.format);
        assert_eq!(restored.mixer, before.mixer);
        assert_eq!(app.engine.undo_history().len(), steps);
        app.run("edit.redo", json!({})).unwrap();
        assert_eq!(input_label(&app, t), mapped);
        assert_eq!(app.engine.session().track(t).unwrap().format, ChannelFormat::from_id(to).unwrap());
        // A second width action is a separate undo step, even without intervening edits.
        app.run("track.change_width", json!({"tracks": [t.0], "format": from})).unwrap();
        assert_eq!(app.engine.undo_history().len(), steps + 2);
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(input_label(&app, t), mapped);
    }
}

#[test]
fn change_width_keeps_bus_inputs_in_both_directions() {
    use transit_model::{ChannelFormat, Route};
    let mut app = app();
    let r = app.add_audio_track("mono");
    let t = new_audio(&mut app, r);
    let bus = app.engine.session_mut().add_bus("Capture", ChannelFormat::Stereo);
    app.run("track.input", json!({"tracks": [t.0], "input": "Capture"})).unwrap();
    for format in ["stereo", "mono"] {
        let steps = app.engine.undo_history().len();
        app.run("track.change_width", json!({"tracks": [t.0], "format": format})).unwrap();
        assert_eq!(app.engine.session().track(t).unwrap().mixer.input, Route::Bus(bus));
        assert_eq!(app.engine.undo_history().len(), steps + 1);
    }
}

#[test]
fn change_width_refuses_recording_without_an_edit() {
    for (from, to) in [("mono", "stereo"), ("stereo", "mono")] {
        let mut app = app();
        let r = app.add_audio_track(from);
        let t = new_audio(&mut app, r);
        app.engine.transport.recording = true;
        let before = app.engine.session_arc();
        let steps = app.engine.undo_history().len();
        let error = app.run("track.change_width", json!({"tracks": [t.0], "format": to})).unwrap_err();
        assert!(error.contains("Stop recording"), "{error}");
        assert_eq!(app.status, error);
        assert!(std::sync::Arc::ptr_eq(&before, &app.engine.session_arc()));
        assert!(app.engine.transport.recording);
        assert_eq!(app.engine.undo_history().len(), steps);
        app.engine.transport.recording = false;
        app.run("track.change_width", json!({"tracks": [t.0], "format": to})).unwrap();
        assert_eq!(app.engine.undo_history().len(), steps + 1);
    }
}

#[test]
fn width_menu_is_disabled_while_recording() {
    for format in ["mono", "stereo"] {
        let mut app = app();
        let r = app.add_audio_track(format);
        let t = new_audio(&mut app, r);
        let ctx = egui::Context::default();
        for recording in [false, true, false] {
            app.engine.transport.recording = recording;
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let response = crate::arrange::width_item(&mut app, ui, t).unwrap();
                assert_eq!(response.enabled(), !recording, "{format}: recording = {recording}");
            });
            out.textures_delta.clear();
            assert_eq!(app.width_change_disabled_reason().is_some(), recording);
        }
    }
}

#[test]
fn width_menu_explains_why_recording_disables_it() {
    fn has_reason(shape: &egui::Shape) -> bool {
        match shape {
            egui::Shape::Text(text) => text.galley.text().contains("Stop recording before changing a track's channel width."),
            egui::Shape::Vec(shapes) => shapes.iter().any(has_reason),
            _ => false,
        }
    }
    let mut app = app();
    let r = app.add_audio_track("stereo");
    let t = new_audio(&mut app, r);
    app.engine.transport.recording = true;
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|style| style.interaction.tooltip_delay = 0.0);
    let mut at = egui::Pos2::ZERO;
    let mut explained = false;
    for frame in 0..5 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600.0, 200.0))),
            time: Some(frame as f64),
            events: vec![egui::Event::PointerMoved(at)],
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            at = crate::arrange::width_item(&mut app, ui, t).unwrap().rect.center();
        });
        explained |= out.shapes.iter().any(|s| has_reason(&s.shape));
        out.textures_delta.clear();
    }
    assert!(explained, "hovering the disabled width action must explain how to enable it");
}
