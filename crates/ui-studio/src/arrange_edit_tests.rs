use crate::tests::*;

#[test]
fn the_cursor_is_not_the_selection() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    app.click_empty(Some(t), 3 * bar);
    app.click_clip(clip, false);
    let s = app.engine.session();
    assert_eq!(s.edit.selected_clips, vec![clip]);
    assert_eq!((s.edit.selection.start, s.edit.selection.end), (0, bar));
    assert_eq!(app.cursor(), 3 * bar, "selecting a clip leaves the cursor where it was");
    assert_eq!(app.cursor_track(), Some(t));
    app.click_ruler(5 * bar);
    assert_eq!(app.cursor(), 5 * bar);
    assert_eq!(app.engine.session().edit.selected_clips, vec![clip], "the ruler moves the cursor, not the selection");
}

#[test]
fn paste_goes_to_the_cursor_and_never_onto_the_copied_clip() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    app.click_empty(Some(t), 0);
    app.click_clip(clip, false);
    app.copy_selected(false).unwrap();
    // The cursor is on the original: refused, nothing replaced.
    assert!(app.paste_at_cursor().is_err());
    assert_eq!(clips_on(&app, t).len(), 1);
    assert!(app.status.contains("replace"), "{}", app.status);
    app.click_empty(Some(t), 4 * bar);
    app.paste_at_cursor().unwrap();
    let clips = clips_on(&app, t);
    assert_eq!(clips.len(), 2, "{clips:?}");
    assert!(clips.iter().any(|c| c.0 == clip.0 && c.1 == 0), "the original stays");
    assert!(clips.iter().any(|c| c.1 == 4 * bar && c.2 == 5 * bar), "the copy is at the cursor: {clips:?}");
}

#[test]
fn alt_drag_copies_and_keeps_the_clipboard() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    let before = app.engine.clipboard.clone();
    app.copy_clip_to(clip, 2 * bar, t).unwrap();
    let clips = clips_on(&app, t);
    assert_eq!(clips.len(), 2);
    assert!(clips.iter().any(|c| c.0 == clip.0 && c.1 == 0));
    assert!(clips.iter().any(|c| c.0 != clip.0 && c.1 == 2 * bar));
    assert_eq!(app.engine.clipboard, before);
    let copy = clips.iter().find(|c| c.0 != clip.0).unwrap().0;
    assert_eq!(app.engine.session().edit.selected_clips, vec![ClipId(copy)], "the copy is selected");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(clips_on(&app, t).len(), 1, "one undo step");
}

#[test]
fn move_trim_split_delete_are_commands() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    app.move_clips(&[clip], bar, Some(t2)).unwrap();
    assert!(clips_on(&app, t).is_empty());
    assert_eq!(clips_on(&app, t2), vec![(clip.0, bar, 2 * bar)]);
    app.trim_clip(clip, false, bar + bar / 2).unwrap();
    assert_eq!(clips_on(&app, t2), vec![(clip.0, bar, bar + bar / 2)]);
    app.click_clip(clip, false);
    app.set_cursor(bar + bar / 4, Some(t2));
    app.split_at_cursor().unwrap();
    let parts = clips_on(&app, t2);
    assert_eq!(parts.len(), 2, "{parts:?}");
    let first = parts.iter().find(|c| c.1 == bar).unwrap().0;
    app.click_clip(ClipId(first), false);
    app.delete_selected().unwrap();
    assert_eq!(clips_on(&app, t2).iter().map(|c| (c.1, c.2)).collect::<Vec<_>>(), vec![(bar + bar / 4, bar + bar / 2)]);
}

