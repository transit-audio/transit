//! The drag preview shows every clip the drag carries (#218). Read through `Layout::drag_ghosts`,
//! the rectangles the preview painted, not pixels; every wait is on the queue of fed events.

use crate::tests::*;

/// Clips A (track 1, bar 0), B (track 1, bar 2), C (track 2, bar 1).
fn three(r: &mut CopyRig) -> (ClipId, ClipId, ClipId) {
    let a = ClipId(clips_on(&r.app, r.t)[0].0);
    let bar = 4 * r.beat;
    let mk = |app: &mut crate::StudioApp, t: TrackId, from: i64| {
        let c = app.run("midi.new_clip", json!({"tracks": [t.0], "start": from * bar, "end": (from + 1) * bar})).unwrap();
        ClipId(c["clip"].as_u64().unwrap())
    };
    let b = mk(&mut r.app, r.t, 2);
    let c = mk(&mut r.app, r.t2, 1);
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    (a, b, c)
}

fn rect_of(r: &CopyRig, id: ClipId) -> [f32; 4] {
    r.app.arrange.layout.clips.iter().find(|c| c.id == id.0).unwrap().rect
}

/// Hold a drag of A by one bar to the right.
fn hold(r: &mut CopyRig, m: egui::Modifiers) {
    let to = (r.grab.0 + 4.0 * r.px_per_beat, r.grab.1);
    hold_drag(&mut r.app, &r.ctx, &mut r.time, r.grab, to, m);
}

fn ghost_x(r: &CopyRig, id: ClipId) -> Option<f32> {
    r.app.arrange.layout.drag_ghosts.iter().find(|g| g.clip == id.0).map(|g| g.rect[0])
}

