//! The Detect, Manual and Use slices panels and the view's states, through the panel's rig.

use super::super::test_rig::{T, mid};
use super::rig::*;
use super::{bench, detect_panel};
use crate::tokens::{layout, sampler as tk};
use serde_json::json;

/// Clicks `segment` of a segmented control's layout.
fn pick(t: &mut T, seg: &crate::widgets::segmented::SegmentedLayout, i: usize) {
    t.click(mid(seg.segments[i]), 1);
}

/// Runs frames until the debounce has passed (input time, 1/60 s a frame).
fn settle(t: &mut T) {
    t.frames((detect_panel::DEBOUNCE * 60.0).ceil() as usize + 2);
}

#[test]
fn candidate_previews_dashed_and_replace_is_one_step() {
    let (mut t, tr) = auto();
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    let d = panels(&t, tr).detect.unwrap();
    pick(&mut t, d.division.as_ref().unwrap(), 1);
    // The click's frames are under the debounce: nothing sent yet.
    assert_eq!(sent(&t, log), Vec::<String>::new());
    settle(&mut t);
    let s = sent(&t, log);
    let at = view(&t).at;
    assert_eq!(s, [format!("mix.sampler_slice_detect {{\"at\":{at},\"division\":\"1/8\"}}")], "the changed field and the placement only");
    wait_candidate(&mut t, tr);
    t.frames(8);
    let d = panels(&t, tr).detect.unwrap();
    assert_eq!(d.candidate.as_deref(), Some("Candidate 4 → 8"));
    let b = bench(&t, tr);
    assert_eq!(b.candidate.len(), 4, "the new markers preview dashed, without flags");
    assert_eq!(b.flags.len(), 4, "the current set stays solid and in use");
    assert_eq!(t.undo_len(), undo, "Detect is not an edit");
    t.click(mid(d.replace.unwrap()), 1);
    assert_eq!(t.undo_len(), undo + 1, "Replace markers is one step");
    assert_eq!(frames(&t, tr).len(), 8);
    let p = panels(&t, tr);
    assert_eq!(p.left_line.unwrap().text, "Replaced the Auto set: 8 markers (was 4)");
    assert!(bench(&t, tr).candidate.is_empty());
    assert_eq!(t.patch(tr).slicing.unwrap().division, transit_model::sampler_patch::Division::Eighth, "the settings are stored with it");
}

#[test]
fn keep_current_discards() {
    let (mut t, tr) = auto();
    let undo = t.undo_len();
    let d = panels(&t, tr).detect.unwrap();
    pick(&mut t, d.division.as_ref().unwrap(), 1);
    settle(&mut t);
    wait_candidate(&mut t, tr);
    t.frames(8);
    let log = view(&t).sent.len();
    let d = panels(&t, tr).detect.unwrap();
    t.click(mid(d.keep.unwrap()), 1);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1);
    assert!(s[0].starts_with("mix.sampler_slice_discard"), "{s:?}");
    assert_eq!(frames(&t, tr).len(), 4, "the current set stays");
    assert_eq!(t.undo_len(), undo);
    assert!(view(&t).detect.pending.is_none(), "the panel returns to the stored settings");
    assert_eq!(panels(&t, tr).detect.unwrap().division.unwrap().selected, 0);
    assert!(bench(&t, tr).candidate.is_empty());
}

#[test]
fn grid_controls_dim_unless_source_is_grid() {
    let (mut t, tr) = auto();
    let d = panels(&t, tr).detect.unwrap();
    assert_eq!(d.grid_opacity, 1.0, "the stored source is Grid");
    pick(&mut t, d.source.as_ref().unwrap(), 1);
    let d = panels(&t, tr).detect.unwrap();
    assert_eq!(d.source.as_ref().unwrap().selected, 1);
    assert_eq!(d.grid_opacity, tk::UNUSED_CONTROL_ALPHA);
    pick(&mut t, d.source.as_ref().unwrap(), 2);
    assert_eq!(panels(&t, tr).detect.unwrap().grid_opacity, 1.0);
}

