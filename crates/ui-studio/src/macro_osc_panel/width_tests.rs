//! [`super::MIN_WIDTH`] from the content (326-X): the whole panel laid out at a width, both
//! extension sections open, every knob at the widest value its parameter prints; no knob label or
//! readout meets its neighbor's, none leaves its panel, the Output column's labels clear their
//! dropdowns, and every segment's text fits its segment.

use super::test_rig::T;
use crate::tokens::{self, TypeRole};
use transit_model::TrackId;

fn painter(t: &T) -> egui::Painter {
    egui::Painter::new(t.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING)
}

fn text_w(p: &egui::Painter, role: &TypeRole, text: &str) -> f32 {
    crate::widgets::role_size(p, role, text).x
}

/// The value of `param` whose printed text is the widest (sampled over the knob's travel, both
/// ends included).
pub fn widest(p: &egui::Painter, param: &transit_dsp::ParamInfo) -> (f32, String) {
    let mut best = (param.min, param.format(param.min));
    for i in 0..=2000 {
        let v = super::value_at(param, i as f32 / 2000.0);
        let s = param.format(v);
        if text_w(p, &tokens::VALUE, &s) > text_w(p, &tokens::VALUE, &best.1) {
            best = (v, s);
        }
    }
    best
}

/// The painted text reading exactly `want` on the line of `row` (its vertical center inside it).
fn painted_rect(t: &T, want: &str, row: [f32; 4]) -> Option<[f32; 4]> {
    fn walk(shape: &egui::Shape, want: &str, row: [f32; 4], out: &mut Option<[f32; 4]>) {
        match shape {
            egui::Shape::Text(s) if s.galley.text() == want => {
                let r = egui::Rect::from_min_size(s.pos, s.galley.size());
                if r.center().y > row[1] && r.center().y < row[3] {
                    *out = Some([r.min.x, r.min.y, r.max.x, r.max.y]);
                }
            }
            egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, want, row, out)),
            _ => {}
        }
    }
    let mut found = None;
    t.shapes.iter().for_each(|s| walk(&s.shape, want, row, &mut found));
    found
}

/// Opens the panel at window width `width` (a lower minimum `min` lets it go under
/// [`super::MIN_WIDTH`]) with the widest values set; returns the widest readouts it set (param,
/// text).
pub fn open_at(width: f32, min: Option<f32>) -> (T, TrackId, Vec<(String, String)>) {
    let (mut t, tr) = T::open();
    t.app.builtin_editor.macro_osc.min_override = min;
    t.screen = egui::vec2(width + 2.0 * crate::device_window::SCREEN_MARGIN, 2400.0);
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
    // Each knob's label and value as painted: centered in their rows.
    struct K {
        param: String,
        y: f32,
        label: [f32; 4],
        value: [f32; 4],
        dial_c: (f32, f32),
    }
    let ks: Vec<K> = panel
        .knobs
        .iter()
        .map(|k| {
            let role = if k.knob.size == crate::widgets::knob::Size::L { tokens::LABEL_HERO } else { tokens::LABEL_CONTROL };
            let value = t.row(tr, &k.param).value;
            let (lw, vw) = (text_w(&p, &role, &k.label), text_w(&p, &tokens::VALUE, &value));
            let (lc, vc) = ((k.knob.label[0] + k.knob.label[2]) / 2.0, (k.knob.value[0] + k.knob.value[2]) / 2.0);
            K {
                param: k.param.clone(),
                y: k.knob.label[1],
                label: [lc - lw / 2.0, k.knob.label[1], lc + lw / 2.0, k.knob.label[3]],
                value: [vc - vw / 2.0, k.knob.value[1], vc + vw / 2.0, k.knob.value[3]],
                dial_c: ((k.knob.dial[0] + k.knob.dial[2]) / 2.0, (k.knob.dial[1] + k.knob.dial[3]) / 2.0),
            }
        })
        .collect();
    for a in &ks {
        // Neighbors: knobs whose label line is the same, the nearest to the right.
        if let Some(b) = ks
            .iter()
            .filter(|b| (b.y - a.y).abs() < 0.5 && b.label[0] + b.label[2] > a.label[0] + a.label[2])
            .min_by(|x, y| x.dial_c.0.total_cmp(&y.dial_c.0))
        {
            if a.label[2] > b.label[0] {
                out.push(format!("label {} meets {} ({:.1} px)", a.param, b.param, a.label[2] - b.label[0]));
            }
            if a.value[2] > b.value[0] {
                out.push(format!("value {} meets {} ({:.1} px)", a.param, b.param, a.value[2] - b.value[0]));
            }
        }
        // In its panel (the card that holds the dial).
        let card = panel
            .panels
            .iter()
            .filter(|(n, _)| n != "hero")
            .find(|(_, r)| a.dial_c.0 > r[0] && a.dial_c.0 < r[2] && a.dial_c.1 > r[1] && a.dial_c.1 < r[3]);
        if let Some((name, r)) = card {
            if !inside(a.label, *r) {
                out.push(format!("label {} leaves {name}", a.param));
            }
            if !inside(a.value, *r) {
                out.push(format!("value {} leaves {name}", a.param));
            }
        }
    }
    // The Output column: each label clears its dropdown.
    for (id, label) in [("mod_wheel", "Mod wheel"), ("voices", "Voices")] {
        let d = panel.dropdowns.iter().find(|d| d.0 == id).unwrap().1;
        match painted_rect(t, label, d) {
            Some(r) if r[2] + 4.0 <= d[0] => {}
            Some(r) => out.push(format!("{label} meets its dropdown ({:.1} px)", r[2] + 4.0 - d[0])),
            None => out.push(format!("{label} not painted")),
        }
    }
    // Every segment's text fits its segment.
    for s in &panel.segments {
        for (o, r) in s.options.iter().zip(&s.segmented.segments) {
            let need = text_w(&p, &tokens::VALUE, o);
            if r[2] - r[0] + 0.01 < need {
                out.push(format!("segment {} {o}: {:.1} < {:.1}", s.param, r[2] - r[0], need));
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
    let f = faults(&t, tr);
    assert!(!f.is_empty(), "at {}", min - 80.0);
}
