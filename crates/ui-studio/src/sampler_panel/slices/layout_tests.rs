//! The Slices view's lower panels stay inside their cards (#472): at the minimum and the design
//! window widths, at 1.0 and 1.5 pixels per point, in every state that changes what the three
//! cards draw (Detect with each source, a candidate and its replacement; Manual with one marker,
//! with four, after a refused click, with the Notice; Use slices stale, refused, created and
//! overlapping), nothing painted by a card leaves the card's padding.
//!
//! The check reads what was actually painted: every shape of the last frame (text, rectangle,
//! line, path) whose top left lies inside a card must lie inside that card shrunk by the panel
//! padding, so a new row or label is checked without anyone adding it to a list. The panels'
//! own records (the Manual rows, the Use widgets) are checked on top, with names, where a named
//! failure is easier to read.

use super::super::test_rig::{T, mid};
use super::panels::PanelsLayout;
use super::rig::*;
use super::{bench, detect_panel};
use crate::tokens::layout;
use egui::epaint::Shape;
use serde_json::json;
use transit_model::TrackId;

type R = [f32; 4];

/// The panel padding: vertical, horizontal.
const PAD: (f32, f32) = (layout::PANEL_PADDING[0], layout::PANEL_PADDING[1]);

fn inside(r: R, card: R, slack: f32) -> bool {
    r[0] >= card[0] - slack && r[1] >= card[1] - slack && r[2] <= card[2] + slack && r[3] <= card[3] + slack
}

fn meets(a: R, b: R) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

fn pad(c: R) -> R {
    [c[0] + PAD.1, c[1] + PAD.0, c[2] - PAD.1, c[3] - PAD.0]
}

fn arr(r: egui::Rect) -> R {
    [r.min.x, r.min.y, r.max.x, r.max.y]
}

/// Every widget and text rectangle the Manual panel records.
fn manual_rects(p: &PanelsLayout) -> Vec<(&'static str, R)> {
    let Some(m) = p.manual.as_ref() else { return Vec::new() };
    let mut v = vec![("label", p.left_label), ("text", m.text), ("from auto", m.from_auto), ("clear", m.clear)];
    v.extend(m.gestures.iter().map(|g| ("gesture", *g)));
    v.extend(m.notice.iter().map(|n| ("notice", n.rect)));
    v.extend(p.left_error.iter().map(|e| ("error", e.rect)));
    v.extend(p.left_line.iter().map(|l| ("last edit", l.rect)));
    v
}

/// Every widget and text rectangle the Use slices panel records.
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

/// Every shape the last frame painted, flattened, with its bounding rectangle.
fn painted(t: &T) -> Vec<(String, R)> {
    fn walk(s: &Shape, out: &mut Vec<(String, R)>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
            Shape::Noop => {}
            s => {
                let b = s.visual_bounding_rect();
                if b.is_finite() && b.width() > 0.0 && b.height() > 0.0 {
                    out.push((format!("{s:?}").chars().take(70).collect(), arr(b)));
                }
            }
        }
    }
    let mut out = Vec::new();
    t.shapes.iter().for_each(|c| walk(&c.shape, &mut out));
    out
}

/// Checks the three cards in the state the test is in.
fn verify(t: &T, tr: TrackId, at: &str) {
    let win = t.panel(tr).window;
    let p = panels(t, tr);
    assert!(!meets(p.left, p.right), "the cards overlap: {at}");
    for (name, r) in manual_rects(&p) {
        assert!(inside(r, pad(p.left), 0.01), "Manual {name} {r:?} breaks the panel padding of {:?}: {at}", p.left);
        assert!(!meets(r, p.right), "Manual {name} {r:?} reaches the Use slices card: {at}");
    }
    for (name, r) in use_rects(&p) {
        assert!(inside(r, pad(p.right), 0.01), "Use slices {name} {r:?} breaks the panel padding of {:?}: {at}", p.right);
        assert!(inside(r, win, 0.01), "Use slices {name} {r:?} leaves the window: {at}");
        assert!(!meets(r, p.left), "Use slices {name} {r:?} reaches the Manual card: {at}");
    }
    // What was painted, in both cards (Detect included).
    let mut checked = 0;
    for (shape, r) in painted(t) {
        for (card, name) in [(p.left, "left"), (p.right, "right")] {
            let starts_in = r[0] >= card[0] - 1.0 && r[0] < card[2] && r[1] >= card[1] - 1.0 && r[1] < card[3];
            let is_card = r[0] <= card[0] + 1.0 && r[1] <= card[1] + 1.0 && r[2] >= card[2] - 1.0 && r[3] >= card[3] - 1.0;
            if starts_in && !is_card {
                checked += 1;
                assert!(inside(r, pad(card), 1.0), "painted {shape} {r:?} leaves the {name} card's padding {:?}: {at}", pad(card));
            }
        }
    }
    assert!(checked > 20, "the traversal saw {checked} shapes in the cards: {at}");
    // The Playback panel's loop row under the cards.
    let play = t.panel(tr).panel("playback").expect("the Playback panel");
    let row = &p.loop_row;
    for (name, r) in [("track", row.track), ("crossfade", row.crossfade)].into_iter().chain(row.warning.iter().map(|w| ("warning", w.1))) {
        assert!(inside(r, pad(play), 0.01), "Playback loop row {name} {r:?} breaks the panel padding in {play:?}: {at}");
    }
}

