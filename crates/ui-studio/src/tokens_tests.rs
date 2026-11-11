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
    let in_json: BTreeSet<&str> = obj.iter().filter(|(_, x)| x.is_string()).map(|(k, _)| k.as_str()).collect();
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

/// The names under `key` whose value is a color string (the arrangement group also holds metrics).
fn color_names(v: &Value, key: &str) -> Vec<String> {
    at(v, key).as_object().unwrap().iter().filter(|(_, x)| x.is_string()).map(|(k, _)| k.clone()).collect()
}

#[test]
fn the_loop_strip_height_matches_tokens_json() {
    assert_eq!(num(&json(), "arrangement/loopStripHeight"), tokens::arrangement::LOOP_STRIP_HEIGHT);
    assert_eq!(tokens::arrangement::LOOP_STRIP_HEIGHT, 14.0);
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
    let dark: BTreeSet<String> =
        ["color", "arrangement"].iter().flat_map(|k| color_names(&v, k)).chain(tokens::component::ALL.iter().map(|(n, _)| n.to_string())).collect();
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
    assert_eq!(num(&v, "layout/denseControlHeight"), l::DENSE_CONTROL_HEIGHT);
    assert_eq!(num(&v, "layout/chromeControlHeight"), l::CHROME_CONTROL_HEIGHT);
    assert_eq!(num(&v, "layout/listRowHeight"), l::LIST_ROW_HEIGHT);
    assert_eq!(at(&v, "layout").as_object().unwrap().len(), 11, "a layout metric without a Rust constant");
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
        ("fader/meter/peakAlphaWithoutRms", f::METER_PEAK_ALPHA_WITHOUT_RMS),
        ("fader/meter/fillRadius", f::METER_FILL_RADIUS),
        ("fader/meter/wellRadius", f::METER_WELL_RADIUS),
        ("fader/fill/fromDb", f::FILL_FROM_DB),
        ("fader/unityNotch/width", f::UNITY_NOTCH[0]),
        ("fader/unityNotch/height", f::UNITY_NOTCH[1]),
    ] {
        assert_eq!(num(&v, path), x, "{path}");
    }
    let taper: Vec<(Option<f32>, f32)> =
        at(&v, "fader/taper").as_array().unwrap().iter().map(|p| (p[0].as_f64().map(|d| d as f32), p[1].as_f64().unwrap() as f32)).collect();
    assert_eq!(taper, f::TAPER);
    let labels: Vec<Option<f32>> = at(&v, "fader/scaleLabels").as_array().unwrap().iter().map(|d| d.as_f64().map(|d| d as f32)).collect();
    assert_eq!(labels, f::SCALE_LABELS);
}

