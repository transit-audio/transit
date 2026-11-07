//! The Pads view's layout: the grid (4 by 4, 56 px pads, 4 px gaps, pad 1 at the bottom left), the
//! bank buttons and their peer dots, the status line, the selected-pad strip, and the window that
//! keeps its size across banks and re-fits when the strip's peers line grows. Each with a negative
//! control.

use super::test_rig::{T, mid};
use crate::theme::TOKENS;
use crate::widgets::test_support::inside;
use serde_json::json;

#[test]
fn the_grid_is_four_by_four_with_pad_one_at_the_bottom_left() {
    let (t, tr) = T::open();
    let p = t.panel(tr);
    assert_eq!(p.pads.len(), 16);
    let r = |i: u8| p.pad(i).unwrap().pad.rect;
    for i in 0..16u8 {
        let x = r(i);
        assert_eq!((x[2] - x[0], x[3] - x[1]), (56.0, 56.0), "pad {i}");
        assert!(inside(x, p.grid) && inside(p.grid, p.panel("pads").unwrap()));
    }
    // MPC order: 1 (index 0) bottom left, 4 bottom right, 13 top left, 16 top right.
    assert_eq!((r(0)[0], r(0)[3]), (p.grid[0], p.grid[3]));
    assert_eq!((r(3)[2], r(3)[3]), (p.grid[2], p.grid[3]));
    assert_eq!((r(12)[0], r(12)[1]), (p.grid[0], p.grid[1]));
    assert_eq!((r(15)[2], r(15)[1]), (p.grid[2], p.grid[1]));
    // 4 px gaps across and up.
    assert_eq!(r(1)[0] - r(0)[2], 4.0);
    assert_eq!(r(0)[1] - r(4)[3], 4.0);
    assert_eq!(p.grid[2] - p.grid[0], 236.0);
    // Ids are bank and slot; every pad of a new kit is empty.
    assert_eq!(p.pad(5).unwrap().pad.id, "A6");
    assert!(p.pads.iter().all(|r| r.pad.empty && r.pad.drop_label.is_some()));
    assert_eq!(p.status, "Bank A · 0 of 16 loaded");
    assert!(p.status_rect[1] >= p.grid[3] + 10.0 - 0.01);
    // Negative control: pad 1 is not at the top.
    assert_ne!(r(0)[1], p.grid[1]);
}

#[test]
fn four_bank_buttons_switch_the_grid_and_the_window_keeps_its_size() {
    let (mut t, tr) = T::kit();
    let before = t.panel(tr);
    let doc = t.patch(tr);
    let banks = before.banks.clone().expect("the bank buttons");
    assert_eq!(banks.segments.len(), 4, "banks A to D (the model's four)");
    assert_eq!(banks.selected, 0);
    assert!(banks.segments.iter().all(|s| s[2] - s[0] >= 34.0 - 0.01), "at least the board's 34");
    assert_eq!(before.status, "Bank A · 3 of 16 loaded");
    t.click(mid(banks.segments[1]), 1);
    let p = t.panel(tr);
    assert_eq!(p.bank, 1);
    assert_eq!(p.pads.first().map(|r| r.index), Some(16), "bank B starts at pad 17");
    assert_eq!(p.pad(16).unwrap().pad.id, "B1");
    assert_eq!(p.status, "Bank B · 0 of 16 loaded");
    // The same content height: the window keeps its place and size.
    assert_eq!(p.window, before.window);
    // The selection stays where it was (A1), and a bank is view state, not the document.
    assert_eq!(p.selected, 0);
    assert_eq!(t.patch(tr), doc);
    // Negative control: bank A again shows A1.
    t.click(mid(banks.segments[0]), 1);
    assert_eq!(t.panel(tr).pads.first().map(|r| r.index), Some(0));
}

#[test]
fn a_bank_holding_a_peer_of_the_selection_gets_a_dot() {
    let (mut t, tr) = T::kit();
    // B1 holds a sample in group 1 with A2 and A3.
    let p = t.sample("Hat FX.wav", 2400);
    t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": 4, "pad": 17, "path": p.to_string_lossy()}));
    for pad in [2, 3, 17] {
        t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": pad, "choke": 1}));
    }
    // Negative control: with A1 (no group) selected there is no dot and no peer outline.
    let p = t.panel(tr);
    assert!(p.bank_dots.is_empty());
    assert!(p.pads.iter().all(|r| r.pad.peer.is_none()));
    t.click(t.pad_at(tr, 1), 1);
    let p = t.panel(tr);
    assert_eq!(p.selected, 1);
    assert_eq!(p.bank_dots.len(), 1, "bank B holds B1");
    let b = p.banks.clone().unwrap().segments[1];
    assert!(inside(p.bank_dots[0], b));
    assert_eq!(p.bank_dots[0][2] - p.bank_dots[0][0], 5.0);
    // A3 is outlined as A2's peer in this bank; A2 itself is selected, A1 is not a peer.
    assert!(p.pad(2).unwrap().pad.peer.is_some());
    assert!(p.pad(1).unwrap().pad.selected && p.pad(1).unwrap().pad.peer.is_none());
    assert!(p.pad(0).unwrap().pad.peer.is_none());
    assert_eq!(p.pad(1).unwrap().pad.badge_text, "1");
    assert_eq!(p.strip.peers, "Chokes with A3 OH Hat, B1 Hat FX.");
    assert_eq!(p.strip.choke_value, "Group 1 · 3 pads");
    // A used group's segment reads brighter.
    assert!(p.strip.segments[1].used && p.strip.segments[1].selected);
    assert!(!p.strip.segments[2].used && p.strip.segments[2].ink == TOKENS.ink_arrow);
}