fn click_source(t: &mut T, tr: TrackId, i: usize) {
    let d = panels(t, tr).detect.expect("Detect");
    t.click(mid(d.source.as_ref().unwrap().segments[i]), 1);
    t.frames(4);
}

/// A candidate from the second Grid division, waited for.
fn candidate(t: &mut T, tr: TrackId) {
    let d = panels(t, tr).detect.expect("Detect");
    t.click(mid(d.division.as_ref().unwrap().segments[1]), 1);
    t.frames((detect_panel::DEBOUNCE * 60.0).ceil() as usize + 2);
    wait_candidate(t, tr);
    t.frames(8);
}

fn manual(t: &mut T, tr: TrackId) {
    t.run("mix.sampler_slice_mode", json!({"track": tr.0, "mode": "manual"}));
    t.frames(4);
}

/// A state: a name and what builds it from the fixture the test opened (the window already at the
/// width and the scale under test).
type State = (&'static str, fn(&mut T, TrackId));

const AUTO_STATES: [State; 5] = [
    ("auto grid", |_, _| {}),
    ("auto cue points", |t, tr| click_source(t, tr, 0)),
    ("auto transients", |t, tr| click_source(t, tr, 1)),
    ("auto candidate", candidate),
    ("auto replaced", |t, tr| {
        candidate(t, tr);
        let d = panels(t, tr).detect.expect("Detect");
        t.click(mid(d.replace.expect("Replace markers")), 1);
        t.frames(4);
    }),
];

const MANUAL_STATES: [State; 4] = [
    ("manual one marker (refusal beside the preview)", manual),
    ("manual refused click", |t, tr| {
        manual(t, tr);
        let u = panels(t, tr).uses;
        t.click(mid(u.to_pads), 1);
        t.frames(4);
    }),
    ("manual notice after an Auto gesture", |t, tr| {
        let b = bench(t, tr);
        t.click(at_frame(&b, 10_000), 2);
        t.frames(4);
        assert!(panels(t, tr).manual.and_then(|m| m.notice).is_some(), "the Notice shows");
    }),
    ("manual from auto", |t, tr| {
        manual(t, tr);
        let m = panels(t, tr).manual.unwrap();
        t.click(mid(m.from_auto), 1);
        t.frames(4);
    }),
];

const SLICED_STATES: [State; 4] = [
    ("sliced manual", |_, _| {}),
    ("created clip", |t, tr| {
        let u = panels(t, tr).uses;
        t.click(mid(u.create), 1);
        t.frames(4);
    }),
    ("created clip twice (overlap)", |t, tr| {
        for _ in 0..2 {
            let u = panels(t, tr).uses;
            t.click(mid(u.create), 1);
            t.frames(4);
        }
    }),
    ("sliced keep duration", |t, tr| {
        let timing = panels(t, tr).uses.timing.unwrap();
        t.click(mid(timing.segments[1]), 1);
        t.frames(4);
    }),
];

fn resize(t: &mut T, width: f32, ppp: f32) {
    t.ctx.set_pixels_per_point(ppp);
    t.screen = egui::vec2(width + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    t.frames(4);
}

#[test]
fn the_lower_panels_stay_inside_their_cards() {
    for width in [super::super::MIN_WIDTH, super::super::DESIGN_WIDTH] {
        for ppp in [1.0, 1.5] {
            let at = |state: &str| format!("{state}, {width} wide at {ppp} px/pt");
            for (name, build) in AUTO_STATES.iter().chain(MANUAL_STATES.iter()) {
                let (mut t, tr) = auto();
                resize(&mut t, width, ppp);
                build(&mut t, tr);
                verify(&t, tr, &at(name));
            }
            for (name, build) in SLICED_STATES {
                let (mut t, tr) = sliced();
                resize(&mut t, width, ppp);
                build(&mut t, tr);
                verify(&t, tr, &at(name));
            }
            let (mut t, tr) = super::panel_tests::stale();
            resize(&mut t, width, ppp);
            verify(&t, tr, &at("stale mapping"));
            assert!(panels(&t, tr).uses.map.is_some(), "the stale state offers Map slices to keys");
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
    assert!(bench::HOVER_ADD.contains("double-click adds a marker"));
}
