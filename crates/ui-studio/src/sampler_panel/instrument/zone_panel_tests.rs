//! The Zone panel and its waveform through the panel's rig: the geometry against the boards, the
//! crossfade field at the source's rate, the warning, the fades, the loop segment, the handles,
//! a slice-bound zone's locks, and the shared sections.

use super::super::test_rig::{T, mid};
use super::rig::*;
use super::zone_panel;
use crate::tokens::{SPACE, layout as space, sampler as tk};
use crate::widgets::section_bar;
use serde_json::json;
use transit_model::sampler_loops::crossfade_ms_text;
use transit_model::sampler_patch::LoopMode;

#[test]
fn zone_panel_rects_match_the_board_at_both_scales() {
    for scale in [1.0, 1.5] {
        let (mut t, tr) = layered();
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let p = t.panel(tr);
        let l = layout(&t, tr);
        let z = l.zone.clone().unwrap();
        eprintln!(
            "scale {scale}: zone {:?} group {:?} label {:?} name {:?} file {:?} fields {:?} well {:?} loop {:?} crossfade {:?} sections {:?} bars {:?}",
            z.panel,
            l.group,
            z.label,
            z.name.1,
            z.file.1,
            z.fields.iter().map(|f| f.1).collect::<Vec<_>>(),
            z.wave.well,
            z.loop_mode.as_ref().map(|s| s.track),
            z.crossfade,
            l.sections,
            l.bars.iter().map(|b| b.bar).collect::<Vec<_>>()
        );
        let (pv, ph) = (space::PANEL_PADDING[0], space::PANEL_PADDING[1]);
        let w = p.bar.rect[2] - p.bar.rect[0];
        // The zone row one panel gap under the key map: Zone 1.45 parts, Group 1, a panel gap apart.
        assert_eq!(h(z.panel[1] - l.key_map.panel[3]), h(space::PANEL_GAP));
        let [a, b] = tk::ZONE_COLUMNS;
        assert_eq!((h(z.panel[0]), h(z.panel[2] - z.panel[0])), (h(p.bar.rect[0]), h(((w - space::PANEL_GAP) * a / (a + b)).round())));
        assert_eq!((h(l.group[0] - z.panel[2]), h(l.group[2])), (h(space::PANEL_GAP), h(p.bar.rect[2])));
        assert_eq!((h(l.group[1]), h(l.group[3])), (h(z.panel[1]), h(z.panel[3])));
        // Header: ZONE at the padding, the name and the file line 10 apart.
        assert_eq!(h(z.label[0]), h(z.panel[0] + ph));
        assert_eq!(h(z.name.1[0] - z.label[2]), h(tk::ZONE_HEADER_GAP));
        assert_eq!(h(z.file.1[0] - z.name.1[2]), h(tk::ZONE_HEADER_GAP));
        // The fields 14 apart from the padding; the well 10 under them, 108 high, across the inside.
        assert_eq!(h(z.fields[0].1[0]), h(z.panel[0] + ph));
        for pair in z.fields.windows(2) {
            assert_eq!(h(pair[1].1[0] - pair[0].1[2]), h(tk::ZONE_FIELD_GAP));
        }
        let wl = z.wave.well;
        assert_eq!(h(wl[1] - z.fields[0].1[3]), h(tk::ZONE_ROW_GAP));
        assert_eq!((h(wl[0]), h(wl[2]), h(wl[3] - wl[1])), (h(z.panel[0] + ph), h(z.panel[2] - ph), h(tk::ZONE_WAVE_HEIGHT)));
        // S and E: 2 px lines, 14 px flags at the foot.
        let e = crate::tokens::chrome::FIELD_EDGE_WIDTH;
        assert_eq!(h(z.wave.start_line[2] - z.wave.start_line[0]), h(tk::WAVE_HANDLE_WIDTH));
        assert_eq!((h(z.wave.start_flag[3]), h(z.wave.start_flag[3] - z.wave.start_flag[1])), (h(wl[3] - e), h(tk::WAVE_FLAG_HEIGHT)));
        assert_eq!(h(z.wave.start_flag[0]), h(z.wave.start_line[0]));
        assert_eq!(h(z.wave.end_flag[2]), h(z.wave.end_line[2]));
        assert_eq!(h(z.wave.readout.1[1]), h(wl[1] + e + tk::READOUT_TOP));
        assert_eq!(h(z.wave.readout.1[2]), h(wl[2] - e - tk::TAG_INSET));
        // The loop row 10 under the well: the 40 px label column, the segments, Crossfade 6 more
        // than a row gap after them, the 76 px field, the caption.
        let seg = z.loop_mode.clone().unwrap().track;
        assert_eq!(h(seg[1]), h(wl[3] + tk::ZONE_ROW_GAP));
        assert_eq!(h(seg[0]), h(z.panel[0] + ph + tk::LOOP_LABEL_WIDTH + tk::ZONE_ROW_GAP));
        assert_eq!(h(z.crossfade_label[0] - seg[2]), h(tk::ZONE_ROW_GAP + tk::CROSSFADE_LABEL_GAP));
        assert_eq!(h(z.crossfade[0] - z.crossfade_label[2]), h(tk::ZONE_ROW_GAP));
        assert_eq!(
            (h(z.crossfade[2] - z.crossfade[0]), h(z.crossfade[3] - z.crossfade[1])),
            (h(tk::CROSSFADE_WIDTH), h(space::DENSE_CONTROL_HEIGHT))
        );
        assert_eq!(h(z.forward_only[0] - z.crossfade[2]), h(tk::ZONE_ROW_GAP));
        assert_eq!(h(z.panel[3]), h(seg[3] + pv));
        assert_eq!(h(z.panel[3] - z.panel[1]), h(zone_panel::height(&super::super::measurer(&t.ctx), false)));
        // The sections row a panel gap under, then the two bars.
        assert_eq!((h(l.sections[1] - z.panel[3]), h(l.sections[0]), h(l.sections[2])), (h(space::PANEL_GAP), h(p.bar.rect[0]), h(p.bar.rect[2])));
        assert_eq!(l.bars.len(), 2);
        assert_eq!(h(l.bars[0].bar[1] - l.sections[3]), h(space::PANEL_GAP));
        assert_eq!(h(l.bars[1].bar[1] - l.bars[0].bar[3]), h(SPACE[1]));
        assert_eq!(h(l.bars[1].bar[3] - l.bars[1].bar[1]), h(section_bar::HEIGHT));
    }
}

