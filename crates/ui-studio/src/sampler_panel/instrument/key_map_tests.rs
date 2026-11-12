//! The key map through the panel's rig: its geometry against the board, the three drags as one
//! step each, the pad zone's lock, the conflict preview, Escape, the reads and the colors.

use super::super::test_rig::{T, mid};
use super::key_map::{self, is_black};
use super::rig::*;
use crate::tokens::{SPACE, chrome, layout as space, sampler as tk};
use serde_json::json;

#[test]
fn key_map_rects_match_the_board_at_both_scales() {
    for scale in [1.0, 1.5] {
        let (mut t, tr) = layered();
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let p = t.panel(tr);
        let k = layout(&t, tr).key_map;
        eprintln!(
            "scale {scale}: panel {:?} label {:?} hint {:?} axis {:?} well {:?} keyboard {:?} zones {:?}",
            k.panel,
            k.label,
            k.hint,
            k.axis,
            k.well,
            k.keyboard,
            k.zones.iter().map(|z| (z.zone, z.rect, z.dot)).collect::<Vec<_>>()
        );
        let (pv, ph) = (space::PANEL_PADDING[0], space::PANEL_PADDING[1]);
        // The panel one panel gap under the bar, across it.
        assert_eq!(h(k.panel[1] - p.bar.rect[3]), h(space::PANEL_GAP));
        assert_eq!((h(k.panel[0]), h(k.panel[2])), (h(p.bar.rect[0]), h(p.bar.rect[2])));
        // The header: the label at the padding, the hint one item gap after it.
        assert_eq!((h(k.label[0]), h(k.label[1]) >= h(k.panel[1] + pv)), (h(k.panel[0] + ph), true));
        assert_eq!(h(k.hint[0] - k.label[2]), h(SPACE[2]));
        // The axis 26 wide at the padding; the well 6 after it to the padding, 196 high, 8 under the
        // header; the keyboard 4 under the well, 52 high, across the well's inside.
        let header = k.well[1] - SPACE[1];
        assert!(h(k.label[3]) <= h(header) && h(k.hint[3]) <= h(header));
        assert_eq!((h(k.axis[0]), h(k.axis[2] - k.axis[0])), (h(k.panel[0] + ph), h(tk::AXIS_WIDTH)));
        assert_eq!((h(k.axis[1]), h(k.axis[3])), (h(k.well[1]), h(k.well[3])));
        assert_eq!(h(k.well[0] - k.axis[2]), h(tk::AXIS_GAP));
        assert_eq!(h(k.well[2]), h(k.panel[2] - ph));
        assert_eq!(h(k.well[3] - k.well[1]), h(tk::KEY_MAP_WELL_HEIGHT));
        let e = chrome::FIELD_EDGE_WIDTH;
        assert_eq!(h(k.keyboard[1] - k.well[3]), h(tk::KEYBOARD_GAP));
        assert_eq!(h(k.keyboard[3] - k.keyboard[1]), h(tk::KEYBOARD_HEIGHT));
        assert_eq!((h(k.keyboard[0]), h(k.keyboard[2])), (h(k.well[0] + e), h(k.well[2] - e)));
        assert_eq!(h(k.panel[3]), h(k.keyboard[3] + pv), "the panel's padding under the keyboard");
        // The axis labels right-aligned 2 inside the axis: 127 at the top, 64 on its line, 1 at the foot.
        let (top, bottom) = (k.well[1] + e, k.well[3] - e);
        let y64 = bottom - 64.0 / 127.0 * (bottom - top);
        for (text, r) in &k.axis_labels {
            assert_eq!(h(r[2]), h(k.axis[2] - tk::AXIS_INSET), "{text}");
        }
        assert_eq!(h(k.axis_labels[0].1[1]), h(top));
        assert_eq!(h((k.axis_labels[1].1[1] + k.axis_labels[1].1[3]) / 2.0), h(y64));
        assert_eq!(h(k.axis_labels[2].1[3]), h(bottom));
        assert_eq!(h(k.mid_line[1]), h(y64.round()));
        // The whole keyboard, keys 0 to 127 (#492): equal white keys across the inside, black keys
        // 0.6 of one centered on the boundary, 32 high; octave lines at the inner Cs.
        assert_eq!((k.lo, k.hi), (0, 127));
        // The drawn keys are the formula's spans across the well's inside.
        assert_eq!((h(k.frame.0), h(k.frame.0 + k.frame.1)), (h(k.well[0] + e), h(k.well[2] - e)));
        let keys = key_map::Keys::new(0, 127, k.frame.0, k.frame.1);
        let q = |v: f32| (v * 100.0).round() / 100.0;
        for x in &k.keys {
            let (a, w) = keys.span(x.note);
            assert_eq!((x.rect[0], x.rect[2]), (q(a), q(a + w)), "key {}", x.note);
            let tall = if is_black(x.note) { tk::BLACK_KEY_HEIGHT } else { tk::KEYBOARD_HEIGHT };
            assert_eq!((h(x.rect[1]), h(x.rect[3] - x.rect[1])), (h(k.keyboard[1]), h(tall)), "key {}", x.note);
        }
        let whites: Vec<_> = k.keys.iter().filter(|x| !is_black(x.note)).collect();
        assert_eq!(whites.len(), 75);
        let ww = k.frame.1 / 75.0;
        for (i, n) in (0..=127u8).filter(|n| !is_black(*n)).enumerate() {
            assert_eq!(keys.span(n), (k.frame.0 + i as f32 * ww, ww), "white {n}: equal keys from the left");
        }
        for n in (0..=127u8).filter(|n| is_black(*n)) {
            let (a, w) = keys.span(n);
            assert_eq!(w, ww * tk::BLACK_KEY_WIDTH, "black {n}");
            assert_eq!(h(a + w / 2.0), h(keys.span(n + 1).0), "black {n} centered on the boundary");
        }
        let c48 = whites.iter().find(|w| w.note == 48).unwrap();
        let inner_cs: Vec<i32> = whites.iter().filter(|w| w.note % 12 == 0 && w.note > 0).map(|w| h(w.rect[0])).collect();
        assert_eq!(k.octave_lines.iter().map(|x| h(*x)).collect::<Vec<_>>(), inner_cs);
        assert_eq!(h(c48.name.as_ref().unwrap().1[3]), h(c48.rect[3] - tk::OCTAVE_NAME_INSET));
        // Zones: one inside each side of the keys and velocities they cover; label 4 and 3 in; the
        // 7 px dot 4 above the foot under the root key.
        let vy = |v: f32| bottom - v / 127.0 * (bottom - top);
        let z1 = k.zone(1).unwrap();
        let z = tk::ZONE_INSET;
        assert_eq!(
            [h(z1.rect[0]), h(z1.rect[1]), h(z1.rect[2]), h(z1.rect[3])],
            [h(keys.span(48).0 + z), h(vy(63.0) + z), h(keys.span(52).0 + keys.span(52).1 - z), h(vy(0.0) - z)]
        );
        let [lx, ly] = tk::ZONE_LABEL_INSET;
        assert_eq!((h(z1.label_rect[0]), h(z1.label_rect[1])), (h(z1.rect[0] + lx), h(z1.rect[1] + ly)));
        assert_eq!((h(z1.dot[2] - z1.dot[0]), h(z1.dot[3] - z1.dot[1])), (h(tk::ROOT_DOT), h(tk::ROOT_DOT)));
        assert_eq!(h(z1.rect[3] - z1.dot[3]), h(tk::ROOT_DOT_INSET));
        assert_eq!(h((z1.dot[0] + z1.dot[2]) / 2.0), h(keys.span(50).0 + keys.span(50).1 / 2.0));
        let z2 = k.zone(2).unwrap();
        assert_eq!(h(z2.rect[1]), h(vy(127.0) + z));
        // The pad zone: one key (the pad's note) at full velocity.
        let z3 = k.zone(3).unwrap();
        assert_eq!([h(z3.rect[0]), h(z3.rect[1]), h(z3.rect[3])], [h(keys.span(36).0 + z), h(top + z), h(bottom - z)]);
        assert_eq!(h(k.panel[3] - k.panel[1]), h(key_map::height(&super::super::measurer(&t.ctx))));
    }
}

