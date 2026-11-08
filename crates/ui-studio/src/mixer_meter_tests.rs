use crate::tests::*;
use crate::theme::TOKENS;

#[test]
fn the_peak_readout_holds_the_loudest_peak_until_clicked() {
    let (mut app, ctx, mut time, a) = mixer_app();
    let r = app.run("track.new", json!({"kind": "audio", "format": "stereo", "name": "Vox"})).unwrap();
    let b = TrackId(r["tracks"][0].as_u64().unwrap());
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "−inf");
    // A known peak (0.7 is -3.1 dBFS), then silence: the readout holds it.
    for (t, peak) in [(a, 0.7f32), (b, 1.2f32)] {
        let d = app.meters.entry(t).or_default();
        crate::host::feed_meter(d, [peak, 0.1], 0.0, 0.016);
        crate::host::feed_meter(d, [0.0, 0.0], 0.0, 0.016);
    }
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "−3.1");
    assert_eq!(strip(&app, b).peak_text, "+1.6", "over 0 dBFS reads with a plus sign");
    let d = app.meters.get_mut(&a).unwrap();
    crate::host::feed_meter(d, [0.5, 0.0], 0.0, 0.016);
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(strip(&app, a).peak_text, "−3.1", "a quieter peak does not lower the hold");
    // Click resets that strip only.
    let at = mid(strip(&app, a).peak_rect);
    click(&mut app, at);
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(strip(&app, a).peak_text, "−inf");
    assert_eq!(strip(&app, b).peak_text, "+1.6");
    // Ctrl-click resets all.
    let at = mid(strip(&app, b).peak_rect);
    click_with(&mut app, at, json!({"ctrl": true}));
    frames(&mut app, &ctx, 4, &mut time);
    assert_eq!(strip(&app, b).peak_text, "−inf");
    // Exactly 0 dBFS reads plain, as the fill draws no hot at 0; the first tick above is hot with
    // its plus sign. Negative control: +0.1 dB (1.0116) is hot.
    assert_eq!(crate::mixer::peak_text(1.0), ("0.0".to_string(), false));
    assert_eq!(crate::mixer::peak_text(1.0116), ("+0.1".to_string(), true));
    assert_eq!(crate::mixer::peak_text(0.5), ("−6.0".to_string(), false));
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

/// A device-free Player on the app's session, and `n` device blocks with a UI frame after each.
fn step(app: &mut StudioApp, ctx: &egui::Context, time: &mut f64, n: usize) {
    for _ in 0..n {
        app.player.as_ref().unwrap().step_device_free();
        frames(app, ctx, 1, time);
    }
}

/// What an idle strip must show: no latch on either cell, the floor in the peak readout and no
/// `meter.hot` anywhere on it.
fn assert_idle(app: &StudioApp, t: TrackId, why: &str) {
    let d = app.meters.get(&t).copied().unwrap_or_default();
    assert_eq!((d.clip, d.clip_ch, d.peak), (false, [false; 2], 0.0), "{why}: the meter state");
    let s = strip(app, t);
    assert_eq!(s.peak_text, "−inf", "{why}: the readout reads the floor");
    assert_ne!(s.peak_color, TOKENS.meter_hot.to_array(), "{why}: the readout is not hot");
    let m = s.meter_layout.unwrap();
    assert!(m.clip_colors.iter().all(|c| *c == TOKENS.line), "{why}: both clip cells are `line`");
    assert!(m.peaks.iter().chain(&m.peak_over).all(Option::is_none), "{why}: no fill");
}

#[test]
fn a_fresh_instrument_track_that_played_nothing_shows_no_latch_and_the_floor() {
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "instrument"}));
    app.ui.show_mixer = true;
    app.player = Some(transit_playback::Player::prepare_device_free(app.engine.session_arc()).unwrap());
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    // About two seconds of device blocks with the default instrument loaded and no note.
    step(&mut app, &ctx, &mut time, 200);
    assert_idle(&app, t, "a fresh instrument track");
}

