//! The Group panel through the panel's rig: its geometry against the board, the chips, Make group,
//! the add chip, round robin, the seed and the velocity fader.

use super::super::test_rig::{T, mid};
use super::group_panel::{self, GroupPanelLayout};
use super::rig::*;
use crate::tokens::{layout as space, sampler as tk};
use serde_json::json;

fn group(t: &T, tr: transit_model::TrackId) -> GroupPanelLayout {
    layout(t, tr).group_panel.expect("the Group panel")
}

#[test]
fn group_panel_rects_match_the_board_at_both_scales() {
    for scale in [1.0, 1.5] {
        let (mut t, tr) = grouped();
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let l = layout(&t, tr);
        let g = group(&t, tr);
        eprintln!(
            "scale {scale}: panel {:?} label {:?} summary {:?} rows {:?} divider {:?} rr {:?} seed {:?} reroll {:?} fader {:?} note {:?}",
            g.panel,
            g.label.1,
            g.summary.1,
            g.layers.iter().map(|r| (r.row, r.chips.iter().map(|c| c.rect).collect::<Vec<_>>(), r.add)).collect::<Vec<_>>(),
            g.divider,
            g.rr.as_ref().map(|s| s.track),
            g.seed,
            g.reroll,
            g.fader,
            g.note.as_ref().map(|n| n.1)
        );
        let (pv, ph) = (space::PANEL_PADDING[0], space::PANEL_PADDING[1]);
        let gap = tk::GROUP_ROW_GAP;
        assert_eq!(g.panel, l.group, "the card in the zone row's second column");
        let x0 = g.panel[0] + ph;
        assert_eq!(h(g.label.1[0]), h(x0));
        assert_eq!(h(g.summary.1[0] - g.label.1[2]), h(gap), "the summary a gap after the label");
        // Two layers, the higher first: 28 high, 4 apart; the label column 58 and 6 to the chips.
        assert_eq!(g.layers.len(), 2);
        for (i, r) in g.layers.iter().enumerate() {
            assert_eq!((h(r.row[0]), h(r.row[3] - r.row[1])), (h(x0), h(tk::LAYER_HEIGHT)));
            assert_eq!(h(r.label.1[0]), h(x0));
            let first = r.chips[0].rect;
            assert_eq!(h(first[0]), h(x0 + tk::LAYER_LABEL_WIDTH + tk::LAYER_ITEM_GAP));
            for c in &r.chips {
                assert_eq!(h(c.rect[3] - c.rect[1]), h(tk::CHIP), "22 high");
                assert!(h(c.rect[2] - c.rect[0]) >= h(tk::CHIP_MIN_WIDTH), "at least 40 wide");
                assert_eq!(h((c.rect[1] + c.rect[3]) / 2.0), h((r.row[1] + r.row[3]) / 2.0), "centered in the row");
            }
            for pair in r.chips.windows(2) {
                assert_eq!(h(pair[1].rect[0] - pair[0].rect[2]), h(tk::LAYER_ITEM_GAP));
            }
            assert_eq!(h(r.add[0] - r.chips.last().unwrap().rect[2]), h(tk::LAYER_ITEM_GAP), "the add chip after them");
            assert_eq!(h(r.add[3] - r.add[1]), h(tk::CHIP));
            if i > 0 {
                assert_eq!(h(r.row[1] - g.layers[i - 1].row[3]), h(tk::LAYER_GAP));
            }
        }
        let m = super::super::measurer(&t.ctx);
        let header = super::super::line_h(&m, &crate::tokens::LABEL_SECTION).max(super::super::line_h(&m, &crate::tokens::READOUT_DISPLAY));
        assert_eq!(h(g.layers[0].row[1]), h(g.panel[1] + pv + header + gap), "the rows a gap under the header");
        // The divider a gap under the rows, 1 high across the inside; each control row a gap under
        // the one before, at the dense height, its control after the 84 label column and a gap.
        let last = g.layers.last().unwrap().row;
        assert_eq!((h(g.divider[1] - last[3]), h(g.divider[3] - g.divider[1])), (h(gap), h(1.0)));
        assert_eq!((h(g.divider[0]), h(g.divider[2])), (h(x0), h(g.panel[2] - ph)));
        let cx = x0 + tk::GROUP_LABEL_WIDTH + gap;
        let rr = g.rr.clone().unwrap().track;
        assert_eq!((h(rr[0]), h(rr[1] - g.divider[3])), (h(cx), h(gap)));
        assert_eq!((h(g.seed[0]), h(g.seed[1] - rr[3])), (h(cx), h(gap)));
        assert_eq!((h(g.seed[2] - g.seed[0]), h(g.seed[3] - g.seed[1])), (h(tk::SEED_WIDTH), h(space::DENSE_CONTROL_HEIGHT)));
        assert_eq!((h(g.reroll[0] - g.seed[2]), h(g.reroll[2] - g.reroll[0])), (h(gap), h(space::DENSE_CONTROL_HEIGHT)));
        assert_eq!((h(g.fader[0]), h(g.fader[2] - g.fader[0])), (h(cx), h(tk::GROUP_FADER_LENGTH)));
        let vel_row_top = g.seed[3] + gap;
        assert_eq!(h((g.fader[1] + g.fader[3]) / 2.0), h(vel_row_top + space::DENSE_CONTROL_HEIGHT / 2.0));
        let note = g.note.clone().unwrap().1;
        assert_eq!(h(note[1]), h(vel_row_top + space::DENSE_CONTROL_HEIGHT + gap), "the closing line a gap under");
        assert_eq!(h(g.panel[3]), h(note[3] + pv), "the padding under the closing line (the taller card)");
        let m = super::super::measurer(&t.ctx);
        assert_eq!(h(g.panel[3] - g.panel[1]), h(group_panel::height(&m, g.panel[2] - g.panel[0], 2, None)));
    }
}