#[test]
fn a_selection_drag_paints_every_member_at_its_offset() {
    let mut r = copy_rig();
    let (a, b, c) = three(&mut r);
    r.app.run("edit.select", json!({"clips": [a.0, b.0, c.0]})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    let (ra, rb, rc) = (rect_of(&r, a), rect_of(&r, b), rect_of(&r, c));
    hold(&mut r, egui::Modifiers::NONE);
    let ghosts = r.app.arrange.layout.drag_ghosts.clone();
    assert_eq!(ghosts.len(), 3, "{ghosts:?}");
    let bar_px = 4.0 * r.px_per_beat;
    for (id, rect) in [(a, ra), (b, rb), (c, rc)] {
        let x = ghost_x(&r, id).expect("a ghost for every selected clip");
        assert!((x - (rect[0] + bar_px)).abs() < 1.0, "{id:?}: ghost at {x}, expected {}", rect[0] + bar_px);
    }
    let on = |id: ClipId| ghosts.iter().find(|g| g.clip == id.0).unwrap().track;
    assert_eq!((on(a), on(b), on(c)), (r.t.0, r.t.0, r.t2.0), "several clips keep their tracks");
    // Nothing has been committed while the button is down.
    assert_eq!(spans(&r.app, r.t), vec![(0, 4 * r.beat), (8 * r.beat, 12 * r.beat)]);
    release_at(&mut r.app, &r.ctx, &mut r.time, (r.grab.0 + bar_px, r.grab.1), egui::Modifiers::NONE);
    assert!(r.app.arrange.layout.drag_ghosts.is_empty(), "no ghosts once the drag is over");
    assert_eq!(spans(&r.app, r.t), vec![(4 * r.beat, 8 * r.beat), (12 * r.beat, 16 * r.beat)], "the drop lands where the ghosts were");
}

#[test]
fn a_diagonal_drag_of_a_selection_keeps_every_ghost_on_its_own_track() {
    let mut r = copy_rig();
    let (a, b, c) = three(&mut r);
    r.app.run("edit.select", json!({"clips": [a.0, b.0, c.0]})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    let bar_px = 4.0 * r.px_per_beat;
    let to = (r.grab.0 + bar_px, r.lane2_y);
    hold_drag(&mut r.app, &r.ctx, &mut r.time, r.grab, to, egui::Modifiers::NONE);
    let on = |id: ClipId| r.app.arrange.layout.drag_ghosts.iter().find(|g| g.clip == id.0).unwrap().track;
    assert_eq!((on(a), on(b), on(c)), (r.t.0, r.t.0, r.t2.0), "the grabbed clip stays on its track");
    release_at(&mut r.app, &r.ctx, &mut r.time, to, egui::Modifiers::NONE);
    assert_eq!(spans(&r.app, r.t), vec![(4 * r.beat, 8 * r.beat), (12 * r.beat, 16 * r.beat)], "the drop matches the ghosts");
    assert_eq!(spans(&r.app, r.t2).len(), 1);
}

#[test]
fn a_single_clip_dragged_to_another_track_ghosts_on_that_track() {
    let mut r = copy_rig();
    let (a, _b, _c) = three(&mut r);
    r.app.run("edit.select_none", json!({})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    let to = (r.grab.0 + 4.0 * r.px_per_beat, r.lane2_y);
    hold_drag(&mut r.app, &r.ctx, &mut r.time, r.grab, to, egui::Modifiers::NONE);
    let g = &r.app.arrange.layout.drag_ghosts;
    assert_eq!((g.len(), g[0].clip, g[0].track), (1, a.0, r.t2.0));
}

#[test]
fn a_single_unselected_clip_drags_with_one_ghost() {
    let mut r = copy_rig();
    let (a, _b, _c) = three(&mut r);
    r.app.run("edit.select_none", json!({})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    hold(&mut r, egui::Modifiers::NONE);
    let ghosts = &r.app.arrange.layout.drag_ghosts;
    assert_eq!(ghosts.len(), 1, "{ghosts:?}");
    assert_eq!(ghosts[0].clip, a.0);
}

#[test]
fn a_drag_of_an_unselected_clip_ignores_a_selection_it_is_not_part_of() {
    let mut r = copy_rig();
    let (a, b, c) = three(&mut r);
    r.app.run("edit.select", json!({"clips": [b.0, c.0]})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    hold(&mut r, egui::Modifiers::NONE);
    let ids: Vec<u64> = r.app.arrange.layout.drag_ghosts.iter().map(|g| g.clip).collect();
    assert_eq!(ids, vec![a.0], "pressing A selects it alone, as the commit does");
}

#[test]
fn a_copy_drag_ghosts_the_copies_and_leaves_the_originals_where_they_are() {
    let mut r = copy_rig();
    let (a, b, c) = three(&mut r);
    r.app.run("edit.select", json!({"clips": [a.0, b.0, c.0]})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    let before: Vec<[f32; 4]> = [a, b, c].iter().map(|id| rect_of(&r, *id)).collect();
    let copy = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
    hold(&mut r, copy);
    assert_eq!(r.app.arrange.layout.drag_ghosts.len(), 3);
    let after: Vec<[f32; 4]> = [a, b, c].iter().map(|id| rect_of(&r, *id)).collect();
    assert_eq!(before, after, "the originals stay put");
    let bar_px = 4.0 * r.px_per_beat;
    assert!((ghost_x(&r, b).unwrap() - (before[1][0] + bar_px)).abs() < 1.0);
}

#[test]
fn a_loop_drag_draws_no_selection_ghosts() {
    let mut r = copy_rig();
    let (a, b, c) = three(&mut r);
    r.app.run("edit.select", json!({"clips": [a.0, b.0, c.0]})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    // The loop corner of A: the right end of its header strip, clear of the trim edge.
    let rect = rect_of(&r, a);
    let from = (rect[2] - 12.0, rect[1] + 6.0);
    hold_drag(&mut r.app, &r.ctx, &mut r.time, from, (from.0 + 2.0 * r.px_per_beat * 4.0, from.1), egui::Modifiers::NONE);
    assert!(r.app.arrange.layout.drag_ghosts.is_empty(), "loop ghosts are the loop's own");
}
