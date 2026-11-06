//! `crate::tokens` mirrors `docs/design/language/tokens.json` (ADR 0045): every color, type role,
//! space, layout, radius and component metric in the file equals its Rust constant, and a name on
//! either side without a partner fails. The theme sets read those constants, the derived colors
//! keep the contrast the brief settled, and no painted file holds a literal color.

use crate::theme::{DARK, LIGHT, Tokens, contrast};
use crate::tokens::{self, Family, Ink};
use egui::Color32;
use serde_json::Value;
use std::collections::BTreeSet;

const JSON: &str = include_str!("../../../docs/design/language/tokens.json");

fn json() -> Value {
    serde_json::from_str(JSON).expect("tokens.json parses")
}

/// `#RRGGBB` or `#RRGGBBAA` (black with alpha is the same premultiplied or not).
fn parse(hex: &str) -> Color32 {
    let h = hex.strip_prefix('#').unwrap_or_else(|| panic!("{hex}: no #"));
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or_else(|_| panic!("{hex}: not hex"));
    match h.len() {
        6 => Color32::from_rgb(b(0), b(2), b(4)),
        8 => Color32::from_rgba_premultiplied(b(0), b(2), b(4), b(6)),
        _ => panic!("{hex}: not #RRGGBB or #RRGGBBAA"),
    }
}

fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    v.pointer(&format!("/{path}")).unwrap_or_else(|| panic!("tokens.json has no {path}"))
}

fn num(v: &Value, path: &str) -> f32 {
    at(v, path).as_f64().unwrap_or_else(|| panic!("{path} is not a number")) as f32
}

/// The object at `key` and `rust` name the same colors with the same values.
fn same_colors(v: &Value, key: &str, rust: &[(&str, Color32)]) {
    let obj = at(v, key).as_object().unwrap_or_else(|| panic!("{key} is not an object"));
    let in_json: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let in_rust: BTreeSet<&str> = rust.iter().map(|(n, _)| *n).collect();
    assert_eq!(in_rust.len(), rust.len(), "{key}: a name repeats in Rust");
    let missing_in_rust: Vec<_> = in_json.difference(&in_rust).collect();
    let missing_in_json: Vec<_> = in_rust.difference(&in_json).collect();
    assert!(missing_in_rust.is_empty(), "{key}: in tokens.json but not in Rust: {missing_in_rust:?}");
    assert!(missing_in_json.is_empty(), "{key}: in Rust but not in tokens.json: {missing_in_json:?}");
    for (name, c) in rust {
        let hex = obj[*name].as_str().unwrap_or_else(|| panic!("{key}.{name} is not a string"));
        assert_eq!(parse(hex), *c, "{key}.{name}: tokens.json {hex}, Rust {c:?}");
    }
}

#[test]
fn the_color_table_matches_tokens_json_both_ways() {
    same_colors(&json(), "color", tokens::color::ALL);
}

#[test]
fn the_arrangement_colors_match_tokens_json_both_ways() {
    let mut rust = tokens::arrangement::OPAQUE.to_vec();
    rust.push(("overlay", tokens::arrangement::OVERLAY));
    same_colors(&json(), "arrangement", &rust);
}

#[test]
fn the_light_set_matches_tokens_json_both_ways() {
    let mut rust = tokens::color_light::ALL.to_vec();
    rust.push(("overlay", tokens::color_light::OVERLAY));
    same_colors(&json(), "colorLight", &rust);
    // Every dark name has a light value, and the light set has no name of its own.
    let v = json();
    let dark: BTreeSet<String> = ["color", "arrangement"]
        .iter()
        .flat_map(|k| at(&v, k).as_object().unwrap().keys().cloned())
        .chain(tokens::component::ALL.iter().map(|(n, _)| n.to_string()))
        .collect();
    let light: BTreeSet<String> = at(&v, "colorLight").as_object().unwrap().keys().cloned().collect();
    assert_eq!(dark, light, "the light set names exactly the dark names");
}

