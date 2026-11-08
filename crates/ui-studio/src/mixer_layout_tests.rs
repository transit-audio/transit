use crate::tests::*;

#[test]
fn mixer_aux_block_keeps_groups_inline_and_leaves_the_document_and_controls_intact() {
    let (mut app, ctx, mut time, audio) = mixer_app();
    let fx = app.new_fx_channel(Some(audio), -12.0).unwrap();
    let r = app.run("track.new", json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let keys = TrackId(r["tracks"][0].as_u64().unwrap());
    let group = app.group_tracks(&[audio, keys]).unwrap();
    let bus = app.new_fx_channel(None, 0.0).unwrap();
    let r = app.run("track.new", json!({"kind": "midi", "name": "MIDI"})).unwrap();
    let midi = TrackId(r["tracks"][0].as_u64().unwrap());
    app.run("track.new", json!({"kind": "master"})).unwrap();
    app.focus(audio);
    let before = serde_json::to_value(app.engine.session()).unwrap();
    let history = undo_len(&mut app);
    for full in [false, true] {
        app.ui.mixer_full = full;
        frames(&mut app, &ctx, 3, &mut time);
        let layout = &app.mixer.layout;
        assert_eq!(layout.strips.iter().map(|s| s.track).collect::<Vec<_>>(), [group.0, audio.0, keys.0, midi.0, fx.0, bus.0]);
        let divider = layout.fx_divider.expect("two blocks have a divider");
        assert!(strip(&app, midi).rect[2] < divider[0]);
        assert!(divider[2] < strip(&app, fx).rect[0]);
        assert!(strip(&app, bus).rect[2] < layout.master.as_ref().unwrap().rect[0]);
        assert_eq!(layout.groups.len(), 1);
        assert_eq!(layout.groups[0].members, [audio.0, keys.0]);
        assert_eq!(layout.groups[0].rect[2], strip(&app, keys).rect[2] - 2.0);
        assert_eq!(serde_json::to_value(app.engine.session()).unwrap(), before);
        assert_eq!(undo_len(&mut app), history);
    }
    // The relocated strip still sends a click to its own track, not the old positional neighbor.
    let mute = strip(&app, fx).mute;
    click(&mut app, mid(mute));
    frames(&mut app, &ctx, 30, &mut time);
    assert!(app.engine.session().track(fx).unwrap().mixer.mute);
    assert!(!app.engine.session().track(midi).unwrap().mixer.mute);
    assert!(app.engine.undo());
    assert_eq!(serde_json::to_value(app.engine.session()).unwrap(), before);
}

#[test]
fn mixer_aux_divider_scrolls_with_the_block_while_master_stays_pinned() {
    let (mut app, ctx, mut time, _) = mixer_app();
    let fx = app.new_fx_channel(None, 0.0).unwrap();
    for _ in 0..18 {
        app.run("track.new", json!({"kind": "audio"})).unwrap();
    }
    frames(&mut app, &ctx, 3, &mut time);
    let master = app.mixer.layout.master.clone().unwrap().rect;
    assert!(!app.mixer.layout.strips.iter().any(|s| s.track == fx.0));
    assert!(app.mixer.layout.fx_divider.is_none(), "offscreen divider is clipped");
    app.mixer.hscroll = f32::MAX;
    frames(&mut app, &ctx, 3, &mut time);
    let layout = &app.mixer.layout;
    let divider = layout.fx_divider.expect("scrolling reaches the aux divider");
    let fx_rect = strip(&app, fx).rect;
    assert_eq!(layout.master.as_ref().unwrap().rect, master);
    assert!(divider[0] > layout.area[0] && divider[2] < fx_rect[0]);
    assert!(fx_rect[2] < layout.add_fx[0] && layout.add_fx[2] < master[0], "the final strip and Add FX remain reachable");
    let old_x = divider[0];
    app.mixer.hscroll -= 20.0;
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.mixer.layout.fx_divider.unwrap()[0], old_x + 20.0);
}

