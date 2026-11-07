//! The controls and the Detector section: their layout from the board, every knob driving its
//! parameter in one undo step per gesture, the mode's segmented control, the sidechain key's
//! routing (unchanged), the SC HPF toggle, and the readouts' formats.

use super::sections::{DYNAMICS, KEY_H, KEY_W, MODES, SLOT as KNOB_SLOT, SLOT_GAP};
use super::test_rig::{SLOT, T, info, mid};
use transit_model::{KeyFrom, Tap};

#[test]
fn the_controls_panel_lays_out_the_boards_row() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    let controls = p.section("controls").unwrap();
    // Seven knobs in the board's order, 84 + 8 apart, on one line.
    let order: Vec<&str> = p.knobs.iter().map(|k| k.param.as_str()).collect();
    assert_eq!(order, DYNAMICS.map(|d| d.0));
    for w in p.knobs.windows(2) {
        assert_eq!(w[1].knob.dial[0] - w[0].knob.dial[0], KNOB_SLOT + SLOT_GAP);
        assert_eq!(w[1].knob.dial[1], w[0].knob.dial[1]);
    }
    assert_eq!(p.knobs[0].knob.size, crate::widgets::knob::Size::M);
    // The divider after the knobs, then the sidechain column: the label left of its dropdown, the
    // dropdown 110 by 24 at the right, the mode spread under it.
    let last = p.knobs.last().unwrap().knob.rect;
    assert!(p.divider[0] > last[2] && p.divider[2] - p.divider[0] == 1.0);
    let k = &p.key;
    assert_eq!((k.dropdown[2] - k.dropdown[0], k.dropdown[3] - k.dropdown[1]), (KEY_W, KEY_H));
    assert!(k.label[2] < k.dropdown[0] && k.label[0] > p.divider[2]);
    assert_eq!(k.dropdown[2], controls[2] - crate::tokens::layout::PANEL_PADDING[1], "right-aligned in the panel");
    assert_eq!(t.painted_all("Sidechain").len(), 1);
    let mode = p.mode.as_ref().unwrap();
    assert_eq!(p.mode_options, MODES.map(String::from));
    assert_eq!(mode.segments.len(), 2);
    assert_eq!(mode.track[0], k.label[0]);
    assert_eq!(mode.track[2], k.dropdown[2]);
    assert!(mode.track[1] > k.dropdown[3]);
    for m in MODES {
        assert_eq!(t.painted_all(m).len(), 1, "{m}");
    }
    // The sections the record names.
    for s in ["dynamics", "sidechain", "mode"] {
        let r = p.section(s).unwrap_or_else(|| panic!("{s}"));
        assert!(r[0] >= controls[0] && r[2] <= controls[2] && r[1] >= controls[1] && r[3] <= controls[3], "{s} {r:?}");
    }
    // Negative control: the RMS segment the board shows is not drawn (no such detector).
    assert_eq!(t.painted_all("RMS").len(), 0);
}

#[test]
fn the_readouts_print_in_the_languages_formats() {
    let (mut t, tr) = T::open();
    let value = |t: &T, id: &str| t.row(tr, id).value;
    assert_eq!(value(&t, "threshold"), "-20.0 dB");
    assert_eq!(value(&t, "ratio"), "4.0:1");
    assert_eq!(value(&t, "attack"), "10.0 ms");
    assert_eq!(value(&t, "release"), "100 ms");
    assert_eq!(value(&t, "mix"), "100%");
    t.set(tr, "attack", 0.01);
    t.set(tr, "release", 1500.0);
    assert_eq!(value(&t, "attack"), "0.01 ms", "the attack's floor");
    assert_eq!(value(&t, "release"), "1.50 s");
    assert!(t.painted_all("0.01 ms").len() == 1, "printed under the knob");
}

#[test]
fn every_knob_drives_its_parameter_as_one_undo_step_per_gesture() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    for id in DYNAMICS.map(|d| d.0).into_iter().chain(["lookahead", "sc_hpf_freq"]) {
        let p = info().param(id).unwrap();
        let before = t.undo_len();
        let v0 = t.param(tr, id);
        let dial = t.panel(tr).knob(id).unwrap().knob.dial;
        let from = mid(dial);
        // Up raises; a parameter at its top (Mix) goes down instead.
        let dy = if v0 >= p.max { 40.0 } else { -40.0 };
        t.drag(from, (from.0, from.1 + dy), false);
        let v1 = t.param(tr, id);
        assert!(if dy < 0.0 { v1 > v0 } else { v1 < v0 }, "{id}: {v0} -> {v1}");
        assert_eq!(t.undo_len(), before + 1, "{id}: one undo step for the whole drag");
        // A double click resets it.
        t.click(mid(t.panel(tr).knob(id).unwrap().knob.dial), 2);
        assert_eq!(t.param(tr, id), p.default, "{id}: reset");
    }
    // Negative control: the same drag on the panel between two knobs changes nothing.
    let n = t.undo_len();
    let (a, b) = (t.panel(tr).knob("threshold").unwrap().knob.rect, t.panel(tr).knob("ratio").unwrap().knob.rect);
    let gap = ((a[2] + b[0]) / 2.0, (a[1] + a[3]) / 2.0);
    t.drag(gap, (gap.0, gap.1 - 40.0), false);
    assert_eq!(t.undo_len(), n);
    assert_eq!(t.param(tr, "threshold"), info().param("threshold").unwrap().default);
}

