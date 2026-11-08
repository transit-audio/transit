//! The fader: the one dB taper (marks, interpolation, the compressed bottom, the inverse), the
//! variants' geometry and tokens, mute dimming, and the gestures (drag, Shift drag, Alt-click and
//! double-click to unity, scroll by 0.5 dB).

use super::fader::{self, Variant};
use super::test_support::{Rig, inside, mid, size};
use crate::theme::DARK;
use egui::{Modifiers, vec2};

#[test]
fn the_taper_hits_every_mark_and_is_linear_between() {
    for (db, f) in crate::tokens::fader::TAPER {
        assert_eq!(fader::taper(db.unwrap_or(f32::NEG_INFINITY)), f, "{db:?}");
    }
    assert!((fader::taper(-3.0) - 0.21).abs() < 1e-6, "halfway between 0 and -6");
    assert!((fader::taper(-75.0) - 0.97).abs() < 1e-6, "-60..-90 compresses into the last 6 percent");
    assert_eq!(fader::taper(-90.0), 1.0);
    assert_eq!(fader::taper(-120.0), 1.0);
    assert_eq!(fader::taper(12.0), 0.0, "clamped at +6");
    for db in [6.0, 3.0, 0.0, -1.5, -9.0, -30.0, -54.0, -60.0, -80.0] {
        assert!((fader::db_at(fader::taper(db)) - db).abs() < 1e-3, "{db} round trips");
    }
    assert_eq!(fader::db_at(1.0), f32::NEG_INFINITY);
    let labels: Vec<String> = crate::tokens::fader::SCALE_LABELS.iter().map(|m| fader::label(*m)).collect();
    assert_eq!(labels, ["+6", "0", "−6", "−12", "−24", "−36", "−48", "−∞"]);
}

#[test]
fn each_variant_has_its_geometry_and_tokens() {
    let mut rig = Rig::new();
    for (variant, cap) in [(Variant::Channel, (32.0, 16.0)), (Variant::Compact, (28.0, 14.0)), (Variant::Horizontal, (12.0, 18.0))] {
        let l = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), variant, 220.0, 0.0, false)).layout;
        assert_eq!(size(l.cap), cap, "{variant:?}");
        assert_eq!((l.fill_color, l.cap_fill, l.cap_rim), (DARK.signal, DARK.knob_cap_active, DARK.ink_arrow));
        assert!(inside(l.track, l.rect) && inside(l.fill, l.track));
        let unity = 0.14;
        match variant {
            Variant::Horizontal => {
                assert_eq!(size(l.rect).1, 24.0);
                assert_eq!(size(l.track), (220.0 - 20.0, 4.0), "4 px track, 10 px inset");
                assert!((mid(l.cap).x - (l.track[2] - unity * (l.track[2] - l.track[0]))).abs() < 0.02);
                assert_eq!(l.fill[0], l.track[0], "fill from the left");
            }
            _ => {
                assert_eq!(size(l.rect).0, if variant == Variant::Channel { 116.0 } else { 40.0 });
                assert_eq!(size(l.track), (4.0, 220.0 - 20.0));
                assert!((mid(l.cap).y - (l.track[1] + unity * (l.track[3] - l.track[1]))).abs() < 0.02, "0 dB sits at 14 percent");
                assert_eq!(l.fill[3], l.track[3], "fill from the bottom");
                assert!((l.fill[1] - mid(l.cap).y).abs() < 0.02, "fill up to the cap");
                assert_eq!(l.marks.len(), 8);
            }
        }
        if variant == Variant::Channel {
            assert_eq!(l.marks[1].0, "0");
            assert_eq!(l.mark_inks[1], DARK.text, "0 in text");
            assert!(l.mark_inks.iter().enumerate().all(|(i, c)| i == 1 || *c == DARK.text_faint), "others faint");
            let m = l.meter.unwrap();
            assert!(inside(m, l.rect) && l.marks.iter().all(|(_, r)| r[2] <= m[0]), "the meter slot right of the scale");
            // The scale shares the taper: each label is centered on its mark.
            for ((_, r), db) in l.marks.iter().zip(crate::tokens::fader::SCALE_LABELS) {
                let y = l.track[1] + fader::taper(db.unwrap_or(f32::NEG_INFINITY)) * (l.track[3] - l.track[1]);
                assert!((mid(*r).y - y).abs() < 0.6, "{db:?}");
            }
        }
    }
    let muted = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), Variant::Compact, 200.0, 0.0, true)).layout;
    assert_eq!(muted.fill_color, DARK.ink_ghost, "a muted channel dims its fill");
}

