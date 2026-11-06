//! The Shaper: its section bar (enable is the parameter, collapse the per-user view setting), the
//! dim of a disabled section, the filter's segmented controls and response well, and the LFOs.

use super::shaper::{SHAPES, env_curve, freq_x, lfo_curve, response_db, section_key, x_freq};
use super::test_rig::{T, mid};
use crate::tokens::alpha;

#[test]
fn the_switch_toggles_the_shaper_parameter_and_collapse_persists_in_ui_state() {
    let (mut t, tr) = T::open();
    let p = t.panel(tr);
    let bar = p.shaper_bar.clone().unwrap();
    assert!(p.shaper_body.is_none(), "collapsed while the shaper is off and nobody chose");
    assert_eq!(bar.chevron_deg, -90.0);
    let sw = bar.switch.clone().unwrap();
    assert_eq!(sw.readout_text, "OFF");
    let before = t.undo_len();
    t.click(mid(sw.rect), 1);
    assert_eq!(t.param(tr, "shaper"), 1.0, "the switch writes the enable parameter");
    assert_eq!(t.undo_len(), before + 1);
    assert!(t.app.ui.device_sections.is_empty(), "negative control: enabling never touches collapse");
    assert!(t.panel(tr).shaper_body.is_some(), "in use: open until the user chooses");
    // Collapse: a view setting in UiState, never the parameter.
    let button = t.panel(tr).shaper_bar.unwrap().button;
    t.click(mid(button), 1);
    assert_eq!(t.app.ui.device_sections.get(&section_key("shaper")), Some(&true));
    assert!(t.panel(tr).shaper_body.is_none());
    assert_eq!(t.param(tr, "shaper"), 1.0, "negative control: collapsing never touches the parameter");
    // The setting saves with the preferences and holds on the next window.
    let json = serde_json::to_string(&t.app.ui).unwrap();
    let back: crate::UiState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.device_sections.get(&section_key("shaper")), Some(&true));
    let sw = t.panel(tr).shaper_bar.unwrap().switch.unwrap();
    t.click(mid(sw.rect), 1);
    assert_eq!(t.param(tr, "shaper"), 0.0);
    assert!(t.panel(tr).shaper_body.is_none(), "still collapsed: the user's choice");
}

#[test]
fn a_disabled_but_open_shaper_dims_and_stays_editable() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    assert_eq!(t.panel(tr).shaper_opacity, alpha::DISABLED_SECTION, "off and open: dimmed");
    let dial = t.knob(tr, "resonance");
    t.drag(mid(dial), (mid(dial).0, mid(dial).1 - 40.0), false);
    assert!(t.param(tr, "resonance") > 15.0, "still editable");
    t.set(tr, "shaper", 1.0);
    assert_eq!(t.panel(tr).shaper_opacity, 1.0, "negative control: on is full strength");
}

#[test]
fn the_filter_segments_set_mode_and_slope_and_the_cutoff_point_drags() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    let seg = |t: &T, id: &str| t.panel(tr).segments.iter().find(|s| s.param == id).cloned().unwrap();
    assert_eq!(seg(&t, "filter_mode").options, ["LP", "BP", "HP"]);
    assert_eq!(seg(&t, "filter_slope").options, ["12 dB", "24 dB"]);
    let hp = t.in_view(tr, |p| p.segments.iter().find(|s| s.param == "filter_mode").unwrap().segmented.segments[2]);
    t.click(mid(hp), 1);
    assert_eq!(t.param(tr, "filter_mode"), 2.0);
    assert_eq!(seg(&t, "filter_mode").segmented.selected, 2);
    let slope = t.in_view(tr, |p| p.segments.iter().find(|s| s.param == "filter_slope").unwrap().segmented.segments[1]);
    t.click(mid(slope), 1);
    assert_eq!(t.param(tr, "filter_slope"), 1.0);
    // The response well's point sits at the cutoff; dragging it across sets the cutoff.
    t.set(tr, "cutoff", 1000.0);
    let well = t.in_view(tr, |p| p.wells.iter().find(|w| w.name == "filter").unwrap().well.well);
    let w = t.panel(tr).wells.iter().find(|w| w.name == "filter").cloned().unwrap().well;
    let point = w.points[0];
    let plot = w.plot;
    assert!((mid(point).0 - (plot[0] + (plot[2] - plot[0]) * freq_x(1000.0))).abs() < 0.5);
    let _ = well;
    let to = plot[0] + (plot[2] - plot[0]) * freq_x(4000.0);
    t.drag(mid(point), (to, mid(point).1), false);
    let c = t.param(tr, "cutoff");
    assert!((c / 4000.0 - 1.0).abs() < 0.03, "{c}");
}