/// A zone over a one-second WAV at 44.1 kHz (the session runs at 48 kHz) with a forward loop over
/// the whole of it, the Instrument view showing.
fn looped() -> (T, transit_model::TrackId) {
    let (mut t, tr) = T::open();
    let path = t.sample_at("Pad.wav", 44_100, 44_100);
    t.run("mix.sampler_zone_source", json!({"track": tr.0, "zone": 1, "path": path.to_string_lossy()}));
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "region": {"loop_mode": "forward"}}));
    show_instrument(&mut t, tr);
    (t, tr)
}

#[test]
fn crossfade_field_converts_ms_to_frames_at_the_source_rate_and_dims_under_ping_pong() {
    let (mut t, tr) = looped();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let field = layout(&t, tr).zone.unwrap().crossfade;
    t.click(mid(field), 1);
    t.send("ui.key", json!({"key": "A", "cmd": true}));
    t.send("ui.text", json!({"text": "12.0"}));
    t.key("Enter");
    // floor(12.0 * 44100 / 1000 + 1/2) = 529 source frames (576 at the session's 48 kHz).
    assert_eq!(t.patch(tr).zone(0).unwrap().crossfade, 529);
    assert_eq!(sent(&t, log), vec!["mix.sampler_zone_edit {\"region\":{\"crossfade\":529},\"zone\":1}".to_string()], "only the field it changes");
    assert_eq!(t.undo_len(), undo + 1);
    let z = layout(&t, tr).zone.unwrap();
    assert_eq!(z.crossfade_text, "12.0 ms");
    assert_eq!(z.crossfade_tip, "529 frames at 44.1 kHz");
    assert_eq!(z.crossfade_opacity, 1.0);
    assert_eq!(z.wave.brace.as_ref().unwrap().1, "loop · fwd · xf 12.0 ms");
    assert!(z.wave.hatch.is_some());
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "region": {"loop_mode": "ping_pong", "crossfade": 0}}));
    let z = layout(&t, tr).zone.unwrap();
    assert_eq!(z.crossfade_opacity, tk::UNUSED_CONTROL_ALPHA, "dimmed under Ping-pong");
    assert_eq!(z.loop_mode.unwrap().selected, 2);
}