#[test]
fn dragging_a_clip_in_the_window_moves_copies_and_constrains_it() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t2 = TrackId(r["tracks"][0].as_u64().unwrap());
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    let layout = app.arrange.layout.clone();
    let rect = layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    let lane2 = layout.tracks.iter().find(|r| r.id == t2.0).unwrap().lane;
    let px_per_bar = bar as f64 / layout.samples_per_px;
    let grab = (rect[0] + 20.0, (rect[1] + rect[3]) / 2.0);
    // Plain drag: one bar later, onto the second track.
    let to = (grab.0 + px_per_bar as f32, (lane2[1] + lane2[3]) / 2.0);
    drag(&mut app, grab, to, json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    let moved = clips_on(&app, t2);
    assert_eq!(moved.len(), 1, "moved to the second track: {moved:?}");
    assert!((moved[0].1 - bar).abs() <= layout.samples_per_px.ceil() as i64, "{moved:?}");
    // Alt-drag it back up: a copy on the first track, the original stays.
    let rect = app.arrange.layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    let lane1 = app.arrange.layout.tracks.iter().find(|r| r.id == t.0).unwrap().lane;
    let grab = (rect[0] + 20.0, (rect[1] + rect[3]) / 2.0);
    drag(&mut app, grab, (grab.0, (lane1[1] + lane1[3]) / 2.0), json!({"alt": true}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(clips_on(&app, t2).len(), 1);
    assert_eq!(clips_on(&app, t).len(), 1, "the Alt-drag copied");
    // Ctrl-drag (Cmd on a Mac) copies too: one bar later onto the first track, the original stays.
    let start = clips_on(&app, t2)[0].1;
    drag(&mut app, grab, (grab.0 + px_per_bar as f32, (lane1[1] + lane1[3]) / 2.0), json!({"cmd": true}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(clips_on(&app, t2).len(), 1, "the original stays");
    assert_eq!(clips_on(&app, t2)[0].1, start);
    assert_eq!(clips_on(&app, t).len(), 2, "the Ctrl-drag copied onto the first track");
}

#[test]
fn drag_modifiers_choose_copy_and_the_snap_bypass() {
    use crate::arrange::{drag_copies, drop_start};
    use egui::Modifiers;
    assert!(!drag_copies(Modifiers::NONE));
    assert!(!drag_copies(Modifiers::SHIFT));
    assert!(drag_copies(Modifiers::ALT));
    assert!(drag_copies(Modifiers::COMMAND));
    let beat = |d: i64| (d as f64 / 1000.0).round() as i64 * 1000;
    assert_eq!(drop_start(0, 1500, false, beat), 2000, "snapped");
    assert_eq!(drop_start(0, 1400, false, beat), 1000);
    assert_eq!(drop_start(500, 1500, true, beat), 2000, "Shift keeps the raw delta");
    assert_eq!(drop_start(500, 1400, true, beat), 1900);
    assert_eq!(drop_start(500, -3000, false, beat), 0, "never before zero");
}

#[test]
fn ctrl_drag_copies_a_clip_snapped_in_one_undo_step() {
    let mut r = copy_rig();
    let before = undo_len(&mut r.app);
    let to = (r.grab.0 + 1.5 * r.px_per_beat, r.lane2_y);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t).len(), 1, "the original stays");
    let copy = clips_on(&r.app, r.t2);
    assert_eq!(copy.len(), 1, "the copy lands on the other track: {copy:?}");
    assert!(copy[0].1 == r.beat || copy[0].1 == 2 * r.beat, "a half-beat drag snaps to a beat: {copy:?}");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    r.app.run("edit.undo", json!({})).unwrap();
    assert!(clips_on(&r.app, r.t2).is_empty(), "undo removes the copy");
    assert_eq!(clips_on(&r.app, r.t).len(), 1);
}

#[test]
fn ctrl_drag_snaps_to_the_nearest_beat_and_shift_bypasses_the_snap() {
    let mut r = copy_rig();
    let to = (r.grab.0 + 1.4 * r.px_per_beat, r.lane2_y);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t2)[0].1, r.beat, "1.4 beats snaps to 1");
    r.app.run("edit.undo", json!({})).unwrap();
    drag(&mut r.app, r.grab, to, json!({"cmd": true, "shift": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    let at = clips_on(&r.app, r.t2)[0].1;
    assert!((at - (1.4 * r.beat as f64) as i64).abs() < 2 * r.beat / 20, "Shift leaves it unsnapped: {at}");
}

#[test]
fn ctrl_drag_that_lands_where_it_started_does_nothing() {
    let mut r = copy_rig();
    let before = undo_len(&mut r.app);
    let to = (r.grab.0 + 0.2 * r.px_per_beat, r.grab.1);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t).len(), 1, "no copy");
    assert_eq!(undo_len(&mut r.app), before, "no undo entry");
    drag(&mut r.app, r.grab, r.grab, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t).len(), 1);
    assert_eq!(undo_len(&mut r.app), before);
}

