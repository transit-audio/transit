use crate::tests::*;
// ---- loop strip (backlog #9) ----

/// An app after a few frames, so the arrangement layout is known.
fn loop_app() -> (StudioApp, egui::Context, f64) {
    let mut app = app();
    let ctx = egui::Context::default();
    let mut time = 0.0;
    frames(&mut app, &ctx, 3, &mut time);
    (app, ctx, time)
}

/// The loop strip's point for the sample position `at` (plus `dx` points), at its middle height.
fn strip_at(app: &StudioApp, at: i64, dx: f32) -> (f32, f32) {
    let l = &app.arrange.layout;
    let x = l.timeline[0] + ((at - l.scroll) as f64 / l.samples_per_px) as f32 + dx;
    (x, (l.loop_strip[1] + l.loop_strip[3]) / 2.0)
}

fn strip_drag(app: &mut StudioApp, from: (i64, f32), to: (i64, f32), mods: Value) {
    let (a, b) = (strip_at(app, from.0, from.1), strip_at(app, to.0, to.1));
    drag(app, a, b, mods);
}

fn loop_range(app: &StudioApp) -> (i64, i64) {
    let r = app.engine.session().edit.loop_range;
    (r.start, r.end)
}

#[test]
fn dragging_the_empty_loop_strip_creates_a_snapped_region_in_one_undo_step() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    let steps = undo_len(&mut app);
    // Press a little off the 2nd beat and release a little off the 6th: both snap.
    strip_drag(&mut app, (2 * b, 3.0), (6 * b, -4.0), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(loop_range(&app), (2 * b, 6 * b));
    assert!(app.engine.session().edit.loop_playback, "a new region turns looping on");
    assert_eq!(undo_len(&mut app), steps + 1, "one undo step for the whole gesture");
    assert!(app.arrange.loop_drag.is_none());
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(loop_range(&app), (0, 0), "undo removes the region");
    assert!(!app.engine.session().edit.loop_playback);
}

