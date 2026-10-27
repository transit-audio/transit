use crate::tests::*;

#[test]
fn the_fader_cap_has_a_line_at_the_value() {
    use crate::{fine::Target, mixer::db_to_k};
    let (mut app, ctx, mut time, t) = mixer_app();
    ctx.set_pixels_per_point(1.5);
    frames(&mut app, &ctx, 3, &mut time);
    let want = |s: &crate::mixer::StripLayout, db: f32| s.fader[3] - db_to_k(db) * (s.fader[3] - s.fader[1]);
    // At 0 dB the cap's line is the 0 dB tick.
    let s = strip(&app, t);
    let (cap, zero) = (s.cap_line_y.expect("the cap line was painted"), s.zero_tick_y.expect("the 0 dB tick was painted"));
    assert_eq!(cap, zero, "at 0 dB the cap line sits on the 0 dB line");
    assert_eq!(s.cap_line_span, s.zero_tick_span, "the same physical pixel rows as the tick");
    let [a, b] = s.zero_tick_span.unwrap();
    assert_eq!(b - a, 2, "a one-point tick is 2 physical pixels at 1.5x");
    assert!((cap - want(&s, 0.0)).abs() <= 0.5, "and on the value position: {cap} vs {}", want(&s, 0.0));
    // At -12 dB it follows the value, away from the 0 dB line.
    app.set_value(Target::Fader(t), -12.0).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let s = strip(&app, t);
    let cap = s.cap_line_y.unwrap();
    assert!((cap - want(&s, -12.0)).abs() <= 0.5, "-12 dB: {cap} vs {}", want(&s, -12.0));
    assert!(cap > s.zero_tick_y.unwrap() + 10.0, "well below the 0 dB line");
}

#[test]
fn fader_and_balance_laws_round_trip() {
    use crate::mixer::{balance_of, db_to_k, k_to_db, pans_for};
    for db in [-60.0f32, -24.0, -6.0, 0.0, 6.0, 12.0] {
        assert!((k_to_db(db_to_k(db)) - db).abs() < 0.01, "{db}");
    }
    assert_eq!(db_to_k(-144.0), 0.0);
    assert_eq!(k_to_db(0.0), -144.0);
    assert!((db_to_k(12.0) - 1.0).abs() < 1e-6);
    assert_eq!(pans_for(0.0, 2), vec![-1.0, 1.0], "center keeps the stereo image");
    assert_eq!(pans_for(-1.0, 2), vec![-1.0, -1.0]);
    assert_eq!(pans_for(1.0, 2), vec![1.0, 1.0]);
    assert_eq!(pans_for(0.5, 1), vec![0.5]);
    for b in [-1.0f32, -0.3, 0.0, 0.4, 1.0] {
        assert!((balance_of(&pans_for(b, 2)) - b).abs() < 1e-6);
    }
}

#[test]
fn a_fader_drag_is_one_undo_step() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let f = strip(&app, t).fader;
    let k0 = crate::mixer::db_to_k(0.0);
    let cap_y = f[3] - k0 * (f[3] - f[1]);
    let x = (f[0] + f[2]) / 2.0;
    let before = undo_len(&mut app);
    drag(&mut app, (x, cap_y), (x, cap_y + 40.0), json!({}));
    frames(&mut app, &ctx, 30, &mut time);
    let db = app.engine.session().track(t).unwrap().mixer.volume_db;
    assert!(db < -3.0, "the fader went down: {db}");
    assert_eq!(undo_len(&mut app), before + 1, "one gesture, one undo step");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().track(t).unwrap().mixer.volume_db, 0.0);
}

