use crate::tests::*;

#[test]
fn the_output_dropdown_runs_track_output() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "name": "Vox"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    let fx = app.add_fx_channel().unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, t).output);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.label == "FX 1").cloned().expect("the FX channel is offered by name");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    app.run("track.output", json!({"tracks": [t2.0], "output": item.value})).unwrap();
    let s = app.engine.session();
    let (a, b) = (s.track(t).unwrap().mixer.output.clone(), s.track(t2).unwrap().mixer.output.clone());
    assert_eq!(a, b, "the dropdown and the command agree");
    assert_eq!(s.track(fx).unwrap().mixer.input, a, "the FX channel listens to that bus");
    assert_eq!(strip(&app, t).output_label, "FX 1");
}

#[test]
fn add_fx_channel_is_one_undo_step_and_adds_no_send() {
    let mut app = app();
    let r = app.run("track.new", json!({"kind": "audio", "name": "Gtr"}));
    let t = TrackId(r.unwrap()["tracks"][0].as_u64().unwrap());
    app.focus(t);
    let before = undo_len(&mut app);
    let tracks = app.engine.session().tracks.len();
    let fx = app.add_fx_channel().unwrap();
    let s = app.engine.session();
    let aux = s.track(fx).unwrap();
    assert_eq!(aux.kind, transit_model::TrackKind::Aux);
    let bus = s.bus_by_name("FX 1").unwrap().id;
    assert_eq!(aux.mixer.input, transit_model::Route::Bus(bus));
    assert!(s.track(t).unwrap().mixer.sends.iter().all(Option::is_none), "the selected, focused track gets no send (#145)");
    assert_eq!(undo_len(&mut app), before + 1);
    app.run("edit.undo", json!({})).unwrap();
    let s = app.engine.session();
    assert_eq!(s.tracks.len(), tracks);
    assert!(s.bus_by_name("FX 1").is_none());
    // A second one gets the next name and its own undo step.
    app.run("edit.redo", json!({})).unwrap();
    app.add_fx_channel().unwrap();
    assert!(app.engine.session().bus_by_name("FX 2").is_some());
    assert_eq!(undo_len(&mut app), before + 2);
    assert!(app.engine.session().track(t).unwrap().mixer.sends.iter().all(Option::is_none));
}

#[test]
fn the_plus_fx_channel_button_adds_the_channel_and_no_send() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(app.mixer.layout.add_fx);
    click(&mut app, at);
    frames(&mut app, &ctx, 3, &mut time);
    let s = app.engine.session();
    assert!(s.tracks.iter().any(|x| x.name == "FX 1"), "the FX channel exists");
    assert!(s.track(t).unwrap().mixer.sends.iter().all(Option::is_none), "and the focused track has no send");
}

#[test]
fn new_fx_channel_from_a_strip_is_one_undo_step_at_minus_12_db() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let tracks = app.engine.session().tracks.len();
    let before = undo_len(&mut app);
    let at = mid(strip(&app, t).add_send);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == crate::mixer::NEW_FX).cloned().expect("New FX Channel… in + Send");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let s = app.engine.session();
    let bus = s.bus_by_name("FX 1").expect("a bus").id;
    let fx = s.tracks.iter().find(|x| x.name == "FX 1").expect("an FX channel");
    assert_eq!((fx.kind, &fx.mixer.input), (transit_model::TrackKind::Aux, &transit_model::Route::Bus(bus)));
    let send = s.track(t).unwrap().mixer.sends[0].clone().unwrap();
    assert_eq!((send.target, send.level_db), (transit_model::Route::Bus(bus), -12.0));
    let fx_id = TrackId(fx.id.0);
    assert_eq!(undo_len(&mut app), before + 1);
    let fs = strip(&app, fx_id);
    assert!(fs.fx, "the FX channel's strip is marked");
    assert_eq!(fs.input_label, "Bus: FX 1", "its input shows the bus");
    app.run("edit.undo", json!({})).unwrap();
    let s = app.engine.session();
    assert_eq!(s.tracks.len(), tracks);
    assert!(s.bus_by_name("FX 1").is_none() && s.track(t).unwrap().mixer.sends.iter().all(Option::is_none));
}