#[test]
fn leaving_the_crossfade_field_untouched_sends_nothing() {
    let (mut t, tr) = looped();
    // 530 frames read 12.0 ms at 44.1 kHz, which would convert back to 529.
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "region": {"crossfade": 530}}));
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let z = layout(&t, tr).zone.unwrap();
    assert_eq!(z.crossfade_text, "12.0 ms");
    t.click(mid(z.crossfade), 1);
    t.click(mid(z.label), 1);
    t.click(mid(z.crossfade), 1);
    t.key("Enter");
    assert_eq!(t.patch(tr).zone(0).unwrap().crossfade, 530);
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0), "nothing typed, nothing sent");
}

#[test]
fn the_warning_says_how_much_crossfade_is_used_when_the_loop_shrank() {
    let (mut t, tr) = looped();
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "region": {"crossfade": 2000}}));
    assert!(layout(&t, tr).zone.unwrap().warning.is_none(), "all of it is used");
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "region": {"loop_start": 0, "loop_end": 1000}}));
    let z = layout(&t, tr).zone.unwrap();
    let root = transit_midi::note_name(60, t.app.ui.middle_c);
    let text = format!("Zone {root} is {} ms long now: {} ms of crossfade is used.", crossfade_ms_text(1000, 44_100), crossfade_ms_text(500, 44_100));
    assert_eq!(z.warning.expect("the warning line").0, text);
    assert_eq!(z.crossfade_text, format!("{} ms", crossfade_ms_text(2000, 44_100)), "the field keeps the stored value");
}

#[test]
fn fades_read_none_when_zero_and_the_widths_otherwise() {
    let (mut t, tr) = layered();
    let fades = |t: &T| layout(t, tr).zone.unwrap().fields[3].0.clone();
    assert_eq!(fades(&t), "fades none");
    t.run("mix.sampler_zone_edit", json!({"track": tr.0, "zone": 1, "playback": {"fades": {"key_lo": 2, "key_hi": 1, "vel_lo": 0, "vel_hi": 0}}}));
    assert_eq!(fades(&t), "fades 2 · 1 · 0 · 0");
    let mc = t.app.ui.middle_c;
    let n = |k: u8| transit_midi::note_name(k, mc);
    let f = layout(&t, tr).zone.unwrap().fields;
    assert_eq!(f[0].0, format!("keys {}–{}", n(48), n(52)));
    assert_eq!(f[1].0, "vel 1–63");
    assert_eq!(f[2].0, format!("root {}", n(50)));
}

#[test]
fn loop_mode_sends_only_the_mode_and_a_refusal_is_the_engines_text() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let forward = |t: &T| mid(layout(t, tr).zone.unwrap().loop_mode.unwrap().segments[1]);
    t.click(forward(&t), 1);
    assert_eq!(t.patch(tr).zone(0).unwrap().loop_mode, LoopMode::Forward);
    assert_eq!(sent(&t, log), vec!["mix.sampler_zone_edit {\"region\":{\"loop_mode\":\"forward\"},\"zone\":1}".to_string()]);
    assert_eq!(t.undo_len(), undo + 1);
    // The pad's zone is a one-shot: the engine refuses, in its words.
    let z3 = layout(&t, tr).key_map.zone(3).unwrap().rect;
    t.click(mid(z3), 1);
    assert_eq!(view(&t).selected, Some(3));
    t.click(forward(&t), 1);
    assert_eq!(t.patch(tr).zone(2).unwrap().loop_mode, LoopMode::Off);
    assert_eq!(t.undo_len(), undo + 1);
    assert_eq!(layout(&t, tr).note.expect("the note row").0, "A one-shot cannot loop: choose Gate, or turn the loop off.");
}

