use crate::tests::*;

#[test]
fn the_peak_readout_holds_the_loudest_peak_until_clicked() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-inf");
    // A known peak (0.7 is -3.1 dBFS), then silence: the readout holds it.
    for (t, peak) in [(a, 0.7f32), (b, 1.2f32)] {
        let d = app.meters.entry(t).or_default();
        crate::host::feed_meter(d, [peak, 0.1], 0.0, 0.016);
        crate::host::feed_meter(d, [0.0, 0.0], 0.0, 0.016);
    }
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-3.1");
    assert_eq!(strip(&app, b).peak_text, "+1.6", "over 0 dBFS reads with a plus sign");
    let d = app.meters.get_mut(&a).unwrap();
    crate::host::feed_meter(d, [0.5, 0.0], 0.0, 0.016);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-3.1", "a quieter peak does not lower the hold");
    // Click resets that strip only.
    let at = mid(strip(&app, a).peak_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(strip(&app, a).peak_text, "-inf");
    assert_eq!(strip(&app, b).peak_text, "+1.6");
    // Ctrl-click resets all.
    let at = mid(strip(&app, b).peak_rect);
    click_with(&mut app, at, json!({"ctrl": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(strip(&app, b).peak_text, "-inf");
    assert_eq!(crate::mixer::peak_text(1.0), ("0.0".to_string(), true));
    assert_eq!(crate::mixer::peak_text(0.5), ("-6.0".to_string(), false));
}

#[test]
fn a_clip_latch_shows_as_a_cap_and_clears_on_click_play_and_alt_c() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    let latch = |app: &mut StudioApp| {
        for t in [a, b] {
            crate::host::feed_meter(app.meters.entry(t).or_default(), [1.2, 0.2], 0.0, 0.016);
        }
        crate::host::feed_meter(&mut app.main_meter, [1.5, 0.2], 0.0, 0.016);
    };
    let clips = |app: &StudioApp| (app.meters[&a].clip, app.meters[&b].clip, app.main_meter.clip);
    latch(&mut app);
    assert_eq!(clips(&app), (true, true, true), "a peak of 1.0 or more latches");
    frames(&mut app, &ctx, 3, &mut time);
    // The bar keeps its level colors; the readout turns red (text), the latch is a cap.
    assert_eq!(strip(&app, a).peak_text, "+1.6");
    // A click on the meter clears that strip only.
    let at = mid(strip(&app, a).meter);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!((clips(&app), app.meters[&a].peak), ((false, true, true), 0.0));
    // A click on the readout too; Ctrl-click clears all.
    latch(&mut app);
    frames(&mut app, &ctx, 3, &mut time);
    let at = mid(strip(&app, b).peak_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!((app.meters[&b].clip, app.meters[&b].peak, app.meters[&a].clip), (false, 0.0, true));
    let at = mid(strip(&app, a).meter);
    click_with(&mut app, at, json!({"ctrl": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(clips(&app), (false, false, false));
    assert_eq!(app.main_meter.peak, 0.0);
    // Play start clears every latch (and keeps the holds).
    latch(&mut app);
    app.run("transport.play", json!({})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(clips(&app), (false, false, false), "starting playback clears the latches");
    assert!(app.meters[&a].peak > 1.0, "the peak hold stays");
    app.run("transport.stop", json!({})).unwrap();
    // The command and Alt+C.
    latch(&mut app);
    app.run("track.clear_clip_indicators", json!({})).unwrap();
    assert_eq!(clips(&app), (false, false, false));
    latch(&mut app);
    send_control(&mut app, "ui.key", json!({"key": "C", "alt": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(clips(&app), (false, false, false), "Alt+C");
}

#[test]
fn the_main_out_strip_has_a_peak_readout_and_clears_on_click() {
    let (mut app, ctx, mut time, _a) = mixer_app();
    crate::host::feed_meter(&mut app.main_meter, [0.5, 1.1], 0.0, 0.016);
    frames(&mut app, &ctx, 3, &mut time);
    let m = app.mixer.layout.master.clone().unwrap();
    assert_eq!(m.peak_text, "+0.8");
    assert!(app.main_meter.clip);
    let at = mid(m.peak_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    assert!(!app.main_meter.clip && app.main_meter.peak == 0.0);
}

#[test]
fn clearing_the_main_meter_clears_the_master_tracks_meter_and_back() {
    let (mut app, _ctx, _time, _a) = mixer_app();
    app.run("track.new", json!({"kind": "master"})).unwrap();
    let master = app.engine.session().tracks.iter().find(|t| t.kind == transit_model::TrackKind::Master).map(|t| t.id).expect("a Master track");
    let hot = |app: &mut StudioApp| {
        crate::host::feed_meter(&mut app.main_meter, [1.2, 1.2], 0.0, 0.016);
        crate::host::feed_meter(app.meters.entry(master).or_default(), [1.2, 1.2], 0.0, 0.016);
    };
    hot(&mut app);
    app.reset_main_meter();
    assert!(!app.main_meter.clip && !app.meters[&master].clip && app.main_meter.peak == 0.0, "the transport meter clears both");
    hot(&mut app);
    app.reset_meters(Some(master));
    assert!(!app.main_meter.clip && !app.meters[&master].clip, "the master strip clears both");
    hot(&mut app);
    app.reset_meters(None);
    assert!(!app.main_meter.clip && !app.meters[&master].clip, "Ctrl-click clears everything");
}
