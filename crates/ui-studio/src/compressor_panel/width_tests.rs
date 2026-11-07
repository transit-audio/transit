//! [`super::MIN_WIDTH`] from the content (the 326-X method): the panel laid out at a width with
//! the Detector section open and every knob at the widest value its parameter prints (sampled
//! over its travel and at its floor and just above it); no knob label or readout meets its
//! neighbor's or leaves its panel, the sidechain label clears its dropdown, and every segment's
//! text fits its segment.

use super::test_rig::T;
use crate::tokens::{self, TypeRole};
use transit_model::TrackId;

/// The room a label keeps from the control right of it (the Macro Oscillator's Output column's).
const LABEL_CLEAR: f32 = 4.0;

fn painter(t: &T) -> egui::Painter {
    egui::Painter::new(t.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING)
}

fn text_w(p: &egui::Painter, role: &TypeRole, text: &str) -> f32 {
    crate::widgets::role_size(p, role, text).x
}

/// The value of `param` whose printed text is the widest: 2001 points over the knob's travel (both
/// ends included), and the floor and just above it (1e-9, 1e-6 and 1e-3 of the range), where a
/// leftover of a drag that came back sits.
pub fn widest(p: &egui::Painter, param: &transit_dsp::ParamInfo) -> (f32, String) {
    let travel = (0..=2000).map(|i| crate::device_editor::value_at(param, i as f32 / 2000.0));
    let above = [0.0, 1e-9, 1e-6, 1e-3].map(|f| param.clamp(param.min + f * (param.max - param.min)));
    let mut best = (param.min, param.format(param.min));
    for v in travel.chain(above) {
        let s = param.format(v);
        if text_w(p, &tokens::VALUE, &s) > text_w(p, &tokens::VALUE, &best.1) {
            best = (v, s);
        }
    }
    best
}

