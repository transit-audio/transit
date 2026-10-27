use super::*;
use crate::edit_window::test_support::*;

#[test]
fn drag_selection_spans_the_rows_from_anchor_to_pointer() {
    let mut h = harness(4);
    let ids = row_ids(&h);
    for (from, to) in [(3, 2), (0, 1), (2, 0), (1, 3)] {
        let (a, b) = (lane_point(&h, from), lane_point(&h, to) + vec2(80.0, 0.0));
        drag(&mut h, a, b);
        let want = ids.get(from.min(to)..=from.max(to)).unwrap();
        assert_eq!(h.state().engine.session().edit.selected_tracks, want, "drag from row {from} to row {to}");
    }
}

#[test]
fn the_frame_a_drag_reaches_a_row_paints_it_selected() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 3), lane_point(&h, 2) + vec2(60.0, 0.0));
    start_drag(&mut h, a);
    h.hover_at(b);
    h.step();
    assert_eq!(highlighted_rows(&h), [2, 3]);
    h.drop_at(b);
    h.step();
}

#[test]
fn a_drag_paints_each_frame_once() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 3), lane_point(&h, 0) + vec2(300.0, 0.0));
    start_drag(&mut h, a);
    for k in 1..=10 {
        h.hover_at(a + (b - a) * (k as f32 / 10.0));
        h.step();
        assert_eq!(h.output().platform_output.num_completed_passes, 1, "move {k}");
    }
    h.drop_at(b);
    h.step();
}

#[test]
fn a_drag_frame_changes_the_document_once() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 3), lane_point(&h, 2) + vec2(60.0, 0.0));
    start_drag(&mut h, a);
    let rev = h.state().engine.revision;
    h.hover_at(b);
    h.step();
    assert_eq!(h.state().engine.revision, rev + 1);
    h.drop_at(b);
    h.step();
}

#[test]
fn a_drag_selection_starts_where_the_button_went_down() {
    let mut h = harness(4);
    let a = lane_point(&h, 1);
    drag(&mut h, a, a + vec2(200.0, 0.0));
    let s = h.state().engine.session();
    assert_eq!(s.edit.selection.start, sample_at(s, timeline(&h), a.x));
}

#[test]
fn pressing_in_a_lane_places_the_insertion_point_before_release() {
    let mut h = harness(4);
    let a = lane_point(&h, 2);
    h.hover_at(a);
    h.drag_at(a);
    h.step();
    let s = h.state().engine.session();
    assert_eq!(s.edit.selection, Range::point(sample_at(s, timeline(&h), a.x)));
    assert_eq!(s.edit.selected_tracks, row_ids(&h)[2..3]);
    h.drop_at(a);
    h.step();
}

#[test]
fn a_moved_insertion_point_shows_at_once_even_mid_blink() {
    let mut h = harness(4);
    let (a, b) = (lane_point(&h, 0), lane_point(&h, 2) + vec2(150.0, 0.0));
    click(&mut h, a);
    for _ in 0..90 {
        if insertion_rows(&h).is_empty() {
            break;
        }
        h.step();
    }
    assert!(insertion_rows(&h).is_empty(), "the insertion point never blinked off");
    click(&mut h, b);
    assert!(!insertion_rows(&h).is_empty());
}

#[test]
fn the_insertion_point_shows_only_on_selected_tracks() {
    let mut h = harness(4);
    let a = lane_point(&h, 2);
    click(&mut h, a);
    assert_eq!(insertion_rows(&h), [2]);
}

#[test]
fn shift_click_adds_another_track_to_the_selection() {
    let mut h = harness(4);
    let ids = row_ids(&h);
    let (a, b, c) = (lane_point(&h, 0), lane_point(&h, 2) + vec2(100.0, 0.0), lane_point(&h, 3) + vec2(300.0, 0.0));
    drag(&mut h, a, a + vec2(200.0, 0.0));
    let sel = h.state().engine.session().edit.selection;
    click_with(&mut h, b, egui::Modifiers::SHIFT);
    assert_eq!(h.state().engine.session().edit.selected_tracks, [ids[0], ids[2]]);
    assert_eq!(h.state().engine.session().edit.selection, sel);
    click_with(&mut h, c, egui::Modifiers::SHIFT);
    let s = h.state().engine.session();
    assert_eq!(s.edit.selected_tracks, [ids[0], ids[2], ids[3]]);
    assert_eq!(s.edit.selection, Range::new(sel.start, sample_at(s, timeline(&h), c.x)));
    assert_eq!(highlighted_rows(&h), [0, 2, 3]);
}

#[test]
fn a_quick_drag_from_a_clip_edge_trims_it() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xe > tl.min.x + 100.0 && xe < tl.max.x - 20.0 && xe - xs > 100.0);
    let press = pos2(x_of(h.state().engine.session(), tl, clip.end()) - 2.0, lane.min.y + lane.height() * 0.75);
    quick_drag(&mut h, press, press - vec2(40.0, 0.0));
    let after = h.state().engine.session().find_clip(clip.id).unwrap().1.clone();
    assert_eq!(after.start, clip.start, "the clip moved instead of being trimmed");
    assert!(after.end() < clip.end(), "the clip end did not move");
}