#[test]
fn no_free_send_slot_leaves_no_bus_or_channel_behind() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "audio", "name": "Gtr"}));
    for i in 0..10 {
        app.run("mix.send", json!({"track": t.0, "bus": format!("B{i}")})).unwrap();
    }
    let (busses, tracks, undo) = (app.engine.session().busses.len(), app.engine.session().tracks.len(), undo_len(&mut app));
    assert!(app.new_fx_channel(Some(t), 0.0).is_err());
    assert!(app.status.contains("sends"), "{}", app.status);
    let s = app.engine.session();
    assert_eq!((s.busses.len(), s.tracks.len()), (busses, tracks));
    assert_eq!(undo_len(&mut app), undo);
    // A step that fails after the bus is made (no such track to send from) is taken back.
    assert!(app.new_fx_channel(Some(TrackId(999_999)), 0.0).is_err());
    let s = app.engine.session();
    assert_eq!((s.busses.len(), s.tracks.len()), (busses, tracks), "rolled back");
    assert_eq!(undo_len(&mut app), undo);
}

#[test]
fn picking_the_default_input_changes_nothing() {
    let (mut app, ctx, mut time, t) = mixer_app();
    assert_eq!(strip(&app, t).input_label, "In 1-2");
    let before = undo_len(&mut app);
    let at = mid(strip(&app, t).input);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.label == "In 1-2").cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(undo_len(&mut app), before, "no document edit");
    assert_eq!(app.engine.session().track(t).unwrap().mixer.input, transit_model::Route::None);
    // Another input is a real change.
    app.set_input(t, "In 3-4").unwrap();
    assert_eq!(undo_len(&mut app), before + 1);
}

#[test]
fn routing_menus_never_offer_a_loop() {
    use crate::mixer::{input_choices, output_choices, send_targets};
    let mut app = app();
    let a = new_track(&mut app, json!({"kind": "audio", "name": "A"}));
    app.focus(a);
    let fx1 = app.add_fx_channel().unwrap();
    app.add_send(a, "FX 1").unwrap();
    let fx2 = app.new_fx_channel(None, 0.0).unwrap();
    let fx3 = app.new_fx_channel(None, 0.0).unwrap();
    // A -> FX 1 -> FX 2 -> FX 3.
    app.set_output(fx1, "FX 2").unwrap();
    app.set_output(fx2, "FX 3").unwrap();
    let s = app.engine.session_arc();
    let io = app.engine.io_map().clone();
    let labels = |v: Vec<RoutingChoice>| {
        v.into_iter()
            .map(|choice| match choice {
                RoutingChoice::Separator => String::new(),
                RoutingChoice::Entry { label, .. } => label,
            })
            .collect::<Vec<_>>()
    };
    let out3 = labels(output_choices(&s, s.track(fx3).unwrap(), &io));
    assert!(!out3.iter().any(|l| l == "FX 1" || l == "FX 2" || l == "FX 3"), "three hops back: {out3:?}");
    let out2 = labels(output_choices(&s, s.track(fx2).unwrap(), &io));
    assert!(!out2.contains(&"FX 1".to_string()) && out2.contains(&"FX 3".to_string()), "{out2:?}");
    let sends3 = labels(send_targets(&s, s.track(fx3).unwrap(), None, &io));
    assert!(sends3.is_empty(), "an FX 3 send could only loop: {sends3:?}");
    let sends1 = labels(send_targets(&s, s.track(fx1).unwrap(), None, &io));
    assert_eq!(sends1, vec!["FX 2", "FX 3"], "a bus may feed another bus downstream");
    let ins = labels(input_choices(&s, s.track(a).unwrap(), 2, &io));
    assert!(ins.iter().all(|l| !l.starts_with("Bus")), "A feeds all three busses: {ins:?}");
    // The dropdown in the window agrees.
    app.ui.show_mixer = true;
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, fx3).output);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let menu: Vec<String> = app.mixer.layout.menu.iter().map(|m| m.label.clone()).collect();
    assert!(menu.contains(&"Main".to_string()) && !menu.contains(&"FX 1".to_string()), "{menu:?}");
}