/// The idle-strip latch (#326): a new document reuses track ids, and the meter snapshot the last
/// document's Player published was fed into the new document's strips. Track 1 of the old document
/// played at +0.1 dBFS; track 1 of the new one is an instrument that never played.
#[test]
fn a_new_document_does_not_inherit_the_last_documents_meters() {
    use transit_model::SourceId;
    let mut app = app();
    let loud = new_track(&mut app, json!({"kind": "audio", "format": "stereo"}));
    {
        // A square wave at 1.0116 (+0.1 dBFS) for ten seconds.
        let demo = transit_engine::demo::demo_session();
        let mut buf = demo.pool.get(demo.pool.ids().next().unwrap()).unwrap().buffer.clone();
        let len = 480_000usize;
        let sig: Vec<f32> = (0..len).map(|i| if (i / 50) % 2 == 0 { 1.0116 } else { -1.0116 }).collect();
        (buf.sample_rate, buf.channels) = (48_000, vec![sig.clone(), sig]);
        let s = app.engine.session_mut();
        s.pool.insert(SourceId(1), std::sync::Arc::new(transit_model::SourceAudio::new(buf)));
        let id = s.new_clip_id();
        s.track_mut(loud).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::audio(id, "x", SourceId(1), 0, 0, len as i64));
    }
    app.ui.show_mixer = true;
    app.player = Some(transit_playback::Player::prepare_device_free(app.engine.session_arc()).unwrap());
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    app.run("transport.play", json!({})).unwrap();
    step(&mut app, &ctx, &mut time, 100);
    // Control: the old document's strip did latch and read +0.1.
    assert_eq!((app.meters[&loud].clip_ch, strip(&app, loud).peak_text.as_str()), ([true; 2], "+0.1"));
    // A new document while it plays; its first track takes the same id.
    app.run("session.new", json!({})).unwrap();
    let idle = new_track(&mut app, json!({"kind": "instrument"}));
    assert_eq!(idle, loud, "the new document reuses the id");
    frames(&mut app, &ctx, 1, &mut time);
    step(&mut app, &ctx, &mut time, 50);
    assert_idle(&app, idle, "the new document's idle instrument track");
}

/// #395: the engine's per-channel RMS reaches the UI snapshot, the strip's meter state and the
/// solid RMS layer, end to end through a device-free Player. A full-scale sine on the left reads
/// −3.01 dBFS RMS at a 0 dBFS peak; a half-scale square on the right reads −6.02 for both.
#[test]
fn the_strip_meter_draws_the_engines_rms() {
    use transit_model::SourceId;
    let mut app = app();
    let t = new_track(&mut app, json!({"kind": "audio", "format": "stereo"}));
    {
        let demo = transit_engine::demo::demo_session();
        let mut buf = demo.pool.get(demo.pool.ids().next().unwrap()).unwrap().buffer.clone();
        let len = 480_000usize;
        let sine: Vec<f32> = (0..len).map(|i| (std::f32::consts::TAU * (i % 48) as f32 / 48.0).sin()).collect();
        let square: Vec<f32> = (0..len).map(|i| if (i / 24) % 2 == 0 { 0.5 } else { -0.5 }).collect();
        (buf.sample_rate, buf.channels) = (48_000, vec![sine, square]);
        let s = app.engine.session_mut();
        s.pool.insert(SourceId(1), std::sync::Arc::new(transit_model::SourceAudio::new(buf)));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::audio(id, "x", SourceId(1), 0, 0, len as i64));
    }
    app.ui.show_mixer = true;
    app.player = Some(transit_playback::Player::prepare_device_free(app.engine.session_arc()).unwrap());
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    // Silence first: no RMS layer.
    step(&mut app, &ctx, &mut time, 10);
    assert!(strip(&app, t).meter_layout.unwrap().rms.iter().all(Option::is_none), "stopped: silence");
    app.run("transport.play", json!({})).unwrap();
    // 100 blocks of 512 frames: past the 14 400-frame window.
    step(&mut app, &ctx, &mut time, 100);
    let db = |lin: f32| 20.0 * lin.log10();
    let snap = &app.meter_snapshot.tracks[&t];
    assert_eq!(snap.rms_channels.len(), 2, "the snapshot carries an RMS per channel");
    let rms = app.meters[&t].rms.map(db);
    assert!((rms[0] + 3.01).abs() < 0.05 && (rms[1] + 6.02).abs() < 0.05, "the strip's RMS: {rms:?}");
    let s = strip(&app, t);
    let m = s.meter_layout.clone().expect("the meter");
    let well = m.wells[0];
    let y = |db: f32| well[1] + crate::widgets::fader::taper(db) * (well[3] - well[1]);
    for (c, want) in [(0, rms[0]), (1, rms[1])] {
        let fill = m.rms[c].unwrap_or_else(|| panic!("channel {c}: a solid RMS layer"));
        assert!((fill[1] - y(want)).abs() < 0.02, "channel {c}: the RMS fill tops at {want} dB on the taper: {fill:?}");
    }
    // Negative control: the sine's RMS fill tops below its peak fill.
    let (peak, sine_rms) = (m.peaks[0].unwrap(), m.rms[0].unwrap());
    assert!(sine_rms[1] > peak[1] + 1.0, "RMS below peak: {sine_rms:?} {peak:?}");
}

