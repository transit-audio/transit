use crate::tests::*;

#[test]
fn the_loop_handle_is_the_header_corner_clear_of_the_trim_edge_and_the_move_area() {
    use crate::arrange::{DragKind, loop_ghosts, zone};
    use egui::pos2;
    // A 200-point wide clip at x 100..300, y 50..110 (the header strip is its top 14 points).
    let r = [100.0, 50.0, 300.0, 110.0];
    assert_eq!(zone(r, pos2(290.0, 55.0), false), DragKind::Loop, "upper right of the header");
    assert_eq!(zone(r, pos2(281.0, 63.0), false), DragKind::Loop, "the zone is 20 points wide, the strip tall");
    assert_eq!(zone(r, pos2(299.0, 55.0), false), DragKind::TrimEnd, "the right edge still trims, in the header too");
    assert_eq!(zone(r, pos2(294.0, 55.0), false), DragKind::TrimEnd, "up to the edge width");
    assert_eq!(zone(r, pos2(290.0, 80.0), false), DragKind::Move, "below the strip: move");
    assert_eq!(zone(r, pos2(270.0, 55.0), false), DragKind::Move, "the rest of the header moves");
    assert_eq!(zone(r, pos2(200.0, 55.0), false), DragKind::Move);
    assert_eq!(zone(r, pos2(101.0, 55.0), false), DragKind::TrimStart);
    // A narrow clip keeps a move area between the edges and the loop zone.
    let n = [0.0, 0.0, 24.0, 60.0];
    assert_eq!(zone(n, pos2(12.0, 5.0), false), DragKind::Move);
    assert_eq!(zone(n, pos2(23.0, 5.0), false), DragKind::TrimEnd);
    // The ghosts: whole repeats, then a tail trimmed at the drop point; nothing at or before the end.
    assert_eq!(loop_ghosts(100, 100, 450), vec![(100, 200), (200, 300), (300, 400), (400, 450)]);
    assert_eq!(loop_ghosts(100, 100, 300), vec![(100, 200), (200, 300)]);
    assert!(loop_ghosts(100, 100, 100).is_empty());
    assert!(loop_ghosts(100, 100, 40).is_empty());
}

/// The loop-handle rig: a bar-long clip and the point in its header corner.
fn loop_rig() -> (CopyRig, ClipId, (f32, f32), f32) {
    let r = copy_rig();
    let clip = ClipId(clips_on(&r.app, r.t)[0].0);
    let rect = r.app.arrange.layout.clips.iter().find(|c| c.id == clip.0).unwrap().rect;
    let handle = (rect[2] - 12.0, rect[1] + 5.0);
    let px_bar = r.px_per_beat * 4.0;
    (r, clip, handle, px_bar)
}

/// The point on the clip that now ends furthest right, in its header corner.
fn handle_of_last(r: &CopyRig) -> (f32, f32) {
    let rect = r.app.arrange.layout.clips.iter().map(|c| c.rect).fold([0.0_f32; 4], |a, b| if b[2] > a[2] { b } else { a });
    (rect[2] - 12.0, rect[1] + 5.0)
}

#[test]
fn dragging_the_loop_handle_extends_the_clip_as_cycles_in_one_undo_step() {
    let (mut r, clip, handle, px_bar) = loop_rig();
    let bar = 4 * r.beat;
    let before = undo_len(&mut r.app);
    // Two bars right at the beat grid: one clip, three cycles long.
    drag(&mut r.app, handle, (handle.0 + 2.0 * px_bar, handle.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, 3 * bar)], "a looped clip, not copies");
    let c = the_clip(&r.app, clip);
    assert_eq!(c.looping, Some(transit_model::ClipLoop { start: 0, end: bar }));
    assert_eq!(c.loop_id, None);
    assert_eq!(c.cycle_starts(0, c.length).collect::<Vec<_>>(), vec![bar, 2 * bar], "two seams");
    assert_eq!(undo_len(&mut r.app), before + 1, "one undo step");
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(0, bar)], "one undo restores the clip");
    assert_eq!(the_clip(&r.app, clip).looping, None);
}