#[test]
fn the_drag_snap_uses_the_same_grid_as_the_automation_snap() {
    let r = copy_rig();
    let s = r.app.engine.session();
    let spp = r.app.arrange.layout.samples_per_px;
    for start in [0, 700] {
        for dx in [r.beat / 3, r.beat * 14 / 10, r.beat * 16 / 10, r.beat * 5] {
            let landed = start + crate::automation::snap_delta(s, spp, &r.app.arrange.grid, start, dx);
            if start == 0 {
                assert_eq!(landed, crate::automation::snap(s, spp, &r.app.arrange.grid, dx), "start {start} dx {dx}");
            } else {
                assert_eq!((landed - start) % r.beat, 0, "an off-grid start keeps its offset");
            }
        }
    }
}

#[test]
fn a_plain_drag_still_moves_and_snaps() {
    let mut r = copy_rig();
    let to = (r.grab.0 + 2.2 * r.px_per_beat, r.grab.1);
    drag(&mut r.app, r.grab, to, json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    let c = clips_on(&r.app, r.t);
    assert_eq!(c.len(), 1, "moved, not copied: {c:?}");
    assert_eq!(c[0].1, 2 * r.beat);
    r.app.run("edit.undo", json!({})).unwrap();
    let to = (r.grab.0 + 1.4 * r.px_per_beat, r.grab.1);
    drag(&mut r.app, r.grab, to, json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t)[0].1, r.beat, "1.4 beats moves 1");
    r.app.run("edit.undo", json!({})).unwrap();
    let to = (r.grab.0 + 1.6 * r.px_per_beat, r.grab.1);
    drag(&mut r.app, r.grab, to, json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t)[0].1, 2 * r.beat, "1.6 beats moves 2");
    r.app.run("edit.undo", json!({})).unwrap();
    let to = (r.grab.0 + 1.4 * r.px_per_beat, r.grab.1);
    drag(&mut r.app, r.grab, to, json!({"shift": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    let at = clips_on(&r.app, r.t)[0].1;
    assert!((at - r.beat * 14 / 10).abs() < r.beat / 10, "Shift bypasses the snap: {at}");
}

#[test]
fn setting_the_cursor_does_not_dirty_the_document() {
    let mut app = app();
    let (t, _clip, bar) = track_with_clip(&mut app);
    let path = std::env::temp_dir().join(format!("transit-cursor-{}.sc", std::process::id()));
    app.run("session.save_as", json!({"path": path.to_string_lossy(), "replace": true})).unwrap();
    assert!(!app.engine.is_dirty());
    app.set_cursor(bar, Some(t));
    assert_eq!(app.cursor(), bar);
    assert!(!app.engine.is_dirty(), "the cursor is view state");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_shuffle_document_keeps_its_mode_and_stays_clean() {
    let mut e = transit_engine::Engine::default();
    e.execute("edit.mode", &json!({"mode": "shuffle"})).unwrap();
    let path = std::env::temp_dir().join(format!("transit-shuffle-{}.sc", std::process::id()));
    e.execute("session.save_as", &json!({"path": path.to_string_lossy(), "replace": true})).unwrap();
    let mut app = StudioApp::new(e, None, Services::default());
    assert_eq!(app.engine.session().edit.edit_mode, transit_model::EditMode::Shuffle);
    assert!(!app.engine.is_dirty());
    app.run("session.open", json!({"path": path.to_string_lossy()})).unwrap();
    app.handle_transport(0.0);
    assert_eq!(app.engine.session().edit.edit_mode, transit_model::EditMode::Shuffle);
    assert!(!app.engine.is_dirty());
    // Edits still behave as Slip, and the mode is put back afterwards.
    let (t, clip, bar) = track_with_clip(&mut app);
    app.run("edit.mode", json!({"mode": "shuffle"})).unwrap();
    app.move_clips(&[clip], bar, None).unwrap();
    assert_eq!(clips_on(&app, t), vec![(clip.0, bar, 2 * bar)]);
    assert_eq!(app.engine.session().edit.edit_mode, transit_model::EditMode::Shuffle);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_slip_gesture_undoes_without_touching_the_mode() {
    let mut app = app();
    let (t, clip, bar) = track_with_clip(&mut app);
    app.run("edit.mode", json!({"mode": "shuffle"})).unwrap();
    let before = clips_on(&app, t);
    app.move_clips(&[clip], bar, None).unwrap();
    assert_ne!(clips_on(&app, t), before);
    assert_eq!(app.engine.session().edit.edit_mode, transit_model::EditMode::Shuffle);
    assert!(app.engine.undo());
    assert_eq!(clips_on(&app, t), before);
    assert_eq!(app.engine.session().edit.edit_mode, transit_model::EditMode::Shuffle);
    assert!(app.engine.redo());
    assert_eq!(app.engine.session().edit.edit_mode, transit_model::EditMode::Shuffle);
}

#[test]
fn alt_drag_onto_its_own_range_is_refused() {
    let mut app = app();
    let (t, clip, _bar) = track_with_clip(&mut app);
    let before = clips_on(&app, t);
    assert!(app.copy_clip_to(clip, 10_000, t).is_err());
    assert_eq!(clips_on(&app, t), before, "the original is not truncated");
    assert!(app.status.contains("Copy would land"));
}

fn grid_key(r: &mut CopyRig, name: &str) {
    send_control(&mut r.app, "ui.key", json!({"key": name, "cmd": true}));
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
}

/// Ctrl-drag the rig's clip 1.4 beats onto the second track; where the copy starts (samples).
fn drag_14_beats(r: &mut CopyRig) -> i64 {
    let to = (r.grab.0 + 1.4 * r.px_per_beat, r.lane2_y);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    let at = clips_on(&r.app, r.t2)[0].1;
    r.app.run("edit.undo", json!({})).unwrap();
    at
}

#[test]
fn ctrl_1_and_2_pin_the_grid_finer_and_wider_and_the_snap_follows() {
    let mut r = copy_rig();
    assert_eq!(r.app.arrange.layout.grid_text, "Adaptive 1/4");
    grid_key(&mut r, "1");
    assert_eq!(r.app.arrange.layout.grid_text, "Grid: 1/8", "one finer than the beat in force");
    let tol = r.beat / 20;
    assert!((drag_14_beats(&mut r) - r.beat * 3 / 2).abs() <= tol, "1.4 beats snaps to 1.5 on a 1/8 grid");
    grid_key(&mut r, "1");
    assert_eq!(r.app.arrange.layout.grid_text, "Grid: 1/16");
    grid_key(&mut r, "2");
    grid_key(&mut r, "2");
    assert_eq!(r.app.arrange.layout.grid_text, "Grid: 1/4");
    assert!((drag_14_beats(&mut r) - r.beat).abs() <= tol, "1.4 beats snaps to 1 on a beat grid");
    // A pinned grid ignores the zoom.
    r.app.engine.execute("view.zoom_in", &json!({})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    assert_eq!(r.app.arrange.layout.grid_text, "Grid: 1/4");
}

#[test]
fn ctrl_3_toggles_triplets() {
    let mut r = copy_rig();
    grid_key(&mut r, "1");
    grid_key(&mut r, "3");
    assert_eq!(r.app.arrange.layout.grid_text, "Grid: 1/8T");
    let s = r.app.engine.session();
    assert_eq!(crate::automation::grid_ticks(s, r.app.arrange.layout.samples_per_px, &r.app.arrange.grid), transit_time::TICKS_PER_QUARTER / 3);
    grid_key(&mut r, "3");
    assert_eq!(r.app.arrange.layout.grid_text, "Grid: 1/8");
}

#[test]
fn ctrl_4_turns_the_snap_off_so_a_drag_lands_where_it_was_dropped() {
    let mut r = copy_rig();
    let tol = r.beat / 20;
    grid_key(&mut r, "4");
    assert_eq!(r.app.arrange.layout.grid_text, "Snap off");
    assert!(!r.app.arrange.grid.snap_enabled);
    let at = drag_14_beats(&mut r);
    assert!((at - (1.4 * r.beat as f64) as i64).abs() <= tol * 2, "unsnapped: {at} vs {}", r.beat * 14 / 10);
    grid_key(&mut r, "4");
    assert_eq!(r.app.arrange.layout.grid_text, "Adaptive 1/4");
    assert!((drag_14_beats(&mut r) - r.beat).abs() <= tol, "snapping is back");
}

#[test]
fn snap_off_returns_the_input_unchanged_and_the_loop_strip_uses_the_grid() {
    let r = copy_rig();
    let s = r.app.engine.session();
    let off = crate::grid::GridView { snap_enabled: false, ..Default::default() };
    assert_eq!(crate::automation::snap(s, 100.0, &off, 123_457), 123_457);
    assert_eq!(crate::automation::snap_delta(s, 100.0, &off, 700, 1_234), 1_234);
    let fine = crate::grid::GridView { mode: crate::grid::GridMode::Fixed(30), ..Default::default() };
    let a = crate::automation::snap(s, 100.0, &fine, 123_457);
    assert_ne!(a, crate::automation::snap(s, 100.0, &Default::default(), 123_457), "the finer grid lands closer");
    assert!((a - 123_457).abs() <= s.tempo.tick_to_samples(15, s.sample_rate) + 1);
}

#[test]
fn the_last_add_track_row_is_inside_the_scrollable_content_at_the_minimum_header_width() {
    let mut app = StudioApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    app.ui.header_width = 160.0;
    app.ui.show_mixer = false;
    app.ui.show_devices = false;
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 4, &mut time);
    // Scroll to the bottom; the button rows must then be inside the track list.
    app.arrange.vscroll = 1.0e6;
    frames(&mut app, &ctx, 4, &mut time);
    let l = &app.arrange.layout;
    assert!(l.add_more[3] <= l.track_list[3], "the \"+ ...\" row {:?} is inside the list {:?}", l.add_more, l.track_list);
    assert!(l.add_more[2] <= l.track_list[2] && l.add_group[2] <= l.track_list[2], "and inside its width");
}

// ---- #165: an edge trim snaps to the grid ----

/// Drags the clip's end (`end` true) or start edge by `beats` beats at its middle height, then settles.
fn trim_drag(r: &mut CopyRig, clip: ClipId, end: bool, beats: f32, mods: Value) {
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    let rect = r.app.arrange.layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    let x = if end { rect[2] - 2.0 } else { rect[0] + 2.0 };
    let y = (rect[1] + rect[3]) / 2.0 + 6.0;
    drag(&mut r.app, (x, y), (x + beats * r.px_per_beat, y), mods);
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
}

#[test]
fn trimming_the_end_edge_snaps_to_the_nearest_grid_line_and_shift_bypasses() {
    let mut r = copy_rig();
    let clip = ClipId(clips_on(&r.app, r.t)[0].0);
    let (bar, b) = (4 * r.beat, r.beat);
    // 1.37 beats off the grid right of the end (a bar, on the grid): the edge lands on the beat.
    // The old delta-free code left it at the pointer.
    trim_drag(&mut r, clip, true, 1.37, json!({}));
    assert_eq!(spans(&r.app, r.t), vec![(0, bar + b)]);
    r.app.run("edit.undo", json!({})).unwrap();
    // Left 2.62 beats: nearest line is 3 beats in.
    trim_drag(&mut r, clip, true, -2.62, json!({}));
    assert_eq!(spans(&r.app, r.t), vec![(0, bar - 3 * b)]);
    r.app.run("edit.undo", json!({})).unwrap();
    // Shift: the edge stays at the pointer, off the grid.
    trim_drag(&mut r, clip, true, 1.37, json!({"shift": true}));
    let end = the_clip(&r.app, clip).end();
    assert_ne!(end % b, 0, "off the grid: {end}");
    assert!((end - (bar + b * 137 / 100)).abs() <= b / 20, "at the pointer: {end}");
}

#[test]
fn trimming_the_start_edge_snaps_and_clamps_to_the_other_edge() {
    let mut r = copy_rig();
    let clip = ClipId(clips_on(&r.app, r.t)[0].0);
    let (bar, b) = (4 * r.beat, r.beat);
    trim_drag(&mut r, clip, false, 1.42, json!({}));
    assert_eq!(the_clip(&r.app, clip).start, b, "the start lands on the beat");
    assert_eq!(the_clip(&r.app, clip).end(), bar, "the other edge stays");
    r.app.run("edit.undo", json!({})).unwrap();
    // Left of zero (the clip starts at zero): never before zero.
    trim_drag(&mut r, clip, false, -3.0, json!({}));
    assert_eq!(the_clip(&r.app, clip).start, 0);
    // Past the end edge: stops at the end edge, not past it.
    trim_drag(&mut r, clip, false, 9.0, json!({}));
    let c = the_clip(&r.app, clip);
    assert!(c.start < c.end(), "never past the other edge: {:?}", (c.start, c.end()));
}

#[test]
fn a_snapped_trim_still_stops_at_the_neighbor() {
    let mut r = copy_rig();
    let clip = ClipId(clips_on(&r.app, r.t)[0].0);
    let (bar, b) = (4 * r.beat, r.beat);
    // The neighbor starts 0.4 beat past the end, off the grid; the snapped beat would cross it.
    let next = bar + b * 2 / 5;
    r.app.run("midi.new_clip", json!({"tracks": [r.t.0], "start": next, "end": next + bar})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    assert_eq!(spans(&r.app, r.t).len(), 2);
    trim_drag(&mut r, clip, true, 0.9, json!({}));
    assert_eq!(the_clip(&r.app, clip).end(), next, "clamped to the neighbor's start, not the grid line past it");
}

#[test]
fn an_audio_clip_end_trim_snaps_and_stays_within_its_source() {
    let mut app = StudioApp::new(transit_engine::demo::demo_engine(), None, Services::default());
    let (t, clip) = {
        let s = app.engine.session();
        let t = s.track_by_name("Hats").unwrap();
        (t.id, t.clips()[0].id)
    };
    let b = beat(&app);
    let ctx = egui::Context::default();
    let mut time = 0.0;
    app.ui.show_mixer = false;
    app.ui.show_devices = false;
    app.engine.session_mut().find_clip_mut(clip).unwrap().length = 8 * b;
    let c0 = the_clip(&app, clip);
    let transit_model::ClipContent::Audio { source, .. } = c0.content else { panic!("audio") };
    let file_end = c0.start - c0.source_offset() + app.engine.session().source(source).unwrap().frames as i64;
    frames(&mut app, &ctx, 4, &mut time);
    let px_beat = (b as f64 / app.arrange.layout.samples_per_px) as f32;
    let go = |app: &mut StudioApp, time: &mut f64, beats: f32, mods: Value| {
        let rect = app.arrange.layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
        let y = (rect[1] + rect[3]) / 2.0 + 6.0;
        drag(app, (rect[2] - 2.0, y), (rect[2] - 2.0 + beats * px_beat, y), mods);
        frames(app, &ctx, 20, time);
    };
    assert_eq!(clips_on(&app, t).len(), 1);
    let start = c0.start;
    go(&mut app, &mut time, -1.37, json!({}));
    let end = the_clip(&app, clip).end();
    // 6.63 beats in: the nearest line is 7 beats (ticks round, so allow a couple of samples).
    assert!((end - (start + 7 * b)).abs() <= 3, "end snaps to the 7th beat line: {end}");
    // Far past the source: snapped, then clamped to the file.
    go(&mut app, &mut time, 400.0, json!({}));
    let end = the_clip(&app, clip).end();
    assert_eq!(end, file_end, "snapped past the file, then clamped to its end");
}