#[test]
fn zones_read_their_root_and_label_and_the_selected_zone_takes_the_signal_edge() {
    let (t, tr) = layered();
    let k = layout(&t, tr).key_map;
    let mc = t.app.ui.middle_c;
    let n50 = transit_midi::note_name(50, mc);
    assert_eq!(k.zone(1).unwrap().label, n50);
    assert_eq!(k.zone(2).unwrap().label, format!("{n50} ff"));
    assert_eq!(k.zone(3).unwrap().label, "A1");
    let z1 = k.zone(1).unwrap();
    assert!(z1.selected, "the lowest zone is selected first");
    assert_eq!((z1.edge, z1.edge_width), (crate::theme::TOKENS.signal, tk::SELECTED_BORDER_WIDTH));
    assert_eq!(k.zones.last().unwrap().zone, 1, "the selected zone draws last");
    let z2 = k.zone(2).unwrap();
    assert_eq!((z2.fill, z2.edge), (crate::theme::TOKENS.surface_raised, crate::theme::TOKENS.line_strong));
}

/// Zone 1's right edge, at the middle of its height.
fn right_edge(t: &T, tr: transit_model::TrackId) -> (f32, f32) {
    let r = layout(t, tr).key_map.zone(1).unwrap().rect;
    (r[2] - 2.0, (r[1] + r[3]) / 2.0)
}