#[test]
fn the_response_follows_mode_slope_and_cutoff() {
    // Low pass: flat well below the cutoff, falling above it; high pass the mirror.
    assert!(response_db(0, false, 1000.0, 0.0, 50.0).abs() < 0.1);
    assert!(response_db(0, false, 1000.0, 0.0, 10_000.0) < -35.0);
    assert!(response_db(2, false, 1000.0, 0.0, 50.0) < -45.0);
    assert!(response_db(2, false, 1000.0, 0.0, 15_000.0).abs() < 0.1);
    // 24 dB doubles the slope; resonance lifts the cutoff.
    let (a, b) = (response_db(0, false, 1000.0, 0.0, 8000.0), response_db(0, true, 1000.0, 0.0, 8000.0));
    assert!((b - 2.0 * a).abs() < 1e-3);
    assert!(response_db(0, false, 1000.0, 0.8, 1000.0) > 12.0);
    // Negative control: band pass passes the cutoff and cuts both sides.
    assert!(response_db(1, false, 1000.0, 0.0, 1000.0) > -0.1 && response_db(1, false, 1000.0, 0.0, 30.0) < -20.0);
    for x in [0.0, 0.25, 0.5, 1.0] {
        assert!((freq_x(x_freq(x)) - x).abs() < 1e-5);
    }
    assert_eq!((x_freq(0.0), x_freq(1.0).round()), (20.0, 20_000.0));
}

#[test]
fn envelopes_and_lfo_traces_draw_from_their_settings() {
    let c = env_curve(0.1, 0.2, 0.5, 0.3);
    assert_eq!(c.len(), 5);
    assert!(c[1][1] > c[2][1] && (c[2][1] - c[3][1]).abs() < 1e-6 && c[4] == [1.0, 0.0], "{c:?}");
    let (trace, env) = lfo_curve(0, 2.0, 0.0, 1.0);
    assert!(env.is_empty(), "no fade: no envelope");
    assert!(trace.iter().any(|p| p[1] > 0.9) && trace.iter().any(|p| p[1] < 0.1));
    let (trace, env) = lfo_curve(3, 2.0, 0.5, 0.5);
    assert!(!env.is_empty());
    assert!(trace[2][1].abs() - 0.5 < 0.05, "faded in from nothing");
    assert_eq!(SHAPES.len(), super::test_rig::info().param("lfo1_shape").unwrap().choices.len());
}

#[test]
fn sync_and_hz_set_the_mode_and_the_rate_knob_follows_it() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    let knob = |t: &T| t.panel(tr).knobs.iter().filter(|k| k.label == "Rate").map(|k| k.param.clone()).collect::<Vec<_>>();
    assert_eq!(knob(&t), ["lfo1_rate", "lfo2_rate"]);
    let sync = t.in_view(tr, |p| p.toggles.iter().find(|x| x.param == "lfo1_sync").unwrap().toggle.rect);
    t.click(mid(sync), 1);
    assert_eq!(t.param(tr, "lfo1_sync"), 1.0);
    assert_eq!(knob(&t), ["lfo1_division", "lfo2_rate"], "synced: the Rate knob sets the division");
    let hz = t.in_view(tr, |p| p.toggles.iter().find(|x| x.param == "lfo1_sync:hz").unwrap().toggle.rect);
    t.click(mid(hz), 1);
    assert_eq!(t.param(tr, "lfo1_sync"), 0.0);
    // Negative control: HZ again keeps it free (it names a mode, it does not flip).
    t.click(mid(hz), 1);
    assert_eq!(t.param(tr, "lfo1_sync"), 0.0);
    let retrig = t.in_view(tr, |p| p.toggles.iter().find(|x| x.param == "lfo2_retrigger").unwrap().toggle.rect);
    assert!(t.panel(tr).toggles.iter().find(|x| x.param == "lfo2_retrigger").unwrap().toggle.on, "on by default");
    t.click(mid(retrig), 1);
    assert_eq!(t.param(tr, "lfo2_retrigger"), 0.0);
}