#[test]
fn component_literals_match_their_paths_in_tokens_json() {
    let v = json();
    for (path, c) in tokens::component::ALL {
        let hex = at(&v, &path.replace('.', "/")).as_str().unwrap();
        assert_eq!(parse(hex), *c, "{path}");
    }
    // The other literals the components repeat are tokens already.
    assert_eq!(parse(at(&v, "toggle/on/monitor/fill").as_str().unwrap()), tokens::component::TOGGLE_OPTION_FILL);
    assert_eq!(parse(at(&v, "fader/cap/fill").as_str().unwrap()), tokens::color::KNOB_CAP_ACTIVE);
}

#[test]
fn type_roles_match_tokens_json_both_ways() {
    let v = json();
    assert_eq!(at(&v, "font/sans").as_str(), Some(tokens::FONT_SANS));
    assert_eq!(at(&v, "font/mono").as_str(), Some(tokens::FONT_MONO));
    let obj = at(&v, "type").as_object().unwrap();
    let in_json: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    let in_rust: BTreeSet<&str> = tokens::TYPE_ROLES.iter().map(|(n, _)| *n).collect();
    assert_eq!(in_json, in_rust, "type roles differ between tokens.json and Rust");
    for (name, r) in tokens::TYPE_ROLES {
        let j = &obj[*name];
        assert_eq!(j["font"].as_str(), Some(r.family.key()), "{name}: family");
        assert_eq!(j["weight"].as_u64(), Some(u64::from(r.weight)), "{name}: weight");
        assert_eq!(j["size"].as_f64().map(|x| x as f32), Some(r.size), "{name}: size");
        assert_eq!(j["tracking"].as_f64().unwrap_or(0.0) as f32, r.tracking, "{name}: tracking");
        assert_eq!(j["uppercase"].as_bool().unwrap_or(false), r.uppercase, "{name}: uppercase");
        assert_eq!(j["color"].as_str(), r.ink.map(Ink::key), "{name}: color");
    }
    let t = at(&v, "toggle/font");
    assert_eq!(t["font"].as_str(), Some(Family::Sans.key()));
    assert_eq!(
        (t["weight"].as_u64(), t["size"].as_f64()),
        (Some(u64::from(tokens::toggle::FONT.weight)), Some(f64::from(tokens::toggle::FONT.size)))
    );
}

#[test]
fn space_layout_radius_and_alpha_match_tokens_json() {
    let v = json();
    let space: Vec<f32> = at(&v, "space").as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
    assert_eq!(space, tokens::SPACE);
    use tokens::layout as l;
    assert_eq!(num(&v, "layout/windowPadding"), l::WINDOW_PADDING);
    assert_eq!(num(&v, "layout/panelGap"), l::PANEL_GAP);
    assert_eq!([num(&v, "layout/panelPadding/0"), num(&v, "layout/panelPadding/1")], l::PANEL_PADDING);
    assert_eq!(num(&v, "layout/gridColumns"), l::GRID_COLUMNS as f32);
    assert_eq!(num(&v, "layout/titleBarHeight"), l::TITLE_BAR_HEIGHT);
    assert_eq!(num(&v, "layout/routingRowHeight"), l::ROUTING_ROW_HEIGHT);
    assert_eq!([num(&v, "layout/controlHeight/0"), num(&v, "layout/controlHeight/1")], l::CONTROL_HEIGHT);
    assert_eq!([num(&v, "layout/segmentHeight/0"), num(&v, "layout/segmentHeight/1")], l::SEGMENT_HEIGHT);
    assert_eq!(at(&v, "layout").as_object().unwrap().len(), 8, "a layout metric without a Rust constant");
    for (key, rust) in [("radius", tokens::radius::ALL), ("alpha", tokens::alpha::ALL)] {
        let obj = at(&v, key).as_object().unwrap();
        let in_json: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        let in_rust: BTreeSet<&str> = rust.iter().map(|(n, _)| *n).collect();
        assert_eq!(in_json, in_rust, "{key}: names differ");
        for (name, x) in rust {
            assert_eq!(obj[*name].as_f64().map(|j| j as f32), Some(*x), "{key}.{name}");
        }
    }
}

