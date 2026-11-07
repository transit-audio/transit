//! The history well (README, Compressor displays; job 326-U): the ring per instance, the traces
//! drawn from it, the threshold line, the time labels and the legend. Each assertion has its
//! control.

use super::hero;
use super::history::{HISTORY_CAP, History, Sample};
use super::test_rig::T;
use super::transfer_tests::{open_with_tap, rect};
use crate::theme::TOKENS;
use egui::Rect;

fn inside(r: [f32; 4], outer: Rect) -> bool {
    r[0] >= outer.min.x - 0.01 && r[2] <= outer.max.x + 0.01 && r[1] >= outer.min.y - 0.01 && r[3] <= outer.max.y + 0.01
}

/// A 4 s history at 60 frames a second ending now: input −12 dB, output −18 dB, gain reduction
/// 6 dB, with a 12 dB reduction in the last second.
fn synthetic(now: f64) -> History {
    let mut h = History::default();
    for i in 0..240 {
        let time = now - 4.0 + f64::from(i) / 60.0;
        h.push(Sample { time, input_db: -12.0, output_db: -18.0, gr_db: if i >= 180 { 12.0 } else { 6.0 } });
    }
    h
}

#[test]
fn a_synthetic_history_draws_its_input_area_output_line_and_gr_band_within_the_well() {
    let (mut t, tr) = T::open();
    let uid = t.window(tr).uid;
    // The next frame's clock: the test rig steps 1/60 s.
    t.app.builtin_editor.devices.history.insert(uid, synthetic(t.time + 1.0 / 60.0));
    t.frames(1);
    let p = t.panel(tr);
    let well = p.well("history").unwrap();
    let plot = rect(well.well.plot);
    let h = &p.hero.history;
    assert!(h.samples >= 239, "the window holds the 4 s: {}", h.samples);
    let (input, output, gr) = (h.input_area.unwrap(), h.output_line.unwrap(), h.gr_band.unwrap());
    for r in [input, output, gr] {
        assert!(inside(r, plot), "{r:?} inside {plot:?}");
    }
    let y = |db: f32| plot.max.y - hero::scale(db) * plot.height();
    // The input: an area from the bottom up to −12 dB, across the window.
    assert!((input[1] - y(-12.0)).abs() < 0.01 && (input[3] - plot.max.y).abs() < 0.01);
    assert!(input[2] - input[0] > plot.width() - 6.0, "{input:?}");
    // The output: a line at −18 dB.
    assert!((output[1] - y(-18.0)).abs() < 0.01 && (output[3] - y(-18.0)).abs() < 0.01);
    // The gain reduction hangs from the top on its 0 to 24 dB scale: its deepest reading, 12 dB,
    // reaches half way down.
    assert!((gr[1] - plot.min.y).abs() < 0.01);
    assert!((gr[3] - (plot.min.y + 0.5 * plot.height())).abs() < 0.01, "{gr:?}");
    // Control: on a scale rising from the bottom the 6 dB readings would reach lower.
    assert!((gr[3] - (plot.max.y - 0.25 * plot.height())).abs() > 10.0);
    // The tokens.
    assert_eq!(h.input_fill, TOKENS.ink_ghost.gamma_multiply(0.35));
    assert_ne!(h.input_fill, TOKENS.ink_ghost, "control: the area is not opaque");
    assert_eq!((h.output_stroke.width, h.output_stroke.color), (1.75, TOKENS.signal));
    assert_eq!(h.gr_fill, TOKENS.modulation.gamma_multiply(0.22));
    assert_eq!((h.gr_stroke.width, h.gr_stroke.color), (1.75, TOKENS.modulation));
    // The grid: the quarters across and a line each second.
    assert_eq!(well.grid.len(), 6);
    let verticals: Vec<f32> = well.grid.iter().filter(|l| l[0] == l[2]).map(|l| l[0]).collect();
    let want: Vec<f32> = (1..4).map(|k| plot.min.x + k as f32 / 4.0 * plot.width()).collect();
    assert_eq!(verticals, want);
}

#[test]
fn the_threshold_line_moves_with_the_parameter() {
    let (mut t, tr) = T::open();
    let plot = rect(t.panel(tr).well("history").unwrap().well.plot);
    let y = |db: f32| plot.max.y - hero::scale(db) * plot.height();
    let h = t.panel(tr).hero.history;
    assert_eq!(h.threshold_db, -20.0);
    assert!((h.threshold[1] - y(-20.0)).abs() < 0.01 && h.threshold[1] == h.threshold[3]);
    assert_eq!((h.threshold[0], h.threshold[2]), (plot.min.x, plot.max.x));
    assert_eq!((h.threshold_stroke.width, h.threshold_stroke.color, h.threshold_dash), (1.0, TOKENS.text.gamma_multiply(0.45), [4.0, 4.0]));
    t.set(tr, "threshold", -36.0);
    let moved = t.panel(tr).hero.history.threshold;
    assert!((moved[1] - y(-36.0)).abs() < 0.01, "{moved:?}");
    // Control: it left the old level.
    assert!((moved[1] - y(-20.0)).abs() > 10.0);
}