#[test]
fn a_send_target_click_retargets_and_a_right_click_has_pre_post_and_remove() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.new_fx_channel(Some(t), 0.0).unwrap();
    app.new_fx_channel(None, 0.0).unwrap();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let send = |app: &StudioApp| app.engine.session().track(t).unwrap().mixer.sends[0].clone().unwrap();
    for wide in [false, true] {
        let menu_of = |app: &StudioApp| if wide { app.mixer.devices.menu.clone() } else { app.mixer.layout.menu.clone() };
        let row_of = |app: &StudioApp| if wide { app.mixer.devices.sends[0].clone() } else { strip(app, t).sends[0].clone() };
        let pre0 = send(&app).pre_fader;
        // A primary click opens the retarget menu and none of the context items.
        let at = mid(row_of(&app).target_rect);
        click(&mut app, at);
        frames(&mut app, &ctx, 6, &mut time);
        let menu = menu_of(&app);
        let labels: Vec<&str> = menu.iter().map(|m| m.label.as_str()).collect();
        assert!(labels.contains(&"FX 1") && labels.contains(&"FX 2"), "targets listed (wide={wide}): {labels:?}");
        assert!(!labels.iter().any(|l| l.ends_with("-fader") || *l == "Remove Send"), "no context items: {labels:?}");
        let cur = send(&app).target;
        let current = row_of(&app).target;
        let pick = menu.iter().find(|m| m.label != current && !m.value.is_empty()).cloned().expect("another target");
        click(&mut app, mid(pick.rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert_ne!(send(&app).target, cur, "picking {} retargeted (wide={wide})", pick.label);
        // A right-click opens only the context items; Pre-fader toggles the send.
        let at = mid(row_of(&app).target_rect);
        click_right(&mut app, at);
        frames(&mut app, &ctx, 6, &mut time);
        let labels: Vec<String> = menu_of(&app).iter().map(|m| m.label.clone()).collect();
        assert_eq!(labels, ["Pre-fader", "Remove Send"], "only the context items (wide={wide})");
        let item = menu_of(&app).into_iter().find(|m| m.label == "Pre-fader").unwrap();
        click(&mut app, mid(item.rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert_eq!(send(&app).pre_fader, !pre0, "Pre-fader toggled (wide={wide})");
        // Back to post for the next pass.
        let at = mid(row_of(&app).target_rect);
        click_right(&mut app, at);
        frames(&mut app, &ctx, 6, &mut time);
        let item = menu_of(&app).into_iter().find(|m| m.label == "Post-fader").expect("Post-fader");
        click(&mut app, mid(item.rect));
        frames(&mut app, &ctx, 6, &mut time);
        assert_eq!(send(&app).pre_fader, pre0);
    }
}

fn status_of(app: &StudioApp) -> String {
    app.status.clone()
}

#[test]
fn deleting_an_fx_channel_takes_its_bus_and_sends_in_one_undo_step() {
    let (mut app, _ctx, _time, t) = mixer_app();
    let fx = app.new_fx_channel(Some(t), -7.0).unwrap();
    let before = undo_len(&mut app);
    app.delete_track(fx).unwrap();
    let s = app.engine.session();
    assert!(s.track(fx).is_none() && s.busses.is_empty(), "track and bus gone");
    assert!(s.track(t).unwrap().mixer.sends.iter().all(Option::is_none), "the send is gone");
    assert_eq!(status_of(&app), "Removed FX 1 and its send");
    assert_eq!(undo_len(&mut app), before + 1, "one undo entry");
    app.run("edit.undo", json!({})).unwrap();
    let s = app.engine.session();
    let bus = s.bus_by_name("FX 1").expect("bus is back").id;
    assert_eq!(s.track(fx).unwrap().mixer.input, transit_model::Route::Bus(bus));
    let send = s.track(t).unwrap().mixer.sends[0].clone().expect("send is back");
    assert_eq!((send.target, send.level_db), (transit_model::Route::Bus(bus), -7.0));
}

#[test]
fn deleting_an_fx_channel_with_two_senders_says_so() {
    let (mut app, _ctx, _time, t) = mixer_app();
    let v = new_track(&mut app, json!({"kind": "audio", "name": "Vox"}));
    let fx = app.new_fx_channel(Some(t), 0.0).unwrap();
    app.add_send(v, "FX 1").unwrap();
    let before = undo_len(&mut app);
    app.delete_track(fx).unwrap();
    assert_eq!(status_of(&app), "Removed FX 1 and its 2 sends");
    assert_eq!(undo_len(&mut app), before + 1);
    assert!(app.engine.session().busses.is_empty());
}

#[test]
fn deleting_an_fx_channel_whose_bus_feeds_another_aux_keeps_the_bus() {
    let (mut app, _ctx, _time, t) = mixer_app();
    let fx = app.new_fx_channel(Some(t), 0.0).unwrap();
    let other = new_track(&mut app, json!({"kind": "aux", "format": "stereo", "name": "Sub"}));
    app.run("track.input", json!({"tracks": [other.0], "input": "FX 1"})).unwrap();
    app.delete_track(fx).unwrap();
    let s = app.engine.session();
    assert!(s.track(fx).is_none());
    assert!(s.bus_by_name("FX 1").is_some(), "the bus stays for the other aux");
    assert!(s.track(t).unwrap().mixer.sends[0].is_some(), "and its send");
    assert!(status_of(&app).contains("bus stays"));
}

#[test]
fn delete_key_on_a_selected_fx_track_runs_the_composite() {
    let (mut app, _ctx, _time, t) = mixer_app();
    let fx = app.new_fx_channel(Some(t), 0.0).unwrap();
    app.focus_header(fx, false);
    app.delete_selected().unwrap();
    let s = app.engine.session();
    assert!(s.track(fx).is_none() && s.busses.is_empty());
}

#[test]
fn track_header_menu_delete_uses_the_composite() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let fx = app.new_fx_channel(Some(t), 0.0).unwrap();
    app.ui.show_mixer = false;
    frames(&mut app, &ctx, 4, &mut time);
    let hdr = app.arrange.layout.tracks.iter().find(|r| r.id == fx.0).unwrap().header;
    let head_mid = (hdr[0] + 30.0, (hdr[1] + hdr[3]) / 2.0);
    right_click(&mut app, head_mid);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.arrange.menu.iter().find(|m| m.label == "Delete Track").cloned().expect("Delete Track");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().busses.is_empty(), "the bus went with it");
}

#[test]
fn orphan_buses_leave_the_routing_menus_and_one_entry_removes_them() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.run("mix.new_bus", json!({"name": "Lost 1"})).unwrap();
    app.run("mix.new_bus", json!({"name": "Lost 2"})).unwrap();
    let fx = app.new_fx_channel(Some(t), 0.0).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    // The + Send menu: the FX channel, never the orphans, and the entry at the end.
    let at = mid(strip(&app, t).output);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let labels: Vec<String> = app.mixer.layout.menu.iter().map(|m| m.label.clone()).collect();
    assert!(labels.contains(&"FX 1".to_string()), "{labels:?}");
    assert!(!labels.iter().any(|l| l.contains("Lost")), "{labels:?}");
    assert_eq!(labels.last().map(String::as_str), Some("Remove unused buses (2)"));
    let s = app.engine.session_arc();
    assert!(crate::mixer::send_targets(&s, s.track(fx).unwrap(), None, &Default::default()).is_empty());
    assert!(
        !crate::mixer::input_choices(&s, s.track(t).unwrap(), 2, &Default::default())
            .iter()
            .any(|choice| matches!(choice, RoutingChoice::Entry { label, .. } if label.contains("Lost")))
    );
    let before = undo_len(&mut app);
    let item = app.mixer.layout.menu.last().cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let names: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    assert_eq!(names, vec!["FX 1"], "both orphans removed");
    assert_eq!(undo_len(&mut app), before + 1, "one undo entry");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().busses.len(), 3);
}