#[test]
fn component_metrics_match_tokens_json() {
    let v = json();
    use tokens::{fader as f, knob as k, switch as s, toggle as t};
    for (path, x) in [
        ("knob/sizes/L", k::L),
        ("knob/sizes/M", k::M),
        ("knob/sizes/S", k::S),
        ("knob/box", k::BOX),
        ("knob/sweepDegrees", k::SWEEP_DEGREES),
        ("knob/startDegrees", k::START_DEGREES),
        ("knob/track/radius", k::TRACK_RADIUS),
        ("knob/track/stroke", k::TRACK_STROKE),
        ("knob/value/radius", k::TRACK_RADIUS),
        ("knob/value/stroke", k::TRACK_STROKE),
        ("knob/modRing/radius", k::MOD_RING_RADIUS),
        ("knob/modRing/stroke", k::MOD_RING_STROKE),
        ("knob/cap/radius", k::CAP_RADIUS),
        ("knob/cap/rimStroke", k::CAP_RIM_STROKE),
        ("knob/pointer/from", k::POINTER_FROM),
        ("knob/pointer/to", k::POINTER_TO),
        ("knob/pointer/stroke", k::POINTER_STROKE),
        ("switch/width", s::WIDTH),
        ("switch/height", s::HEIGHT),
        ("switch/knob", s::KNOB),
        ("toggle/height", t::HEIGHT),
        ("toggle/radius", t::RADIUS),
        ("fader/inset", f::INSET),
        ("fader/track/width", f::TRACK_WIDTH),
        ("fader/track/radius", f::TRACK_RADIUS),
        ("fader/cap/width", f::CAP[0]),
        ("fader/cap/height", f::CAP[1]),
        ("fader/cap/radius", f::CAP_RADIUS),
        ("fader/cap/centerLineWidth", f::CAP_CENTER_LINE),
        ("fader/capCompact/width", f::CAP_COMPACT[0]),
        ("fader/capCompact/height", f::CAP_COMPACT[1]),
        ("fader/capHorizontal/width", f::CAP_HORIZONTAL[0]),
        ("fader/capHorizontal/height", f::CAP_HORIZONTAL[1]),
        ("fader/variants/channel/width", f::CHANNEL_WIDTH),
        ("fader/variants/compact/width", f::COMPACT_WIDTH),
        ("fader/variants/horizontal/height", f::HORIZONTAL_HEIGHT),
        ("fader/meter/barWidth", f::METER_BAR),
        ("fader/meter/barWidthMain", f::METER_BAR_MAIN),
        ("fader/meter/gap", f::METER_GAP),
        ("fader/meter/peakAlpha", f::METER_PEAK_ALPHA),
        ("fader/meter/holdHeight", f::METER_HOLD),
        ("fader/meter/clipCell", f::METER_CLIP_CELL),
    ] {
        assert_eq!(num(&v, path), x, "{path}");
    }
    let taper: Vec<(Option<f32>, f32)> =
        at(&v, "fader/taper").as_array().unwrap().iter().map(|p| (p[0].as_f64().map(|d| d as f32), p[1].as_f64().unwrap() as f32)).collect();
    assert_eq!(taper, f::TAPER);
    let labels: Vec<Option<f32>> = at(&v, "fader/scaleLabels").as_array().unwrap().iter().map(|d| d.as_f64().map(|d| d as f32)).collect();
    assert_eq!(labels, f::SCALE_LABELS);
}

#[test]
fn the_old_token_names_read_the_design_values() {
    use tokens::color as c;
    let t: &Tokens = &DARK;
    for (old, design, name) in [
        (t.bg, c::BG_WINDOW, "bg = bg.window"),
        (t.panel, c::SURFACE_PANEL, "panel = surface.panel"),
        (t.panel_raised, c::SURFACE_RAISED, "panel_raised = surface.raised"),
        (t.line, c::LINE, "line = line"),
        (t.text, c::TEXT, "text = text"),
        (t.text_dim, c::TEXT_MUTED, "text_dim = text.muted"),
        (t.accent, c::SIGNAL, "accent = signal"),
        (t.button, c::SURFACE_RAISED, "button = surface.raised"),
        (t.button_hover, c::KNOB_CAP_ACTIVE, "button_hover = knob.capActive, one step above surface.raised"),
        (t.record, c::METER_HOT, "record = meter.hot"),
        (t.mute, c::TEXT, "mute = text (inverted toggle)"),
        (t.solo, c::SIGNAL, "solo = signal"),
        (t.monitor, c::METER_OK, "monitor = meter.ok"),
        (t.focus_ring, c::SIGNAL, "focus_ring = signal"),
        (t.ink_dark, c::BG_WINDOW, "ink_dark = bg.window"),
    ] {
        assert_eq!(old, design, "{name}");
    }
    // Each light alias follows its token too.
    let l: &Tokens = &LIGHT;
    assert_eq!(
        (l.bg, l.panel, l.accent, l.text_dim, l.record, l.mute, l.solo),
        (l.bg_window, l.surface_panel, l.signal, l.text_muted, l.meter_hot, l.text, l.signal)
    );
    assert!(l.button_hover != l.button && t.button_hover != t.button, "hover is a visible step");
}