#[test]
fn mixer_aux_block_respects_collapsed_groups_and_omits_empty_dividers() {
    let (mut app, ctx, mut time, audio) = mixer_app();
    assert!(app.mixer.layout.fx_divider.is_none());
    let fx = app.new_fx_channel(None, 0.0).unwrap();
    let group = app.group_tracks(&[audio, fx]).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.mixer.layout.strips.iter().map(|s| s.track).collect::<Vec<_>>(), [group.0, audio.0, fx.0]);
    assert!(app.mixer.layout.fx_divider.is_some());
    assert_eq!(app.mixer.layout.groups[0].members, [audio.0]);
    app.toggle_group(group);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.mixer.layout.strips.iter().map(|s| s.track).collect::<Vec<_>>(), [group.0]);
    assert!(app.mixer.layout.fx_divider.is_none());
    assert!(app.mixer.layout.groups.is_empty());
    app.run("track.hide", json!({"tracks": [group.0], "hidden": true})).unwrap();
    let outside = app.new_fx_channel(None, 0.0).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.mixer.layout.strips.iter().map(|s| s.track).collect::<Vec<_>>(), [outside.0]);
    assert!(app.mixer.layout.fx_divider.is_none(), "an aux-only mixer needs no separator");
    app.run("track.hide", json!({"tracks": [outside.0], "hidden": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.mixer.layout.strips.is_empty());
    assert!(app.mixer.layout.fx_divider.is_none());
}

#[test]
fn a_click_on_empty_mixer_space_clears_the_chosen_track() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.chosen_track(), Some(t));
    let add = app.mixer.layout.add_fx;
    click(&mut app, (add[2] + 60.0, add[1] + 200.0));
    frames(&mut app, &ctx, 8, &mut time);
    assert_eq!(app.chosen_track(), None);
    assert!(!strip(&app, t).focused, "no strip is highlighted");
    assert_eq!(app.cursor_track(), Some(t), "the edit cursor keeps its track");
    // A click on a strip's name still focuses it.
    let at = mid(strip(&app, t).name_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.chosen_track(), Some(t));
}

#[test]
fn escape_and_a_click_on_empty_space_clear_the_focused_track() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.focus_track(), Some(t));
    assert!(strip(&app, t).focused, "the strip is highlighted");
    key(&mut app, "Escape");
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(app.chosen_track(), None, "Escape drops the chosen track");
    assert_eq!(app.focus_track(), Some(t), "the edit cursor keeps its track (paste, split, new clip)");
    assert_eq!(app.cursor_track(), Some(t));
    assert!(app.engine.session().edit.selected_tracks.is_empty());
    assert!(!strip(&app, t).focused, "no strip is highlighted");
    // Click on empty space below the lanes.
    app.focus(t);
    assert_eq!(app.focus_track(), Some(t));
    app.click_empty(None, 1000);
    assert_eq!(app.chosen_track(), None, "a click on empty space drops it too");
    assert_eq!(app.cursor_track(), Some(t));
    frames(&mut app, &ctx, 3, &mut time);
    assert!(!strip(&app, t).focused);
    // + FX Channel then has no implicit source.
    app.add_fx_channel().unwrap();
    assert!(app.engine.session().track(t).unwrap().mixer.sends.iter().all(Option::is_none));
}