#[test]
fn the_collapsed_bar_summarizes_the_settings() {
    let (mut t, tr) = T::open();
    t.set(tr, "cutoff", 1200.0);
    let bar = t.panel(tr).shaper_bar.unwrap();
    let (text, _) = bar.middle.expect("the summary");
    assert!(text.starts_with("Amp ") && text.contains("LP 12 dB · 1.20 kHz") && text.contains("LFO 1 2.0 Hz"), "{text}");
}

#[test]
fn a_plain_wheel_over_a_knob_scrolls_the_body_and_ctrl_wheel_nudges_it() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    t.set(tr, "resonance", 50.0);
    let dial = t.knob(tr, "resonance");
    let before = (t.param(tr, "resonance"), t.undo_len());
    t.send("ui.scroll", serde_json::json!({"x": mid(dial).0, "y": mid(dial).1, "dy": -120.0}));
    let after = t.panel(tr).knob("resonance").unwrap().knob.dial;
    assert!(after[1] < dial[1] - 20.0, "the body scrolled under the pointer: {dial:?} -> {after:?}");
    assert_eq!((t.param(tr, "resonance"), t.undo_len()), before, "and the knob wrote nothing");
    // Ctrl+wheel: one notch nudges 1 percent of the travel and the body stays put.
    let dial = t.knob(tr, "resonance");
    t.send("ui.scroll", serde_json::json!({"x": mid(dial).0, "y": mid(dial).1, "dy": 40.0, "ctrl": true}));
    assert!((t.param(tr, "resonance") - 51.0).abs() < 1e-3, "{}", t.param(tr, "resonance"));
    assert_eq!(t.panel(tr).knob("resonance").unwrap().knob.dial, dial, "no scroll");
}

#[test]
fn a_ctrl_wheel_gesture_is_one_undo_step() {
    let (mut t, tr) = T::open();
    t.expand(tr);
    t.set(tr, "resonance", 50.0);
    let dial = t.knob(tr, "resonance");
    let before = t.undo_len();
    // Eight frames of Ctrl+wheel, one notch each (a trackpad's smooth scroll arrives this way).
    t.send("ui.scroll", serde_json::json!({"x": mid(dial).0, "y": mid(dial).1, "dy": 320.0, "steps": 8, "ctrl": true}));
    assert!((t.param(tr, "resonance") - 58.0).abs() < 1e-3, "eight nudges: {}", t.param(tr, "resonance"));
    assert_eq!(t.undo_len(), before + 1, "one gesture, one undo step");
    // A frame without the wheel ends it: the next gesture is a step of its own.
    t.frames(2);
    t.send("ui.scroll", serde_json::json!({"x": mid(dial).0, "y": mid(dial).1, "dy": 80.0, "steps": 2, "ctrl": true}));
    assert!((t.param(tr, "resonance") - 60.0).abs() < 1e-3);
    assert_eq!(t.undo_len(), before + 2, "two gestures, two steps");
    t.app.run("edit.undo", serde_json::json!({})).unwrap();
    t.frames(2);
    assert!((t.param(tr, "resonance") - 58.0).abs() < 1e-3, "undo takes the second gesture back whole");
}