#[test]
fn remove_unused_buses_says_what_it_cleared() {
    let (mut app, _ctx, _time, t) = mixer_app();
    app.run("mix.new_bus", json!({"name": "Lost 1"})).unwrap();
    app.run("mix.new_bus", json!({"name": "Lost 2"})).unwrap();
    app.run("mix.send", json!({"tracks": [t.0], "slot": 0, "target": "Lost 1", "level": 0.0})).ok();
    app.run("track.output", json!({"tracks": [t.0], "output": "Lost 2"})).ok();
    app.remove_unused_busses().unwrap();
    assert!(app.status.starts_with("Removed 2 unused buses"), "{}", app.status);
    assert!(app.status.contains("cleared"), "{}", app.status);
}

mod output_paths {
    use super::*;
    use crate::mixer::{input_choices, output_choices, path_choices, send_targets};
    use transit_model::{IoDirection, IoMap, Route};

    /// Eight in, six out; "Out 3-4" renamed "Headphones", "In 5" renamed "Guitar".
    fn map() -> IoMap {
        let mut io = IoMap::for_device("ASIO", "Test", 8, 6);
        io.rename_path("Out 3-4", "Headphones").unwrap();
        io.rename_path("In 5", "Guitar").unwrap();
        io
    }

    fn click_item(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, label: &str) {
        frames(app, ctx, 6, time);
        let item = app.mixer.layout.menu.iter().find(|m| m.label == label).cloned().unwrap_or_else(|| panic!("no `{label}` in the menu"));
        click(app, mid(item.rect));
        frames(app, ctx, 6, time);
    }