#[test]
fn the_segmented_control_switches_between_peak_and_limit() {
    let (mut t, tr) = T::open();
    let segs = t.panel(tr).mode.unwrap().segments;
    assert_eq!(t.panel(tr).mode.unwrap().selected, 0, "Peak at rest");
    let before = t.undo_len();
    t.click(mid(segs[1]), 1);
    assert_eq!(t.param(tr, "limiter"), 1.0);
    assert_eq!(t.undo_len(), before + 1);
    assert_eq!(t.panel(tr).mode.unwrap().selected, 1);
    // Negative control: a click on the selected segment writes nothing.
    t.click(mid(segs[1]), 1);
    assert_eq!(t.undo_len(), before + 1);
    t.click(mid(segs[0]), 1);
    assert_eq!(t.param(tr, "limiter"), 0.0);
    // The row's control is the Limit segment (the older panel's toggle click turns Limit on).
    assert_eq!(t.row(tr, "limiter").control, segs[1]);
}

#[test]
fn the_sc_hpf_toggle_flips_its_parameter() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    let tg = t.panel(tr).hpf_toggle.unwrap();
    assert!(!tg.on);
    assert_eq!(tg.text, "SC HPF");
    // It sits on the dials' line, in the slot between Lookahead and HPF Freq.
    let (la, hf) = (t.panel(tr).knob("lookahead").unwrap().knob.dial, t.panel(tr).knob("sc_hpf_freq").unwrap().knob.dial);
    assert!(tg.rect[0] > la[2] && tg.rect[2] < hf[0]);
    assert!((mid(tg.rect).1 - mid(la).1).abs() < 0.5);
    let before = t.undo_len();
    t.click(mid(tg.rect), 1);
    assert_eq!(t.param(tr, "sc_hpf_on"), 1.0);
    assert_eq!(t.undo_len(), before + 1);
    assert!(t.panel(tr).hpf_toggle.unwrap().on);
    assert_eq!(t.panel(tr).hpf_toggle.unwrap().fill, crate::theme::TOKENS.toggle_option_fill, "the option role when on");
    // Negative control: the click beside it (still in the slot) does nothing.
    let r = t.panel(tr).hpf_toggle.unwrap().rect;
    t.click((r[0] - 6.0, (r[1] + r[3]) / 2.0), 1);
    assert_eq!(t.param(tr, "sc_hpf_on"), 1.0);
}

#[test]
fn the_sidechain_key_routes_as_before() {
    let mut t = T::new();
    let kick = t.compressor_track();
    let tr = t.compressor_track();
    let kick_name = t.app.engine.session().track(kick).unwrap().name.clone();
    let own_name = t.app.engine.session().track(tr).unwrap().name.clone();
    t.open_card(0);
    assert_eq!(t.panel(tr).key.text, "None");
    assert!(t.panel(tr).key.list.is_empty(), "closed");
    let dd = t.panel(tr).key.dropdown;
    let pick = |t: &mut T, row: &str| {
        t.click(mid(dd), 1);
        let list = t.panel(tr).key.list;
        let r = list.iter().find(|r| r.0 == row).unwrap_or_else(|| panic!("{row} in {list:?}")).1;
        t.click(mid(r), 1);
    };
    // The list: None, the taps, the other tracks; never this track.
    t.click(mid(dd), 1);
    let names: Vec<String> = t.panel(tr).key.list.iter().map(|r| r.0.clone()).collect();
    assert_eq!(names, ["None", "Post-fader", "Pre-fader", kick_name.as_str()]);
    assert!(!names.contains(&own_name), "negative control: this track is not a source");
    t.key("Escape");
    let before = t.undo_len();
    pick(&mut t, &kick_name);
    let key = t.app.engine.session().track(tr).unwrap().mixer.inserts[SLOT].as_ref().unwrap().key.unwrap();
    assert_eq!(key.from, KeyFrom::Track(kick));
    assert_eq!(key.tap, Tap::Post);
    assert_eq!(t.undo_len(), before + 1, "one mix.insert_key");
    assert_eq!(t.panel(tr).key.text, format!("{kick_name} · post"));
    // Pre-fader keeps the source at the other tap.
    pick(&mut t, "Pre-fader");
    let key = t.app.engine.session().track(tr).unwrap().mixer.inserts[SLOT].as_ref().unwrap().key.unwrap();
    assert_eq!((key.from, key.tap), (KeyFrom::Track(kick), Tap::Pre));
    assert_eq!(t.panel(tr).key.text, format!("{kick_name} · pre"));
    // None clears it.
    pick(&mut t, "None");
    assert!(t.app.engine.session().track(tr).unwrap().mixer.inserts[SLOT].as_ref().unwrap().key.is_none());
    assert_eq!(t.panel(tr).key.text, "None");
}