#[test]
fn focus_follows_both_views_and_views_never_dirty_the_document() {
    let (mut app, ctx, mut time, t) = mixer_app();
    // The test window is 800 tall: the docked pane at its minimum leaves the arrangement room for
    // the second track's header beside the device chain.
    app.ui.mixer_height = crate::mixer::min_pane_height();
    let r = app.run("track.new", json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    let path = std::env::temp_dir().join(format!("transit-mixer-{}.sc", std::process::id()));
    app.run("session.save_as", json!({"path": path.to_string_lossy(), "replace": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    // Mixer to arrangement: click the first strip's name.
    let at = mid(strip(&app, t).name_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(app.focus_track(), Some(t));
    assert_eq!(app.engine.session().edit.selected_tracks, vec![t], "the arrangement's selected track");
    assert_eq!(app.mixer.devices.track, Some(t.0));
    // Arrangement to mixer: click the second track's header.
    let hdr = app.arrange.layout.tracks.iter().find(|r| r.id == t2.0).unwrap().header;
    click(&mut app, (hdr[0] + 30.0, (hdr[1] + hdr[3]) / 2.0));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(app.focus_track(), Some(t2));
    assert_eq!(app.mixer.devices.track, Some(t2.0));
    // View toggles: F3 twice, Ctrl+Alt+M and Ctrl+Alt+L are view state only.
    for (key, mods) in [("F3", json!({})), ("F3", json!({})), ("M", json!({"cmd": true, "alt": true})), ("L", json!({"cmd": true, "alt": true}))] {
        let mut p = json!({"key": key});
        if let (Some(o), Some(m)) = (p.as_object_mut(), mods.as_object()) {
            o.extend(m.clone());
        }
        let (req, _rx) = crate::ControlRequest::new("ui.key", p);
        let _ = crate::control::handle_for_test(&mut app, &ctx, &req);
        frames(&mut app, &ctx, 4, &mut time);
    }
    assert!(!app.ui.show_mixer && !app.ui.show_devices && !app.ui.mixer_full, "{:?}", (app.ui.show_mixer, app.ui.show_devices, app.ui.mixer_full));
    assert!(!app.engine.is_dirty(), "focus and panes are view state");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_master_or_main_out_strip_is_inside_the_mixer_pane_at_any_scale() {
    for ppp in [1.0f32, 1.5] {
        for master in [false, true] {
            let (mut app, ctx, mut time, _t) = mixer_app();
            ctx.set_pixels_per_point(ppp);
            app.new_fx_channel(None, 0.0).unwrap();
            if master {
                app.run("track.new", json!({"kind": "master"})).unwrap();
            }
            frames(&mut app, &ctx, 6, &mut time);
            let l = &app.mixer.layout;
            let m = l.master.clone().unwrap_or_else(|| panic!("no master strip (ppp {ppp}, master {master})"));
            let (a, r) = (l.area, m.rect);
            eprintln!("ppp {ppp} master {master}: area {a:?} strip {r:?} strips {}", l.strips.len());
            assert!(r[0] >= a[0] && r[2] <= a[2] && r[1] >= a[1] && r[3] <= a[3], "ppp {ppp}, master {master}: {r:?} not inside {a:?}");
            assert!(r[2] > r[0] && r[3] > r[1]);
            if master {
                assert_eq!(m.kind, Some(transit_model::TrackKind::Master));
            } else {
                assert!(l.add_master[2] > l.add_master[0], "the offer to add a master fader");
            }
        }
    }
}

#[test]
fn a_strip_plus_adds_an_insert_to_that_track_whatever_is_focused() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    app.focus(a);
    full_mixer(&mut app, &ctx, &mut time);
    assert!(strip(&app, a).focused && !strip(&app, b).focused, "the focused strip is marked");
    let before = undo_len(&mut app);
    let plus = mid(strip(&app, b).add_insert);
    click(&mut app, plus);
    frames(&mut app, &ctx, 6, &mut time);
    app.mixer.effect_filter = "gain".into();
    frames(&mut app, &ctx, 3, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == "gain").cloned().expect("the strip + menu lists Gain");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    let s = app.engine.session();
    assert!(s.track(b).unwrap().mixer.inserts[0].is_some(), "Vox got the insert");
    assert!(s.track(a).unwrap().mixer.inserts.iter().all(Option::is_none), "the focused track did not");
    assert_eq!(undo_len(&mut app), before + 1);
    assert_eq!(strip(&app, b).inserts.len(), 1);
    assert_eq!(strip(&app, b).inserts[0].name, "Gain");
    // Right-click: bypass, then remove.
    let row = strip(&app, b).inserts[0].rect;
    click_right(&mut app, mid(row));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == "bypass").cloned().expect("Bypass");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(b).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
    let row = strip(&app, b).inserts[0].rect;
    click_right(&mut app, mid(row));
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == "remove").cloned().expect("Remove");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(app.engine.session().track(b).unwrap().mixer.inserts.iter().all(Option::is_none));
}

#[test]
fn insert_rows_cap_and_keep_every_fader_aligned() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    for _ in 0..5 {
        app.add_device(a, "gain").unwrap();
    }
    full_mixer(&mut app, &ctx, &mut time);
    let (sa, sb) = (strip(&app, a), strip(&app, b));
    // Past three the rows scroll in the fixed box (Strip details 12); the "n more…" row is gone.
    let ov = sa.inserts_overflow.clone().expect("five inserts overflow");
    assert_eq!((sa.inserts.len(), ov.hidden), (5, 2), "every insert is a row; two lie outside the view");
    assert!(sa.add_insert[3] > sa.add_insert[1], "the + stays while the list overflows");
    assert!(sa.add_insert[1] >= ov.view[3], "the + is pinned below the rows' view");
    assert_eq!((sa.fader[1], sa.fader[3]), (sb.fader[1], sb.fader[3]), "the faders line up with and without inserts");
    assert!(sb.inserts_overflow.is_none(), "a strip without inserts does not overflow");
}

#[test]
fn the_insert_plus_is_always_there_and_opens_its_menu() {
    // The docked pane at both heights shows the short variant (its + is in the FX list); the full
    // mixer window shows the full-height section.
    for height in [260.0, 420.0, 0.0] {
        // 10 is every slot: nothing left to add to, so no +.
        for n in [0usize, 1, 4, 9, 10] {
            let (mut app, ctx, mut time, a) = mixer_app();
            app.ui.mixer_height = height;
            for _ in 0..n {
                app.add_device(a, "gain").unwrap();
            }
            frames(&mut app, &ctx, 4, &mut time);
            if height == 0.0 {
                full_mixer(&mut app, &ctx, &mut time);
            } else {
                assert!(strip(&app, a).short, "height {height}: the docked pane is short");
                let fx = mid(strip(&app, a).fx_button);
                click(&mut app, fx);
                frames(&mut app, &ctx, 4, &mut time);
            }
            let s = strip(&app, a);
            if n == 10 {
                assert_eq!(s.add_insert, [0.0; 4], "a full strip has no +");
                continue;
            }
            assert!(s.add_insert[2] > s.add_insert[0] && s.add_insert[3] > s.add_insert[1], "height {height}, {n} inserts: a + is drawn");
            click(&mut app, mid(s.add_insert));
            frames(&mut app, &ctx, 6, &mut time);
            assert!(app.mixer.layout.menu.iter().any(|m| m.value == "gain"), "height {height}, {n} inserts: the + opens the menu");
        }
    }
}

#[test]
fn adding_through_a_strip_plus_focuses_that_strip() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    app.focus(a);
    full_mixer(&mut app, &ctx, &mut time);
    assert!(!strip(&app, b).focused);
    let plus = mid(strip(&app, b).add_insert);
    click(&mut app, plus);
    frames(&mut app, &ctx, 6, &mut time);
    app.mixer.effect_filter = "gain".into();
    frames(&mut app, &ctx, 3, &mut time);
    let item = app.mixer.layout.menu.iter().find(|m| m.value == "gain").cloned().unwrap();
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert!(strip(&app, b).focused && !strip(&app, a).focused, "the device chain follows the strip that got the insert");
}

#[test]
fn clicking_anywhere_on_a_strip_focuses_it_and_the_highlight_follows() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    for t in [b, a, b] {
        let s = strip(&app, t);
        // The strip's empty margin below the input row, not a control.
        click(&mut app, (s.rect[0] + 2.0, (s.rect[1] + s.rect[3]) / 2.0));
        frames(&mut app, &ctx, 4, &mut time);
        assert_eq!(app.focus_track(), Some(t));
        let other = if t == a { b } else { a };
        assert!(strip(&app, t).focused && !strip(&app, other).focused, "highlight follows focus");
    }
}