/// Opens the panel at window width `width` (a lower minimum `min` lets it go under
/// [`super::MIN_WIDTH`]) with the Detector section open and the widest values set; returns the
/// readouts it set (param, text).
pub fn open_at(width: f32, min: Option<f32>) -> (T, TrackId, Vec<(String, String)>) {
    let (mut t, tr) = T::open();
    t.app.builtin_editor.devices.min_override = min;
    t.screen = egui::vec2(width + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    t.frames(3);
    t.expand(tr);
    let p = painter(&t);
    let info = super::test_rig::info();
    let mut set = Vec::new();
    for k in t.panel(tr).knobs {
        let param = info.param(&k.param).unwrap();
        let (v, s) = widest(&p, param);
        t.set(tr, &k.param, v);
        set.push((k.param.clone(), s));
    }
    t.frames(3);
    (t, tr, set)
}

/// Every crowding fault of the panel as last drawn.
pub fn faults(t: &T, tr: TrackId) -> Vec<String> {
    let panel = t.panel(tr);
    let p = painter(t);
    let mut out = Vec::new();
    let inside = |r: [f32; 4], o: [f32; 4]| r[0] >= o[0] - 0.01 && r[2] <= o[2] + 0.01;
    struct K {
        param: String,
        y: f32,
        label: [f32; 4],
        value: [f32; 4],
        cx: f32,
        cy: f32,
    }
    let ks: Vec<K> = panel
        .knobs
        .iter()
        .map(|k| {
            let value = t.row(tr, &k.param).value;
            let (lw, vw) = (text_w(&p, &tokens::LABEL_CONTROL, &k.label), text_w(&p, &tokens::VALUE, &value));
            let (lc, vc) = ((k.knob.label[0] + k.knob.label[2]) / 2.0, (k.knob.value[0] + k.knob.value[2]) / 2.0);
            K {
                param: k.param.clone(),
                y: k.knob.label[1],
                label: [lc - lw / 2.0, k.knob.label[1], lc + lw / 2.0, k.knob.label[3]],
                value: [vc - vw / 2.0, k.knob.value[1], vc + vw / 2.0, k.knob.value[3]],
                cx: (k.knob.dial[0] + k.knob.dial[2]) / 2.0,
                cy: (k.knob.dial[1] + k.knob.dial[3]) / 2.0,
            }
        })
        .collect();
    for a in &ks {
        if let Some(b) = ks.iter().filter(|b| (b.y - a.y).abs() < 0.5 && b.cx > a.cx).min_by(|x, y| x.cx.total_cmp(&y.cx)) {
            if a.label[2] > b.label[0] {
                out.push(format!("label {} meets {} ({:.1} px)", a.param, b.param, a.label[2] - b.label[0]));
            }
            if a.value[2] > b.value[0] {
                out.push(format!("value {} meets {} ({:.1} px)", a.param, b.param, a.value[2] - b.value[0]));
            }
        }
        let card = panel.panels.iter().find(|(_, r)| a.cx > r[0] && a.cx < r[2] && a.cy > r[1] && a.cy < r[3]);
        match card {
            Some((name, r)) => {
                if !inside(a.label, *r) {
                    out.push(format!("label {} leaves {name}", a.param));
                }
                if !inside(a.value, *r) {
                    out.push(format!("value {} leaves {name}", a.param));
                }
            }
            None => out.push(format!("knob {} outside every panel", a.param)),
        }
        // The dynamics knobs keep left of the divider.
        if a.label[2].max(a.value[2]) > panel.divider[0] && a.cy < panel.divider[3] && a.cy > panel.divider[1] {
            out.push(format!("{} crosses the divider", a.param));
        }
    }
    // The sidechain label clears its dropdown; the dropdown stays right of the divider.
    let k = &panel.key;
    let label_w = text_w(&p, &tokens::LABEL_CONTROL, "Sidechain");
    if k.label[0] + label_w + LABEL_CLEAR > k.dropdown[0] {
        out.push(format!("Sidechain meets its dropdown ({:.1} px)", k.label[0] + label_w + LABEL_CLEAR - k.dropdown[0]));
    }
    if k.dropdown[0] < panel.divider[2] {
        out.push("the key dropdown crosses the divider".to_string());
    }
    // Every segment's text fits its segment.
    if let Some(m) = &panel.mode {
        for (o, r) in panel.mode_options.iter().zip(&m.segments) {
            let need = text_w(&p, &tokens::VALUE, o);
            if r[2] - r[0] + 0.01 < need {
                out.push(format!("segment {o}: {:.1} < {need:.1}", r[2] - r[0]));
            }
        }
    }
    out
}

#[test]
fn the_minimum_width_is_the_narrowest_multiple_of_8_where_the_widest_readouts_fit() {
    let min = super::MIN_WIDTH;
    assert_eq!(min % 8.0, 0.0);
    let (t, tr, set) = open_at(min, None);
    assert_eq!(t.panel(tr).window[2] - t.panel(tr).window[0], min);
    assert_eq!(faults(&t, tr), Vec::<String>::new(), "at {min} with {set:?}");
    // One step narrower already crowds: the minimum is the narrowest that passes.
    let (t, tr, _) = open_at(min - 8.0, Some(min - 8.0));
    assert!(!faults(&t, tr).is_empty(), "at {}", min - 8.0);
    // Negative control (the brief's): 80 narrower crowds.
    let (t, tr, _) = open_at(min - 80.0, Some(min - 80.0));
    assert!(!faults(&t, tr).is_empty(), "at {}", min - 80.0);
    // The design width is clean too.
    let (t, tr, _) = open_at(super::DESIGN_WIDTH, None);
    assert_eq!(faults(&t, tr), Vec::<String>::new());
}

#[test]
fn the_attack_floor_prints_its_smallest_step() {
    let info = super::test_rig::info();
    let attack = info.param("attack").unwrap();
    assert_eq!(attack.format(attack.min), "0.01 ms");
    let p = painter(&T::new());
    let (_, s) = widest(&p, attack);
    assert!(!s.is_empty());
    // The floor and the values just above it print the floor (no long leftover digits).
    for f in [1e-9, 1e-6] {
        assert_eq!(attack.format(attack.clamp(attack.min + f * (attack.max - attack.min))), "0.01 ms", "{f}");
    }
    // Negative control: a value a step up prints differently.
    assert_ne!(attack.format(0.02), "0.01 ms");
}