#[test]
fn a_handle_drag_sends_one_zone_edit_with_only_its_end() {
    let (mut t, tr) = layered();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let w = layout(&t, tr).zone.unwrap().wave;
    let from = mid(w.start_flag);
    let to = (from.0 + 100.0, from.1);
    drag(&mut t, from, to);
    let (left, right) = (w.well[0] + 1.0, w.well[2] - 1.0);
    let frame = (((to.0 - left) / (right - left)) as f64 * 48_000.0).round() as u64;
    assert_eq!(t.patch(tr).zone(0).unwrap().start, frame);
    assert_eq!(sent(&t, log), vec![format!("mix.sampler_zone_edit {{\"region\":{{\"start\":{frame}}},\"zone\":1}}")]);
    assert_eq!(t.undo_len(), undo + 1);
}

#[test]
fn a_slice_bound_zone_draws_its_handles_locked() {
    let (mut t, tr) = super::super::slices::rig::sliced();
    show_instrument(&mut t, tr);
    let z = layout(&t, tr).zone.expect("the zone panel");
    assert_eq!(z.wave.locks.len(), 2, "a lock by S and by E");
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let from = mid(z.wave.end_flag);
    drag(&mut t, from, (from.0 - 60.0, from.1));
    assert_eq!((t.undo_len(), sent(&t, log).len()), (undo, 0), "locked handles do not drag");
}

#[test]
fn the_sections_edit_the_selected_zone_and_unbuilt_parts_are_unavailable() {
    let (mut t, tr) = layered();
    let z2 = layout(&t, tr).key_map.zone(2).unwrap().rect;
    t.click(mid(z2), 1);
    let rows = t.window(tr).rows;
    assert!(rows.iter().any(|r| r.param == "zone002_transpose"), "the Playback knobs edit zone 2");
    assert!(!rows.iter().any(|r| r.param == "zone001_transpose"));
    let l = layout(&t, tr);
    assert_eq!((l.group_opacity, l.bar_opacity), (tk::UNUSED_CONTROL_ALPHA, tk::UNUSED_CONTROL_ALPHA));
    assert_eq!(l.bars.iter().map(|b| b.chevron_deg).collect::<Vec<_>>(), vec![-90.0, -90.0], "collapsed");
    // A click on the unbuilt parts changes nothing.
    let (undo, before) = (t.undo_len(), serde_json::to_value(t.app.engine.session()).unwrap());
    t.click(mid(l.bars[0].button), 1);
    t.click(mid(l.group), 1);
    assert_eq!((t.undo_len(), serde_json::to_value(t.app.engine.session()).unwrap()), (undo, before));
}

#[test]
fn nothing_leaves_its_panel_at_the_minimum_width() {
    let (mut t, tr) = layered();
    t.screen = egui::vec2(super::super::MIN_WIDTH + 2.0 * crate::device_window::SCREEN_MARGIN, 1600.0);
    t.frames(4);
    let p = t.panel(tr);
    assert_eq!(h(p.window[2] - p.window[0]), h(super::super::MIN_WIDTH), "laid out at the minimum");
    let l = layout(&t, tr);
    let (pv, ph) = (space::PANEL_PADDING[0], space::PANEL_PADDING[1]);
    let within = |r: [f32; 4], panel: [f32; 4], what: &str| {
        assert!(h(r[0]) >= h(panel[0] + ph) && h(r[2]) <= h(panel[2] - ph), "{what} {r:?} leaves {panel:?}");
        assert!(h(r[1]) >= h(panel[1] + pv) && h(r[3]) <= h(panel[3] - pv), "{what} {r:?} leaves {panel:?}");
    };
    let k = &l.key_map;
    for (r, what) in [(k.label, "label"), (k.hint, "hint"), (k.well, "well"), (k.keyboard, "keyboard")] {
        within(r, k.panel, what);
    }
    let z = l.zone.clone().unwrap();
    eprintln!("caption {:?} field {:?} panel {:?}", z.forward_only, z.crossfade, z.panel);
    let mut items = vec![(z.label, "label"), (z.name.1, "name"), (z.file.1, "file"), (z.wave.well, "well"), (z.loop_label, "loop")];
    items.extend([
        (z.loop_mode.clone().unwrap().track, "segments"),
        (z.crossfade_label, "crossfade label"),
        (z.crossfade, "field"),
        (z.forward_only, "caption"),
    ]);
    items.extend(z.fields.iter().map(|f| (f.1, "field text")));
    for (r, what) in items {
        within(r, z.panel, what);
    }
    assert!(h(p.bar.summary_rect[2]) <= h(p.bar.summary_slot[2]), "the summary keeps a bar gap before the actions");
}