/// Capture the real UI's paint, with a scripted frame time rather than wall-clock pacing.
fn mixer_paint(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64) -> Vec<egui::Shape> {
    *time += 1.0 / 60.0;
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
        time: Some(*time),
        ..Default::default()
    };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    fn flatten(shape: egui::Shape, result: &mut Vec<egui::Shape>) {
        if let egui::Shape::Vec(shapes) = shape {
            for shape in shapes {
                flatten(shape, result);
            }
        } else {
            result.push(shape);
        }
    }
    let mut result = Vec::new();
    for shape in out.shapes {
        flatten(shape.shape, &mut result);
    }
    result
}

#[test]
fn the_pan_dial_paints_with_the_kit_tokens_on_normal_and_selected_audio_and_aux_strips() {
    for appearance in [crate::theme::Appearance::Dark, crate::theme::Appearance::Light] {
        let (mut app, ctx, mut time, audio) = mixer_app();
        let aux = app.new_fx_channel(Some(audio), -12.0).unwrap();
        app.ui.appearance = appearance;
        app.ui.show_devices = false;
        for focus in [audio, aux] {
            app.focus(focus);
            frames(&mut app, &ctx, 3, &mut time);
            let shapes = mixer_paint(&mut app, &ctx, &mut time);
            let t: &crate::theme::Tokens = &crate::theme::TOKENS;
            for track in [audio, aux] {
                let sl = strip(&app, track);
                let rect = |r: [f32; 4]| egui::Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]));
                let background = shapes
                    .iter()
                    .find_map(|s| match s {
                        egui::Shape::Rect(r) if r.rect == rect(sl.rect) && r.fill != egui::Color32::TRANSPARENT => Some(r.fill),
                        _ => None,
                    })
                    .expect("painted strip background");
                assert_eq!(background, t.surface_panel, "{appearance:?}: every strip is a panel, selected or not");
                let dial = sl.knob_paint.clone().expect("the pan dial");
                assert_eq!((dial.cap, dial.arc, dial.pointer), (t.knob_cap, t.signal, t.text), "{appearance:?}: the kit's knob tokens");
                let edge = egui::Color32::from_rgba_premultiplied(sl.border_color[0], sl.border_color[1], sl.border_color[2], sl.border_color[3]);
                assert_eq!(edge, if sl.focused { t.signal } else { t.line }, "{appearance:?} focused={}", sl.focused);
            }
        }
    }
}

