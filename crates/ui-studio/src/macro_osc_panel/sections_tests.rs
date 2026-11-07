//! The module's sections (Tone, Gate · LPG, Pitch, Output): each knob reads its parameter and
//! prints its unit, the sizes follow the README, routes draw rings, and the Output panel's
//! dropdowns and voice lights.

use super::test_rig::{T, info, mid};
use crate::widgets::knob::Size;

/// (parameter, label, size) of every knob in the four sections, as the board places them.
const KNOBS: [(&str, &str, Size); 9] = [
    // The Tone labels follow the model (the default, Virtual analog; see the hero tests).
    ("harmonics", "Detune", Size::L),
    ("timbre", "Square shape", Size::L),
    ("morph", "Saw shape", Size::L),
    ("decay", "Decay", Size::M),
    ("lpg_color", "LPG color", Size::M),
    ("tune", "Tune", Size::M),
    ("bend_range", "Bend", Size::M),
    ("aux_mix", "Aux mix", Size::M),
    ("level", "Level", Size::M),
];

#[test]
fn each_section_knob_reads_its_parameter_and_prints_its_unit() {
    let (mut t, tr) = T::open();
    // Distinct values, so a knob wired to another parameter shows the wrong number.
    let values = [30.0, 40.0, 60.0, 20.0, 70.0, -5.0, 7.0, 25.0, -12.0];
    for ((id, ..), v) in KNOBS.iter().zip(values) {
        t.set(tr, id, v);
    }
    let p = t.panel(tr);
    for ((id, label, size), v) in KNOBS.iter().zip(values) {
        let k = p.knob(id).unwrap_or_else(|| panic!("a knob for {id}"));
        assert_eq!(k.label, *label, "{id}");
        assert_eq!(k.knob.size, *size, "{id}");
        let want = info().param(id).unwrap().format(v);
        assert_eq!(t.row(tr, id).value, want, "{id}: the row prints the value");
        let unit = match *id {
            "tune" | "bend_range" => " st",
            "level" => " dB",
            _ => "%",
        };
        assert!(want.ends_with(unit), "{id}: {want}");
    }
    // The Tone knobs are the hero's (L), above Gate · LPG and Pitch (M).
    let (h, d) = (p.knob("harmonics").unwrap().knob.dial, p.knob("decay").unwrap().knob.dial);
    assert!((h[2] - h[0] - crate::tokens::knob::L).abs() < 0.02, "{h:?}");
    assert!((d[2] - d[0] - crate::tokens::knob::M).abs() < 0.02, "{d:?}");
    assert!(h[3] < d[1], "Tone above Gate · LPG");
    // Tune spans zero: its arc grows from 12 o'clock.
    let tune = p.knob("tune").unwrap().knob.arc_deg;
    assert!(tune[0] < 0.0 && tune[1] <= 0.5, "bipolar arc from 12 o'clock toward -5 st: {tune:?}");
}

#[test]
fn turning_a_section_knob_writes_its_parameter_in_one_undo_step() {
    let (mut t, tr) = T::open();
    let dial = t.knob(tr, "decay");
    let before = t.undo_len();
    t.drag(mid(dial), (mid(dial).0, mid(dial).1 - 40.0), false);
    // 40 px of 200 is at most 20 percent (egui starts the drag once the pointer leaves the click
    // threshold, so the first few points do not turn it).
    let v = t.param(tr, "decay");
    assert!(v > 60.0 && v <= 70.0, "{v}");
    assert_eq!(t.undo_len(), before + 1);
    // Negative control: the knob beside it did not move.
    assert_eq!(t.param(tr, "lpg_color"), 50.0);
}

#[test]
fn a_routed_knob_shows_its_modulation_ring_from_the_base_value() {
    let (mut t, tr) = T::open();
    assert!(t.panel(tr).knob("harmonics").unwrap().knob.mod_deg.is_none(), "no route: no ring");
    t.set(tr, "mod2_source", 5.0);
    t.set(tr, "mod2_dest", 2.0);
    t.set(tr, "mod2_amount", 25.0);
    let p = t.panel(tr);
    let k = &p.knob("harmonics").unwrap().knob;
    let ring = k.mod_deg.expect("a ring");
    assert_eq!(k.state, crate::widgets::knob::State::Modulated);
    assert!((ring[0] - k.pointer_deg).abs() < 0.01 && (ring[1] - ring[0] - 0.25 * 270.0).abs() < 0.01, "{ring:?}");
    // Negative control: the knob of another destination keeps no ring.
    assert!(p.knob("timbre").unwrap().knob.mod_deg.is_none());
}

#[test]
fn the_output_panel_holds_the_dropdowns_and_the_voice_lights() {
    let (mut t, tr) = T::open();
    let p = t.panel(tr);
    let drops: Vec<(&str, &str)> = p.dropdowns.iter().map(|d| (d.0.as_str(), d.2.as_str())).collect();
    assert_eq!(drops, [("mod_wheel", "Timbre"), ("voices", "8")]);
    for d in &p.dropdowns {
        assert_eq!(d.1[2] - d.1[0], super::sections::DROPDOWN_W);
    }
    assert_eq!(p.lights.len(), 8, "one light per voice");
    let lr = p.lights_rect;
    assert!((lr[2] - lr[0] - (8.0 * super::sections::LIGHT + 7.0 * 5.0)).abs() < 0.01);
    let output = p.panels.iter().find(|x| x.0 == "output").unwrap().1;
    assert!(crate::widgets::test_support::inside(lr, output) && crate::widgets::test_support::inside(p.dropdowns[1].1, output));
    // Choosing from the Voices list writes the parameter.
    t.click(mid(p.dropdowns[1].1), 1);
    let at = t.listed(p.dropdowns[1].1, "4").expect("the list offers 4");
    t.click(at, 1);
    assert_eq!(t.param(tr, "voices"), 3.0);
    assert_eq!(t.panel(tr).lights.len(), 4);
}