#[test]
fn a_sensitivity_drag_sends_one_detect() {
    let (mut t, tr) = auto();
    let log = view(&t).sent.len();
    let f = panels(&t, tr).detect.unwrap().fader;
    let y = (f[1] + f[3]) / 2.0;
    steps(
        &mut t,
        vec![moved((f[0] + 20.0, y)), press((f[0] + 20.0, y), true), moved((f[0] + 50.0, y)), moved((f[0] + 80.0, y)), moved((f[0] + 110.0, y))],
    );
    // Held past the debounce: still nothing.
    t.frames(30);
    assert_eq!(sent(&t, log), Vec::<String>::new(), "nothing while the fader is held");
    t.input(vec![press((f[0] + 110.0, y), false)]);
    settle(&mut t);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "{s:?}");
    assert!(s[0].contains("\"sensitivity\":") && !s[0].contains("division"), "{s:?}");
    assert_ne!(panels(&t, tr).detect.unwrap().sensitivity, "0.50");
}

#[test]
fn slices_to_pads_previews_the_bank_then_applies() {
    let (mut t, tr) = auto();
    let u = panels(&t, tr).uses;
    assert_eq!(u.plan, "bank A from A1 · replaces none");
    assert_eq!(u.cells.iter().filter(|c| c.1).count(), 4, "the four slices' pads");
    assert!(u.cells[0].2, "the selected slice's cell is edged");
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(u.to_pads), 1);
    assert_eq!(sent(&t, log), ["mix.sampler_slices_to_pads {}"], "only what the panel changed");
    assert_eq!(t.undo_len(), undo + 1);
    assert!(view(&t).snapshot.data.unwrap().auto.iter().all(|s| s.pad.is_some()));
}

/// The sliced fixture with slices 1 and 2 swapped between pads A1 and A2 by the pad commands: the
/// keys no longer run in order.
fn stale() -> (T, transit_model::TrackId) {
    let (mut t, tr) = sliced();
    let (za, zb) = (t.patch(tr).pad(0).zone.unwrap(), t.patch(tr).pad(1).zone.unwrap());
    for (cmd, pad, zone) in [
        ("mix.sampler_pad_unbind", 1, None),
        ("mix.sampler_pad_unbind", 2, None),
        ("mix.sampler_pad_bind", 1, Some(zb)),
        ("mix.sampler_pad_bind", 2, Some(za)),
    ] {
        let mut p = json!({"track": tr.0, "pad": pad});
        if let Some(z) = zone {
            p["zone"] = json!(z + 1);
        }
        t.run(cmd, p);
    }
    (t, tr)
}

#[test]
fn slice_to_midi_panel_shows_stale_refusal_and_offers_map() {
    let (mut t, tr) = stale();
    assert_eq!(bench(&t, tr).tag.unwrap().0, bench::STALE_TAG);
    let p = panels(&t, tr);
    let e = p.use_error.expect("the stale mapping's refusal");
    assert!(e.text.starts_with("The slices' keys no longer run in order"), "{}", e.text);
    let (undo, log) = (t.undo_len(), view(&t).sent.len());
    t.click(mid(p.uses.map.expect("Map slices to keys beside it")), 1);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1);
    assert!(s[0].starts_with("mix.sampler_map_slices {\"root\":"), "{s:?}");
    assert_eq!(t.undo_len(), undo + 1);
    assert!(bench(&t, tr).tag.is_none());
    assert!(panels(&t, tr).use_error.is_none());
}

#[test]
fn create_clip_repeats_the_reason() {
    let (mut t, tr) = stale();
    let (undo, clips, log) = (t.undo_len(), t.clip_count(), view(&t).sent.len());
    let reason = panels(&t, tr).use_error.unwrap().text;
    t.click(mid(panels(&t, tr).uses.create), 1);
    let s = sent(&t, log);
    assert_eq!(s.len(), 1, "Create clip stays clickable: {s:?}");
    assert!(s[0].starts_with("mix.slice_to_midi"));
    assert_eq!(panels(&t, tr).use_error.unwrap().text, reason, "and repeats the reason");
    assert_eq!((t.undo_len(), t.clip_count()), (undo, clips));
}