/// The fader's construction keys (the Fader construction board: label priority and gap, the
/// frame, the Channel columns, the tick, the 0 dB grid line) equal their mirrors, and the `fader`
/// object's keys and `tokens::fader::KEYS` are the same set, both ways.
#[test]
fn the_fader_construction_keys_match_tokens_json_both_ways() {
    use tokens::fader as f;
    let v = json();
    let fd = at(&v, "fader");
    let in_json: BTreeSet<&str> = fd.as_object().unwrap().keys().map(String::as_str).collect();
    let in_rust: BTreeSet<&str> = f::KEYS.iter().copied().collect();
    assert_eq!(in_rust.len(), f::KEYS.len(), "a fader key repeats in Rust");
    assert_eq!(in_json, in_rust, "fader keys differ between tokens.json and Rust");
    let marks = |path: &str| -> Vec<Option<f32>> { at(fd, path).as_array().unwrap().iter().map(|d| d.as_f64().map(|d| d as f32)).collect() };
    assert_eq!(marks("labelPriority"), f::LABEL_PRIORITY);
    let order: Vec<&str> = at(fd, "meterDrawOrder").as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(order, f::METER_DRAW_ORDER, "fader/meterDrawOrder");
    let pair = |path: &str| [num(fd, &format!("{path}/0")), num(fd, &format!("{path}/1"))];
    for (path, x) in [
        ("labelMinGap", f::LABEL_MIN_GAP),
        ("frame/inset", f::FRAME_INSET),
        ("frame/clipCellGap", f::FRAME_CLIP_CELL_GAP),
        ("channelColumns/labelRight", f::COLUMN_LABEL_RIGHT),
        ("channelColumns/meterX", f::COLUMN_METER_X),
        ("channelColumns/blockWidth", f::BLOCK_WIDTH),
        ("channelColumns/mainBlockWidth", f::MAIN_BLOCK_WIDTH),
        ("tick/length", f::TICK_LENGTH),
        ("tick/gapAfterLabel", f::TICK_GAP_AFTER_LABEL),
    ] {
        assert_eq!(num(fd, path), x, "fader/{path}");
    }
    assert_eq!(pair("channelColumns/fader"), f::COLUMN_FADER);
    assert_eq!(pair("channelColumns/tick"), f::COLUMN_TICK);
    assert_eq!(at(fd, "frame/sharedAcrossRow").as_bool(), Some(f::FRAME_SHARED_ACROSS_ROW));
    for (path, name) in [
        ("tick/color", f::TICK_COLOR),
        ("tick/zeroColor", f::TICK_ZERO_COLOR),
        ("zeroGridLine", f::ZERO_GRID_LINE),
        ("meterGrid/at", f::METER_GRID_AT),
        ("meterGrid/color", f::METER_GRID_COLOR),
        ("meterGrid/zeroColor", f::METER_GRID_ZERO_COLOR),
        ("mainAddFader", f::MAIN_ADD_FADER),
        ("unityNotch/color", f::UNITY_NOTCH_COLOR),
    ] {
        assert_eq!(at(fd, path).as_str(), Some(name), "fader/{path}");
    }
    // Every number under the new keys has a mirror above (a new leaf in tokens.json fails here).
    let mut leaves = Vec::new();
    for key in ["labelMinGap", "frame", "channelColumns", "tick", "unityNotch"] {
        number_leaves(at(fd, key), key, &mut leaves);
    }
    let mirrored: BTreeSet<&str> = [
        "labelMinGap",
        "frame/inset",
        "frame/clipCellGap",
        "channelColumns/fader/0",
        "channelColumns/fader/1",
        "channelColumns/labelRight",
        "channelColumns/tick/0",
        "channelColumns/tick/1",
        "channelColumns/meterX",
        "channelColumns/blockWidth",
        "channelColumns/mainBlockWidth",
        "tick/length",
        "tick/gapAfterLabel",
        "unityNotch/width",
        "unityNotch/height",
    ]
    .into_iter()
    .collect();
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(in_json, mirrored, "fader construction numbers differ between tokens.json and Rust");
    // The construction agrees with itself: the frame inset is the track inset, the columns are the
    // Channel variant's width, the tick is its length after the label's gap.
    assert_eq!(f::FRAME_INSET, f::INSET);
    assert_eq!(f::BLOCK_WIDTH, f::CHANNEL_WIDTH);
    assert_eq!(f::COLUMN_TICK, [f::COLUMN_LABEL_RIGHT + f::TICK_GAP_AFTER_LABEL, f::COLUMN_LABEL_RIGHT + f::TICK_GAP_AFTER_LABEL + f::TICK_LENGTH]);
    let names: BTreeSet<String> = color_names(&v, "color").into_iter().collect();
    assert_eq!(f::METER_GRID_ZERO_COLOR, f::ZERO_GRID_LINE, "fader/meterGrid/zeroColor is fader/zeroGridLine");
    for name in [f::TICK_COLOR, f::TICK_ZERO_COLOR, f::ZERO_GRID_LINE, f::METER_GRID_COLOR, f::METER_GRID_ZERO_COLOR, f::UNITY_NOTCH_COLOR] {
        assert!(names.contains(name), "{name} is not a color token");
    }
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
        ("cursor", t.cursor, old(0xe8e8ea), old_bg, t.bg),
        ("selection_outline", t.selection_outline, old(0xf4f4f6), old_bg, t.bg),
    ] {
        let (was, is) = (contrast(old_c, ground_old), contrast(new, ground_new));
        assert!((was - is).abs() <= 0.15 * (was - 1.0).max(0.05), "{name}: {was:.3} on the old ground, {is:.3} on the new");
    }
}