#[test]
fn drag_shift_drag_unity_and_scroll() {
    let mut rig = Rig::new();
    let l = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), Variant::Compact, 220.0, 0.0, false)).layout;
    let travel = l.track[3] - l.track[1];
    let run = |rig: &mut Rig| {
        let mut db = 0.0f32;
        rig.drag(mid(l.cap), vec2(0.0, 0.14 * travel), 2, |ui| {
            if let Some(d) = fader::show(ui, egui::Id::new("f"), Variant::Compact, 220.0, db, false).db {
                db = d;
            }
        });
        db
    };
    let coarse = run(&mut rig);
    assert!((coarse - -6.0).abs() < 0.05, "dragging down 14 percent from unity reaches -6 dB: {coarse}");
    rig.modifiers = Modifiers::SHIFT;
    let fine = run(&mut rig);
    assert!((fine - -0.6).abs() < 0.05, "Shift is ten times finer: {fine}");
    rig.modifiers = Modifiers::ALT;
    let out = rig.click(mid(l.cap), |ui| fader::show(ui, egui::Id::new("f"), Variant::Compact, 220.0, -12.0, false));
    assert_eq!(out.db, Some(0.0), "Alt-click to unity");
    rig.modifiers = Modifiers::NONE;
    let out =
        rig.frame(Rig::wheel(mid(l.cap), 40.0, Modifiers::NONE), |ui| fader::show(ui, egui::Id::new("f"), Variant::Compact, 220.0, -12.0, false));
    assert_eq!(out.db, Some(-11.5), "one notch up is +0.5 dB");
    let mut got = None;
    let p = mid(l.cap);
    for events in [Rig::press(p, Modifiers::NONE), Rig::release(p, Modifiers::NONE), Rig::press(p, Modifiers::NONE), Rig::release(p, Modifiers::NONE)]
    {
        got = got.or(rig.frame(events, |ui| fader::show(ui, egui::Id::new("f"), Variant::Compact, 220.0, -12.0, false)).db);
    }
    assert_eq!(got, Some(0.0), "double-click to unity");
}

/// Whether a tick `t` sits where the Fader construction board puts it in a block whose left edge
/// is `x0`: x 63 to 67, right of its label's right edge.
fn tick_in_place(t: [f32; 4], label: [f32; 4], x0: f32) -> bool {
    (t[0] - x0 - 63.0).abs() < 0.01 && (t[2] - x0 - 67.0).abs() < 0.01 && t[0] > label[2]
}

/// Fault 2 of the Fader construction board: the label right-aligned at x 60, then 3 px, then the
/// 4 px tick (x 63 to 67), then the meter at 72; never a dash on the cap side of `0`.
#[test]
fn the_channel_tick_sits_after_its_label_toward_the_meter() {
    let mut rig = Rig::new();
    // The cap at +6 and at minus infinity: the ticks stay where the board puts them either way.
    for db in [6.0, f32::NEG_INFINITY] {
        let l = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), Variant::Channel, 220.0, db, false)).layout;
        let x0 = l.rect[0];
        assert_eq!(l.ticks.len(), 8, "a tick with each placed label");
        assert_eq!(l.meter.unwrap()[0] - x0, 72.0, "the meter stays at x 72");
        for (i, (t, (text, label))) in l.ticks.iter().zip(&l.marks).enumerate() {
            assert_eq!(size(*t).0, 4.0, "{text}: 4 px long");
            assert!((size(*t).1 - 1.0).abs() < 0.01, "one pixel row");
            assert!(tick_in_place(*t, *label, x0), "{text}: x 63 to 67, right of the label: {t:?} {label:?} (block at {x0})");
            assert!((label[2] - x0 - 60.0).abs() < 0.6, "{text}: right-aligned at x 60: {label:?}");
            assert!((mid(*t).y - mid(*label).y).abs() < 0.51, "{text}: the label's center on its tick's row");
            let ink = if text == "0" { DARK.text_muted } else { DARK.line_strong };
            assert_eq!(l.tick_inks[i], ink, "{text}: `line.strong`, `text.muted` at 0 dB");
            assert_eq!(l.mark_inks[i], if text == "0" { DARK.text } else { DARK.text_faint }, "{text}");
        }
        // Negative control: the old tick (6 px, 2 right of the cap column: x 36 to 42) fails.
        let t = l.ticks[1];
        let old = [mid(l.track).x + 16.0 + 2.0, t[1], mid(l.track).x + 16.0 + 8.0, t[3]];
        assert_eq!((old[0] - x0, old[2] - x0), (36.0, 42.0));
        assert!(!tick_in_place(old, l.marks[1].1, x0), "the old tick is on the cap side of 0");
    }
    // The other variants draw no Channel ticks (Compact keeps its own beside the track).
    for variant in [Variant::Compact, Variant::Horizontal] {
        let l = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), variant, 220.0, 0.0, false)).layout;
        assert!(l.ticks.is_empty() && l.tick_inks.is_empty(), "{variant:?}");
    }
}