#[test]
fn dragging_the_right_edge_extends_the_region_and_the_body_moves_it() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    app.run("transport.loop", json!({"start": 4 * b, "end": 8 * b, "enabled": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let steps = undo_len(&mut app);
    strip_drag(&mut app, (8 * b, -2.0), (12 * b, 0.0), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(loop_range(&app), (4 * b, 12 * b), "the right edge moved, the left stayed");
    assert_eq!(undo_len(&mut app), steps + 1);
    // The body: press in the middle, drag two beats left; the length stays.
    strip_drag(&mut app, (8 * b, 0.0), (6 * b, 0.0), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(loop_range(&app), (2 * b, 10 * b), "moved, same length");
    assert_eq!(undo_len(&mut app), steps + 2);
    // Dragged far left, the body stops at zero and keeps its length.
    strip_drag(&mut app, (6 * b, 0.0), (0, 0.0), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(loop_range(&app), (0, 8 * b));
    // Undo goes back one gesture at a time.
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(loop_range(&app), (2 * b, 10 * b));
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(loop_range(&app), (4 * b, 12 * b));
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(loop_range(&app), (4 * b, 8 * b), "undo restores the previous region");
}

#[test]
fn dragging_the_left_edge_past_the_right_one_swaps_them() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    app.run("transport.loop", json!({"start": 4 * b, "end": 8 * b})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    strip_drag(&mut app, (4 * b, 2.0), (10 * b, 0.0), json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(loop_range(&app), (8 * b, 10 * b));
}

#[test]
fn double_clicking_the_strip_toggles_looping_and_the_cycle_button_follows() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    app.run("transport.loop", json!({"start": 4 * b, "end": 8 * b, "enabled": false})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let at = strip_at(&app, 6 * b, 0.0);
    click_with(&mut app, at, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    assert!(app.engine.session().edit.loop_playback, "enabled");
    assert_eq!(loop_range(&app), (4 * b, 8 * b), "the region stays");
    frames(&mut app, &ctx, 60, &mut time); // past egui's multi-click window
    let at = strip_at(&app, 6 * b, 0.0);
    click_with(&mut app, at, json!({"count": 2}));
    frames(&mut app, &ctx, 12, &mut time);
    assert!(!app.engine.session().edit.loop_playback, "disabled");
    // The transport's cycle command and the strip share the one flag.
    app.run("options.loop_playback", json!({})).unwrap();
    assert!(app.engine.session().edit.loop_playback);
    // A plain click on the strip leaves the region and the cursor alone.
    let cursor = app.cursor();
    let at = strip_at(&app, 20 * b, 0.0);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!((loop_range(&app), app.cursor()), ((4 * b, 8 * b), cursor));
}

#[test]
fn shift_dragging_the_loop_strip_does_not_snap() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    strip_drag(&mut app, (2 * b, 0.0), (6 * b, 20.0), json!({"shift": true}));
    frames(&mut app, &ctx, 20, &mut time);
    let (s, e) = loop_range(&app);
    assert!((s - 2 * b).abs() < 1100 && (e - (6 * b + 20 * 1024)).abs() < 1100, "{s} {e}");
    assert_ne!(e % b, 0, "off the grid");
}

#[test]
fn right_clicking_the_loop_strip_offers_clear_loop_in_one_undo_step() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    app.run("transport.loop", json!({"start": 4 * b, "end": 8 * b, "enabled": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let steps = undo_len(&mut app);
    let at = strip_at(&app, 6 * b, 0.0);
    right_click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    let item = app.arrange.menu.iter().find(|m| m.label == "Clear loop").cloned().expect("the strip menu offers Clear loop");
    click(&mut app, mid(item.rect));
    frames(&mut app, &ctx, 6, &mut time);
    assert_eq!(loop_range(&app), (0, 0), "region cleared");
    assert_eq!(undo_len(&mut app), steps + 1, "one undo step");
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(loop_range(&app), (4 * b, 8 * b));
}

#[test]
fn the_bounce_dialog_cycle_range_is_the_cycle_range() {
    use crate::finish::bounce_range;
    use transit_time::Range;
    let mut e = transit_model::EditState { selection: Range::new(10, 20), ..Default::default() };
    assert_eq!(bounce_range(&e), Range::new(10, 20), "looping off: the selection");
    e.loop_playback = true;
    assert_eq!(bounce_range(&e), Range::new(10, 20), "looping on, no loop region: the selection");
    e.loop_range = Range::new(100, 200);
    assert_eq!(bounce_range(&e), Range::new(100, 200), "looping on: the loop region");
    e.loop_playback = false;
    assert_eq!(bounce_range(&e), Range::new(10, 20), "looping off again");
}

// ---- Cycle makes a real loop region; the ruler block is a live control ----

/// A point in the ruler's loop block (under the strip) at sample `at`, plus `dx` points.
fn block_at(app: &StudioApp, at: i64, dx: f32) -> (f32, f32) {
    let (x, y) = strip_at(app, at, dx);
    (x, y + 20.0)
}

fn one_bar(app: &StudioApp) -> i64 {
    4 * beat(app)
}

fn cycle_on(app: &mut StudioApp, on: bool) {
    app.run("options.loop_playback", json!({"value": on})).unwrap();
}

#[test]
fn cycle_on_promotes_the_time_selection_to_the_loop_region() {
    let (mut app, _ctx, _time) = loop_app();
    let b = beat(&app);
    app.engine.session_mut().edit.selection = transit_time::Range::new(3 * b, 7 * b);
    cycle_on(&mut app, true);
    assert_eq!(loop_range(&app), (3 * b, 7 * b));
    assert!(app.engine.session().edit.loop_playback);
}

#[test]
fn cycle_on_without_a_selection_makes_four_bars_from_the_playheads_bar() {
    let (mut app, _ctx, _time) = loop_app();
    let bar = one_bar(&app);
    app.set_cursor(2 * bar + bar / 2, None);
    cycle_on(&mut app, true);
    assert_eq!(loop_range(&app), (2 * bar, 6 * bar), "bar 3 through bar 6, from the bar holding the playhead");
}

#[test]
fn cycle_off_keeps_the_region_and_on_again_reuses_it() {
    let (mut app, _ctx, _time) = loop_app();
    let bar = one_bar(&app);
    app.set_cursor(bar, None);
    cycle_on(&mut app, true);
    let made = loop_range(&app);
    cycle_on(&mut app, false);
    assert_eq!(loop_range(&app), made, "off leaves the region");
    app.set_cursor(20 * bar, None);
    cycle_on(&mut app, true);
    assert_eq!(loop_range(&app), made, "on again restores it rather than making another");
}

#[test]
fn undoing_cycle_on_reverts_the_switch_and_the_region_together() {
    let (mut app, _ctx, _time) = loop_app();
    let steps = undo_len(&mut app);
    cycle_on(&mut app, true);
    assert_eq!(undo_len(&mut app), steps + 1, "one step for the toggle and the region");
    assert_ne!(loop_range(&app), (0, 0));
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(loop_range(&app), (0, 0));
    assert!(!app.engine.session().edit.loop_playback);
}

#[test]
fn dragging_the_ruler_blocks_right_edge_and_body_issue_transport_loop() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    app.run("transport.loop", json!({"start": 4 * b, "end": 8 * b, "enabled": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let steps = undo_len(&mut app);
    let (a, z) = (block_at(&app, 8 * b, -2.0), block_at(&app, 12 * b, 0.0));
    drag(&mut app, a, z, json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(loop_range(&app), (4 * b, 12 * b), "the right edge moved");
    assert_eq!(undo_len(&mut app), steps + 1);
    let (a, z) = (block_at(&app, 8 * b, 0.0), block_at(&app, 6 * b, 0.0));
    drag(&mut app, a, z, json!({}));
    frames(&mut app, &ctx, 20, &mut time);
    assert_eq!(loop_range(&app), (2 * b, 10 * b), "the body moved, same length");
    assert_eq!(undo_len(&mut app), steps + 2);
}

#[test]
fn clicking_the_ruler_outside_the_block_still_locates() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    app.run("transport.loop", json!({"start": 4 * b, "end": 8 * b, "enabled": true})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let at = block_at(&app, 14 * b, 0.0);
    click(&mut app, at);
    frames(&mut app, &ctx, 6, &mut time);
    assert!((app.cursor() - 14 * b).abs() < b / 2, "cursor {} near {}", app.cursor(), 14 * b);
    assert_eq!(loop_range(&app), (4 * b, 8 * b), "the region is untouched");
}

#[test]
fn the_ruler_block_ignores_the_selection_when_there_is_no_region() {
    let (mut app, ctx, mut time) = loop_app();
    let b = beat(&app);
    app.engine.session_mut().edit.selection = transit_time::Range::new(2 * b, 6 * b);
    app.engine.session_mut().edit.loop_playback = true;
    frames(&mut app, &ctx, 3, &mut time);
    // No region: a drag where the selection is must not move anything (nothing is there to grab).
    let (a, z) = (block_at(&app, 4 * b, 0.0), block_at(&app, 6 * b, 0.0));
    drag(&mut app, a, z, json!({}));
    frames(&mut app, &ctx, 10, &mut time);
    assert_eq!(loop_range(&app), (0, 0));
}

/// Two MIDI clips at bars 2-3 and 5-6 on one track, both selected; returns (first start, last end).
fn two_selected_clips(app: &mut StudioApp) -> (i64, i64) {
    let bar = one_bar(app);
    let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let t = r["tracks"][0].as_u64().unwrap();
    let a = app.run("midi.new_clip", json!({"tracks": [t], "start": bar, "end": 2 * bar})).unwrap()["clip"].as_u64().unwrap();
    let b = app.run("midi.new_clip", json!({"tracks": [t], "start": 4 * bar, "end": 5 * bar})).unwrap()["clip"].as_u64().unwrap();
    app.run("edit.select", json!({"clips": [a, b]})).unwrap();
    (bar, 5 * bar)
}

#[test]
fn pressing_c_with_clips_selected_and_no_region_cycles_their_span_and_a_region_set_by_hand_stays() {
    let (mut app, ctx, mut time) = loop_app();
    let span = two_selected_clips(&mut app);
    let steps = undo_len(&mut app);
    key(&mut app, "c");
    frames(&mut app, &ctx, 3, &mut time);
    assert_eq!(loop_range(&app), span, "the region spans both clips");
    assert!(app.engine.session().edit.loop_playback);
    assert_eq!(undo_len(&mut app), steps + 1);
    key(&mut app, "c");
    frames(&mut app, &ctx, 3, &mut time);
    assert!(!app.engine.session().edit.loop_playback, "C again is Cycle off");
    assert_eq!(loop_range(&app), span, "off keeps the region");
}

#[test]
fn pressing_c_twice_with_clips_selected_keeps_a_region_set_by_hand() {
    let (mut app, ctx, mut time) = loop_app();
    two_selected_clips(&mut app);
    app.run("transport.loop", json!({"start": 0, "end": 10})).unwrap();
    app.run("options.loop_playback", json!({"value": false})).unwrap();
    frames(&mut app, &ctx, 3, &mut time);
    let steps = undo_len(&mut app);
    key(&mut app, "c");
    frames(&mut app, &ctx, 3, &mut time);
    assert!(app.engine.session().edit.loop_playback);
    assert_eq!(loop_range(&app), (0, 10), "an existing region is reused, not replaced by the clips");
    assert_eq!(undo_len(&mut app), steps);
}