#[test]
fn create_clip_places_one_step_with_a_success_line_and_an_overlap_warns() {
    let (mut t, tr) = sliced();
    let (undo, clips, log) = (t.undo_len(), t.clip_count(), view(&t).sent.len());
    let u = panels(&t, tr).uses;
    assert!(u.placement.starts_with("at 1.1.1 · edit cursor"), "{}", u.placement);
    assert!(u.summary.starts_with("4 notes · "), "{}", u.summary);
    assert!(u.overlap.is_none());
    t.click(mid(u.create), 1);
    let s = sent(&t, log);
    let at = view(&t).at;
    assert_eq!(s, [format!("mix.slice_to_midi {{\"at\":{at},\"timing\":\"fit\"}}")], "the timing and the placement only");
    assert_eq!((t.undo_len(), t.clip_count()), (undo + 1, clips + 1));
    assert_eq!(panels(&t, tr).use_line.unwrap().text, "Created clip at 1.1.1 · 4 notes");
    assert!(panels(&t, tr).uses.overlap.is_some(), "the next clip there overlaps: a warning line");
}

#[test]
fn placement_follows_the_selection_then_the_cursor_then_the_playhead() {
    let (mut t, tr) = sliced();
    let sr = t.app.engine.session().sample_rate.hz() as i64;
    // A point selection: the edit cursor.
    t.app.engine.session_mut().edit.selection = transit_time::Range::new(sr * 2, sr * 2);
    let p = super::placement::of(&t.app, tr);
    assert_eq!(p.origin, super::placement::Origin::EditCursor);
    // A range on the track: its start.
    let s = t.app.engine.session_mut();
    s.edit.selection = transit_time::Range::new(sr, sr * 3);
    s.edit.selected_tracks = vec![tr];
    let p = super::placement::of(&t.app, tr);
    assert_eq!(p.origin, super::placement::Origin::SelectionStart);
    assert_eq!(p.tick, t.app.engine.session().tempo.samples_to_ticks(sr, t.app.engine.session().sample_rate));
    // A range that leaves the track out: no edit cursor here, so the playhead.
    t.app.engine.session_mut().edit.selected_tracks.clear();
    let p = super::placement::of(&t.app, tr);
    assert_eq!(p.origin, super::placement::Origin::Playhead);
    t.frames(3);
    assert!(panels(&t, tr).uses.placement.contains("· playhead ·"));
}

#[test]
fn states_render() {
    // Empty: the dashed drop target with Load…, the header and the panels ghosted.
    let (mut t, tr) = T::open();
    show_slices(&mut t, tr);
    let b = bench(&t, tr);
    assert!(b.empty.is_some() && b.flags.is_empty());
    assert_eq!(b.header_opacity, tk::UNUSED_CONTROL_ALPHA);
    assert_eq!(panels(&t, tr).opacity, tk::UNUSED_CONTROL_ALPHA);
    // Refused file: a drop the engine refuses is the note row under the bench.
    let bad = t.dir.join("not audio.wav");
    std::fs::write(&bad, b"not a wav").unwrap();
    t.drop_file(&bad, mid(b.well));
    let note = t.panel(tr).slice_note.expect("the refusal in the note row");
    assert!(!note.0.is_empty());
    assert!(bench(&t, tr).empty.is_some(), "nothing loaded");
    // Loading: the source's peaks not there yet.
    let (mut t, tr) = sliced();
    let id = view(&t).snapshot.data.unwrap().source.unwrap();
    let audio = t.app.engine.session().pool.get(id).unwrap().clone();
    let bare = transit_model::SourceAudio { buffer: audio.buffer.clone(), peaks: Vec::new(), disk: None };
    t.app.engine.session_mut().pool.insert(id, std::sync::Arc::new(bare));
    t.frames(3);
    assert_eq!(bench(&t, tr).tag.unwrap().0, bench::ANALYZING);
    // Missing media: the hatch and the card naming the file; Relink… waits for its command.
    t.app.engine.session_mut().pool.remove(id);
    t.frames(3);
    let b = bench(&t, tr);
    let (_, text, _) = b.missing.expect("the missing-media card");
    assert_eq!(text, "Loop.wav is missing");
    assert!(b.tag.is_none(), "never Analyzing… for a missing source");
    // A refused state read: the engine's words, not an empty bench.
    let v = &mut t.app.builtin_editor.devices.sampler.views.values_mut().next().unwrap().slices;
    v.snapshot.data = None;
    v.snapshot.error = Some("the patch has no slice loop".into());
    t.frames(2);
    assert_eq!(bench(&t, tr).error.unwrap().0, "the patch has no slice loop");
}