#[test]
fn the_group_reads_its_layers_and_a_chip_selects_its_zone() {
    let (mut t, tr) = grouped();
    let mc = t.app.ui.middle_c;
    let n = |k: u8| transit_midi::note_name(k, mc);
    let g = group(&t, tr);
    assert_eq!(g.label.0, format!("Group {}", n(50)));
    assert_eq!(g.summary.0, format!("{}–{} · 2 of 8 layers", n(48), n(52)));
    assert_eq!(g.layers.iter().map(|r| r.label.0.clone()).collect::<Vec<_>>(), vec!["64–127", "1–63"]);
    let chips: Vec<Vec<(String, bool)>> = g.layers.iter().map(|r| r.chips.iter().map(|c| (c.text.clone(), c.selected)).collect()).collect();
    assert_eq!(chips, vec![vec![("rr1".to_string(), false)], vec![("rr1".to_string(), true), ("rr2".to_string(), false)]]);
    let undo = t.undo_len();
    let rr2 = g.layers[1].chips[1].rect;
    t.click(mid(rr2), 1);
    assert_eq!(view(&t).selected, Some(2));
    let c = group(&t, tr).layers[1].chips[1].clone();
    assert!(c.selected && c.edge_width == tk::SELECTED_BORDER_WIDTH, "the 2 px signal edge");
    assert_eq!(t.undo_len(), undo, "selecting changes nothing");
}

#[test]
fn making_a_group_from_a_pad_zone_waits_for_make_group() {
    let (mut t, tr) = grouped();
    let z4 = layout(&t, tr).key_map.zone(4).unwrap().rect;
    t.click(mid(z4), 1);
    assert_eq!(view(&t).selected, Some(4));
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let add = group(&t, tr).layers[0].add;
    t.click(mid(add), 1);
    let notice = group(&t, tr).notice.expect("the Notice");
    assert_eq!(notice.text, "Pad A1's zone becomes a group and leaves pad A1, which will be empty. Its notes still play the group.");
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0), "nothing until Make group");
    // Cancel changes nothing.
    t.click(mid(notice.cancel), 1);
    assert!(group(&t, tr).notice.is_none());
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0));
    // Make group: one step that unbinds the pad and makes the zone a group on the pad's note.
    t.click(mid(add), 1);
    let make = group(&t, tr).notice.unwrap().make;
    t.click(mid(make), 1);
    assert_eq!(t.undo_len(), undo + 1);
    let s = sent(&t, log);
    assert!(s.len() == 1 && s[0].starts_with("mix.sampler_zone_map ") && s[0].contains("\"pad\":null"), "{s:?}");
    let p = t.patch(tr);
    let z = p.zone(3).unwrap();
    let note = 36;
    assert_eq!((z.keys, z.velocities), ([note, note], [1, 127]), "its notes still play it");
    assert_eq!(z.membership.as_ref().map(|m| (m.group, m.layer, m.alternative)), Some((1, 0, 0)), "the lowest free group");
    assert_eq!(p.pad(0).zone, None, "pad A1 is empty");
    assert!(group(&t, tr).notice.is_none());
}

#[test]
fn an_alt_drag_on_a_pad_zone_asks_and_changes_nothing() {
    let (mut t, tr) = grouped();
    let z4 = layout(&t, tr).key_map.zone(4).unwrap().rect;
    let from = mid(z4);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.send("ui.drag", json!({"x": from.0, "y": from.1, "to_x": from.0 + 60.0, "to_y": from.1, "alt": true}));
    assert_eq!(view(&t).selected, Some(4));
    assert!(group(&t, tr).notice.is_some(), "the Notice asks");
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0));
}