#[test]
fn a_quick_clip_drag_keeps_the_clip_under_the_pointer() {
    let mut h = smart_demo();
    let tl = timeline(&h);
    let (clip, lane) = clip_on_screen(&h, |xs, xe| xe - xs > 120.0 && (xs + xe) / 2.0 > tl.min.x + 60.0 && (xs + xe) / 2.0 < tl.max.x - 60.0);
    let s = h.state().engine.session();
    let grab = pos2((x_of(s, tl, clip.start) + x_of(s, tl, clip.end())) / 2.0, lane.min.y + lane.height() * 0.75);
    let by = sample_at(s, tl, grab.x + 40.0) - sample_at(s, tl, grab.x);
    quick_drag(&mut h, grab, grab + vec2(40.0, 0.0));
    let after = h.state().engine.session().find_clip(clip.id).unwrap().1.clone();
    assert_eq!(after.start - clip.start, by, "the clip trails the pointer");
}

#[test]
fn a_drag_keeps_going_when_its_row_scrolls_away() {
    let mut h = harness(20);
    let (ids, a) = (row_ids(&h), lane_point(&h, 0));
    start_drag(&mut h, a);
    h.state_mut().edit_layout.scroll_y = 500.0;
    h.step();
    assert!(row(&h, 0).max.y < timeline(&h).min.y, "the first row is still on screen");
    let b = lane_point(&h, 8);
    h.hover_at(b);
    h.drop_at(b);
    h.step();
    assert_eq!(h.state().engine.session().edit.selected_tracks, ids[..=8]);
    assert!(h.state().gesture.is_none(), "the drag outlived the release");
}

#[test]
fn a_gesture_left_over_after_the_release_is_dropped() {
    let mut h = harness(1);
    h.state_mut().gesture = Some(Gesture::Scrub { last: 0 });
    h.step();
    assert!(h.state().gesture.is_none());
}

#[test]
fn a_flick_past_the_last_track_selects_down_to_it() {
    let mut h = harness(4);
    let ids = row_ids(&h);
    let a = lane_point(&h, 1);
    let below = pos2(a.x + 60.0, row(&h, 3).max.y + 100.0);
    start_drag(&mut h, a);
    h.hover_at(below);
    h.drop_at(below);
    h.step();
    assert_eq!(h.state().engine.session().edit.selected_tracks, ids[1..]);
}

#[test]
fn dragging_past_the_bottom_scrolls_and_keeps_selecting() {
    let mut h = harness(20);
    let first = row_ids(&h)[0];
    let a = lane_point(&h, 0);
    let below = pos2(a.x + 60.0, timeline(&h).max.y + 40.0);
    start_drag(&mut h, a);
    h.hover_at(below);
    h.step();
    h.run_steps(60);
    assert!(h.state().edit_layout.scroll_y > 300.0, "scrolled {} px", h.state().edit_layout.scroll_y);
    h.drop_at(below);
    h.step();
    let tl = timeline(&h);
    let last_seen = h.state().edit_layout.rows.iter().filter(|(_, r)| r[1] < tl.max.y).count();
    let sel = &h.state().engine.session().edit.selected_tracks;
    assert_eq!((sel.first(), sel.len()), (Some(&first), last_seen));
}

#[test]
fn dragging_past_the_right_edge_scrolls_the_timeline() {
    let mut h = harness(2);
    let a = lane_point(&h, 0);
    let right = pos2(timeline(&h).max.x + 40.0, a.y);
    let s = h.state().engine.session();
    let (before, pointed) = (s.edit.zoom.scroll, sample_at(s, timeline(&h), right.x));
    start_drag(&mut h, a);
    h.hover_at(right);
    h.step();
    h.run_steps(30);
    h.drop_at(right);
    h.step();
    let s = h.state().engine.session();
    assert!(s.edit.zoom.scroll > before, "the timeline did not scroll");
    assert!(s.edit.selection.end > pointed, "the selection did not follow the scroll");
}

#[test]
fn a_pencil_click_on_a_plugin_lane_uses_the_plugin_range() {
    let mut engine = transit_engine::demo::demo_engine();
    engine.execute("track.view", &json!({"track": "Kick", "view": "plugin:a:input_gain"})).unwrap();
    engine.execute("edit.tool", &json!({"tool": "pencil"})).unwrap();
    let mut h = harness_with(engine);
    let s = h.state().engine.session_arc();
    let kick = s.track_by_name("Kick").unwrap();
    let i = h.state().edit_layout.rows.iter().position(|(id, _)| *id == kick.id.0).unwrap();
    let lane = lane_rect(kick, row(&h, i), timeline(&h));
    // A quarter up the lane: -12 dB in the Input range of -24..24 dB.
    let at = pos2(lane.min.x + 100.0, lane.max.y - 4.0 - (lane.height() - 8.0) * 0.25);
    click(&mut h, at);
    let s = h.state().engine.session_arc();
    let param = AutoParam::Plugin { slot: 0, param: "input_gain".into() };
    let points = s.track_by_name("Kick").and_then(|t| t.lane(&param)).map(|l| l.points.clone()).unwrap_or_default();
    assert_eq!(points.len(), 1, "status: {}", h.state().ui.status);
    assert!((points[0].value + 12.0).abs() < 0.01, "{}", points[0].value);
}