    #[test]
    fn path_labels_say_the_channels_and_main_is_left_out() {
        let io = map();
        let out: Vec<_> = path_choices(&io, IoDirection::Output)
            .into_iter()
            .map(|choice| match choice {
                RoutingChoice::Entry { label, value } => format!("{label}={value}"),
                RoutingChoice::Separator => panic!("path choices have no separators"),
            })
            .collect();
        assert_eq!(out, ["Headphones (Out 3-4)=Headphones", "Out 5-6=Out 5-6"]);
        let ins = path_choices(&io, IoDirection::Input);
        assert!(
            ins.contains(&RoutingChoice::Entry { label: "Guitar (In 5)".into(), value: "Guitar".into() })
                && ins.contains(&RoutingChoice::Entry { label: "In 1-2".into(), value: "In 1-2".into() })
        );
    }

    #[test]
    fn the_lists_order_main_buses_separator_then_paths() {
        let mut app = app();
        let t = new_track(&mut app, json!({"kind": "audio"}));
        let fx = app.add_fx_channel().unwrap();
        let s = app.engine.session_arc();
        let values = |v: Vec<RoutingChoice>| {
            v.into_iter()
                .map(|choice| match choice {
                    RoutingChoice::Separator => String::new(),
                    RoutingChoice::Entry { label, .. } => label,
                })
                .collect::<Vec<_>>()
        };
        let out = values(output_choices(&s, s.track(t).unwrap(), &map()));
        assert_eq!(out, ["Main", "FX 1", "", "Headphones (Out 3-4)", "Out 5-6", "No output"]);
        let other = new_track(&mut app, json!({"kind": "audio"}));
        let s = app.engine.session_arc();
        let sends = values(send_targets(&s, s.track(other).unwrap(), None, &map()));
        assert_eq!(sends, ["FX 1", "", "Headphones (Out 3-4)", "Out 5-6"]);
        // No paths beyond Main: no separator.
        let plain = values(output_choices(&s, s.track(fx).unwrap(), &IoMap::default()));
        assert!(!plain.contains(&String::new()), "{plain:?}");
    }

    #[test]
    fn a_route_named_like_the_old_separator_is_selectable_beside_a_separator() {
        for send in [false, true] {
            let (mut app, ctx, mut time, t) = mixer_app();
            let name = "\u{1}separator";
            let label = format!("{name} (Out 3-4)");
            let mut io = map();
            io.rename_path("Headphones", name).unwrap();
            let s = app.engine.session();
            let track = s.track(t).unwrap();
            let choices = if send { send_targets(s, track, None, &io) } else { output_choices(s, track, &io) };
            let separator = choices.iter().position(|choice| *choice == RoutingChoice::Separator).unwrap();
            assert_eq!(choices[separator + 1], RoutingChoice::Entry { label: label.clone(), value: name.into() });
            app.engine.set_io_map(io);
            frames(&mut app, &ctx, 3, &mut time);
            let before = undo_len(&mut app);
            let at = mid(if send { strip(&app, t).add_send } else { strip(&app, t).output });
            click(&mut app, at);
            click_item(&mut app, &ctx, &mut time, &label);
            let track = app.engine.session().track(t).unwrap();
            let route = if send { &track.mixer.sends[0].as_ref().unwrap().target } else { &track.mixer.output };
            assert_eq!(route, &Route::Hardware(name.into()));
            assert_eq!(undo_len(&mut app), before + 1, "one routing command");
        }
    }

