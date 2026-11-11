//! [`super::MIN_WIDTH`] from the content (the 326-X method): the panel laid out at a width with
//! every zone knob at the widest value it prints (sampled over its travel and at its floor and just
//! above it); no knob label or readout meets its neighbor's or leaves its panel, every choke and
//! trigger segment's text fits its segment, the toggles stay in their panel and the strip's hint
//! clears the toggles, and the title bar's preset browser keeps clear of the brand and the track.
//! The title bar is what sets the minimum (the knob readouts last fit at 616).

use super::test_rig::T;
use crate::tokens::{self, Family, TypeRole};
use crate::widgets::test_support::inside;
use serde_json::json;
use transit_model::TrackId;

fn painter(t: &T) -> egui::Painter {
    egui::Painter::new(t.ctx.clone(), egui::LayerId::background(), egui::Rect::EVERYTHING)
}

fn text_w(p: &egui::Painter, role: &TypeRole, text: &str) -> f32 {
    crate::widgets::role_size(p, role, text).x
}

/// The zone fields' descriptions by field id.
fn param(field: &str) -> &'static transit_dsp::ParamInfo {
    use transit_dsp::plugins::sampler::{FIELD_LEVEL, FIELD_PAN, field_info};
    match field {
        "level" => field_info(FIELD_LEVEL).unwrap(),
        "pan" => field_info(FIELD_PAN).unwrap(),
        "transpose" => &super::zone::TRANSPOSE,
        "cents" => &super::zone::CENTS,
        "attack_ms" => &super::zone::ATTACK,
        "decay_ms" => &super::zone::DECAY,
        "sustain" => &super::zone::SUSTAIN,
        "release_ms" => &super::zone::RELEASE,
        f => panic!("{f}"),
    }
}

/// The value of `param` whose printed text is the widest: 2001 points over the knob's travel (both
/// ends included), and the floor and just above it (the Compressor's width test's sampling).
fn widest(p: &egui::Painter, param: &transit_dsp::ParamInfo) -> (f32, String) {
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

/// Opens a kit at window width `width` (a lower minimum `min` lets it go under
/// [`super::MIN_WIDTH`]) with pad A1 selected and its zone's knobs at their widest values; returns
/// the readouts it set.
pub fn open_at(width: f32, min: Option<f32>) -> (T, TrackId, Vec<(String, String)>) {
    let (mut t, tr) = T::kit();
    t.app.builtin_editor.devices.min_override = min;
    t.screen = egui::vec2(width + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    t.frames(3);
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 1, "choke": 16}));
    let p = painter(&t);
    let mut set = Vec::new();
    for k in t.panel(tr).knobs {
        let info = param(&k.field);
        let (v, s) = widest(&p, info);
        match k.field.as_str() {
            "level" | "pan" => t.run("mix.sampler_control", json!({"track": tr.0, "id": format!("zone001_{}", k.field), "value": v})),
            "sustain" => t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "playback": {"sustain": v / 100.0}})),
            f => {
                let mut fields = serde_json::Map::new();
                fields.insert(f.to_string(), json!(v));
                t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "playback": fields}));
            }
        }
        set.push((k.field.clone(), s));
    }
    t.frames(3);
    (t, tr, set)
}