/// The light set is the maintainer's designed set (#326; the drift test pins every value to
/// `colorLight`). Its stated targets hold: text roles at 4.5:1 or more on the panel and the inset
/// well, marks at 3:1 or more on the panel, the raised surface and the knob cap white, the inset well
/// darker than the panel so it still recesses.
#[test]
fn the_light_set_meets_its_designed_targets() {
    let l: &Tokens = &LIGHT;
    for (name, ink) in [("text", l.text), ("text.muted", l.text_muted), ("text.faint", l.text_faint)] {
        for (ground, g) in [("panel", l.surface_panel), ("inset", l.surface_inset)] {
            assert!(contrast(ink, g) >= 4.5, "{name} on {ground}: {}", contrast(ink, g));
        }
    }
    for (name, mark) in [("signal", l.signal), ("mod", l.modulation), ("loop", l.loop_region), ("meter.ok", l.meter_ok), ("meter.hot", l.meter_hot)] {
        assert!(contrast(mark, l.surface_panel) >= 3.0, "{name} on the panel: {}", contrast(mark, l.surface_panel));
    }
    assert_eq!((l.surface_raised, l.knob_cap), (Color32::WHITE, Color32::WHITE));
    let lum = crate::theme::luminance;
    assert!(lum(l.surface_inset) < lum(l.surface_panel), "wells recess");
    // Negative controls: the targets discriminate. ink.ghost (a secondary trace, not a mark) is
    // under 3:1 on the panel, ink.arrow (not a text role) under 4.5:1; and mod is the designed
    // orange, not the derived brown it replaces.
    assert!(contrast(l.ink_ghost, l.surface_panel) < 3.0 && contrast(l.ink_arrow, l.surface_panel) < 4.5);
    assert_ne!(l.modulation, tokens::hex(0x633E08));
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
    let known: BTreeSet<String> =
        ["color", "arrangement"].iter().flat_map(|k| color_names(&v, k)).chain(tokens::component::ALL.iter().map(|(n, _)| n.to_string())).collect();
    fn strings(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
        match value {
            Value::String(s) => out.push((path.to_string(), s.clone())),
            Value::Object(o) => o.iter().for_each(|(k, x)| strings(x, &format!("{path}.{k}"), out)),
            _ => {}
        }
    }
    let mut named = Vec::new();
    strings(chrome, "chrome", &mut named);
    // The icons a rule names are Lucide names, mirrored both ways; the patterns' colors too.
    let icons: BTreeSet<(String, &str)> =
        named.iter().filter(|(p, _)| p.ends_with(".icon")).map(|(p, s)| (p.trim_start_matches("chrome.").replace('.', "/"), s.as_str())).collect();
    let rust_icons: BTreeSet<(String, &str)> = tokens::chrome::ICONS.iter().map(|(p, s)| (p.to_string(), *s)).collect();
    assert_eq!(icons, rust_icons, "chrome icons differ between tokens.json and Rust");
    for (path, name) in tokens::chrome::PATTERN_COLORS {
        assert_eq!(at(chrome, path).as_str(), Some(*name), "chrome/{path}");
    }
    // A behavior rule written as a word, not a color: mirrored, checked here.
    assert_eq!(at(chrome, "popup/checkGutterWhen").as_str(), Some(tokens::chrome::CHECK_GUTTER_WHEN));
    for (path, s) in named
        .iter()
        .filter(|(p, _)| p != "chrome.note" && p != "chrome.popup.checkGutterWhen" && !p.starts_with("chrome.type.") && !p.ends_with(".icon"))
    {
        if s.starts_with('#') {
            let c = tokens::component::ALL.iter().find(|(n, _)| n == path).unwrap_or_else(|| panic!("{path}: literal {s} without a component path"));
            assert_eq!(parse(s), c.1, "{path}");
        } else {
            assert!(known.contains(s), "{path}: {s} is not a token");
        }
    }
}

/// The hatch (`hatch` in tokens.json): every number equals its constant, both ways; its colors
/// are tokens.
#[test]
fn the_hatch_matches_tokens_json_both_ways() {
    let v = json();
    let hatch = at(&v, "hatch");
    let mut leaves = Vec::new();
    number_leaves(hatch, "", &mut leaves);
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    let in_rust: BTreeSet<&str> = tokens::hatch::METRICS.iter().map(|(p, _)| *p).collect();
    assert_eq!(in_json, in_rust, "hatch paths differ between tokens.json and Rust");
    for (path, x) in tokens::hatch::METRICS {
        assert_eq!(num(hatch, path), *x, "hatch/{path}");
    }
    assert_eq!(at(hatch, "color").as_str(), Some("text"));
    assert_eq!(at(hatch, "ghostClip/border").as_str(), Some("ink.ghost"));
    assert_eq!(at(hatch, "anchor").as_str(), Some("window"));
}