/// The compact round's rects (the CompactMixer board's proposed row), block-relative (x from the
/// level block's left, y from its top), at 1.0 and 1.5, for the docked strip, the full mixer and
/// the main bus (with and without a master fader): stopped (the strip as the mixer paints it), and
/// with a −6 dB peak over a −12 dB RMS reading and a −6 dB peak-only source, each painted by the
/// kit meter on that strip's frame (a live reading falls off within a frame in a test, so the
/// sources are painted at rest). Run with `--nocapture`. The round's rules are asserted along the
/// way: no fill at unity, the notch on the 0 dB tick's row, the grid on the tick rows, square
/// fills, an opaque peak-only fill.
#[test]
fn the_compact_round_as_rects() {
    use crate::widgets::level_meter::{Channel, paint_channel};
    let (mut app, ctx, mut time, a) = mixer_app();
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
                let ch = strip(&app, a);
                let m = app.mixer.layout.master.clone().unwrap();
                for source in ["stopped", "peak -6 + rms -12", "peak-only -6"] {
                    let what = format!("{} master {master} {ppp}x {source}", if full { "full" } else { "docked" });
                    for (who, s) in [("Inst", &ch), ("Main", &m)] {
                        let x0 = s.meter_layout.as_ref().unwrap().wells[0][0] - crate::tokens::fader::COLUMN_METER_X;
                        let y0 = s.fader[1] - crate::tokens::fader::INSET;
                        let rel = |r: [f32; 4]| [r[0] - x0, r[1] - y0, r[2] - x0, r[3] - y0].map(|v| (v * 100.0).round() / 100.0);
                        let fl = s.fader_layout.as_ref();
                        let scale = fl.map(|f| (f.placed.clone(), f.ticks.clone()));
                        let (placed, ticks) = scale.or_else(|| s.main_scale.as_ref().map(|x| (x.placed.clone(), x.ticks.clone()))).unwrap();
                        let mut ml = s.meter_layout.clone().unwrap();
                        if source != "stopped" {
                            let travel = egui::Rect::from_min_max(egui::pos2(s.fader[0], s.fader[1]), egui::pos2(s.fader[2], s.fader[3]));
                            let bar = ml.wells[0][2] - ml.wells[0][0];
                            let rms_db = source.contains("rms").then_some(-12.0);
                            let one = Channel { peak_db: -6.0, rms_db, hold_db: Some(-6.0), clip: false };
                            let mut rig = crate::widgets::test_support::Rig::new();
                            rig.ctx.set_pixels_per_point(ppp);
                            ml = rig.idle(|ui| paint_channel(ui.painter(), ml.wells[0][0], travel, bar, &placed, &[one, one]));
                            let want = if rms_db.is_some() { TOKENS.meter_ok.gamma_multiply(0.35) } else { TOKENS.meter_ok };
                            assert_eq!(ml.peak_inks[0], want, "{what} {who}: the peak fill's ink");
                        }
                        assert_eq!(ml.fill_radius, 0.0, "{what} {who}");
                        let rows = |v: &[[f32; 4]]| v.iter().map(|r| [(r[1] * ppp).round() as i32, (r[3] * ppp).round() as i32]).collect::<Vec<_>>();
                        assert_eq!(rows(&ml.grid), rows(&ticks), "{what} {who}: the grid on the tick rows");
                        let fader_part = match fl {
                            Some(f) => {
                                let zero = f.marks.iter().position(|(t, _)| t == "0").unwrap();
                                assert_eq!(rows(&[f.notch]), rows(&[f.ticks[zero]]), "{what} {who}: the notch on the 0 dB row");
                                assert_eq!(f.fill[3] - f.fill[1], 0.0, "{what} {who}: no fill at unity");
                                format!("fill {:?} notch {:?} cap {:?} center line {:?}", rel(f.fill), rel(f.notch), rel(f.cap), rel(f.center_line))
                            }
                            None => "fader column empty".into(),
                        };
                        let opt = |v: &[Option<[f32; 4]>]| v.iter().map(|r| r.map(rel)).collect::<Vec<_>>();
                        println!(
                            "{what} {who}: travel {} {fader_part} | wells {:?} grid {:?} peaks {:?} peak over {:?} rms {:?} holds {:?}",
                            s.fader[3] - s.fader[1],
                            ml.wells.iter().map(|w| rel(*w)).collect::<Vec<_>>(),
                            ml.grid.iter().map(|w| rel(*w)).collect::<Vec<_>>(),
                            opt(&ml.peaks),
                            opt(&ml.peak_over),
                            opt(&ml.rms),
                            opt(&ml.holds),
                        );
                    }
                }
            }
        }
    }
}