#[test]
fn the_pan_knob_is_a_balance_control() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let k = mid(strip(&app, t).knob);
    let before = undo_len(&mut app);
    // 50 points up: half way to the right.
    drag(&mut app, k, (k.0, k.1 - 50.0), json!({}));
    frames(&mut app, &ctx, 30, &mut time);
    let s = strip(&app, t);
    assert!((s.balance - 0.5).abs() < 0.03, "{}", s.balance);
    let pan = app.engine.session().track(t).unwrap().mixer.pan.clone();
    let want = crate::mixer::pans_for(s.balance, 2);
    assert!((pan[0] - want[0]).abs() < 1e-5 && (pan[1] - want[1]).abs() < 1e-5, "{pan:?} vs {want:?}");
    assert_eq!(undo_len(&mut app), before + 1, "both panners, one undo step");
}

#[test]
fn a_send_row_drag_is_one_undo_step() {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.new_fx_channel(Some(t), 0.0).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let row = strip(&app, t).sends.first().cloned().expect("a send row");
    assert_eq!((row.slot, row.target.as_str(), row.level_db), (0, "FX 1", 0.0));
    let before = undo_len(&mut app);
    let (x, y) = mid(row.level_rect);
    drag(&mut app, (x, y), (x, y + 30.0), json!({}));
    frames(&mut app, &ctx, 30, &mut time);
    let level = app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().level_db;
    assert!(level < -3.0, "the drag lowered the send: {level}");
    assert_eq!(undo_len(&mut app), before + 1, "one gesture, one undo step");
    assert_eq!(strip(&app, t).sends[0].level_db, level, "the row shows mix.send's level");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().level_db, 0.0);
}

fn move_to(app: &mut StudioApp, at: (f32, f32)) {
    send_control(app, "ui.move", json!({"x": at.0, "y": at.1}));
}

/// One wheel notch (`dy` lines; positive scrolls up) over `at`.
fn wheel(app: &mut StudioApp, at: (f32, f32), dy: f32, extra: Value) {
    let mut p = json!({"x": at.0, "y": at.1, "dy": dy, "unit": "line", "steps": 1});
    if let (Some(o), Some(m)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(m.clone());
    }
    send_control(app, "ui.scroll", p);
}

fn volume(app: &StudioApp, t: TrackId) -> f32 {
    app.engine.session().track(t).unwrap().mixer.volume_db
}

fn send_db(app: &StudioApp, t: TrackId) -> f32 {
    app.engine.session().track(t).unwrap().mixer.sends[0].as_ref().unwrap().level_db
}

fn balance(app: &StudioApp, t: TrackId) -> f32 {
    crate::mixer::balance_of(&app.engine.session().track(t).unwrap().mixer.pan)
}

/// A mixer app whose track has one send, at 0 dB.
fn send_app() -> (StudioApp, egui::Context, f64, TrackId) {
    let (mut app, ctx, mut time, t) = mixer_app();
    app.new_fx_channel(Some(t), 0.0).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time, t)
}

#[test]
fn double_click_alt_click_ctrl_click_and_delete_reset_a_fader_one_undo_step_each() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let at = mid(strip(&app, t).fader);
    for name in ["double-click", "alt-click", "ctrl-click", "delete"] {
        app.run("mix.volume", json!({"tracks": [t.0], "db": 0.2})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        let before = undo_len(&mut app);
        match name {
            "double-click" => click_with(&mut app, at, json!({"count": 2})),
            "alt-click" => click_with(&mut app, at, json!({"alt": true})),
            "ctrl-click" => click_with(&mut app, at, json!({"ctrl": true})),
            _ => {
                move_to(&mut app, at);
                frames(&mut app, &ctx, 3, &mut time);
                key(&mut app, "Delete");
            }
        }
        frames(&mut app, &ctx, 12, &mut time);
        assert_eq!(volume(&app, t), 0.0, "{name} resets the fader to 0.0 dB");
        assert_eq!(undo_len(&mut app), before + 1, "{name}: one undo step");
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(volume(&app, t), 0.2, "{name}: undo brings the nudged value back");
    }
}

