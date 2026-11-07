//! The Modulation section: source chips dropped on knobs make routes, routing rows show only the
//! routes in use and remove clears them, Add route reveals a row, the amount bar drags.

use super::modulation::{columns, group_of};
use super::test_rig::{T, mid};
use serde_json::json;
use transit_dsp::plugins::macro_osc::matrix::{DESTINATIONS, ROWS, SOURCES};

#[test]
fn a_chip_dropped_on_a_knob_makes_a_route_and_the_knob_shows_its_ring() {
    let (mut t, tr) = T::open();
    // Only the Modulation bar opens: the chips sit right under the hero's knobs.
    let b = t.in_view(tr, |p| p.modulation_bar.as_ref().unwrap().button);
    t.click(mid(b), 1);
    assert!(t.panel(tr).knob("timbre").unwrap().knob.mod_deg.is_none());
    let chip = t.in_view(tr, |p| p.chips.iter().find(|c| SOURCES[c.source] == "LFO 1").unwrap().rect);
    let knob = t.panel(tr).knob("timbre").unwrap().knob.dial;
    assert!(knob[1] > t.panel(tr).window[1], "the Timbre knob is on screen above the chips");
    let before = t.undo_len();
    t.drag(mid(chip), mid(knob), false);
    assert_eq!((t.param(tr, "mod1_source"), t.param(tr, "mod1_dest")), (1.0, 3.0), "row 1: LFO 1 to Timbre");
    assert_eq!(t.undo_len(), before + 1, "one undo step");
    // Move off the knob, so it is not drawn as held or hovered.
    let w = t.panel(tr).window;
    t.send("ui.move", json!({"x": w[0] + 6.0, "y": (w[1] + w[3]) / 2.0}));
    let p = t.panel(tr);
    let k = &p.knob("timbre").unwrap().knob;
    assert!(k.mod_deg.is_some() && k.state == crate::widgets::knob::State::Modulated, "the knob shows the ring");
    assert_eq!(p.routes.iter().map(|r| r.row).collect::<Vec<_>>(), [0], "the route's row shows");
    let c = p.chips.iter().find(|c| c.source == 1).unwrap();
    assert!(c.routed && c.badge == "×1", "the chip counts its route");
    // Negative control: a knob without a destination (Aux mix) takes no route.
    let chip = t.in_view(tr, |p| p.chips.iter().find(|c| c.source == 2).unwrap().rect);
    let aux = t.panel(tr).knob("aux_mix").unwrap().knob.dial;
    t.drag(mid(chip), mid(aux), false);
    assert_eq!(t.param(tr, "mod2_source"), 0.0);
}

#[test]
fn only_the_routes_in_use_show_and_add_route_reveals_the_next_row() {
    let (mut t, tr) = T::open();
    t.set(tr, "mod5_source", 10.0);
    t.set(tr, "mod5_dest", 7.0);
    t.frames(2);
    let p = t.panel(tr);
    assert!(p.modulation_body.is_some(), "a route in use opens the section");
    assert_eq!(p.routes.iter().map(|r| r.row).collect::<Vec<_>>(), [4]);
    let r = &p.routes[0];
    assert_eq!(r.texts[0..4], ["Filter".to_string(), "Cutoff".to_string(), "None".to_string(), "Linear".to_string()]);
    assert_eq!(r.texts[4], "+0%");
    let bar = p.modulation_bar.clone().unwrap();
    assert_eq!(bar.middle.unwrap().0, "1 / 16 routes");
    assert!(bar.switch.is_none(), "the matrix has no enable parameter: no switch");
    let add = t.in_view(tr, |p| p.add_route.unwrap());
    t.click(mid(add), 1);
    assert_eq!(t.panel(tr).routes.iter().map(|r| r.row).collect::<Vec<_>>(), [0, 4], "Add route reveals row 1");
    assert_eq!(t.param(tr, "mod1_source"), 0.0, "revealing writes nothing");
    // Rows sit one under another, 40 high, fill bg.titlebar.
    let rs = t.panel(tr).routes;
    assert_eq!(rs[0].rect[3] - rs[0].rect[1], 40.0);
    assert!(rs[1].rect[1] >= rs[0].rect[3]);
}