    #[test]
    fn the_output_dropdown_lists_a_path_and_picking_it_runs_track_output_once() {
        let (mut app, ctx, mut time, t) = mixer_app();
        app.engine.set_io_map(map());
        frames(&mut app, &ctx, 3, &mut time);
        let before = undo_len(&mut app);
        let at = mid(strip(&app, t).output);
        click(&mut app, at);
        click_item(&mut app, &ctx, &mut time, "Headphones (Out 3-4)");
        assert_eq!(undo_len(&mut app), before + 1, "one track.output");
        assert_eq!(app.engine.session().track(t).unwrap().mixer.output, Route::Hardware("Headphones".into()));
        assert_eq!(strip(&app, t).output_label, "Headphones");
        assert!(app.io_warning().is_none());
    }

    #[test]
    fn a_send_to_a_path_runs_mix_send() {
        let (mut app, ctx, mut time, t) = mixer_app();
        app.engine.set_io_map(map());
        frames(&mut app, &ctx, 3, &mut time);
        let at = mid(strip(&app, t).add_send);
        click(&mut app, at);
        click_item(&mut app, &ctx, &mut time, "Out 5-6");
        let sends = &app.engine.session().track(t).unwrap().mixer.sends;
        assert_eq!(sends[0].as_ref().map(|x| x.target.clone()), Some(Route::Hardware("Out 5-6".into())));
        assert_eq!(app.mixer.devices.sends[0].target, "Out 5-6");
    }

    #[test]
    fn a_renamed_input_path_replaces_its_legacy_label() {
        let mut app = app();
        let r = app.add_audio_track("mono");
        let t = new_audio(&mut app, r);
        app.engine.set_io_map(map());
        let s = app.engine.session_arc();
        let labels: Vec<String> = input_choices(&s, s.track(t).unwrap(), 8, app.engine.io_map())
            .into_iter()
            .map(|choice| match choice {
                RoutingChoice::Entry { label, .. } => label,
                RoutingChoice::Separator => String::new(),
            })
            .collect();
        assert!(labels.contains(&"Guitar (In 5)".to_string()), "{labels:?}");
        assert!(!labels.contains(&"In 5".to_string()), "{labels:?}");
        assert_eq!(labels.iter().filter(|l| *l == "In 4").count(), 1, "a path named like its label is listed once: {labels:?}");
        app.set_input(t, "Guitar").unwrap();
        assert_eq!(input_label(&app, t), "Guitar");
    }

    #[test]
    fn the_input_dropdown_shows_the_path_name() {
        let (mut app, ctx, mut time, _) = mixer_app();
        let r = app.add_audio_track("mono");
        let t = new_audio(&mut app, r);
        app.engine.set_io_map(map());
        frames(&mut app, &ctx, 3, &mut time);
        let at = mid(strip(&app, t).input);
        click(&mut app, at);
        frames(&mut app, &ctx, 6, &mut time);
        let labels: Vec<String> = app.mixer.layout.menu.iter().map(|m| m.label.clone()).collect();
        assert!(labels.iter().any(|l| l.starts_with("Guitar")) && !labels.contains(&"In 5".to_string()), "{labels:?}");
    }

    #[test]
    fn an_unresolved_route_is_marked_and_the_status_line_counts_it() {
        let (mut app, ctx, mut time, t) = mixer_app();
        assert!(app.io_warning().is_none());
        app.engine.set_io_map(map());
        app.run("track.output", json!({"tracks": [t.0], "output": "Headphones"})).unwrap();
        assert!(app.io_warning().is_none());
        // The path goes away (another machine): the route keeps its name and plays silent.
        app.engine.set_io_map(IoMap::default());
        frames(&mut app, &ctx, 3, &mut time);
        assert_eq!(app.io_warning().as_deref(), Some("1 route not on this device"));
        assert_eq!(strip(&app, t).output_label, "Headphones");
        assert_eq!(app.engine.session().track(t).unwrap().mixer.output, Route::Hardware("Headphones".into()));
    }

