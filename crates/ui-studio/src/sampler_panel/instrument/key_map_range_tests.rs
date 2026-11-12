//! The key map's whole keyboard (#492): it shows keys 0 to 127 whatever the zones cover, an edit
//! never changes the keys shown, every key fits the well from the window's minimum width up at
//! both scales, and a zone's side edges keep their hold on the narrowest keys.

use super::super::test_rig::T;
use super::key_map::{self, EDGE, MIN_WHITE_KEY, is_black};
use super::rig::*;
use crate::tokens::sampler as tk;
use serde_json::json;
use transit_model::TrackId;

/// Zone 1 over keys 36 to 84 rooted at 60, zone 2 on white key 91 alone, zone 3 on black key 97
/// alone; the window at `width` (its content width) and `scale`, the Instrument view showing.
fn spread(width: f32, scale: f32) -> (T, TrackId) {
    let (mut t, tr) = T::open();
    t.screen = egui::vec2(width + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    t.ctx.set_pixels_per_point(scale);
    zone(&mut t, tr, 1, "Wide.wav", 4_800, json!({"keys": [36, 84], "root": 60}));
    zone(&mut t, tr, 2, "White.wav", 4_800, json!({"keys": [91, 91], "root": 91}));
    zone(&mut t, tr, 3, "Black.wav", 4_800, json!({"keys": [97, 97], "root": 97}));
    show_instrument(&mut t, tr);
    t.frames(4);
    (t, tr)
}

/// Zone `n`'s side edge (`high` or low), `inset` inside it, at the middle of its height.
fn edge_at(t: &T, tr: TrackId, n: u16, high: bool, inset: f32) -> (f32, f32) {
    let r = layout(t, tr).key_map.zone(n).unwrap().rect;
    (if high { r[2] - inset } else { r[0] + inset }, (r[1] + r[3]) / 2.0)
}

#[test]
fn a_narrowed_zone_keeps_the_whole_keyboard_and_its_edge_drags_back_out_and_beyond() {
    let (mut t, tr) = spread(super::super::DESIGN_WIDTH, 1.0);
    let shown = |t: &T| {
        let k = layout(t, tr).key_map;
        let notes: Vec<u8> = k.keys.iter().map(|x| x.note).collect();
        (k.lo, k.hi, notes.len(), notes.contains(&0), notes.contains(&127))
    };
    assert_eq!(shown(&t), (0, 127, 128, true, true));
    // Narrow zone 1 to 36..48.
    let from = edge_at(&t, tr, 1, true, 2.0);
    let to = (key_x(&t, tr, 48), from.1);
    drag(&mut t, from, to);
    assert_eq!(stored(&t, tr, 1).0, [36, 48]);
    assert_eq!(shown(&t), (0, 127, 128, true, true), "an edit never changes the keys shown");
    // Back out to its old key, then beyond it.
    let from = edge_at(&t, tr, 1, true, 2.0);
    let to = (key_x(&t, tr, 84), from.1);
    drag(&mut t, from, to);
    assert_eq!(stored(&t, tr, 1).0, [36, 84], "back to its old key");
    let from = edge_at(&t, tr, 1, true, 2.0);
    let to = (key_x(&t, tr, 88), from.1);
    drag(&mut t, from, to);
    assert_eq!(stored(&t, tr, 1).0, [36, 88], "and past it");
    // The low edge reaches key 0, past every key the zones cover.
    let from = edge_at(&t, tr, 1, false, 1.0);
    let to = (key_x(&t, tr, 0), from.1);
    drag(&mut t, from, to);
    assert_eq!(stored(&t, tr, 1).0, [0, 88], "to the keyboard's first key");
    assert_eq!(shown(&t), (0, 127, 128, true, true));
}

#[test]
fn every_key_fits_the_well_and_the_edges_keep_their_hold_at_both_widths_and_scales() {
    let mut whites = Vec::new();
    for width in [super::super::MIN_WIDTH, super::super::DESIGN_WIDTH] {
        for scale in [1.0, 1.5] {
            let (t, tr) = spread(width, scale);
            let w = t.window(tr).rect;
            let k = layout(&t, tr).key_map;
            let at = format!("width {width} scale {scale}");
            assert_eq!(h(w[2] - w[0]), h(width), "{at}: the window at the width asked for");
            let ww = k.frame.1 / 75.0;
            eprintln!(
                "{at}: well inside {:.2} wide, white key {ww:.2} pt ({:.2} px), black key {:.2} pt",
                k.frame.1,
                ww * scale,
                ww * tk::BLACK_KEY_WIDTH
            );
            whites.push(ww);
            assert!(ww >= MIN_WHITE_KEY, "{at}: a white key {ww} is at least {MIN_WHITE_KEY}");
            // Every key inside the keyboard, the keyboard across the well's inside.
            assert_eq!(k.keys.len(), 128, "{at}");
            for x in &k.keys {
                assert!(
                    x.rect[0] >= k.keyboard[0] - 0.01 && x.rect[2] <= k.keyboard[2] + 0.01,
                    "{at}: key {} {:?} in {:?}",
                    x.note,
                    x.rect,
                    k.keyboard
                );
            }
            assert!(k.keyboard[0] >= k.well[0] && k.keyboard[2] <= k.well[2], "{at}");
            // Every C named, inside the keyboard, no name meeting the next.
            let names: Vec<[f32; 4]> = k.keys.iter().filter_map(|x| x.name.as_ref().map(|n| n.1)).collect();
            assert_eq!(names.len(), 11, "{at}: C-2 (or C-1) to C8");
            for n in &names {
                assert!(n[0] >= k.keyboard[0] - 0.01 && n[2] <= k.keyboard[2] + 0.01, "{at}: name {n:?} in {:?}", k.keyboard);
            }
            for p in names.windows(2) {
                assert!(p[0][2] < p[1][0], "{at}: names {:?} and {:?} apart", p[0], p[1]);
            }
            // A one-white-key zone holds each side edge EDGE deep, apart from the other.
            let z2 = k.zone(2).unwrap();
            let r = crate::sampler_panel::grid_rect(z2.rect);
            let dot = crate::sampler_panel::grid_rect(z2.dot);
            let y = r.min.y + EDGE + 1.0;
            assert!(r.width() >= 2.0 * EDGE, "{at}: a one-white-key zone {} holds both edges", r.width());
            let part = |x: f32| key_map::part_at(r, dot, egui::pos2(x, y));
            assert_eq!(part(r.min.x + EDGE - 0.05), Some(super::key_map_gestures::Part::Low), "{at}");
            assert_eq!(part(r.max.x - EDGE + 0.05), Some(super::key_map_gestures::Part::High), "{at}");
        }
    }
    eprintln!("white keys: {whites:?}");
}

#[test]
fn a_one_black_key_zone_drags_by_both_side_edges() {
    let (mut t, tr) = spread(super::super::MIN_WIDTH, 1.5);
    let r = layout(&t, tr).key_map.zone(3).unwrap().rect;
    assert!(is_black(97) && r[2] - r[0] < 2.0 * EDGE, "narrower than two edge holds: {r:?}");
    let from = edge_at(&t, tr, 3, true, 0.5);
    let to = (key_x(&t, tr, 100), from.1);
    drag(&mut t, from, to);
    assert_eq!(stored(&t, tr, 3).0, [97, 100], "the high edge holds on one black key");
    let (mut t, tr) = spread(super::super::MIN_WIDTH, 1.5);
    let from = edge_at(&t, tr, 3, false, 0.5);
    let to = (key_x(&t, tr, 94), from.1);
    drag(&mut t, from, to);
    assert_eq!(stored(&t, tr, 3).0, [94, 97], "and the low edge");
}