/// The content palette (`palette` in tokens.json): the names, every bar, body and strip value of
/// both themes, the inks and the OKLCH figures, both ways.
#[test]
fn the_palette_matches_tokens_json_both_ways() {
    let v = json();
    let p = at(&v, "palette");
    let names: Vec<&str> = at(p, "names").as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    assert_eq!(names, tokens::palette::NAMES);
    use tokens::palette as pl;
    for (path, rust) in [
        ("dark/bar", pl::DARK_BAR),
        ("dark/body", pl::DARK_BODY),
        ("dark/strip", pl::DARK_STRIP),
        ("light/bar", pl::LIGHT_BAR),
        ("light/body", pl::LIGHT_BODY),
        ("light/strip", pl::LIGHT_STRIP),
    ] {
        let json: Vec<Color32> = at(p, path).as_array().unwrap().iter().map(|h| parse(h.as_str().unwrap())).collect();
        assert_eq!(json, rust, "palette/{path}");
    }
    assert_eq!((parse(at(p, "ink/dark").as_str().unwrap()), parse(at(p, "ink/light").as_str().unwrap())), (pl::INK_DARK, pl::INK_LIGHT));
    let mut leaves = Vec::new();
    number_leaves(at(p, "oklch"), "", &mut leaves);
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    let in_rust: BTreeSet<&str> = pl::OKLCH.iter().map(|(p, _)| *p).collect();
    assert_eq!(in_json, in_rust, "palette.oklch paths differ");
    for (path, x) in pl::OKLCH {
        assert_eq!(num(at(p, "oklch"), path), *x, "palette/oklch/{path}");
    }
    // Negative control: the palette is keyed by theme, so the two sets differ.
    assert_ne!(pl::DARK_BAR, pl::LIGHT_BAR);
}

/// The pad (`pad` in tokens.json): every number both ways; the colors it names are tokens.
#[test]
fn the_pad_matches_tokens_json_both_ways() {
    let v = json();
    let pad = at(&v, "pad");
    let mut leaves = Vec::new();
    number_leaves(pad, "", &mut leaves);
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    let in_rust: BTreeSet<&str> = tokens::pad::METRICS.iter().map(|(p, _)| *p).collect();
    assert_eq!(in_json, in_rust, "pad paths differ between tokens.json and Rust");
    for (path, x) in tokens::pad::METRICS {
        assert_eq!(num(pad, path), *x, "pad/{path}");
    }
    let known: BTreeSet<String> = ["color", "arrangement"].iter().flat_map(|k| color_names(&v, k)).collect();
    for path in [
        "fill",
        "border",
        "hit/color",
        "selected/border",
        "id/color",
        "chokeBadge/border",
        "chokeBadge/text",
        "chokeBadge/chokedFill",
        "chokeBadge/chokedText",
        "peer/border",
        "bankPeerDot/color",
        "chip/mute/fill",
        "chip/mute/text",
        "chip/solo/fill",
        "chip/solo/text",
        "empty/border",
    ] {
        let name = at(pad, path).as_str().unwrap();
        assert!(known.contains(name), "pad/{path}: {name} is not a token");
    }
}

/// The dark loop hue's contrast as the design README's loop row states it (#326): 5.1 on the
/// window, 4.9 on a lane, 5.35 on the key rows (the inset well), and `ink.dark` 5.1 on it.
#[test]
fn the_loop_row_contrast_figures_hold() {
    let d: &Tokens = &DARK;
    let r = |a, b| (contrast(a, b) * 100.0).round() / 100.0;
    assert_eq!(d.loop_region, tokens::hex(0xD05DAD));
    assert_eq!(r(d.loop_region, d.bg_window), 5.10, "on the window");
    assert_eq!((r(d.loop_region, d.lane) * 10.0).round() / 10.0, 4.9, "on a lane");
    assert_eq!(r(d.loop_region, d.surface_inset), 5.35, "on the key rows");
    assert_eq!((r(d.ink_dark, d.loop_region) * 10.0).round() / 10.0, 5.1, "ink.dark on the hue");
    // Negative control: the old amber's figure on the window was 8.9, not the README's 5.1.
    assert_ne!(r(tokens::hex(0xE6AB27), d.bg_window), 5.10);
}

