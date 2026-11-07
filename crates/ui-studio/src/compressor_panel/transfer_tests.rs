//! The transfer well's displays (README, Compressor displays; job 326-U): the operating point on
//! the curve from the published detector level; and the compressor's color names as the theme
//! draws them. Each assertion has its control.

use super::test_rig::{SLOT, T};
use super::{hero, named};
use crate::theme::TOKENS;
use crate::tokens::compressor as tok;
use crate::widgets::display_well;
use egui::{Rect, pos2};
use std::sync::Arc;
use transit_mix::{ScopeHub, ScopeTap};
use transit_model::TrackId;

pub(super) fn rect(r: [f32; 4]) -> Rect {
    Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))
}

/// A Compressor track whose insert has a scope tap in a headless hub, its editor open.
pub(super) fn open_with_tap() -> (T, TrackId, Arc<ScopeTap>) {
    let mut t = T::new();
    let tr = t.compressor_track();
    let uid = t.app.engine.session().track(tr).unwrap().mixer.inserts[SLOT].as_ref().unwrap().uid.get();
    let hub = Arc::new(ScopeHub::default());
    let tap = Arc::new(ScopeTap::new());
    hub.register(tr, uid, &tap);
    t.app.builtin_editor.hub = Some(hub);
    t.open_card(0);
    assert!(tap.enabled(), "the open panel switched its tap on");
    (t, tr, tap)
}

#[test]
fn the_compressor_color_names_draw_as_their_tokens() {
    let dark: std::collections::HashMap<&str, egui::Color32> = crate::tokens::color::ALL.iter().copied().collect();
    let colors = tok::STRINGS.iter().filter(|(p, _)| !p.starts_with("history/timeLabels") && *p != "operatingPoint/source");
    for (path, name) in colors {
        assert_eq!(Some(&named(name)), dark.get(name), "{path}: {name}");
    }
    // Control: a name the design does not have is not a color it draws.
    assert!(!dark.values().any(|c| *c == named("no.such.token")));
}

#[test]
fn the_operating_point_sits_on_the_curve_at_the_published_level() {
    let (mut t, tr, tap) = open_with_tap();
    t.frames(2);
    assert!(t.panel(tr).hero.dot.is_none(), "hidden until a level is published since the editor opened");
    // Defaults: threshold −20, ratio 4, knee 6. At −20 dB in, the knee gives 0.5625 dB of
    // reduction: −20.5625 dB out.
    assert_eq!((t.param(tr, "threshold"), t.param(tr, "ratio")), (-20.0, 4.0));
    tap.push_detector(Some(-20.0));
    t.frames(1);
    let p = t.panel(tr);
    let dot = p.hero.dot.clone().expect("a published level shows the dot");
    let plot = rect(p.well("transfer").unwrap().well.plot);
    let want = display_well::to_screen(plot, [hero::scale(-20.0), hero::scale(-20.5625)]);
    assert!((dot.center[0] - want.x).abs() < 0.01 && (dot.center[1] - want.y).abs() < 0.01, "{:?} vs {want:?}", dot.center);
    assert!((dot.output_db - -20.5625).abs() < 1e-4, "on the curve: {}", dot.output_db);
    // Control: a wrong x mapping (the axis read right to left) lands far from it.
    let wrong_x = plot.max.x - hero::scale(-20.0) * plot.width();
    assert!((dot.center[0] - wrong_x).abs() > 10.0);
    // The token: an 8 px `text` dot with a 2 px `surface.inset` ring.
    assert_eq!((dot.radius * 2.0, dot.fill, dot.ring.width, dot.ring.color), (8.0, TOKENS.text, 2.0, TOKENS.surface_inset));
    assert_ne!(dot.fill, TOKENS.signal, "control: not the curve's color");
    // Peak fall-off: a drop to −40 dB falls over 300 ms instead of jumping.
    tap.push_detector(Some(-40.0));
    t.frames(1);
    let falling = t.panel(tr).hero.dot.unwrap().input_db;
    assert!(falling < -20.0 && falling > -40.0, "one frame in: {falling}");
    t.frames(30);
    assert!((t.panel(tr).hero.dot.unwrap().input_db - -40.0).abs() < 0.5, "settled");
    // A rise shows within the frame.
    tap.push_detector(Some(-10.0));
    t.frames(1);
    assert_eq!(t.panel(tr).hero.dot.unwrap().input_db, -10.0);
    // Below −60 dB it hides.
    tap.push_detector(Some(-80.0));
    t.frames(60);
    assert!(t.panel(tr).hero.dot.is_none(), "hidden below −60 dB");
}