#[test]
fn key_range_drag_is_one_undo_step() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let from = right_edge(&t, tr);
    let to = (key_x(&t, tr, 55), from.1);
    drag(&mut t, from, to);
    assert_eq!(stored(&t, tr, 1).0, [48, 55], "the high key follows the pointer");
    assert_eq!(stored(&t, tr, 2).0, [48, 52], "the other layer stays (no group)");
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "one command for the drag: {s:?}");
    assert!(
        s[0].starts_with("mix.sampler_zone_map ") && s[0].contains("\"zone\":1") && !s[0].contains("fades") && !s[0].contains("\"pad\""),
        "{s:?}"
    );
    assert_eq!(t.undo_len(), undo + 1, "one undo step");
}

#[test]
fn velocity_split_drag_is_one_undo_step() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let k = layout(&t, tr).key_map;
    let r = k.zone(1).unwrap().rect;
    let from = ((r[0] + r[2]) / 2.0 - 10.0, r[1] + 2.0);
    let (top, bottom) = (k.well[1] + 1.0, k.well[3] - 1.0);
    let y = bottom - 40.0 / 127.0 * (bottom - top);
    drag(&mut t, from, (from.0, y));
    assert_eq!(stored(&t, tr, 1).1, [1, 40], "the top edge sets the highest velocity");
    assert_eq!(sent(&t, log).len(), 1);
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn root_dot_drag_is_one_undo_step() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let dot = mid(layout(&t, tr).key_map.zone(1).unwrap().dot);
    // Through two other keys first, so a command per pointer move would show.
    let (k52, k53, to) = (key_x(&t, tr, 52), key_x(&t, tr, 53), (key_x(&t, tr, 49), dot.1));
    steps(&mut t, vec![moved(dot), press(dot, true), moved((k52, dot.1)), moved((k53, dot.1)), moved(to), press(to, false)]);
    assert_eq!(stored(&t, tr, 1).2, 49);
    assert_eq!(sent(&t, log).len(), 1);
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn pad_bound_zone_edges_do_not_drag_and_show_the_lock() {
    let (mut t, tr) = layered();
    let k = layout(&t, tr).key_map;
    let z3 = k.zone(3).unwrap().clone();
    assert!(z3.dashed && z3.edge == super::color(tk::PAD_ZONE_BORDER), "a dashed ink.arrow edge");
    let lock = z3.lock.expect("the lock by the label");
    assert_eq!(h(lock[2] - lock[0]), h(tk::LOCK_GLYPH_SIZE));
    assert!(lock[0] >= z3.rect[0] && lock[1] >= z3.label_rect[3], "by the label: {lock:?} {:?}", z3.label_rect);
    let state = view(&t).snapshot.data.unwrap();
    let tip = key_map::pad_tip(&state, 1, t.app.ui.middle_c);
    assert!(tip.contains("Pad A1") && tip.contains("do not drag"), "{tip}");
    let (undo, log, before) = (t.undo_len(), view(&t).sent.len(), stored(&t, tr, 3));
    for from in [
        (z3.rect[2] - 1.0, (z3.rect[1] + z3.rect[3]) / 2.0),
        (z3.rect[0] + 1.0, (z3.rect[1] + z3.rect[3]) / 2.0),
        (mid(z3.rect).0, z3.rect[1] + 1.0),
        mid(z3.dot),
    ] {
        drag(&mut t, from, (from.0 + 80.0, from.1 + 30.0));
    }
    assert_eq!(stored(&t, tr, 3), before);
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0), "nothing sent");
    assert_eq!(view(&t).selected, Some(3), "a press still selects it");
}