/// The light meter.ok (#1A7943, the maintainer's value) carries a letter at 4.5:1; the canvas's
/// first value, #1E8A4C, did not.
#[test]
fn the_light_meter_ok_carries_a_letter() {
    crate::theme::set_current(egui::Theme::Light);
    let ok = contrast(crate::theme::text_on(LIGHT.meter_ok), LIGHT.meter_ok);
    let old = tokens::hex(0x1E8A4C);
    let was = contrast(crate::theme::text_on(old), old);
    crate::theme::set_current(egui::Theme::Dark);
    assert!(ok >= 4.5, "{ok}");
    assert!(was < 4.5, "negative control: #1E8A4C {was}");
    assert!(contrast(LIGHT.meter_ok, LIGHT.surface_panel) >= 3.0, "the bar still reads on the panel");
}

/// The channel strip (`channelStrip` in tokens.json): every number both ways; the colors it names
/// are tokens.
#[test]
fn the_channel_strip_matches_tokens_json_both_ways() {
    let v = json();
    let cs = at(&v, "channelStrip");
    let mut leaves = Vec::new();
    number_leaves(cs, "", &mut leaves);
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    let in_rust: BTreeSet<&str> = tokens::channel_strip::METRICS.iter().map(|(p, _)| *p).collect();
    assert_eq!(in_json, in_rust, "channelStrip paths differ between tokens.json and Rust");
    for (path, x) in tokens::channel_strip::METRICS {
        assert_eq!(num(cs, path), *x, "channelStrip/{path}");
    }
    assert_eq!(at(cs, "sendBarAnchor").as_str(), Some(tokens::channel_strip::SEND_BAR_ANCHOR));
    let known: BTreeSet<String> = ["color", "arrangement"].iter().flat_map(|k| color_names(&v, k)).collect();
    for path in [
        "selectedBorder",
        "insertSlot/fill",
        "insertSlot/border",
        "insertSlot/on/glyph",
        "insertSlot/on/name",
        "insertSlot/bypassed/glyph",
        "insertSlot/bypassed/name",
        "overflow/hiddenCountFont/color",
        "unresponsive/color",
        "unresponsive/ring",
    ] {
        let name = at(cs, path).as_str().unwrap();
        assert!(known.contains(name), "channelStrip/{path}: {name} is not a token");
    }
}

/// The compressor displays (`compressor` in tokens.json): every number equals its constant, neither
/// side has a path the other lacks, and every named color is a token.
#[test]
fn the_compressor_displays_match_tokens_json_both_ways() {
    let v = json();
    let cp = at(&v, "compressor");
    let mut leaves = Vec::new();
    number_leaves(cp, "", &mut leaves);
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    let in_rust: BTreeSet<&str> = tokens::compressor::METRICS.iter().map(|(p, _)| *p).collect();
    assert_eq!(in_json, in_rust, "compressor paths differ between tokens.json and Rust");
    for (path, x) in tokens::compressor::METRICS {
        assert_eq!(num(cp, path), *x, "compressor/{path}");
    }
    let known: BTreeSet<String> = ["color", "arrangement"].iter().flat_map(|k| color_names(&v, k)).collect();
    for path in [
        "unity/color",
        "operatingPoint/fill",
        "operatingPoint/ring",
        "history/input/fill",
        "history/output/stroke",
        "history/gainReduction/fill",
        "history/gainReduction/stroke",
        "history/threshold/color",
        "history/secondLines",
        "history/quarterLines",
    ] {
        let name = at(cp, path).as_str().unwrap();
        assert!(known.contains(name), "compressor/{path}: {name} is not a token");
    }
    // The strings (color names, labels) both ways, and the one flag.
    let mut strings = Vec::new();
    string_leaves(cp, "", &mut strings);
    let in_json: BTreeSet<(&str, &str)> = strings.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect();
    let in_rust: BTreeSet<(&str, &str)> = tokens::compressor::STRINGS.iter().copied().collect();
    assert_eq!(in_json, in_rust, "compressor strings differ between tokens.json and Rust");
    assert_eq!(at(cp, "history/gainReduction/fromTop").as_bool(), Some(tokens::compressor::HISTORY_GR_FROM_TOP));
}