#[test]
fn the_add_chip_loads_a_sample_as_the_layers_next_alternative() {
    let (mut t, tr) = grouped();
    let path = t.sample("G5.wav", 4_800);
    let p = path.to_string_lossy().to_string();
    t.app.services.pick_open = Some(Box::new(move |purpose, _| {
        assert_eq!(purpose, "mix.sampler_zone_source");
        Some(p.clone())
    }));
    let undo = t.undo_len();
    // Layer 0 (velocities 1 to 63) holds zones 1 and 2: the next is alternative 2, in zone 5.
    let add = group(&t, tr).layers[1].add;
    t.click(mid(add), 1);
    assert_eq!(t.undo_len(), undo + 1);
    let pa = t.patch(tr);
    let z = pa.zone(4).expect("zone 5");
    assert_eq!((z.keys, z.velocities, z.root), ([48, 52], [1, 63], 50));
    assert_eq!(z.membership.as_ref().map(|m| (m.group, m.layer, m.alternative)), Some((0, 0, 2)));
    assert_eq!(view(&t).selected, Some(5), "the new alternative is selected");
    assert_eq!(group(&t, tr).layers[1].chips.len(), 3);
}

#[test]
fn the_add_chip_on_a_zone_in_no_group_makes_it_a_group() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let g = group(&t, tr);
    assert!(g.summary.0.ends_with("· no group"));
    t.click(mid(g.layers[0].add), 1);
    let s = sent(&t, log);
    assert!(s.len() == 1 && s[0].starts_with("mix.sampler_zone_map ") && !s[0].contains("pad"), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(t.patch(tr).zone(0).unwrap().membership.as_ref().map(|m| (m.group, m.layer, m.alternative)), Some((0, 0, 0)));
}

#[test]
fn seed_field_normalizes_zero_to_one_and_rejects_out_of_range() {
    let (mut t, tr) = grouped();
    t.run("mix.sampler_group_set", json!({"track": tr.0, "group": 0, "round_robin": "random"}));
    let typ = |t: &mut T, text: &str| {
        let f = group(t, tr).seed;
        t.click(mid(f), 1);
        t.send("ui.key", json!({"key": "A", "cmd": true}));
        t.send("ui.text", json!({"text": text}));
        t.key("Enter");
    };
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    typ(&mut t, "0");
    assert_eq!(sent(&t, log), vec!["mix.sampler_seed {\"seed\":0}".to_string()], "the value as typed: the engine normalizes");
    assert_eq!((t.patch(tr).seed, group(&t, tr).seed_text.as_str()), (1, "1"), "0 becomes 1");
    assert_eq!(t.undo_len(), undo + 1);
    typ(&mut t, "4294967296");
    assert_eq!(layout(&t, tr).note.expect("the refusal").0, "seed must be an integer from 0 to 4294967295 (0 is stored as 1)");
    assert_eq!((t.patch(tr).seed, group(&t, tr).seed_text.as_str()), (1, "1"));
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn seed_disabled_while_sequential_and_stored_seed_kept() {
    let (mut t, tr) = grouped();
    t.run("mix.sampler_seed", json!({"track": tr.0, "seed": 77}));
    let g = group(&t, tr);
    assert_eq!((g.seed_text.as_str(), g.seed_live), ("—", false), "Sequential: the dash, disabled");
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(g.seed), 1);
    t.click(mid(g.reroll), 1);
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0), "inert while Sequential");
    // Random: one group_set; the stored seed shows and the reroll draws a new one.
    t.click(mid(g.rr.unwrap().segments[1]), 1);
    assert_eq!(sent(&t, log), vec!["mix.sampler_group_set {\"group\":0,\"round_robin\":\"random\"}".to_string()]);
    let g = group(&t, tr);
    assert_eq!((g.seed_text.as_str(), g.seed_live), ("77", true));
    t.click(mid(g.reroll), 1);
    let seed = t.patch(tr).seed;
    assert!(seed != 77 && seed >= 1, "a fresh seed: {seed}");
    assert_eq!(t.undo_len(), undo + 2);
    // Back to Sequential: the dash again, the stored seed kept.
    t.click(mid(group(&t, tr).rr.unwrap().segments[0]), 1);
    assert_eq!(group(&t, tr).seed_text, "—");
    assert_eq!(t.patch(tr).seed, seed);
}

#[test]
fn round_robin_and_seed_are_inert_for_a_zone_in_no_group() {
    let (t, tr) = layered();
    let g = group(&t, tr);
    assert_eq!(g.rr_opacity, tk::UNUSED_CONTROL_ALPHA);
    assert!(!g.seed_live);
}

#[test]
fn the_velocity_fader_sends_one_edit_on_release() {
    let (mut t, tr) = grouped();
    let g = group(&t, tr);
    assert_eq!(g.vel_text, "70%", "the default");
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let from = mid(g.cap);
    let to = ((g.fader[0] + g.fader[2]) / 2.0, from.1);
    steps(&mut t, vec![moved(from), press(from, true), moved((from.0 - 10.0, from.1)), moved((to.0 + 5.0, to.1)), moved(to), press(to, false)]);
    assert_eq!(sent(&t, log), vec!["mix.sampler_zone_edit {\"playback\":{\"velocity_sensitivity\":0.5},\"zone\":1}".to_string()]);
    assert_eq!(t.patch(tr).zone(0).unwrap().velocity_sensitivity, 0.5);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(group(&t, tr).vel_text, "50%");
}