#[test]
fn mixer_insert_overflow_counts_hidden_rows_on_the_pinned_add_row() {
    // The full-height section (the docked pane is short and lists every insert in its FX popup).
    for count in [1usize, 4, 9, 10] {
        let (mut app, ctx, mut time, track) = mixer_app();
        app.focus(track);
        for _ in 0..count {
            app.add_device(track, "gain").unwrap();
        }
        full_mixer(&mut app, &ctx, &mut time);
        let shapes = mixer_paint(&mut app, &ctx, &mut time);
        let sl = strip(&app, track);
        let section = egui::Rect::from_min_max(
            egui::pos2(sl.inserts_section[0], sl.inserts_section[1]),
            egui::pos2(sl.inserts_section[2], sl.inserts_section[3]),
        );
        let want = count.saturating_sub(3);
        let mut labels = Vec::new();
        for shape in &shapes {
            if let egui::Shape::Text(text) = shape
                && text.galley.text().ends_with(" more")
                && (sl.rect[0]..sl.rect[2]).contains(&text.pos.x)
            {
                labels.push((text.galley.text().to_string(), text.galley.rect.translate(text.pos.to_vec2())));
            }
        }
        // The hidden count rides the pinned add row, under the rows' view and inside the box.
        if want > 0 {
            let ov = sl.inserts_overflow.clone().expect("past three, the section overflows");
            assert_eq!(labels.len(), 1, "count={count}: one count");
            let (text, painted) = &labels[0];
            assert_eq!(text, &format!("{want} more"));
            assert!(section.contains_rect(*painted) && painted.min.y >= ov.view[3], "count={count}: {painted:?} under the view in {section:?}");
            if count < 10 {
                let add = egui::Rect::from_min_max(egui::pos2(sl.add_insert[0], sl.add_insert[1]), egui::pos2(sl.add_insert[2], sl.add_insert[3]));
                assert!(add.contains_rect(*painted), "count={count}: in the add row");
            }
        } else {
            assert!(labels.is_empty() && sl.inserts_overflow.is_none(), "count={count}: nothing to count");
        }
        if count < 10 {
            let add_at = mid(strip(&app, track).add_insert);
            click(&mut app, add_at);
            frames(&mut app, &ctx, 4, &mut time);
            assert!(app.mixer.layout.menu.iter().any(|m| m.value == "gain"), "add remains usable");
        }
    }
}

/// The level block's frame of a strip: Y0 and Y1 (the travel's top and bottom).
fn frame_of(s: &crate::mixer::StripLayout) -> (f32, f32) {
    (s.fader[1], s.fader[3])
}