/// Eight zones over keys 60 to 64 at every velocity and a ninth over 70 to 72.
fn crowded() -> (T, transit_model::TrackId) {
    let (mut t, tr) = T::open();
    // The ninth first: a new zone covers every key until its edit narrows it.
    zone(&mut t, tr, 9, "L9.wav", 4_800, json!({"keys": [70, 72]}));
    for n in 1..=8 {
        zone(&mut t, tr, n, &format!("L{n}.wav"), 4_800, json!({"keys": [60, 64]}));
    }
    show_instrument(&mut t, tr);
    (t, tr)
}

#[test]
fn conflicting_drag_shows_the_hot_outline_then_snaps_back_with_the_refusal() {
    let (mut t, tr) = crowded();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let r = layout(&t, tr).key_map.zone(9).unwrap().rect;
    let from = (r[0] + 1.0, (r[1] + r[3]) / 2.0);
    let to = (key_x(&t, tr, 62), from.1);
    steps(&mut t, vec![moved(from), press(from, true), moved((from.0 - 20.0, from.1)), moved(to)]);
    let (rect, hot) = layout(&t, tr).key_map.preview.expect("the drag's preview");
    let reason = hot.expect("the preview conflicts before release");
    assert!(reason.contains("ninth layer"), "{reason}");
    assert!(h(rect[0]) < h(r[0]), "drawn where the drag holds it");
    steps(&mut t, vec![press(to, false)]);
    assert_eq!(stored(&t, tr, 9).0, [70, 72], "snapped back");
    assert_eq!(t.undo_len(), undo, "no step");
    assert_eq!(sent(&t, log).len(), 1, "the engine decides on release");
    let note = layout(&t, tr).note.expect("the refusal in the note row").0;
    let mc = t.app.engine.message_middle_c();
    assert_eq!(note, format!("Zone 9 would be a ninth layer on {}.", transit_midi::note_name(62, mc)));
    assert!(layout(&t, tr).key_map.preview.is_none());
}

#[test]
fn escape_cancels_a_zone_drag_and_sends_nothing() {
    let (mut t, tr) = layered();
    let well = layout(&t, tr).key_map.well;
    t.click((well[2] - 4.0, well[1] + 4.0), 1);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let from = right_edge(&t, tr);
    let to = (key_x(&t, tr, 55), from.1);
    steps(&mut t, vec![moved(from), press(from, true), moved((from.0 + 20.0, from.1)), moved(to)]);
    assert!(view(&t).drag.is_some());
    t.key("Escape");
    steps(&mut t, vec![press(to, false)]);
    assert_eq!(stored(&t, tr, 1).0, [48, 52]);
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0));
    assert!(!t.app.builtin_editor.windows.is_empty(), "Escape canceled the drag; the window stays");
}

#[test]
fn the_state_is_read_once_per_document_change() {
    let (mut t, tr) = layered();
    let reads = |t: &T| view(t).snapshot.reads;
    let r0 = reads(&t);
    t.frames(30);
    assert_eq!(reads(&t), r0, "idle frames read nothing");
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "playback": {"root": 51}}));
    assert_eq!(reads(&t), r0 + 1, "one read for one change");
    assert_eq!(layout(&t, tr).key_map.zone(1).unwrap().label, transit_midi::note_name(51, t.app.ui.middle_c));
}