#[test]
fn over_capacity_reads_64_of_64_and_every_cell_stays_clickable() {
    // Manual from scratch on a loop without pads (no mapping to go stale).
    let (mut t, tr) = auto();
    t.run("mix.sampler_slice_mode", json!({"track": tr.0, "mode": "manual"}));
    for i in 1..64u64 {
        t.app.engine.execute("mix.sampler_slice_marker", &json!({"track": tr.0, "op": "add", "frame": i * 689, "snap": false})).unwrap();
    }
    t.frames(3);
    let b = bench(&t, tr);
    assert_eq!(b.count, "64 of 64 slices");
    assert_eq!(b.tag.as_ref().unwrap().0, "64 of 64 slices");
    assert_eq!(b.cells.len(), 64, "a crowded flag is left out; its cell stays");
    assert!(b.cells.iter().all(|c| c.1[2] > c.1[0]));
    assert!(b.flags.iter().any(|f| f.crowded));
    t.click(mid(b.cells[40].1), 1);
    assert_eq!(view(&t).selected, 40);
    let undo = t.undo_len();
    t.click(at_frame(&b, 43_900), 1);
    assert_eq!(t.undo_len(), undo);
    assert_eq!(panels(&t, tr).left_error.unwrap().text, "Cannot add slice: 64 slices per loop");
}

#[test]
fn the_ruler_visits_only_the_visible_steps() {
    let (mut t, tr) = sliced();
    let z = bench(&t, tr).zoom.unwrap();
    t.click(mid(z.segments[3]), 1);
    let b = bench(&t, tr);
    // 1 s at 8× shows an eighth of a second: two or three 0.1 s steps, not the loop's eleven.
    assert!(b.ruler_steps <= 2, "{}", b.ruler_steps);
}

#[test]
fn a_long_file_name_is_cut_in_the_bar() {
    let (mut t, tr) = T::open();
    let path = t.sample_at(&format!("{}.wav", "A very long loop name ".repeat(8)), 4_410, 44_100);
    t.run("mix.sampler_slice_loop", json!({"track": tr.0, "path": path.to_string_lossy()}));
    show_slices(&mut t, tr);
    let bar = t.panel(tr).bar;
    assert!(bar.summary_rect[2] <= bar.summary_slot[2] + 0.01, "{:?} in {:?}", bar.summary_rect, bar.summary_slot);
}

#[test]
fn idle_frames_send_no_commands() {
    let (mut t, _) = sliced();
    let v0 = view(&t);
    t.frames(60);
    let v1 = view(&t);
    assert_eq!(v1.sent, v0.sent);
    assert_eq!(v1.snapshot.reads, v0.snapshot.reads);
    assert_eq!(v1.usage.previews, v0.usage.previews);
    assert_eq!(v1.detect.polls, v0.detect.polls);
}