#[test]
fn a_loop_drag_off_the_grid_cuts_the_last_cycle_and_shift_leaves_it_at_the_pointer() {
    let (mut r, clip, handle, px_bar) = loop_rig();
    let bar = 4 * r.beat;
    // 2.5 bars: on a beat, so the last cycle is half a bar.
    drag(&mut r.app, handle, (handle.0 + 2.5 * px_bar, handle.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, 3 * bar + bar / 2)]);
    r.app.run("edit.undo", json!({})).unwrap();
    // 2.37 bars with snap on: the clip ends on the grid.
    drag(&mut r.app, handle, (handle.0 + 2.37 * px_bar, handle.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    let end = the_clip(&r.app, clip).end();
    assert_eq!(end % r.beat, 0, "ends on the grid: {end}");
    r.app.run("edit.undo", json!({})).unwrap();
    // Shift: cut at the pointer.
    drag(&mut r.app, handle, (handle.0 + 2.37 * px_bar, handle.1), json!({"shift": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    let end = the_clip(&r.app, clip).end();
    assert!((end - (3.37 * bar as f64) as i64).abs() < r.beat / 8, "Shift cuts at the pointer: {end}");
    assert_ne!(end % r.beat, 0);
}

#[test]
fn dragging_the_loop_handle_back_shortens_and_back_to_the_first_cycle_unloops() {
    let (mut r, clip, handle, px_bar) = loop_rig();
    let bar = 4 * r.beat;
    drag(&mut r.app, handle, (handle.0 + 3.0 * px_bar, handle.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, 4 * bar)]);
    let h2 = handle_of_last(&r);
    let before = undo_len(&mut r.app);
    drag(&mut r.app, h2, (h2.0 - px_bar, h2.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, 3 * bar)], "one cycle shorter, still a loop");
    assert!(the_clip(&r.app, clip).looping.is_some());
    assert_eq!(undo_len(&mut r.app), before + 1);
    let h3 = handle_of_last(&r);
    drag(&mut r.app, h3, (h3.0 - 2.0 * px_bar, h3.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, bar)], "back to the first cycle unloops");
    assert_eq!(the_clip(&r.app, clip).looping, None);
}

#[test]
fn alt_on_the_loop_handle_makes_repeats_as_independent_copies() {
    let (mut r, _, handle, px_bar) = loop_rig();
    let bar = 4 * r.beat;
    let before = undo_len(&mut r.app);
    drag(&mut r.app, handle, (handle.0 + 2.0 * px_bar, handle.1), json!({"alt": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, bar), (bar, 2 * bar), (2 * bar, 3 * bar)]);
    assert_eq!(undo_len(&mut r.app), before + 1);
    let s = r.app.engine.session();
    assert!(clips_on(&r.app, r.t).iter().all(|c| {
        let c = s.find_clip(ClipId(c.0)).unwrap().1;
        c.loop_id.is_none() && c.looping.is_none()
    }));
}

#[test]
fn a_loop_drag_that_changes_nothing_adds_no_undo_step() {
    let (mut r, _, handle, px_bar) = loop_rig();
    let before = undo_len(&mut r.app);
    // Left on an unlooped clip: nothing to loop, nothing cut.
    drag(&mut r.app, handle, (handle.0 - 0.5 * px_bar, handle.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(clips_on(&r.app, r.t).len(), 1);
    assert_eq!(undo_len(&mut r.app), before);
}

#[test]
fn a_looped_clip_is_one_region_divided_into_cycles() {
    let (mut r, clip, handle, px_bar) = loop_rig();
    let bar = 4 * r.beat;
    drag(&mut r.app, handle, (handle.0 + 1.5 * px_bar, handle.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    let c = the_clip(&r.app, clip);
    let l = c.looping.unwrap();
    let segs: Vec<_> = crate::arrange::cycle_segments(&c, l).collect();
    assert_eq!(segs, vec![(0, bar, 0), (bar, 2 * bar, 0), (2 * bar, 2 * bar + bar / 2, 0)], "the last cycle is cut");
    assert_eq!(r.app.arrange.layout.clips.len(), 1, "one clip rect");
}

// ---- #70: loop follow-ups ----

fn two_note_clip(s: &transit_model::Session, notes: &[(u8, i64)]) -> (transit_model::Clip, i64) {
    let q = transit_time::TICKS_PER_QUARTER;
    let seq = transit_midi::Sequence {
        notes: notes
            .iter()
            .map(|&(pitch, beat)| transit_midi::Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: beat * q, length: q })
            .collect(),
        ..Default::default()
    };
    let beat = s.tempo.tick_to_samples(q, s.sample_rate);
    (transit_model::Clip::midi(ClipId(900), "m", 0, 2 * beat, seq), beat)
}

#[test]
fn loop_ghost_cycles_show_the_notes_that_cycle_plays() {
    let app = app();
    let s = app.engine.session();
    let (c, beat) = two_note_clip(s, &[(60, 0), (64, 1)]);
    let l = transit_model::ClipLoop { start: 0, end: 2 * beat };
    // A ghost cycle that begins at the seam plays both notes at their own offsets.
    let spans = crate::arrange::loop_note_spans(s, &c, l, true, 2 * beat);
    assert_eq!(spans, vec![(2 * beat, 3 * beat, 60), (3 * beat, 4 * beat, 64)]);
    // A loop range that starts at beat 1: later cycles skip the intro note, and a ghost that starts
    // at the loop start (phase 1 beat) puts the beat 1 note at the ghost's left edge.
    let l = transit_model::ClipLoop { start: beat, end: 2 * beat };
    let spans = crate::arrange::loop_note_spans(s, &c, l, true, 2 * beat);
    assert_eq!(spans, vec![(2 * beat, 3 * beat, 64)], "the intro note is not in a ghost cycle");
    // The first pass (not a later cycle) keeps the intro note.
    let first = crate::arrange::loop_note_spans(s, &c, l, false, 0);
    assert_eq!(first.len(), 2);
    // A note that crosses the seam ends at it.
    let (long, beat) = {
        let mut x = c.clone();
        if let transit_model::ClipContent::Midi { sequence } = &mut x.content {
            sequence.notes[1].length = 3 * transit_time::TICKS_PER_QUARTER;
        }
        (x, beat)
    };
    let spans = crate::arrange::loop_note_spans(s, &long, transit_model::ClipLoop { start: 0, end: 2 * beat }, true, 2 * beat);
    assert_eq!(spans[1], (3 * beat, 4 * beat, 64));
}

#[test]
fn an_off_tick_loop_edge_draws_its_cycle_notes_where_playback_plays_them() {
    use transit_model::{ChannelFormat, Clip, ClipLoop, Session, TrackKind};
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let (c, beat) = two_note_clip(&s, &[(60, 0), (64, 1)]);
    // The loop edge is 10 samples past the second beat tick (25 samples per tick): playback rounds
    // it to the tick, so the second cycle starts at beat 2.
    let l = ClipLoop { start: 0, end: 2 * beat + 10 };
    let mut clip = Clip::midi(ClipId(900), "m", 0, 4 * beat, Default::default());
    clip.content = c.content.clone();
    clip.looping = Some(l);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip.clone());
    let heard: Vec<(i64, u8)> = transit_mix::clip_note_ons(&s, t, 0, (4 * beat) as usize);
    let mut drawn: Vec<(i64, u8)> = crate::arrange::loop_note_spans(&s, &clip, l, false, 0).into_iter().map(|(a, _, k)| (a, k)).collect();
    drawn.extend(crate::arrange::loop_note_spans(&s, &clip, l, true, l.end).into_iter().map(|(a, _, k)| (a, k)));
    drawn.sort();
    assert_eq!(drawn, heard);
    assert_eq!(heard[2], (2 * beat, 60), "the second cycle starts on the rounded tick");
}

#[test]
fn a_loop_drag_stops_at_the_next_clip_and_ctrl_overwrites_it_in_one_undo_step() {
    let (mut r, clip, handle, px_bar) = loop_rig();
    let bar = 4 * r.beat;
    r.app.run("midi.new_clip", json!({"tracks": [r.t.0], "start": 2 * bar, "end": 5 * bar})).unwrap();
    frames(&mut r.app, &r.ctx, 5, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, bar), (2 * bar, 5 * bar)]);
    // Without the modifier the loop (and its ghosts) stop at the neighbor.
    drag(&mut r.app, handle, (handle.0 + 3.0 * px_bar, handle.1), json!({}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, 2 * bar), (2 * bar, 5 * bar)]);
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(0, bar), (2 * bar, 5 * bar)]);
    // Ctrl: the loop runs on and the neighbor is trimmed, as one undo step.
    let before = undo_len(&mut r.app);
    drag(&mut r.app, handle, (handle.0 + 3.0 * px_bar, handle.1), json!({"cmd": true}));
    frames(&mut r.app, &r.ctx, 20, &mut r.time);
    assert_eq!(spans(&r.app, r.t), vec![(0, 4 * bar), (4 * bar, 5 * bar)]);
    assert!(the_clip(&r.app, clip).looping.is_some());
    assert_eq!(undo_len(&mut r.app), before + 1);
    r.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(spans(&r.app, r.t), vec![(0, bar), (2 * bar, 5 * bar)]);
}