/// Fault 3 of the Fader construction board: every strip in a row, the main bus included, places
/// its level block on the same Y0 and Y1, docked (short) and in the full mixer, with and without a
/// master fader, at 1.0 and 1.5; each meter well spans exactly Y0 to Y1 with its clip cell 4 high,
/// 6 above Y0. Without a master fader the main bus keeps the scale on that frame and its fader
/// column stays empty: `+ Master Fader` in its routing rows is the one way to add the fader.
#[test]
fn the_main_bus_shares_the_channels_frame() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let footer_room = crate::tokens::channel_strip::SECTION_GAP + 13.0;
    for master in [false, true] {
        if master {
            app.run("track.new", json!({"kind": "master"})).unwrap();
        }
        for full in [false, true] {
            app.ui.mixer_full = full;
            app.ui.show_devices = !full;
            for ppp in [1.0f32, 1.5] {
                ctx.set_pixels_per_point(ppp);
                frames(&mut app, &ctx, 4, &mut time);
                let what = format!("master {master}, full {full}, {ppp}x");
                assert_eq!(app.mixer.layout.short, !full, "{what}");
                let ch = strip(&app, a);
                let m = app.mixer.layout.master.clone().unwrap();
                let (c0, c1) = frame_of(&ch);
                let (m0, m1) = frame_of(&m);
                assert!(c1 - c0 >= 80.0 - 0.01, "{what}: travel {}", c1 - c0);
                assert!((c0 - m0).abs() < 0.01 && (c1 - m1).abs() < 0.01, "{what}: Y0 and Y1 shared: {c0}..{c1} vs {m0}..{m1}");
                for (who, s) in [("channel", &ch), ("main", &m)] {
                    let ml = s.meter_layout.as_ref().unwrap();
                    for (w, cell) in ml.wells.iter().zip(&ml.clip_cells) {
                        assert!((w[1] - c0).abs() < 0.01 && (w[3] - c1).abs() < 0.01, "{what}: {who}'s well spans Y0 to Y1: {w:?}");
                        assert!((cell[3] - cell[1] - 4.0).abs() < 0.01 && (c0 - cell[3] - 6.0).abs() < 0.01, "{what}: {who}'s clip cell: {cell:?}");
                    }
                }
                // The main bus's panel fills the row; a channel's ends the footer's room above it.
                assert!((m.rect[3] - ch.rect[3] - footer_room).abs() < 0.01, "{what}: {:?} vs {:?}", m.rect, ch.rect);
                assert!(m.lufs_rect[3] <= m.rect[3] && m.lufs_rect[1] > m.peak_rect[3], "{what}: the footer under the readouts");
                // The scale: the main bus's labels are the channel's, on the same rows.
                let ch_scale: Vec<(String, f32)> =
                    ch.fader_layout.as_ref().unwrap().marks.iter().map(|(t, r)| (t.clone(), (r[1] + r[3]) / 2.0)).collect();
                let m_marks = match (&m.fader_layout, &m.main_scale) {
                    (Some(f), None) if master => f.marks.clone(),
                    (None, Some(s)) if !master => s.marks.clone(),
                    other => panic!("{what}: {other:?}"),
                };
                let m_scale: Vec<(String, f32)> = m_marks.iter().map(|(t, r)| (t.clone(), (r[1] + r[3]) / 2.0)).collect();
                assert_eq!(ch_scale, m_scale, "{what}: one scale across the row");
                // Meter grid: the main bus's grid rows are its scale's tick rows.
                let m_ticks = m.fader_layout.as_ref().map(|f| f.ticks.clone()).or_else(|| m.main_scale.as_ref().map(|s| s.ticks.clone())).unwrap();
                let rows = |v: &[[f32; 4]]| v.iter().map(|r| [r[1], r[3]]).collect::<Vec<_>>();
                assert_eq!(rows(&m.meter_layout.as_ref().unwrap().grid), rows(&m_ticks), "{what}: the main bus's grid at its placed labels");
                if !master {
                    // The fader column (x 0 to 36 of the block, Y0 to Y1) paints nothing.
                    let x0 = m.meter_layout.as_ref().unwrap().wells[0][0] - 72.0;
                    let column = egui::Rect::from_min_max(egui::pos2(x0, c0), egui::pos2(x0 + crate::widgets::fader::CHANNEL_FADER_W, c1));
                    let shapes = mixer_paint(&mut app, &ctx, &mut time);
                    let inside: Vec<&egui::Shape> = shapes
                        .iter()
                        .filter(|s| column.expand(1.0).contains_rect(s.visual_bounding_rect()) && s.visual_bounding_rect().is_positive())
                        .collect();
                    assert!(inside.is_empty(), "{what}: the main bus's fader column stays empty: {inside:?}");
                    let texts: Vec<String> =
                        shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = s { Some(t.galley.text().to_string()) } else { None }).collect();
                    assert!(!texts.iter().any(|t| t == "+ Fader"), "{what}: no second + Fader");
                    assert_eq!(texts.iter().filter(|t| *t == "+ Master Fader").count(), 1, "{what}: + Master Fader in the routing rows, once");
                }
                // Negative control: the old frame (the main fader giving up the footer's height)
                // fails the equality.
                assert!((c1 - (m1 - footer_room)).abs() > 1.0, "{what}: the old main frame would match");
            }
        }
    }
}