/// Every crowding fault of the panel as last drawn.
pub fn faults(t: &T, tr: TrackId) -> Vec<String> {
    let panel = t.panel(tr);
    let p = painter(t);
    let mut out = Vec::new();
    let between = |r: [f32; 4], o: [f32; 4]| r[0] >= o[0] - 0.01 && r[2] <= o[2] + 0.01;
    struct K {
        field: String,
        y: f32,
        label: [f32; 4],
        value: [f32; 4],
        dial: [f32; 4],
        cx: f32,
        cy: f32,
    }
    let ks: Vec<K> = panel
        .knobs
        .iter()
        .map(|k| {
            let (lw, vw) = (text_w(&p, &tokens::LABEL_CONTROL, &k.label), text_w(&p, &tokens::VALUE, &k.value));
            let (lc, vc) = ((k.knob.label[0] + k.knob.label[2]) / 2.0, (k.knob.value[0] + k.knob.value[2]) / 2.0);
            K {
                field: k.field.clone(),
                y: k.knob.label[1],
                label: [lc - lw / 2.0, k.knob.label[1], lc + lw / 2.0, k.knob.label[3]],
                value: [vc - vw / 2.0, k.knob.value[1], vc + vw / 2.0, k.knob.value[3]],
                dial: k.knob.dial,
                cx: (k.knob.dial[0] + k.knob.dial[2]) / 2.0,
                cy: (k.knob.dial[1] + k.knob.dial[3]) / 2.0,
            }
        })
        .collect();
    for a in &ks {
        if let Some(b) = ks.iter().filter(|b| (b.y - a.y).abs() < 0.5 && b.cx > a.cx).min_by(|x, y| x.cx.total_cmp(&y.cx)) {
            if a.label[2] > b.label[0] {
                out.push(format!("label {} meets {} ({:.1} px)", a.field, b.field, a.label[2] - b.label[0]));
            }
            if a.value[2] > b.value[0] {
                out.push(format!("value {} meets {} ({:.1} px)", a.field, b.field, a.value[2] - b.value[0]));
            }
            if a.dial[2] > b.dial[0] {
                out.push(format!("dial {} meets {} ({:.1} px)", a.field, b.field, a.dial[2] - b.dial[0]));
            }
        }
        let card = panel.panels.iter().find(|(_, r)| a.cx > r[0] && a.cx < r[2] && a.cy > r[1] && a.cy < r[3]);
        match card {
            Some((name, r)) => {
                let inner = [r[0] + 1.0, r[1], r[2] - 1.0, r[3]];
                if !between(a.label, inner) {
                    out.push(format!("label {} leaves {name}", a.field));
                }
                if !between(a.value, inner) {
                    out.push(format!("value {} leaves {name}", a.field));
                }
            }
            None => out.push(format!("knob {} outside every panel", a.field)),
        }
        if a.label[2].max(a.value[2]) > panel.divider[0] && a.cx < panel.divider[0] && a.cy > panel.divider[1] && a.cy < panel.divider[3] {
            out.push(format!("{} crosses the divider", a.field));
        }
    }
    // The choke segments' text fits.
    let seg = TypeRole { family: Family::Mono, weight: 500, size: 10.0, tracking: 0.0, uppercase: false, ink: None };
    for g in &panel.strip.segments {
        let need = text_w(&p, &seg, &g.text);
        if g.rect[2] - g.rect[0] + 0.01 < need {
            out.push(format!("choke {}: {:.1} < {need:.1}", g.text, g.rect[2] - g.rect[0]));
        }
    }
    // The trigger segments' text fits; the toggles stay in the Playback panel, right of the divider.
    if let Some(m) = &panel.mode {
        for (o, r) in super::zone::MODES.iter().zip(&m.segments) {
            let need = text_w(&p, &tokens::VALUE, o) + 4.0;
            if r[2] - r[0] + 0.01 < need {
                out.push(format!("segment {o}: {:.1} < {need:.1}", r[2] - r[0]));
            }
        }
    }
    let play = panel.panel("playback").unwrap();
    let play_inner = [play[0] + 16.0, play[1], play[2] - 16.0, play[3]];
    for (name, tg) in [("reverse", &panel.reverse), ("key track", &panel.key_track)] {
        if let Some(tg) = tg
            && (!between(tg.rect, play_inner) || tg.rect[0] < panel.divider[2])
        {
            out.push(format!("{name} leaves its column"));
        }
    }
    // The title bar's preset browser keeps clear of the brand and the track (the device title may
    // give way, 326-X; no control does).
    let tb = &panel.title;
    if tb.prev[0] < tb.track_rect[2] + 12.0 - 0.01 {
        out.push(format!("the preset browser meets the track ({:.1} px)", tb.track_rect[2] + 12.0 - tb.prev[0]));
    }
    // The strip's hint clears the toggles and stays in the strip.
    let s = &panel.strip;
    let strip = panel.panel("strip").unwrap();
    if let Some(so) = &s.solo
        && s.hint_rect[0] < so.rect[2] + 8.0 - 0.01
    {
        out.push("the hint meets S".to_string());
    }
    if !inside(s.hint_rect, strip) {
        out.push("the hint leaves the strip".to_string());
    }
    // The view bar's widest summary keeps a bar gap before the Start note label (the actions,
    // Load kit… and Expand to rack…, are right-aligned and never give way).
    let b = &panel.bar;
    let widest = text_w(&p, &tokens::READOUT_DISPLAY, "Bank D · 16 of 16 loaded · 16 choke groups");
    let over = b.summary_slot[0] + widest + crate::tokens::sampler::VIEW_BAR_GAP - b.start.label_rect[0];
    if over > 0.01 {
        out.push(format!("the widest summary meets the Start note label ({over:.1} px)"));
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
    // The design width is clean too, with the knobs in their full 84 px slots.
    let (t, tr, _) = open_at(super::DESIGN_WIDTH, None);
    assert_eq!(faults(&t, tr), Vec::<String>::new());
    let k = &t.panel(tr).knobs;
    assert_eq!(k[1].knob.rect[0] - k[0].knob.rect[0], super::zone::SLOT + super::zone::SLOT_GAP);
}
