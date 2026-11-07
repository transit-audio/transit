//! The hero: the transfer curve's math (three hand-computed points, a wrong-ratio control, and the
//! plug-in's own steady state), the wells and the gain-reduction meter as laid out, the meter's
//! peak fall-off, and the scope tap the panel switches on.

use super::hero::{self, CURVE_POINTS, GR_BAR_W, WELL, gr_readout, transfer_db, transfer_gr_db};
use super::test_rig::{SLOT, T};

const EPS: f32 = 1e-4;

#[test]
fn the_transfer_curve_matches_three_hand_computed_points() {
    // Threshold −20 dB, ratio 4 (slope 1 − 1/4 = 0.75), knee 6 dB (±3 dB around the threshold).
    let (t, r, k) = (-20.0, 4.0, 6.0);
    // Below the knee: −30 dB is 10 dB under the threshold, outside ±3: no reduction.
    assert!((transfer_db(-30.0, t, r, k, 0.0, false) - -30.0).abs() < EPS);
    // In the knee, at the threshold: over = 0, t = 0 + 6/2 = 3, gr = 0.75 · 3² / (2 · 6) = 0.5625.
    assert!((transfer_gr_db(-20.0, t, r, k) - 0.5625).abs() < EPS);
    assert!((transfer_db(-20.0, t, r, k, 0.0, false) - -20.5625).abs() < EPS);
    // Above the knee: −8 dB is 12 over, gr = 0.75 · 12 = 9, out = −17 dB.
    assert!((transfer_db(-8.0, t, r, k, 0.0, false) - -17.0).abs() < EPS);
    // Negative control: ratio 2 (slope 0.5) gives −14 dB there, not −17.
    let wrong = transfer_db(-8.0, t, 2.0, k, 0.0, false);
    assert!((wrong - -14.0).abs() < EPS && (wrong - -17.0).abs() > 1.0, "{wrong}");
    // Makeup adds after the reduction; Limit is a ratio of 1000.
    assert!((transfer_db(-8.0, t, r, k, 4.0, false) - -13.0).abs() < EPS);
    let limited = transfer_db(-8.0, t, r, k, 0.0, true);
    assert!((limited - (-8.0 - 0.999 * 12.0)).abs() < EPS, "{limited}");
}

#[test]
fn the_transfer_curve_is_the_plugins_own_steady_state() {
    // A constant level through the plug-in itself until its detector settles: the output level is
    // what the curve says.
    for (threshold, ratio, knee, makeup, limit, level_db) in [
        (-20.0, 4.0, 6.0, 0.0, false, -6.0),
        (-30.0, 8.0, 0.0, 6.0, false, -12.0),
        (-18.0, 2.0, 12.0, 0.0, false, -20.0),
        (-10.0, 4.0, 6.0, 0.0, true, -2.0),
    ] {
        let mut p = transit_dsp::create(super::COMPRESSOR).unwrap();
        p.prepare(48_000.0, 512, 2);
        for (id, v) in [("threshold", threshold), ("ratio", ratio), ("knee", knee), ("makeup", makeup), ("limiter", f32::from(u8::from(limit)))] {
            assert!(p.set_param(id, v));
        }
        let a = 10f32.powf(level_db / 20.0);
        let mut io = vec![vec![a; 512]; 2];
        for _ in 0..200 {
            io.iter_mut().for_each(|c| c.fill(a));
            p.process(&mut io, 512);
        }
        let out_db = 20.0 * io[0][511].log10();
        let want = transfer_db(level_db, threshold, ratio, knee, makeup, limit);
        assert!((out_db - want).abs() < 0.01, "{threshold} {ratio} {knee} {makeup} {limit} at {level_db}: plug-in {out_db}, curve {want}");
    }
}

#[test]
fn the_hero_lays_out_the_boards_three_columns() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    let hero_r = p.section("hero").unwrap();
    let tw = p.well("transfer").unwrap();
    let hw = p.well("history").unwrap();
    let gr = p.hero.gr_meter.as_ref().unwrap();
    // The transfer well is the board's 236 square, its plot inside 8 of padding.
    assert_eq!((tw.well.well[2] - tw.well.well[0], tw.well.well[3] - tw.well.well[1]), (WELL, WELL));
    assert_eq!(tw.well.plot[2] - tw.well.plot[0], WELL - 16.0);
    // History fills between the transfer well and the GR column, 12 either side.
    assert_eq!(hw.well.well[0] - tw.well.well[2], crate::tokens::layout::PANEL_GAP);
    assert_eq!(hw.well.well[1], tw.well.well[1]);
    assert_eq!(hw.well.well[3], tw.well.well[3]);
    assert!(gr.well[0] - hw.well.well[2] > crate::tokens::layout::PANEL_GAP, "the GR column is right of the history");
    assert_eq!(gr.well[2] - gr.well[0], GR_BAR_W);
    assert_eq!(gr.tag.map(|t| t.0), Some("GR"), "the kit's gain-reduction tag");
    assert!(p.hero.gr_readout_rect[3] <= tw.well.well[3] + 0.5, "the readout ends with the wells");
    // Everything inside the hero panel; the labels above the wells.
    for r in [tw.well.well, hw.well.well, gr.well, p.hero.gr_readout_rect] {
        assert!(r[0] >= hero_r[0] && r[2] <= hero_r[2] && r[1] >= hero_r[1] && r[3] <= hero_r[3], "{r:?} in {hero_r:?}");
    }
    let labels: Vec<&str> = p.hero.labels.iter().map(|l| l.0.as_str()).collect();
    assert_eq!(labels, ["TRANSFER", "HISTORY"]);
    for (_, l) in &p.hero.labels {
        assert!(l[3] <= tw.well.well[1], "{l:?}");
    }
    // The grids: the quarters across and up in the transfer well, across in the history.
    assert_eq!(tw.grid.len(), 6);
    assert_eq!(hw.grid.len(), 6, "and a line each second");
    assert_eq!(tw.well.curve_color, crate::theme::TOKENS.signal);
}