#[test]
fn remove_clears_the_row_in_one_undo_step() {
    let (mut t, tr) = T::open();
    for (f, v) in [("source", 1.0), ("via", 5.0), ("dest", 4.0), ("amount", -40.0), ("curve", 2.0)] {
        t.set(tr, &format!("mod3_{f}"), v);
    }
    t.set(tr, "mod4_source", 2.0);
    let rm = t.in_view(tr, |p| p.routes.iter().find(|r| r.row == 2).unwrap().remove);
    assert_eq!(t.panel(tr).routes.iter().find(|r| r.row == 2).unwrap().texts[4], "-40%");
    let before = t.undo_len();
    t.click(mid(rm), 1);
    for f in ["source", "via", "dest", "amount", "curve"] {
        assert_eq!(t.param(tr, &format!("mod3_{f}")), 0.0, "{f}");
    }
    assert_eq!(t.undo_len(), before + 1);
    assert_eq!(t.panel(tr).routes.iter().map(|r| r.row).collect::<Vec<_>>(), [3], "the row is gone");
    // Negative control: the other route stays.
    assert_eq!(t.param(tr, "mod4_source"), 2.0);
}

#[test]
fn the_amount_bar_drags_and_double_click_zeroes_it() {
    let (mut t, tr) = T::open();
    t.set(tr, "mod1_source", 1.0);
    t.set(tr, "mod1_dest", 2.0);
    let bar = t.in_view(tr, |p| p.routes[0].amount.rect);
    let w = bar[2] - bar[0];
    t.drag(mid(bar), (mid(bar).0 + w / 4.0, mid(bar).1), false);
    assert!((t.param(tr, "mod1_amount") - 50.0).abs() < 1.5, "a quarter of the width is +50%: {}", t.param(tr, "mod1_amount"));
    assert!(t.panel(tr).routes[0].amount.fill[2] > t.panel(tr).routes[0].amount.tick[2]);
    // The double click lands within 0.6 s of the click that opened the editor, so egui counts its
    // second click as a triple; the kit resets on either (`widgets::reset_click`, 326-X).
    let bar = t.in_view(tr, |p| p.routes[0].amount.rect);
    t.click(mid(bar), 2);
    assert_eq!(t.param(tr, "mod1_amount"), 0.0);
}

#[test]
fn destinations_carry_their_group_and_the_columns_fill_the_row() {
    assert_eq!(group_of(3), ("Tone", "Timbre"));
    assert_eq!(group_of(7), ("Filter", "Cutoff"));
    assert_eq!(group_of(1), ("Pitch", "Pitch"));
    assert_eq!(group_of(10), ("LFO 1", "rate"));
    assert_eq!(group_of(0), ("", "Off"));
    for (d, name) in DESTINATIONS.iter().enumerate().skip(1) {
        assert!(!group_of(d).0.is_empty(), "{name}");
    }
    for width in [1376.0, 1000.0] {
        let c = columns(width);
        assert!((c.iter().sum::<f32>() + 7.0 * 8.0 + 16.0 - width).abs() < 0.01, "{width}");
        assert!(c[3] >= 159.9, "the destination column keeps its minimum");
    }
    let wide = columns(1376.0);
    assert_eq!(wide[1], 200.0, "the board's widths at the board's size");
    assert!(columns(1000.0)[1] < 200.0, "narrower: the fixed columns shrink");
    assert_eq!(ROWS, 16);
}

#[test]
fn the_collapsed_bar_names_the_first_routes() {
    let (mut t, tr) = T::open();
    for (row, s, d) in [(1, 1, 3), (2, 5, 7), (3, 4, 4), (4, 10, 2), (5, 2, 1)] {
        t.set(tr, &format!("mod{row}_source"), s as f32);
        t.set(tr, &format!("mod{row}_dest"), d as f32);
    }
    let b = t.in_view(tr, |p| p.modulation_bar.as_ref().unwrap().button);
    t.click(mid(b), 1);
    let text = t.panel(tr).modulation_bar.unwrap().middle.unwrap().0;
    assert_eq!(text, "5 / 16 routes   ·   LFO 1 → Timbre   ·   Velocity → Cutoff   ·   Filter envelope → Morph   ·   +2 more");
}