#[test]
fn the_time_labels_and_the_legend_sit_where_the_design_puts_them() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    let well = rect(p.well("history").unwrap().well.well);
    let h = &p.hero.history;
    let labels: Vec<&str> = h.time_labels.iter().map(|l| l.0.as_str()).collect();
    assert_eq!(labels, ["\u{2212}4 s", "now"]);
    let (old, new) = (h.time_labels[0].1, h.time_labels[1].1);
    assert!(inside(old, well) && inside(new, well));
    assert!(old[0] < well.center().x && new[2] > well.center().x, "bottom left and bottom right");
    assert!(old[3] > well.center().y && new[3] > well.center().y);
    assert!(!t.painted_all("now").is_empty() && !t.painted_all("\u{2212}4 s").is_empty());
    // The legend: IN, OUT, GR, right of the HISTORY label, ending at the column's right edge.
    let order: Vec<&str> = h.legend.iter().map(|l| l.label.as_str()).collect();
    assert_eq!(order, ["IN", "OUT", "GR"]);
    let label = p.hero.labels.iter().find(|l| l.0 == "HISTORY").unwrap().1;
    assert!(h.legend[0].swatch[0] > label[2], "right of the label");
    assert!((h.legend[2].text[2] - well.max.x).abs() < 0.01, "ends at the well's right edge");
    let sizes: Vec<[f32; 2]> = h.legend.iter().map(|l| [l.swatch[2] - l.swatch[0], l.swatch[3] - l.swatch[1]]).collect();
    assert_eq!(sizes, [[8.0, 8.0], [10.0, 2.0], [8.0, 8.0]]);
    let colors: Vec<egui::Color32> = h.legend.iter().map(|l| l.color).collect();
    assert_eq!(colors, [TOKENS.ink_ghost, TOKENS.signal, TOKENS.modulation]);
    // Control: the swatches are not all one color.
    assert_ne!(colors[0], colors[2]);
    for l in &h.legend {
        assert!(l.text[1] < well.min.y, "in the label row, above the well");
    }
}

#[test]
fn the_ring_records_each_frame_while_the_tap_exists_and_stays_bounded() {
    let (mut t, tr, tap) = open_with_tap();
    let uid = t.window(tr).uid;
    let before = t.app.builtin_editor.devices.history.get(&uid).map_or(0, History::len);
    tap.push_detector(Some(-9.0));
    t.frames(10);
    let h = t.app.builtin_editor.devices.history.get(&uid).expect("a ring for the open instance");
    assert_eq!(h.len(), before + 10, "one sample a frame");
    assert_eq!(h.iter().last().unwrap().input_db, -9.0, "the published detector level");
    // Without a tap nothing is recorded (control).
    let (mut t2, tr2) = T::open();
    let uid2 = t2.window(tr2).uid;
    t2.frames(10);
    assert!(t2.app.builtin_editor.devices.history.get(&uid2).is_none_or(History::is_empty));
    // Bounded: a full ring drops its oldest; a second pass of the same frame is ignored.
    let mut ring = History::default();
    for i in 0..HISTORY_CAP + 100 {
        ring.push(Sample { time: i as f64, input_db: 0.0, output_db: 0.0, gr_db: 0.0 });
    }
    assert_eq!(ring.len(), HISTORY_CAP);
    assert_eq!(ring.iter().next().unwrap().time, 100.0, "oldest first");
    assert_eq!(ring.iter().last().unwrap().time, (HISTORY_CAP + 99) as f64);
    ring.push(Sample { time: (HISTORY_CAP + 99) as f64, input_db: 1.0, output_db: 0.0, gr_db: 0.0 });
    assert_eq!(ring.iter().last().unwrap().input_db, 0.0);
    // Closing the window drops its ring.
    t.open_card(0);
    t.frames(2);
    assert!(!t.app.builtin_editor.devices.history.contains_key(&uid));
}

#[test]
fn the_windows_content_height_includes_the_history_well() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    let well = p.well("history").unwrap().well.well;
    let hero_r = p.section("hero").unwrap();
    assert!(well[3] <= hero_r[3] && hero_r[3] <= p.footer[1], "the well sits in the hero, above the footer");
    // Control: a window one well shorter would cut the well off at the footer.
    assert!(well[3] > p.footer[1] - hero::WELL);
}

#[test]
fn the_legend_and_the_well_carry_the_measurement_convention_as_help() {
    let (mut t, tr) = T::open();
    let p = t.panel(tr);
    let h = &p.hero.history;
    // The text states the convention in plain words.
    for want in ["absolute levels in dBFS at the detector", "OUT includes the makeup gain", "GR is the gain reduction", "from the top"] {
        assert!(h.help_text.contains(want), "{want:?} missing from {:?}", h.help_text);
    }
    // Control: the legend's labels alone do not carry it.
    assert!(!h.legend.iter().all(|l| l.label.contains("dBFS")));
    // Hovering the legend or the well shows that entry in the Info panel.
    t.app.help.open = true;
    t.frames(3);
    let well = rect(p.well("history").unwrap().well.well);
    let legend = h.legend_rect;
    assert!(legend[2] > legend[0] && legend[3] > legend[1], "the legend has bounds: {legend:?}");
    for (name, at) in [("legend", super::test_rig::mid(legend)), ("well", (well.center().x, well.center().y))] {
        t.send("ui.move", serde_json::json!({"x": at.0, "y": at.1}));
        assert_eq!(t.app.help.layout.id.as_deref(), Some("compressor.history"), "hovering the {name}");
        assert_eq!(t.app.help.layout.text, h.help_text);
    }
}
