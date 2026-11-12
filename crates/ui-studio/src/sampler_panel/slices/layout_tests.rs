//! The Slices view's lower panels stay inside their cards (#472): at the minimum and the design
//! window widths, at 1.0 and 1.5 pixels per point, every widget and text rectangle of the Manual
//! panel lies inside the Manual card, every widget of the Use slices panel inside its card and the
//! window, and no rectangle of one panel touches the other.

use super::panels::PanelsLayout;
use super::rig::*;
use serde_json::json;

type R = [f32; 4];

/// The panel padding: vertical, horizontal.
const PAD: (f32, f32) = (crate::tokens::layout::PANEL_PADDING[0], crate::tokens::layout::PANEL_PADDING[1]);

fn inside(r: R, card: R, slack: f32) -> bool {
    r[0] >= card[0] - slack && r[1] >= card[1] - slack && r[2] <= card[2] + slack && r[3] <= card[3] + slack
}

fn meets(a: R, b: R) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

/// Every widget and text rectangle of the Manual panel.
fn manual_rects(p: &PanelsLayout) -> Vec<(&'static str, R)> {
    let m = p.manual.as_ref().expect("the Manual panel");
    let mut v = vec![("label", p.left_label), ("text", m.text), ("from auto", m.from_auto), ("clear", m.clear)];
    v.extend(m.gestures.iter().map(|g| ("gesture", *g)));
    v.extend(m.notice.iter().map(|n| ("notice", n.rect)));
    v.extend(p.left_error.iter().map(|e| ("error", e.rect)));
    v.extend(p.left_line.iter().map(|l| ("last edit", l.rect)));
    v
}

/// Every widget of the Use slices panel.
fn use_rects(p: &PanelsLayout) -> Vec<(&'static str, R)> {
    let u = &p.uses;
    let mut v = vec![("label", p.right_label), ("root", u.root), ("to pads", u.to_pads), ("beat", u.beat), ("create", u.create)];
    v.extend(u.texts.iter().map(|t| ("text", *t)));
    v.extend(u.cells.iter().map(|c| ("cell", c.0)));
    if let Some(t) = &u.timing {
        v.push(("timing", t.track));
        v.extend(t.segments.iter().map(|s| ("segment", *s)));
    }
    v.extend(u.overlap.map(|o| ("overlap", o)));
    v.extend(u.map.map(|m| ("map", m)));
    v.extend(p.use_error.iter().map(|e| ("error", e.rect)));
    v.extend(p.use_line.iter().map(|l| ("last edit", l.rect)));
    v
}

fn check(width: f32, ppp: f32, manual: bool) {
    let (mut t, tr) = auto();
    t.ctx.set_pixels_per_point(ppp);
    t.screen = egui::vec2(width + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    if manual {
        t.run("mix.sampler_slice_mode", json!({"track": tr.0, "mode": "manual"}));
    }
    t.frames(4);
    let win = t.panel(tr).window;
    let p = panels(&t, tr);
    let at = format!("{width} wide at {ppp} px/pt, manual {manual}, window {win:?}, left {:?}, right {:?}", p.left, p.right);
    let rects = if manual { manual_rects(&p) } else { Vec::new() };
    for (name, r) in rects.iter().copied() {
        assert!(inside(r, p.left, 0.01), "Manual {name} {r:?} leaves its card: {at}");
        assert!(!meets(r, p.right), "Manual {name} {r:?} reaches the Use slices card: {at}");
    }
    for (name, r) in use_rects(&p) {
        assert!(inside(r, p.right, 0.01), "Use slices {name} {r:?} leaves its card: {at}");
        assert!(inside(r, win, 0.01), "Use slices {name} {r:?} leaves the window: {at}");
        assert!(!meets(r, p.left), "Use slices {name} {r:?} reaches the Manual card: {at}");
    }
    assert!(!meets(p.left, p.right), "the cards overlap: {at}");
    // The panel padding (horizontal and vertical) holds for the text and widgets of every panel
    // of the row, the Playback panel's loop row included.
    let pad = |c: R| [c[0] + PAD.1, c[1] + PAD.0, c[2] - PAD.1, c[3] - PAD.0];
    for (name, r) in rects.iter().copied() {
        assert!(inside(r, pad(p.left), 0.01), "Manual {name} {r:?} breaks the panel padding: {at}");
    }
    for (name, r) in use_rects(&p) {
        assert!(inside(r, pad(p.right), 0.01), "Use slices {name} {r:?} breaks the panel padding: {at}");
    }
    let play = t.panel(tr).panel("playback").expect("the Playback panel");
    let row = &p.loop_row;
    for (name, r) in [("track", row.track), ("crossfade", row.crossfade)].into_iter().chain(row.warning.iter().map(|w| ("warning", w.1))) {
        assert!(inside(r, pad(play), 0.01), "Playback loop row {name} {r:?} breaks the panel padding in {play:?}: {at}");
    }
    if manual {
        assert_eq!(p.manual.as_ref().unwrap().gestures.len(), 4, "every gesture is still listed: {at}");
    }
}

#[test]
fn the_lower_panels_stay_inside_their_cards() {
    for width in [super::super::MIN_WIDTH, super::super::DESIGN_WIDTH] {
        for ppp in [1.0, 1.5] {
            for manual in [true, false] {
                check(width, ppp, manual);
            }
        }
    }
}

#[test]
fn the_double_click_hint_stays_findable() {
    let (mut t, tr) = auto();
    t.run("mix.sampler_slice_mode", json!({"track": tr.0, "mode": "manual"}));
    t.frames(3);
    assert!(panels(&t, tr).manual.is_some());
    assert!(super::panels::GESTURES.iter().any(|g| g.0 == "double-click" && g.1.contains("add")));
    assert!(super::panels::MANUAL_TEXT.contains("double-click it to add"));
    assert!(super::bench::HOVER_ADD.contains("double-click adds a marker"));
}