/// The labels' centers on a travel (points from the travel's top) for `marks`.
fn centers(marks: &[Option<f32>], travel: f32) -> Vec<f32> {
    marks.iter().map(|m| fader::taper(m.unwrap_or(f32::NEG_INFINITY)) * travel).collect()
}

/// The closest two of `ys`.
fn closest(ys: &[f32]) -> f32 {
    let mut min = f32::INFINITY;
    for (i, a) in ys.iter().enumerate() {
        for b in &ys[i + 1..] {
            min = min.min((a - b).abs());
        }
    }
    min
}

/// Fault 1 of the Fader construction board: on short travel the Channel scale places its labels
/// greedily by priority, 13 apart, and draws a tick with each placed label only.
#[test]
fn the_channel_labels_thin_by_travel_greedily_and_never_collide() {
    let names = |marks: Vec<Option<f32>>| marks.into_iter().map(fader::label).collect::<Vec<_>>();
    for travel in [120.0, 150.0, 200.0] {
        assert_eq!(names(fader::placed_marks(travel)), ["+6", "0", "−6", "−12", "−24", "−36", "−48", "−∞"], "{travel}: all eight");
    }
    assert_eq!(names(fader::placed_marks(100.0)), ["+6", "0", "−6", "−12", "−24", "−48", "−∞"], "100: all but −36");
    for travel in [92.0, 80.0] {
        assert_eq!(names(fader::placed_marks(travel)), ["0", "−12", "−24", "−∞"], "{travel}");
    }
    // A sweep: no two placed label centers are under 13 apart at any travel from 80 to 600.
    let mut travel = 80.0;
    while travel <= 600.0 {
        let placed = fader::placed_marks(travel);
        assert!(placed.contains(&Some(0.0)) && placed.contains(&None), "{travel}: 0 and −∞ always");
        assert!(closest(&centers(&placed, travel)) >= crate::tokens::fader::LABEL_MIN_GAP - 1e-4, "{travel}: {placed:?}");
        travel += 0.25;
    }
    // The painted scale follows: at 80 of travel (a 100 tall fader) four labels, four ticks.
    let mut rig = Rig::new();
    let l = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), Variant::Channel, 100.0, 0.0, false)).layout;
    let painted: Vec<&str> = l.marks.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(painted, ["0", "−12", "−24", "−∞"]);
    assert_eq!(l.ticks.len(), 4, "a tick with each placed label only");
    let ys: Vec<f32> = l.marks.iter().map(|(_, r)| mid(*r).y).collect();
    assert!(closest(&ys) >= 13.0 - 0.6, "the painted labels are 13 apart: {ys:?}");
    // Negative control: the old placement (every mark at every travel) fails the 80 set and
    // collides there.
    let old: Vec<Option<f32>> = crate::tokens::fader::SCALE_LABELS.to_vec();
    assert_ne!(names(old.clone()), ["0", "−12", "−24", "−∞"]);
    assert!(closest(&centers(&old, 80.0)) < 13.0, "all eight at 80 overlap");
}