#[test]
fn double_click_resets_a_pan_knob_and_a_send_level() {
    let (mut app, ctx, mut time, t) = send_app();
    let knob = mid(strip(&app, t).knob);
    for (name, extra) in [("double-click", json!({"count": 2})), ("alt-click", json!({"alt": true}))] {
        app.set_balance(t, 0.3).unwrap();
        app.engine.end_merge();
        frames(&mut app, &ctx, 3, &mut time);
        let before = undo_len(&mut app);
        frames(&mut app, &ctx, 60, &mut time);
        click_with(&mut app, knob, extra);
        frames(&mut app, &ctx, 12, &mut time);
        assert_eq!(balance(&app, t), 0.0, "knob {name}");
        assert_eq!(app.engine.session().track(t).unwrap().mixer.pan, vec![-1.0, 1.0], "both panners back at center");
        assert_eq!(undo_len(&mut app), before + 1, "knob {name}: one undo step");
    }
    for (name, bar, extra) in [
        ("bar double-click", true, json!({"count": 2})),
        ("number alt-click", false, json!({"alt": true})),
        ("number ctrl-click", false, json!({"ctrl": true})),
    ] {
        app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": -3.5})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        let row = strip(&app, t).sends[0].clone();
        let at = mid(if bar { row.level_rect } else { row.readout_rect });
        let before = undo_len(&mut app);
        frames(&mut app, &ctx, 60, &mut time);
        click_with(&mut app, at, extra);
        frames(&mut app, &ctx, 12, &mut time);
        assert_eq!(send_db(&app, t), 0.0, "send {name}");
        assert_eq!(undo_len(&mut app), before + 1, "send {name}: one undo step");
    }
    app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": -3.5})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, t).sends[0].level_rect);
    move_to(&mut app, at);
    frames(&mut app, &ctx, 3, &mut time);
    key(&mut app, "Delete");
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(send_db(&app, t), 0.0, "Delete over the send level resets it");
}