/// The Sampler views (`sampler` in tokens.json): every number and string equals its constant,
/// neither side has a path the other lacks, and every named color is a token.
#[test]
fn the_sampler_views_match_tokens_json_both_ways() {
    let v = json();
    let sp = at(&v, "sampler");
    let mut leaves = Vec::new();
    number_leaves(sp, "", &mut leaves);
    let in_json: BTreeSet<&str> = leaves.iter().map(|(p, _)| p.as_str()).collect();
    let in_rust: BTreeSet<&str> = tokens::sampler::METRICS.iter().chain(tokens::sampler::INSTRUMENT_METRICS).map(|(p, _)| *p).collect();
    assert_eq!(in_json, in_rust, "sampler paths differ between tokens.json and Rust");
    for (path, x) in tokens::sampler::METRICS.iter().chain(tokens::sampler::INSTRUMENT_METRICS) {
        assert_eq!(num(sp, path), *x, "sampler/{path}");
    }
    let known: BTreeSet<String> = ["color", "arrangement"].iter().flat_map(|k| color_names(&v, k)).collect();
    for path in [
        "keyMap/zoneFill",
        "keyMap/zoneBorder",
        "keyMap/selectedBorder",
        "keyMap/padZoneBorder",
        "keyMap/whiteKey",
        "keyMap/padKey",
        "keyMap/blackKey",
        "waveform/region",
        "waveform/outside",
        "waveform/handle",
        "waveform/playhead",
        "bench/wave",
        "bench/end",
        "bench/autoLine",
        "bench/manualLine",
        "bench/ghost/line",
        "bench/ghost/refused",
        "bench/sliceLoop/color",
        "bench/flagStripLine",
        "bench/autoFlag/fill",
        "bench/autoFlag/ink",
        "bench/autoFlag/edge",
        "bench/manualFlag/fill",
        "bench/manualFlag/ink",
        "bench/manualFlag/edge",
        "bench/selectedFlag/fill",
        "bench/selectedFlag/ink",
        "bench/selectedFlag/edge",
        "bench/selectedLine",
        "bench/endFlag/fill",
        "bench/endFlag/ink",
        "bench/handleLine",
        "bench/handleFill",
        "bench/midline",
        "tag/ink",
        "tag/edge",
        "candidate/line",
        "lockGlyph/color",
        "seed/sequential",
        "instrument/keyMap/keyEdge",
        "instrument/keyMap/blackKeyEdge",
        "instrument/keyMap/octaveNameInk",
        "instrument/keyMap/zoneInk",
        "instrument/keyMap/selectedInk",
        "instrument/keyMap/rootDotInk",
        "instrument/keyMap/selectedRootDotInk",
        "instrument/keyMap/rrTagEdge",
        "instrument/keyMap/rrTagInk",
        "instrument/keyMap/padZoneFill",
        "instrument/keyMap/padZoneInk",
        "instrument/keyMap/padZoneDot",
        "instrument/keyMap/conflictEdge",
        "instrument/waveform/flagInk",
        "instrument/waveform/braceInk",
        "instrument/groupPanel/chipFill",
        "instrument/groupPanel/chipEdge",
        "instrument/groupPanel/addChipEdge",
        "instrument/groupPanel/addChipInk",
        "instrument/groupPanel/divider",
    ] {
        let name = at(sp, path).as_str().unwrap();
        assert!(known.contains(name), "sampler/{path}: {name} is not a token");
    }
    // The flag and the exact seed range (an f32 cannot hold 4 294 967 295).
    assert_eq!(at(sp, "candidate/flag").as_bool(), Some(tokens::sampler::CANDIDATE_FLAG));
    assert_eq!(at(sp, "seed/min").as_u64(), Some(u64::from(tokens::sampler::SEED_MIN)));
    assert_eq!(at(sp, "seed/max").as_u64(), Some(u64::from(tokens::sampler::SEED_MAX)));
    let mut strings = Vec::new();
    string_leaves(sp, "", &mut strings);
    let in_json: BTreeSet<(&str, &str)> = strings.iter().map(|(p, s)| (p.as_str(), s.as_str())).collect();
    let in_rust: BTreeSet<(&str, &str)> = tokens::sampler::STRINGS.iter().chain(tokens::sampler::INSTRUMENT_STRINGS).copied().collect();
    assert_eq!(in_json, in_rust, "sampler strings differ between tokens.json and Rust");
}

/// Every string leaf under `value` by its path (`a/b/0`).
fn string_leaves(value: &Value, prefix: &str, out: &mut Vec<(String, String)>) {
    let join = |k: &str| if prefix.is_empty() { k.to_string() } else { format!("{prefix}/{k}") };
    match value {
        Value::String(s) => out.push((prefix.to_string(), s.clone())),
        Value::Array(a) => a.iter().enumerate().for_each(|(i, x)| string_leaves(x, &join(&i.to_string()), out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| string_leaves(x, &join(k), out)),
        _ => {}
    }
}