#[test]
fn the_curve_follows_the_devices_values() {
    let (mut t, tr) = T::open();
    let curve = t.panel(tr).hero.curve;
    assert_eq!(curve.len(), CURVE_POINTS);
    // Defaults: threshold −20, ratio 4, knee 6. 0 dB in is 20 over, outside the knee:
    // 0 − 0.75 · 20 = −15 dB out, 0.75 up the well.
    assert_eq!(curve[0], [0.0, 0.0], "−60 in, −60 out");
    assert!((curve[CURVE_POINTS - 1][1] - hero::scale(-15.0)).abs() < 1e-5, "{:?}", curve[CURVE_POINTS - 1]);
    assert!((hero::scale(-15.0) - 0.75).abs() < 1e-6);
    t.set(tr, "ratio", 2.0);
    let curve = t.panel(tr).hero.curve;
    // Negative control: ratio 2 lifts the top to −10 dB.
    assert!((curve[CURVE_POINTS - 1][1] - hero::scale(-10.0)).abs() < 1e-5);
    t.set(tr, "limiter", 1.0);
    let curve = t.panel(tr).hero.curve;
    assert!((curve[CURVE_POINTS - 1][1] - hero::scale(-20.0 + 20.0 / 1000.0)).abs() < 1e-4, "Limit flattens it at the threshold");
}

#[test]
fn the_gain_reduction_readout_and_its_peak_fall_off() {
    assert_eq!(gr_readout(4.2), "-4.2 dB");
    assert_eq!(gr_readout(0.0), "0.0 dB");
    assert_eq!(gr_readout(0.04), "0.0 dB");
    let (mut t, tr) = T::open();
    let p = t.panel(tr);
    assert_eq!(p.hero.gr_readout, "0.0 dB");
    let fill = p.hero.gr_meter.as_ref().unwrap().fill;
    assert_eq!(fill[3] - fill[1], 0.0, "no reduction, no bar");
    assert_eq!(t.window(tr).gr, "GR 0.0 dB", "the editor record reads it too");
    // A reading of 6 dB that stops: the bar falls off over 300 ms instead of vanishing.
    let uid = t.window(tr).uid;
    t.app.builtin_editor.devices.gr.insert(uid, 6.0);
    t.frames(1);
    let p = t.panel(tr);
    assert!(p.hero.gr_db > 0.5 && p.hero.gr_db < 6.0, "one frame in: falling, not gone: {}", p.hero.gr_db);
    let m = p.hero.gr_meter.as_ref().unwrap();
    assert!(m.fill[3] > m.fill[1], "the bar has a height");
    assert_eq!(m.fill[1], m.well[1] + 1.0, "it grows down from the top");
    t.frames(30);
    let p = t.panel(tr);
    assert!(p.hero.gr_db < 0.05, "half a second later: recovered: {}", p.hero.gr_db);
    assert_eq!(p.hero.gr_readout, "0.0 dB");
}

#[test]
fn the_open_panel_switches_its_scope_tap_on_and_off() {
    let mut t = T::new();
    let tr = t.compressor_track();
    let uid = t.app.engine.session().track(tr).unwrap().mixer.inserts[SLOT].as_ref().unwrap().uid.get();
    let hub = std::sync::Arc::new(transit_mix::ScopeHub::default());
    let tap = std::sync::Arc::new(transit_mix::ScopeTap::new());
    hub.register(tr, uid, &tap);
    t.app.builtin_editor.hub = Some(hub);
    assert!(!tap.enabled(), "a closed panel leaves the tap off");
    t.open_card(0);
    assert!(tap.enabled(), "opening the panel switches the tap on");
    assert_eq!(t.window(tr).level, "-inf dB", "the output level from the tap");
    t.open_card(0);
    t.frames(2);
    assert!(!tap.enabled(), "closing the panel switches it off");
}