/// The tokens the design does not name keep the contrast they had on the old ground (each value
/// is the 8-bit color nearest the old ratio, so the ratio matches to rounding).
#[test]
fn derived_arrangement_tokens_keep_their_old_contrast() {
    let old = |h: u32| tokens::hex(h);
    let (old_bg, old_lane) = (old(0x1b1c1e), old(0x1f2023));
    let t: &Tokens = &DARK;
    for (name, new, old_c, ground_old, ground_new) in [
        ("lane", t.lane, old(0x1f2023), old_bg, t.bg),
        ("lane_alt", t.lane_alt, old(0x222326), old_bg, t.bg),
        ("ruler", t.ruler, old(0x2b2c30), old_bg, t.bg),
        ("header", t.header, old(0x2a2b2f), old_bg, t.bg),
        ("header_selected", t.header_selected, old(0x3a3c42), old_bg, t.bg),
        ("grid_bar", t.grid_bar, old(0x38393e), old_lane, t.lane),
        ("grid_beat", t.grid_beat, old(0x2a2b2f), old_lane, t.lane),
        ("grid_sub", t.grid_sub, old(0x232427), old_lane, t.lane),
        ("loop_region", t.loop_region, old(0xe8b33c), old_bg, t.bg),
        ("cursor", t.cursor, old(0xe8e8ea), old_bg, t.bg),
        ("selection_outline", t.selection_outline, old(0xf4f4f6), old_bg, t.bg),
    ] {
        let (was, is) = (contrast(old_c, ground_old), contrast(new, ground_new));
        assert!((was - is).abs() <= 0.15 * (was - 1.0).max(0.05), "{name}: {was:.3} on the old ground, {is:.3} on the new");
    }
}

/// The light rule (ADR 0045): grounds mirror their dark contrast to the window, inks are at least
/// as legible as in the dark set.
#[test]
fn the_light_set_follows_its_rule() {
    let (d, l) = (&DARK, &LIGHT);
    let grounds = |t: &Tokens| {
        [
            t.bg_titlebar,
            t.surface_panel,
            t.surface_raised,
            t.surface_inset,
            t.surface_selected,
            t.knob_cap,
            t.knob_cap_active,
            t.line,
            t.line_inset,
            t.line_strong,
            t.grid,
            t.lane,
            t.lane_alt,
            t.ruler,
            t.header,
            t.header_selected,
        ]
    };
    for (i, (dg, lg)) in grounds(d).into_iter().zip(grounds(l)).enumerate() {
        let (dr, lr) = (contrast(dg, d.bg_window), contrast(lg, l.bg_window));
        assert!((dr - lr).abs() <= 0.01, "ground {i}: dark {dr:.3}, light {lr:.3}");
        // Mirrored: lighter than the window in dark is darker than it in light.
        let lum = crate::theme::luminance;
        assert_eq!(lum(dg) > lum(d.bg_window), lum(lg) < lum(l.bg_window), "ground {i} keeps its direction mirrored");
    }
    let inks = |t: &Tokens| {
        [
            t.text,
            t.text_muted,
            t.text_faint,
            t.ink_ghost,
            t.ink_arrow,
            t.signal,
            t.modulation,
            t.meter_ok,
            t.meter_hot,
            t.loop_region,
            t.cursor,
            t.selection_outline,
        ]
    };
    for (i, (di, li)) in inks(d).into_iter().zip(inks(l)).enumerate() {
        let (dr, lr) = (contrast(di, d.bg_window), contrast(li, l.bg_window));
        assert!(lr >= dr, "ink {i}: light {lr:.3} is below dark {dr:.3}");
    }
    assert_eq!(l.surface_inset, Color32::WHITE, "the light anchor");
}