/// The board's section C as rects: the main bus beside a channel on one frame, with and without a
/// master fader, block-relative, at 1.0 and 1.5 (run with `--nocapture`). The widths stay as they
/// are (the board's withdrawn fault: the main's bars are 14 · 3 · 14 in a 144 block, a channel's
/// 10 · 3 · 10 in 116), and the dump pins them.
#[test]
fn the_board_section_c_as_rects() {
    use crate::tokens::fader as f;
    let (mut app, ctx, mut time, a) = mixer_app();
    for master in [false, true] {
        if master {
            app.run("track.new", json!({"kind": "master"})).unwrap();
        }
        for ppp in [1.0f32, 1.5] {
            ctx.set_pixels_per_point(ppp);
            frames(&mut app, &ctx, 4, &mut time);
            let what = format!("master {master}, {ppp}x");
            let ch = strip(&app, a);
            let m = app.mixer.layout.master.clone().unwrap();
            let bars = |s: &crate::mixer::StripLayout| {
                let w = &s.meter_layout.as_ref().unwrap().wells;
                (w[0][2] - w[0][0], w[1][0] - w[0][2], w[1][2] - w[1][0])
            };
            assert_eq!(bars(&m), (f::METER_BAR_MAIN, f::METER_GAP, f::METER_BAR_MAIN), "{what}: main 14 · 3 · 14");
            assert_eq!(bars(&ch), (f::METER_BAR, f::METER_GAP, f::METER_BAR), "{what}: channel 10 · 3 · 10");
            // Negative control: the channel's bars are not the main bus's.
            assert_ne!(bars(&ch), bars(&m));
            let block = |s: &crate::mixer::StripLayout| s.meter_layout.as_ref().unwrap().wells[0][0] - f::COLUMN_METER_X;
            let (cx, mx) = (block(&ch), block(&m));
            if let Some(fl) = &m.fader_layout {
                assert_eq!((fl.rect[0], fl.rect[2] - fl.rect[0]), (mx, f::MAIN_BLOCK_WIDTH), "{what}: the main block is 144 wide");
            }
            let cf = ch.fader_layout.as_ref().unwrap();
            assert_eq!((cf.rect[0], cf.rect[2] - cf.rect[0]), (cx, f::BLOCK_WIDTH), "{what}: the channel block is 116 wide");
            assert!(mx + f::MAIN_BLOCK_WIDTH <= m.rect[2] - 8.0 + 0.01, "{what}: the main block inside the strip's padding");
            // The dump: section C, block-relative (x from the block's left, y from its top).
            for (who, s, x0) in [("Inst", &ch, cx), ("Main", &m, mx)] {
                let y0 = s.fader[1] - f::INSET;
                let rel = |r: [f32; 4]| [r[0] - x0, r[1] - y0, r[2] - x0, r[3] - y0].map(|v| (v * 100.0).round() / 100.0);
                let ml = s.meter_layout.as_ref().unwrap();
                println!(
                    "C {what} {who}: Y0 {} Y1 {} (row y {}..{}) wells {:?} clip cells {:?}",
                    s.fader[1] - y0,
                    s.fader[3] - y0,
                    s.fader[1],
                    s.fader[3],
                    ml.wells.iter().map(|w| rel(*w)).collect::<Vec<_>>(),
                    ml.clip_cells.iter().map(|w| rel(*w)).collect::<Vec<_>>(),
                );
            }
        }
    }
}