#[test]
fn every_instrument_color_name_is_a_token() {
    for (path, name) in tk::INSTRUMENT_STRINGS.iter().filter(|(p, _)| !p.ends_with("/font")) {
        assert_ne!(super::color(name), egui::Color32::MAGENTA, "{path}: {name}");
    }
    for name in [
        tk::ZONE_FILL,
        tk::ZONE_BORDER,
        tk::SELECTED_BORDER,
        tk::PAD_ZONE_BORDER,
        tk::WHITE_KEY,
        tk::PAD_KEY,
        tk::BLACK_KEY,
        tk::WAVE_REGION,
        tk::WAVE_OUTSIDE,
        tk::WAVE_HANDLE,
        tk::LOCK_GLYPH_COLOR,
    ] {
        assert_ne!(super::color(name), egui::Color32::MAGENTA, "{name}");
    }
}

#[test]
fn view_switch_keeps_selection_and_changes_no_parameter() {
    let (mut t, tr) = layered();
    let r = layout(&t, tr).key_map.zone(2).unwrap().rect;
    t.click(((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0), 1);
    assert_eq!(view(&t).selected, Some(2));
    let before = serde_json::to_value(t.app.engine.session()).unwrap();
    let undo = t.undo_len();
    let switch = t.panel(tr).bar.switch.unwrap();
    t.click(mid(switch.segments[1]), 1);
    t.click(t.pad_at(tr, 0), 1);
    let pad = t.panel(tr).selected;
    t.click(mid(switch.segments[0]), 1);
    assert_eq!(view(&t).selected, Some(2), "the Instrument view keeps its zone");
    assert!(layout(&t, tr).key_map.zone(2).unwrap().selected);
    t.click(mid(switch.segments[1]), 1);
    assert_eq!(t.panel(tr).selected, pad, "the Pads view keeps its pad");
    assert_eq!(serde_json::to_value(t.app.engine.session()).unwrap(), before, "no document field changed");
    assert_eq!(t.undo_len(), undo);
}

#[test]
fn instrument_bar_reads_the_map_and_its_actions_are_unavailable() {
    let (t, tr) = layered();
    let bar = t.panel(tr).bar;
    let mc = t.app.ui.middle_c;
    let n = |k: u8| transit_midi::note_name(k, mc);
    assert_eq!(bar.summary, format!("3 zones · {}–{} · 1 pad zone", n(48), n(52)));
    assert_eq!(bar.instrument_actions.len(), 2);
    assert!(bar.instrument_actions.iter().all(|(_, o)| *o == tk::UNUSED_CONTROL_ALPHA), "{:?}", bar.instrument_actions);
    assert_eq!(h(bar.instrument_actions[1].0[2]), h(bar.rect[2]), "right-aligned");
    assert_eq!(h(bar.instrument_actions[1].0[0] - bar.instrument_actions[0].0[2]), h(tk::VIEW_BAR_GAP));
}

#[test]
fn a_group_members_side_edge_moves_the_whole_group_in_one_command() {
    let (mut t, tr) = grouped();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let from = right_edge(&t, tr);
    let to = (key_x(&t, tr, 55), from.1);
    drag(&mut t, from, to);
    for z in 1..=3 {
        assert_eq!(stored(&t, tr, z).0, [48, 55], "zone {z}: one key range per group");
    }
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "{s:?}");
    for z in 1..=3 {
        assert!(s[0].contains(&format!("\"zone\":{z}")), "zone {z} listed: {s:?}");
    }
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn a_layers_top_edge_moves_all_its_alternatives_in_one_command() {
    let (mut t, tr) = grouped();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let k = layout(&t, tr).key_map;
    let r = k.zone(1).unwrap().rect;
    let from = ((r[0] + r[2]) / 2.0 - 10.0, r[1] + 2.0);
    let (top, bottom) = (k.well[1] + 1.0, k.well[3] - 1.0);
    drag(&mut t, from, (from.0, bottom - 40.0 / 127.0 * (bottom - top)));
    assert_eq!((stored(&t, tr, 1).1, stored(&t, tr, 2).1), ([1, 40], [1, 40]), "the layer's alternatives together");
    assert_eq!(stored(&t, tr, 3).1, [64, 127], "the other layer stays");
    let s = sent(&t, log);
    assert!(s.len() == 1 && !s[0].contains("\"zone\":3"), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1);
}