/// The design's own claims: faint text reads at 4.5:1 on panel and inset; signal and mod differ
/// in lightness as well as hue.
#[test]
fn the_design_contrast_claims_hold_in_both_sets() {
    for t in [&DARK, &LIGHT] {
        for bg in [t.surface_panel, t.surface_inset] {
            assert!(contrast(t.text_faint, bg) >= 4.5, "text.faint {}", contrast(t.text_faint, bg));
        }
        let lum = crate::theme::luminance;
        assert!((lum(t.signal) - lum(t.modulation)).abs() > 0.02, "signal and mod differ in lightness");
    }
}

/// Every number leaf under `value`, by its path below `prefix` (arrays index by position).
fn number_leaves(value: &Value, prefix: &str, out: &mut Vec<(String, f32)>) {
    match value {
        Value::Number(n) => out.push((prefix.to_string(), n.as_f64().unwrap() as f32)),
        Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| number_leaves(x, &format!("{prefix}/{i}"), out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| number_leaves(x, &if prefix.is_empty() { k.clone() } else { format!("{prefix}/{k}") }, out)),
        _ => {}
    }
}

/// The application chrome (`chrome` in tokens.json): every number equals its constant and neither
/// side has a path the other lacks; every type role matches; every named color is a token.
#[test]
fn the_chrome_matches_tokens_json_both_ways() {
    let v = json();
    let chrome = at(&v, "chrome");
    let mut leaves = Vec::new();
    for (k, x) in chrome.as_object().unwrap().iter().filter(|(k, _)| k.as_str() != "type") {
        number_leaves(x, k, &mut leaves);
    }
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    let in_rust: BTreeSet<&str> = tokens::chrome::METRICS.iter().map(|(p, _)| *p).collect();
    assert_eq!(in_rust.len(), tokens::chrome::METRICS.len(), "a chrome path repeats in Rust");
    assert_eq!(in_json, in_rust, "chrome paths differ between tokens.json and Rust");
    for (path, x) in tokens::chrome::METRICS {
        assert_eq!(num(chrome, path), *x, "chrome/{path}");
    }
    let obj = at(chrome, "type").as_object().unwrap();
    let names: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
    assert_eq!(names, tokens::chrome::TYPE_ROLES.iter().map(|(n, _)| *n).collect::<BTreeSet<_>>(), "chrome type roles differ");
    for (name, r) in tokens::chrome::TYPE_ROLES {
        let j = &obj[*name];
        assert_eq!(j["font"].as_str(), Some(r.family.key()), "{name}: family");
        assert_eq!(j["weight"].as_u64(), Some(u64::from(r.weight)), "{name}: weight");
        assert_eq!(j["size"].as_f64().map(|x| x as f32), Some(r.size), "{name}: size");
        assert_eq!(j["color"].as_str(), r.ink.map(Ink::key), "{name}: color");
    }
    // A color a chrome rule names is a token of the color tables, or a component literal
    // registered under its path.
    let known: BTreeSet<String> = ["color", "arrangement"]
        .iter()
        .flat_map(|k| at(&v, k).as_object().unwrap().keys().cloned())
        .chain(tokens::component::ALL.iter().map(|(n, _)| n.to_string()))
        .collect();
    fn strings(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
        match value {
            Value::String(s) => out.push((path.to_string(), s.clone())),
            Value::Object(o) => o.iter().for_each(|(k, x)| strings(x, &format!("{path}.{k}"), out)),
            _ => {}
        }
    }
    let mut named = Vec::new();
    strings(chrome, "chrome", &mut named);
    for (path, s) in named.iter().filter(|(p, _)| p != "chrome.note" && !p.starts_with("chrome.type.")) {
        if s.starts_with('#') {
            let c = tokens::component::ALL.iter().find(|(n, _)| n == path).unwrap_or_else(|| panic!("{path}: literal {s} without a component path"));
            assert_eq!(parse(s), c.1, "{path}");
        } else {
            assert!(known.contains(s), "{path}: {s} is not a token");
        }
    }
}