#[test]
fn the_strip_names_the_pad_and_lays_out_the_picker_and_toggles() {
    let (mut t, tr) = T::kit();
    t.click(t.pad_at(tr, 2), 1);
    let p = t.panel(tr);
    let s = &p.strip;
    let panel = p.panel("strip").unwrap();
    assert_eq!(s.name, "OH Hat");
    assert_eq!(s.id_line, "Pad A3 · note 62");
    assert_eq!(s.choke_value, "Off");
    assert!(s.peers.starts_with("Not in a choke group."));
    // The picker: Off and 1 to 16 in nine columns, two rows, inside the panel.
    assert_eq!(s.segments.len(), 17);
    assert_eq!(s.segments[0].text, "Off");
    assert!(s.segments[0].selected && s.segments[0].ink == TOKENS.text);
    assert_eq!(s.segments[9].rect[1], s.segments[0].rect[3] + 3.0, "group 9 starts the second row, 3 below");
    assert_eq!(s.segments[8].rect[1], s.segments[0].rect[1], "group 8 ends the first row");
    assert!(s.segments.iter().all(|g| inside(g.rect, s.picker) && (g.rect[3] - g.rect[1] - 22.0).abs() < 0.02));
    assert!(inside(s.picker, panel));
    // Unused groups are ink.arrow (none is in use yet).
    assert!(s.segments[1..].iter().all(|g| !g.used && g.ink == TOKENS.ink_arrow));
    // M and S after the label column, the hint after them, all inside the panel.
    let (m, so) = (s.mute.clone().unwrap(), s.solo.clone().unwrap());
    assert_eq!((m.text.as_str(), so.text.as_str()), ("M", "S"));
    assert_eq!(m.rect[2] - m.rect[0], 28.0);
    assert!(m.rect[0] >= s.pad_label[2] && so.rect[0] > m.rect[2] && s.hint_rect[0] > so.rect[2]);
    assert!(inside(s.hint_rect, panel) && inside(s.peers_rect, panel) && inside(s.name_rect, panel));
    // Negative control: an empty pad reads "Empty pad".
    t.click(t.pad_at(tr, 7), 1);
    assert_eq!(t.panel(tr).strip.name, "Empty pad");
    assert_eq!(t.panel(tr).strip.id_line, "Pad A8 · note 67");
}

#[test]
fn the_window_refits_when_the_peers_line_grows() {
    let (mut t, tr) = T::kit();
    t.screen = egui::vec2(1400.0, 1400.0);
    t.frames(3);
    // Twelve pads in group 2 with long names: A1's peers line wraps over several lines.
    for i in 4..16u8 {
        let p = t.sample(&format!("A very long sample name made for this test, number {i}.wav"), 480);
        t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": i + 1, "pad": i + 1, "path": p.to_string_lossy()}));
        t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": i + 1, "choke": 2}));
    }
    t.run("mix.sampler_pad_set", json!({"track": tr.0, "pad": 1, "choke": 2}));
    // Pad A2 (no group) selected: one line.
    t.click(t.pad_at(tr, 1), 1);
    let one = t.panel(tr);
    t.click(t.pad_at(tr, 0), 1);
    let many = t.panel(tr);
    let h = |r: [f32; 4]| r[3] - r[1];
    assert!(h(many.strip.peers_rect) > h(one.strip.peers_rect), "the peers line wraps: {}", many.strip.peers);
    // The window grew by what the hero row grew, its top left kept.
    let hero = |p: &super::SamplerLayout| h(p.panel("pads").unwrap());
    assert!(hero(&many) > hero(&one));
    assert_eq!((many.window[0], many.window[1]), (one.window[0], one.window[1]));
    assert!((h(many.window) - h(one.window) - (hero(&many) - hero(&one))).abs() < 0.02);
    // Negative control: re-selecting the one-line pad shrinks it back.
    t.click(t.pad_at(tr, 1), 1);
    assert_eq!(t.panel(tr).window, one.window);
}
