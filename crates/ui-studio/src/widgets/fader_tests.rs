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

#[test]
fn the_channel_scale_has_a_six_px_tick_beside_the_cap_column_at_every_mark() {
    let mut rig = Rig::new();
    // The cap at +6 and at minus infinity: the ticks stay where the board puts them either way.
    for db in [6.0, f32::NEG_INFINITY] {
        let l = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), Variant::Channel, 220.0, db, false)).layout;
        assert_eq!(l.ticks.len(), 8, "a tick at each scale mark");
        for (i, (t, db)) in l.ticks.iter().zip(crate::tokens::fader::SCALE_LABELS).enumerate() {
            assert_eq!(size(*t).0, 6.0, "6 px long");
            assert!((size(*t).1 - 1.0).abs() < 0.01, "one pixel row");
            let cap_right = mid(l.track).x + 16.0;
            assert_eq!(t[0], cap_right + 2.0, "2 px right of the cap column");
            let y = l.track[1] + fader::taper(db.unwrap_or(f32::NEG_INFINITY)) * (l.track[3] - l.track[1]);
            assert!((mid(*t).y - y).abs() < 0.6, "{db:?} on its mark");
            assert!(t[2] < l.marks[i].1[0], "{db:?}: left of its label");
            let ink = if i == 1 { DARK.text_muted } else { DARK.line_strong };
            assert_eq!(l.tick_inks[i], ink, "{db:?}: `line.strong`, `text.muted` at 0 dB");
        }
    }
    // Negative control: the other variants draw no Channel ticks (Compact keeps its own beside the track).
    for variant in [Variant::Compact, Variant::Horizontal] {
        let l = rig.idle(|ui| fader::show(ui, egui::Id::new("f"), variant, 220.0, 0.0, false)).layout;
        assert!(l.ticks.is_empty() && l.tick_inks.is_empty(), "{variant:?}");
    }
}