    #[test]
    fn overlapping_output_paths_are_named_in_the_status_line() {
        let mut app = app();
        let mut io = map();
        io.set_channels("Out 5-6", vec![2, 3, 4]).unwrap();
        app.engine.set_io_map(io);
        assert_eq!(app.io_warning().as_deref(), Some("Output paths overlap: Headphones, Out 5-6"));
    }
}

#[test]
fn routing_menus_reject_loops_through_sidechain_keys() {
    use crate::mixer::{input_choices, output_choices, send_targets};
    let mut app = app();
    let source = new_track(&mut app, json!({"kind": "aux", "name": "Source"}));
    let fx = app.new_fx_channel(None, 0.0).unwrap();
    app.add_device(source, "compressor").unwrap();
    app.run("mix.insert_key", json!({"track": source.0, "slot": 0, "from": {"track": fx.0}})).unwrap();
    let s = app.engine.session();
    let track = s.track(source).unwrap();
    let io = app.engine.io_map().clone();
    for choices in [output_choices(s, track, &io), send_targets(s, track, None, &io)] {
        assert!(
            !choices.iter().any(|choice| matches!(choice, RoutingChoice::Entry { value, .. } if value == "FX 1")),
            "a key edge returns to Source"
        );
    }
    // Reversing the dependency: Source now feeds FX 1, so FX 1 cannot key Source
    // through a bus that Source listens to.
    app.run("mix.insert_key", json!({"track": source.0, "slot": 0, "from": null})).unwrap();
    app.add_device(fx, "compressor").unwrap();
    app.run("mix.insert_key", json!({"track": fx.0, "slot": 0, "from": {"track": source.0}})).unwrap();
    app.new_fx_channel(None, 0.0).unwrap();
    app.set_output(fx, "FX 2").unwrap();
    let s = app.engine.session();
    assert!(
        !input_choices(s, s.track(source).unwrap(), 2, &io)
            .iter()
            .any(|choice| matches!(choice, RoutingChoice::Entry { value, .. } if value == "FX 2"))
    );
}

#[test]
fn audio_bus_input_policy_is_preserved_when_the_input_is_chosen_first() {
    let mut app = app();
    let source = new_track(&mut app, json!({"kind": "audio", "name": "Source"}));
    app.new_fx_channel(None, 0.0).unwrap();
    app.run("track.input", json!({"tracks": [source.0], "input": "FX 1"})).unwrap();
    let s = app.engine.session();
    let track = s.track(source).unwrap();
    for choices in [crate::mixer::output_choices(s, track, app.engine.io_map()), crate::mixer::send_targets(s, track, None, app.engine.io_map())] {
        assert!(!choices.iter().any(|choice| matches!(choice, RoutingChoice::Entry { value, .. } if value == "FX 1")));
    }
}

#[test]
fn a_midi_strip_has_the_output_destination_button_next_to_its_input() {
    // #61: the destination picker lives on the mixer strip; the header picker is a follow-up.
    let (mut app, ctx, mut time, _) = mixer_app();
    let t = TrackId(app.run("track.new", json!({"kind": "midi", "name": "Synth"})).unwrap()["tracks"][0].as_u64().unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    let s = strip(&app, t);
    assert!(
        s.midi_output[2] > s.midi_output[0] && s.midi_output[0] >= s.input[2],
        "Out sits right of the MIDI input: {:?} {:?}",
        s.midi_output,
        s.input
    );
    assert_eq!(s.midi_output_label, "MIDI to the track's instrument");
    app.run("midi.output", json!({"track": t.0, "port": {"backend": "winmm", "name": "Synth OUT"}, "channel": 2})).unwrap();
    frames(&mut app, &ctx, 2, &mut time);
    let s = strip(&app, t);
    assert_eq!(s.midi_output_label, "MIDI to Synth OUT · Ch 2");
    assert!(!s.midi_output_missing, "without a player nothing is known to be missing");
}