#[test]
fn shift_drag_moves_a_tenth_as_far() {
    use crate::mixer::db_to_k;
    let (mut app, ctx, mut time, t) = send_app();
    let f = strip(&app, t).fader;
    let x = (f[0] + f[2]) / 2.0;
    let cap_y = f[3] - db_to_k(0.0) * (f[3] - f[1]);
    let mut moved = Vec::new();
    for mods in [json!({}), json!({"shift": true})] {
        app.run("mix.volume", json!({"tracks": [t.0], "db": 0.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, (x, cap_y), (x, cap_y + 40.0), mods);
        frames(&mut app, &ctx, 30, &mut time);
        moved.push(db_to_k(0.0) - db_to_k(volume(&app, t)));
    }
    assert!(moved[0] > 0.1, "a plain drag moves the fader: {moved:?}");
    assert!((moved[1] / moved[0] - 0.1).abs() < 0.01, "shift-drag moves a tenth as far: {moved:?}");
    let knob = mid(strip(&app, t).knob);
    let mut moved = Vec::new();
    for mods in [json!({}), json!({"shift": true})] {
        app.set_balance(t, 0.0).unwrap();
        app.engine.end_merge();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, knob, (knob.0, knob.1 - 40.0), mods);
        frames(&mut app, &ctx, 30, &mut time);
        moved.push(balance(&app, t));
    }
    assert!((moved[0] - 0.4).abs() < 0.02 && (moved[1] - 0.04).abs() < 0.005, "{moved:?}");
    let bar = mid(strip(&app, t).sends[0].level_rect);
    let mut moved = Vec::new();
    for mods in [json!({}), json!({"shift": true})] {
        app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": 0.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, bar, (bar.0, bar.1 + 30.0), mods);
        frames(&mut app, &ctx, 30, &mut time);
        moved.push(db_to_k(0.0) - db_to_k(send_db(&app, t)));
    }
    assert!((moved[1] / moved[0] - 0.1).abs() < 0.01, "{moved:?}");
}

#[test]
fn a_slow_drag_snaps_to_a_mark_and_a_fast_one_does_not() {
    use crate::mixer::db_to_k;
    let (mut app, ctx, mut time, t) = send_app();
    let f = strip(&app, t).fader;
    let (x, h) = ((f[0] + f[2]) / 2.0, f[3] - f[1]);
    let cap_y = f[3] - db_to_k(0.0) * h;
    let travel = (db_to_k(0.0) - db_to_k(-6.1)) * h;
    for (steps, snaps) in [(150, true), (4, false)] {
        app.run("mix.volume", json!({"tracks": [t.0], "db": 0.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, (x, cap_y), (x, cap_y + travel), json!({"steps": steps}));
        frames(&mut app, &ctx, 200, &mut time);
        let db = volume(&app, t);
        if snaps {
            assert_eq!(db, -6.0, "slow: snapped to -6 dB");
        } else {
            assert!((db + 6.1).abs() < 0.03, "fast: not snapped, {db}");
        }
    }
    let knob = mid(strip(&app, t).knob);
    for (steps, snaps) in [(150, true), (4, false)] {
        app.set_balance(t, 0.1).unwrap();
        app.engine.end_merge();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, knob, (knob.0, knob.1 + 9.0), json!({"steps": steps}));
        frames(&mut app, &ctx, 200, &mut time);
        let b = balance(&app, t);
        if snaps {
            assert_eq!(b, 0.0, "slow: snapped to center");
        } else {
            assert!((b - 0.01).abs() < 0.003, "fast: {b}");
        }
    }
    let bar = mid(strip(&app, t).sends[0].level_rect);
    let up = (db_to_k(-0.1) - db_to_k(-4.0)) * 150.0;
    for (steps, snaps) in [(150, true), (4, false)] {
        app.run("mix.send_level", json!({"track": t.0, "slot": 0, "db": -4.0})).unwrap();
        frames(&mut app, &ctx, 3, &mut time);
        drag(&mut app, bar, (bar.0, bar.1 - up), json!({"steps": steps}));
        frames(&mut app, &ctx, 200, &mut time);
        let db = send_db(&app, t);
        if snaps {
            assert_eq!(db, 0.0, "slow: snapped to the 0 dB mark");
        } else {
            assert!((db + 0.1).abs() < 0.05, "fast: {db}");
        }
    }
}

#[test]
fn the_wheel_nudges_and_one_gesture_is_one_undo_step() {
    let (mut app, ctx, mut time, t) = send_app();
    let at = mid(strip(&app, t).fader);
    let before = undo_len(&mut app);
    for _ in 0..3 {
        wheel(&mut app, at, 1.0, json!({}));
        frames(&mut app, &ctx, 4, &mut time);
    }
    assert!((volume(&app, t) - 0.3).abs() < 1e-4, "three notches up: {}", volume(&app, t));
    assert_eq!(undo_len(&mut app), before + 1, "the notches merge into one undo step");
    wheel(&mut app, at, -1.0, json!({"shift": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((volume(&app, t) - 0.29).abs() < 1e-4, "Shift+wheel is 0.01 dB: {}", volume(&app, t));
    assert_eq!(undo_len(&mut app), before + 1);
    frames(&mut app, &ctx, 45, &mut time);
    wheel(&mut app, at, 1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(undo_len(&mut app), before + 2, "after the pause a new step");
    move_to(&mut app, (3.0, 3.0));
    frames(&mut app, &ctx, 4, &mut time);
    wheel(&mut app, at, 1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(undo_len(&mut app), before + 3, "after leaving the control a new step");
    assert!((volume(&app, t) - 0.49).abs() < 1e-3, "{}", volume(&app, t));
    app.run("edit.undo", json!({})).unwrap();
    assert!((volume(&app, t) - 0.39).abs() < 1e-3, "{}", volume(&app, t));
    let knob = mid(strip(&app, t).knob);
    wheel(&mut app, knob, 1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((balance(&app, t) - 0.01).abs() < 1e-4, "{}", balance(&app, t));
    wheel(&mut app, knob, -1.0, json!({"shift": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((balance(&app, t) - 0.009).abs() < 1e-4, "Shift+wheel is a tenth of that: {}", balance(&app, t));
    let bar = mid(strip(&app, t).sends[0].level_rect);
    wheel(&mut app, bar, -1.0, json!({}));
    frames(&mut app, &ctx, 4, &mut time);
    assert!((send_db(&app, t) + 0.1).abs() < 1e-4, "{}", send_db(&app, t));
}

#[test]
fn the_wheel_over_a_control_does_not_scroll_the_strips() {
    let (mut app, ctx, mut time, t) = mixer_app();
    for _ in 0..30 {
        app.run("track.new", json!({"kind": "audio", "format": "stereo"})).unwrap();
    }
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, t).fader);
    wheel(&mut app, at, -150.0, json!({"unit": "point"}));
    frames(&mut app, &ctx, 8, &mut time);
    assert_eq!(app.mixer.hscroll, 0.0, "over a fader the wheel nudges it");
    let name = mid(strip(&app, t).name_rect);
    wheel(&mut app, name, -150.0, json!({"unit": "point"}));
    frames(&mut app, &ctx, 8, &mut time);
    assert!(app.mixer.hscroll > 0.0, "elsewhere on the strip it scrolls");
}

#[test]
fn parsing_what_a_readout_takes() {
    use crate::fine::{parse_db, parse_pan};
    for (text, want) in [
        ("-6", -6.0),
        ("-6.0", -6.0),
        ("-6 dB", -6.0),
        ("  +1.5 ", 1.5),
        ("+1.5dB", 1.5),
        ("0", 0.0),
        ("99", 12.0),
        ("-500", -144.0),
        ("-inf", -144.0),
        ("\u{2212}6", -6.0),
        ("\u{2212}6.5 dB", -6.5),
    ] {
        assert_eq!(parse_db(text), Some(want), "{text:?}");
    }
    for bad in ["", "dB", "abc", "1,5", "--3", "nan", "inf", "+inf", "infinity"] {
        assert_eq!(parse_db(bad), None, "{bad:?}");
    }
    for (text, want) in
        [("L20", -0.2), ("R35", 0.35), ("C", 0.0), ("c", 0.0), ("-20", -0.2), ("20", 0.2), ("+20", 0.2), ("l 100", -1.0), ("R250", 1.0), ("50%", 0.5)]
    {
        assert!((parse_pan(text).unwrap() - want).abs() < 1e-6, "{text:?}");
    }
    for bad in ["", "L", "xyz", "LR5"] {
        assert_eq!(parse_pan(bad), None, "{bad:?}");
    }
    assert_eq!(crate::fine::pan_text(-0.2), "L20");
    assert_eq!(crate::fine::pan_text(0.0), "C");
}

/// Open a readout's field with a double-click, type `text`, and finish with `end` (a key name).
fn edit_readout(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, at: (f32, f32), text: &str, end: &str) {
    // Let the last click go stale, or this one counts as a triple click.
    frames(app, ctx, 60, time);
    click_with(app, at, json!({"count": 2}));
    frames(app, ctx, 12, time);
    assert!(app.mixer.edit.is_some(), "double-click opens the field");
    typed(app, text);
    frames(app, ctx, 3, time);
    key(app, end);
    frames(app, ctx, 6, time);
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

#[test]
fn the_fader_readout_edits_in_place_and_applies_cancels_and_clamps() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let ro = mid(strip(&app, t).readout_rect);
    let f = mid(strip(&app, t).fader);
    assert!(!rects_overlap(strip(&app, t).readout_rect, strip(&app, t).fader), "distinct hit rects");
    assert_eq!(strip(&app, t).volume_text, "+0.0");
    for (text, want) in [("-6", -6.0), ("-6.0", -6.0), ("-6 dB", -6.0), ("+1.5", 1.5), ("99", 12.0), ("-500", -144.0)] {
        let before = undo_len(&mut app);
        edit_readout(&mut app, &ctx, &mut time, ro, text, "Enter");
        assert_eq!(volume(&app, t), want, "{text:?} applies");
        if want != -6.0 || text == "-6" {
            assert_eq!(undo_len(&mut app), before + 1, "{text:?}: one undo step");
        }
        assert!(app.mixer.edit.is_none(), "Enter closes the field");
    }
    assert_eq!(strip(&app, t).volume_text, "-inf");
    app.run("mix.volume", json!({"tracks": [t.0], "db": 2.0})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let before = undo_len(&mut app);
    edit_readout(&mut app, &ctx, &mut time, ro, "-20", "Escape");
    assert_eq!((volume(&app, t), app.mixer.edit.is_none()), (2.0, true), "Escape cancels");
    frames(&mut app, &ctx, 60, &mut time);
    click_with(&mut app, ro, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    typed(&mut app, "-20");
    frames(&mut app, &ctx, 3, &mut time);
    let name = mid(strip(&app, t).name_rect);
    click_with(&mut app, name, json!({}));
    frames(&mut app, &ctx, 8, &mut time);
    assert_eq!((volume(&app, t), app.mixer.edit.is_none()), (2.0, true), "losing focus cancels");
    edit_readout(&mut app, &ctx, &mut time, ro, "loud", "Enter");
    assert_eq!((volume(&app, t), app.mixer.edit.is_none()), (2.0, true), "garbage changes nothing");
    let status = app.status.clone();
    assert!(status.contains("not a level"), "{status}");
    assert_eq!(undo_len(&mut app), before, "no cancel made an undo step");
    frames(&mut app, &ctx, 60, &mut time);
    click_with(&mut app, ro, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    let v = app.inspect(&ctx);
    assert_eq!(v["readout_edit"]["text"], json!("2.0"));
    assert!(v["readout_edit"]["rect"][2].as_f64().unwrap() > 0.0);
    key(&mut app, "Escape");
    frames(&mut app, &ctx, 6, &mut time);
    frames(&mut app, &ctx, 60, &mut time);
    click_with(&mut app, f, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    assert_eq!(volume(&app, t), 0.0, "double-click on the fader is still the reset");
    assert!(app.mixer.edit.is_none());
}

#[test]
fn the_pan_and_send_readouts_edit_in_place() {
    let (mut app, ctx, mut time, t) = send_app();
    let pan = mid(strip(&app, t).pan_readout_rect);
    assert_eq!(strip(&app, t).pan_text, "C");
    for (text, want) in [("L20", -0.2), ("R35", 0.35), ("C", 0.0), ("-20", -0.2), ("20", 0.2), ("R250", 1.0)] {
        let before = undo_len(&mut app);
        edit_readout(&mut app, &ctx, &mut time, pan, text, "Enter");
        assert!((balance(&app, t) - want).abs() < 1e-5, "{text:?}: {}", balance(&app, t));
        let n = undo_len(&mut app);
        assert_eq!(n, before + 1, "{text:?}: both panners, one undo step");
    }
    edit_readout(&mut app, &ctx, &mut time, pan, "L50", "Escape");
    assert_eq!(balance(&app, t), 1.0, "Escape leaves the pan alone");
    let row = strip(&app, t).sends[0].clone();
    assert_eq!(row.level_text, "0");
    let num = mid(row.readout_rect);
    for (text, want) in [("-6", -6.0), ("+1.5", 1.5), ("40", 12.0), ("-7.25 dB", -7.25)] {
        edit_readout(&mut app, &ctx, &mut time, num, text, "Enter");
        assert_eq!(send_db(&app, t), want, "{text:?}");
    }
    edit_readout(&mut app, &ctx, &mut time, num, "3", "Escape");
    assert_eq!(send_db(&app, t), -7.25, "Escape leaves the send alone");
    app.focus(t);
    frames(&mut app, &ctx, 3, &mut time);
    let wide = mid(app.mixer.devices.sends[0].readout_rect);
    assert!(app.mixer.devices.sends[0].level_text.ends_with("dB"));
    edit_readout(&mut app, &ctx, &mut time, wide, "-1", "Enter");
    assert_eq!(send_db(&app, t), -1.0);
}

#[test]
fn inspect_exposes_the_readouts() {
    let (mut app, ctx, mut time, t) = send_app();
    app.run("mix.volume", json!({"tracks": [t.0], "db": -6.0})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let v = app.inspect(&ctx);
    let s = v["mixer"]["strips"].as_array().unwrap().iter().find(|s| s["track"] == json!(t.0)).unwrap().clone();
    assert_eq!(s["volume_text"], json!("-6.0"));
    assert!(s["readout_rect"][2].as_f64().unwrap() > s["readout_rect"][0].as_f64().unwrap());
    assert_eq!(s["pan_text"], json!("C"));
    assert!(s["pan_readout_rect"][3].as_f64().unwrap() > 0.0);
    assert_eq!(s["sends"][0]["level_text"], json!("0"));
    assert!(s["sends"][0]["readout_rect"][2].as_f64().unwrap() > 0.0);
}

#[test]
fn numbers_use_the_mono_medium_family_only_where_it_exists() {
    let ctx = egui::Context::default();
    run_probe(&ctx);
    assert_eq!(crate::theme::mono(&ctx, 12.0).family, egui::FontFamily::Monospace, "default fonts: Monospace");
    let mut defs = egui::FontDefinitions::default();
    let base = defs.families[&egui::FontFamily::Monospace].clone();
    defs.families.insert(egui::FontFamily::Name("mono-medium".into()), base);
    ctx.set_fonts(defs);
    for _ in 0..2 {
        run_probe(&ctx);
    }
    assert_eq!(crate::theme::mono(&ctx, 12.0).family, egui::FontFamily::Name("mono-medium".into()));
}

fn run_probe(ctx: &egui::Context) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::theme::probe_fonts(ui.ctx()));
    out.textures_delta.clear();
}

#[test]
fn shift_drag_does_not_snap_to_a_mark() {
    use crate::mixer::db_to_k;
    let (mut app, ctx, mut time, t) = send_app();
    let f = strip(&app, t).fader;
    let (x, h) = ((f[0] + f[2]) / 2.0, f[3] - f[1]);
    app.run("mix.volume", json!({"tracks": [t.0], "db": -0.5})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let from = f[3] - db_to_k(-0.5) * h;
    // Shift moves a tenth as far, so ten times the travel for +0.6 dB; slowly, so a plain drag would snap to 0.
    let travel = (db_to_k(0.1) - db_to_k(-0.5)) * h * 10.0;
    drag(&mut app, (x, from), (x, from - travel), json!({"shift": true, "steps": 150}));
    frames(&mut app, &ctx, 200, &mut time);
    let db = volume(&app, t);
    assert!(db != 0.0 && (db - 0.1).abs() < 0.05, "shift-drag lands off the 0 dB mark: {db}");
}

#[test]
fn backspace_over_a_control_resets_it_like_delete_and_keeps_the_clip() {
    let (mut app, ctx, mut time, t) = mixer_app();
    let (ct, clip, _bar) = track_with_clip(&mut app);
    app.click_clip(clip, false);
    app.run("mix.volume", json!({"tracks": [t.0], "db": 0.2})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, t).fader);
    move_to(&mut app, at);
    frames(&mut app, &ctx, 3, &mut time);
    key(&mut app, "Backspace");
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(volume(&app, t), 0.0, "Backspace over the fader resets it");
    assert_eq!(clips_on(&app, ct).len(), 1, "and the selected clip stays");
}