#[test]
fn panel_rects_match_the_boards_at_both_scales() {
    for scale in [1.0, 1.5] {
        let (mut t, tr) = auto();
        t.ctx.set_pixels_per_point(scale);
        t.frames(4);
        let b = bench(&t, tr);
        let p = panels(&t, tr);
        eprintln!("scale {scale}: left {:?} right {:?}", p.left, p.right);
        let (l, r) = (p.left, p.right);
        // Two equal columns a panel gap apart, a panel gap under the bench, across it.
        assert_eq!(h(l[1] - b.panel[3]), h(layout::PANEL_GAP));
        assert_eq!((h(l[0]), h(r[2])), (h(b.panel[0]), h(b.panel[2])));
        assert_eq!(h(r[0] - l[2]), h(layout::PANEL_GAP));
        assert_eq!(h(l[2] - l[0]), h(r[2] - r[0]));
        assert_eq!((h(l[1]), h(l[3])), (h(r[1]), h(r[3])), "the cards share the row's height");
        assert!(h(l[3] - l[1]) >= h(tk::PANEL_MIN_HEIGHT));
        let (pv, ph) = (layout::PANEL_PADDING[0], layout::PANEL_PADDING[1]);
        assert_eq!((h(p.left_label[0]), h(p.left_label[1])), (h(l[0] + ph), h(l[1] + pv)));
        assert_eq!((h(p.right_label[0]), h(p.right_label[1])), (h(r[0] + ph), h(r[1] + pv)));
        // Detect: the controls after the 70 px label column and its 10 px gap; rows 10 apart.
        let d = p.detect.unwrap();
        let src = d.source.unwrap().track;
        assert_eq!(h(src[0]), h(l[0] + ph + tk::DETECT_LABEL_WIDTH + tk::PANEL_ITEM_GAP));
        assert_eq!(h(src[1] - p.left_label[3]), h(tk::PANEL_ROW_GAP));
        assert_eq!((h(d.fader[0]), h(d.fader[2] - d.fader[0])), (h(src[0]), h(tk::FADER_LENGTH)));
        assert_eq!(h(d.fader[1] - src[3]), h(tk::PANEL_ROW_GAP));
        let div = d.division.unwrap().track;
        assert_eq!((h(div[0]), h(div[1] - d.fader[3])), (h(src[0]), h(tk::PANEL_ROW_GAP)));
        assert_eq!(h(d.bars[0] - div[2]), h(tk::PANEL_ITEM_GAP));
        assert_eq!((h(d.bpm[0] - d.bars[2]), h(d.bpm[2] - d.bpm[0])), (h(tk::PANEL_ITEM_GAP), h(tk::BEAT_WIDTH)));
        // Use slices: Root after the 62 px label column, 92 wide; the 112 px action; the
        // 12 px preview cells 2 apart in rows of eight; the 84 px beat field.
        let u = p.uses;
        assert_eq!((h(u.root[0]), h(u.root[2] - u.root[0])), (h(r[0] + ph + tk::USE_LABEL_WIDTH + tk::PANEL_ITEM_GAP), h(tk::ROOT_WIDTH)));
        assert_eq!((h(u.to_pads[0]), h(u.to_pads[2] - u.to_pads[0])), (h(r[0] + ph), h(tk::PANEL_ACTION_WIDTH)));
        assert_eq!(h(u.cells[0].0[0] - u.to_pads[2]), h(tk::PANEL_ITEM_GAP));
        assert_eq!(h(u.cells[0].0[2] - u.cells[0].0[0]), h(tk::PREVIEW_CELL));
        assert_eq!(h(u.cells[1].0[0] - u.cells[0].0[2]), h(tk::PREVIEW_GAP));
        assert_eq!(h(u.cells[8].0[1] - u.cells[0].0[3]), h(tk::PREVIEW_GAP));
        assert_eq!(h(u.cells[8].0[0]), h(u.cells[0].0[0]));
        let timing = u.timing.unwrap().track;
        assert_eq!(h(timing[0]), h(r[0] + ph + tk::PANEL_ACTION_WIDTH + tk::PANEL_ITEM_GAP));
        assert_eq!((h(u.beat[0] - timing[2]), h(u.beat[2] - u.beat[0])), (h(tk::PANEL_ITEM_GAP), h(tk::BEAT_WIDTH)));
        assert_eq!(h(u.create[2]), h(r[2] - ph), "Create clip right-aligned");
        assert_eq!(h(u.create[3] - u.create[1]), h(layout::DENSE_CONTROL_HEIGHT));
    }
}
