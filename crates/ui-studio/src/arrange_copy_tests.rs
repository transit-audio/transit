//! Ctrl- or Alt-drag of a multi-clip selection copies every selected clip (#214).

use crate::tests::*;

/// Clips A (track 1, bar 0), B (track 1, bar 2), C (track 2, bar 1), all selected.
fn three(r: &mut CopyRig) -> (ClipId, ClipId, ClipId, i64) {
    let a = ClipId(clips_on(&r.app, r.t)[0].0);
    let bar = 4 * r.beat;
    let mk = |app: &mut crate::StudioApp, t: TrackId, from: i64| {
        let c = app.run("midi.new_clip", json!({"tracks": [t.0], "start": from * bar, "end": (from + 1) * bar})).unwrap();
        ClipId(c["clip"].as_u64().unwrap())
    };
    let b = mk(&mut r.app, r.t, 2);
    let c = mk(&mut r.app, r.t2, 1);
    r.app.run("edit.select", json!({"clips": [a.0, b.0, c.0]})).unwrap();
    frames(&mut r.app, &r.ctx, 3, &mut r.time);
    (a, b, c, bar)
}

fn last_undo(app: &mut crate::StudioApp) -> String {
    app.engine.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().last().unwrap().to_string()
}

#[test]
fn ctrl_drag_of_a_selection_copies_every_clip_by_the_same_offset_as_one_undo_step() {
    let mut r = copy_rig();
    let (_, _, _, bar) = three(&mut r);
    let before = undo_len(&mut r.app);
    let to = (r.grab.0 + 4.0 * r.px_per_beat, r.grab.1);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, bar), (bar, 2 * bar), (2 * bar, 3 * bar), (3 * bar, 4 * bar)], "A, its copy, B, B's copy");
    assert_eq!(spans(&r.app, r.t2), vec![(bar, 2 * bar), (2 * bar, 3 * bar)], "C and its copy");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    assert!(last_undo(&mut r.app).contains("Copy"), "{}", last_undo(&mut r.app));
    let sel = r.app.engine.session().edit.selected_clips.clone();
    assert_eq!(sel.len(), 3, "the copies are selected");
    assert!(sel.iter().all(|c| the_clip(&r.app, *c).start >= bar), "and they are the new ones");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(0, bar), (2 * bar, 3 * bar)], "undo removes all copies");
    assert_eq!(spans(&r.app, r.t2), vec![(bar, 2 * bar)]);
}

#[test]
fn a_selection_copy_changes_time_only_like_the_move_and_a_second_ctrl_drag_works_on_the_copies() {
    let mut r = copy_rig();
    let (_, _, _, bar) = three(&mut r);
    // Dragged onto the other track's lane: several clips never change track.
    let to = (r.grab.0 + 4.0 * r.px_per_beat, r.lane2_y);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t).len(), 4);
    assert_eq!(clips_on(&r.app, r.t2).len(), 2);
    // The copies are the selection: a second drag copies those three again.
    let rect = r.app.arrange.layout.clips.iter().find(|c| c.track == r.t.0 && c.start == bar).unwrap().rect;
    let grab = (rect[0] + 20.0, (rect[1] + rect[3]) / 2.0);
    drag(&mut r.app, grab, (grab.0 + 12.0 * r.px_per_beat, grab.1), json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t).len(), 6, "{:?}", spans(&r.app, r.t));
    assert_eq!(clips_on(&r.app, r.t2).len(), 3);
}

#[test]
fn a_selection_copy_that_would_land_on_an_original_is_refused_and_changes_nothing() {
    let mut r = copy_rig();
    let (_, _, _, bar) = three(&mut r);
    let before = undo_len(&mut r.app);
    // Half a bar: A's copy would cover half of A.
    let to = (r.grab.0 + 2.0 * r.px_per_beat, r.grab.1);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, bar), (2 * bar, 3 * bar)]);
    assert_eq!(undo_len(&mut r.app), before, "no undo entry");
    assert!(r.app.status.contains("original"), "{}", r.app.status);
}

#[test]
fn a_single_selected_clip_still_copies_to_another_track() {
    let mut r = copy_rig();
    let a = ClipId(clips_on(&r.app, r.t)[0].0);
    r.app.run("edit.select", json!({"clips": [a.0]})).unwrap();
    let before = undo_len(&mut r.app);
    let to = (r.grab.0 + 4.0 * r.px_per_beat, r.lane2_y);
    drag(&mut r.app, r.grab, to, json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t).len(), 1);
    assert_eq!(spans(&r.app, r.t2).len(), 1, "one clip may change track");
    assert_eq!(undo_len(&mut r.app), before + 1);
}
