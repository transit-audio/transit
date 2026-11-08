//! Several MIDI clips in the piano roll at once (#329, slice 1), driven the way a user drives it.

use super::*;

/// Two instrument tracks with a one-bar clip each, A at bar 1 (C4 at its start) and B at bar 2
/// (E4 at its start), both selected, then A double-clicked in the arrangement.
fn two() -> (T, ClipId) {
    let mut t = T::new();
    let r = t.app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let tb = TrackId(r["tracks"][0].as_u64().unwrap());
    let c = t.app.run("midi.new_clip", json!({"tracks": [tb.0], "start": 96_000, "end": 192_000})).unwrap();
    let b = ClipId(c["clip"].as_u64().unwrap());
    t.add(60, 0, 480);
    t.app.run("midi.note_add", json!({"clip": b.0, "pitch": 64, "start_ticks": 0, "length_ticks": 480})).unwrap();
    t.app.run("edit.select", json!({"clips": [t.clip.0, b.0]})).unwrap();
    t.frames(3);
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
    t.click_n(mid(r), 2, json!({}));
    assert!(t.app.keys.layout.visible, "double-click opened the piano roll");
    (t, b)
}

fn notes_of(t: &T, clip: ClipId) -> Vec<Note> {
    match &t.app.engine.session().find_clip(clip).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => Vec::new(),
    }
}

#[test]
fn several_selected_clips_open_together_with_the_double_clicked_one_in_front() {
    let (t, b) = two();
    let l = &t.app.keys.layout;
    assert_eq!(l.clip, Some(t.clip.0), "the double-clicked clip is the foreground");
    let open: Vec<_> = l.clips.iter().map(|c| (c.clip, c.offset_ticks, c.ticks, c.foreground)).collect();
    assert_eq!(open, vec![(t.clip.0, 0, 3840, true), (b.0, 3840, 3840, false)]);
    assert!(l.clips.iter().all(|c| c.bar[2] > c.bar[0]), "each clip has a bar in the ruler");
    // Both clips' notes are drawn, each tagged with its clip, in the foreground's ticks.
    let drawn: Vec<_> = l.notes.iter().map(|n| (n.pitch, n.start, n.clip)).collect();
    assert_eq!(drawn, vec![(60, 0, Some(t.clip.0)), (64, 3840, Some(b.0))]);
    assert!(t.painted_text.iter().any(|s| s.starts_with("2 clips · ")), "{:?}", t.painted_text);
}

#[test]
fn one_clip_opens_as_before_with_nothing_new_in_inspect() {
    let mut t = T::open();
    t.add(60, 0, 480);
    let v = serde_json::to_value(&t.app.keys.layout).unwrap();
    assert!(v.get("clips").is_none(), "no clip list for one clip");
    assert!(v["notes"][0].get("clip").is_none(), "no clip tag on its notes");
    assert!(!t.painted_text.iter().any(|s| s.contains(" clips · ")));
}

#[test]
fn a_band_across_both_clips_selects_both_and_one_drag_moves_them_in_one_undo_step() {
    let (mut t, b) = two();
    let (from, to) = (t.at(10.0, 66), t.at(4600.0, 58));
    t.drag(from, to, json!({}));
    assert_eq!(t.selected(), 2, "the band took a note from each clip");
    let before = t.undo_len();
    let grab = mid(t.note_rect(60, 0));
    let step = (480.0 * t.app.keys.layout.px_per_tick) as f32;
    t.drag(grab, (grab.0 + step, grab.1 - t.app.keys.layout.key_h * 2.0), json!({}));
    assert_eq!(notes_of(&t, t.clip).iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), vec![(62, 480)]);
    assert_eq!(notes_of(&t, b).iter().map(|n| (n.pitch, n.start)).collect::<Vec<_>>(), vec![(66, 480)]);
    assert_eq!(t.undo_len(), before + 1, "one undo step for both clips");
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert_eq!(notes_of(&t, t.clip)[0].start, 0);
    assert_eq!(notes_of(&t, b)[0].start, 0);
}

#[test]
fn new_notes_go_to_the_clip_they_are_drawn_in_and_are_refused_outside_every_clip() {
    let (mut t, b) = two();
    // Inside the foreground's span.
    t.click_n(t.at(1000.0, 67), 2, json!({}));
    assert!(notes_of(&t, t.clip).iter().any(|n| n.pitch == 67), "drawn in the foreground");
    // Inside B's span: B gets it, at B's own ticks.
    t.click_n(t.at(3840.0 + 1000.0, 69), 2, json!({}));
    let in_b: Vec<_> = notes_of(&t, b).iter().filter(|n| n.pitch == 69).map(|n| n.start).collect();
    assert_eq!(in_b.len(), 1, "drawn in B's span");
    assert!(in_b[0] < 3840, "stored in B's own ticks: {in_b:?}");
    // Past both: refused, nothing added, and a status line says why (zoomed out so it is on screen).
    t.app.ui.keys.px_per_quarter = 40.0;
    t.frames(2);
    let before = (notes_of(&t, t.clip).len(), notes_of(&t, b).len());
    t.click_n(t.at(3840.0 * 2.0 + 500.0, 62), 2, json!({}));
    assert_eq!((notes_of(&t, t.clip).len(), notes_of(&t, b).len()), before);
    assert!(t.app.status.contains("Outside the open clips"), "{}", t.app.status);
}

#[test]
fn a_click_on_the_other_clips_note_or_bar_brings_that_clip_forward_without_moving_anything() {
    let (mut t, b) = two();
    let x_before = t.note_rect(64, 3840)[0];
    t.click(mid(t.note_rect(64, 3840)));
    let l = &t.app.keys.layout;
    assert_eq!(l.clip, Some(b.0), "B is the foreground now");
    assert_eq!(l.clips.iter().find(|c| c.foreground).map(|c| c.clip), Some(b.0));
    // The roll's ticks are now B's: its note is at 0 and A's at -3840, both where they were drawn.
    let x_after = t.note_rect(64, 0)[0];
    assert!((x_after - x_before).abs() < 0.5, "{x_before} -> {x_after}");
    assert!(t.app.keys.layout.notes.iter().any(|n| n.pitch == 60 && n.start == -3840));
    assert_eq!(t.selected(), 1, "the clicked note stays selected through the switch");
    // A's bar in the ruler brings A back.
    let bar = t.app.keys.layout.clips.iter().find(|c| c.clip == t.clip.0).unwrap().bar;
    t.click(mid(bar));
    assert_eq!(t.app.keys.layout.clip, Some(t.clip.0));
}

#[test]
fn select_all_delete_and_transpose_work_across_clips_one_undo_step_each() {
    let (mut t, b) = two();
    t.click(t.at(2000.0, 62));
    t.key("A", json!({"cmd": true}));
    assert_eq!(t.selected(), 2, "Ctrl+A selects every open clip's notes");
    let before = t.undo_len();
    t.key("ArrowUp", json!({"cmd": true}));
    assert_eq!((notes_of(&t, t.clip)[0].pitch, notes_of(&t, b)[0].pitch), (72, 76), "an octave up in both");
    assert_eq!(t.undo_len(), before + 1);
    t.key("Delete", json!({}));
    assert!(notes_of(&t, t.clip).is_empty() && notes_of(&t, b).is_empty());
    assert_eq!(t.undo_len(), before + 2, "one undo step for the delete across both clips");
}

#[test]
fn an_earlier_clip_cannot_be_moved_before_its_own_start() {
    let (mut t, b) = two();
    // Bring B forward: A's notes are then before the roll's zero, and still movable only to A's start.
    t.click(mid(t.note_rect(64, 3840)));
    assert_eq!(t.app.keys.layout.clip, Some(b.0));
    t.click(mid(t.note_rect(60, -3840)));
    t.key("ArrowLeft", json!({}));
    assert_eq!(notes_of(&t, t.clip)[0].start, 0, "stays at A's start");
    t.key("ArrowRight", json!({}));
    assert!(notes_of(&t, t.clip)[0].start > 0, "moved later inside A");
}

// ─── The refutation review of 01fae850 (ADR 0009): each finding fails without its fix ───

/// Like [`two`], but B's clip is at the same bar as A's on its own track, with the same note:
/// a doubled part.
fn doubled() -> (T, ClipId) {
    let mut t = T::new();
    let r = t.app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let tb = TrackId(r["tracks"][0].as_u64().unwrap());
    let c = t.app.run("midi.new_clip", json!({"tracks": [tb.0], "start": 0, "end": 96_000})).unwrap();
    let b = ClipId(c["clip"].as_u64().unwrap());
    t.add(60, 0, 480);
    t.app.run("midi.note_add", json!({"clip": b.0, "pitch": 60, "start_ticks": 0, "length_ticks": 480})).unwrap();
    t.app.run("edit.select", json!({"clips": [t.clip.0, b.0]})).unwrap();
    t.frames(3);
    t.app.toggle_keys();
    t.frames(3);
    assert_eq!(t.app.keys.layout.clips.len(), 2);
    (t, b)
}

#[test]
fn r1_a_cross_clip_quantize_with_one_clip_on_the_grid_is_one_step_of_its_own() {
    let (mut t, b) = two();
    t.app.run("midi.note_add", json!({"clip": b.0, "pitch": 67, "start_ticks": 100, "length_ticks": 240})).unwrap();
    t.frames(2);
    t.click(t.at(2000.0, 62));
    let before = t.undo_len();
    t.key("U", json!({"cmd": true}));
    assert_eq!(t.undo_len(), before + 1, "the quantize is one step of its own");
    assert!(notes_of(&t, b).iter().all(|n| n.start != 100), "B's off-grid note quantized");
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(2);
    assert!(notes_of(&t, b).iter().any(|n| n.pitch == 67 && n.start == 100), "one undo reverts only the quantize");
}

#[test]
fn r2_identical_notes_in_two_clips_are_both_band_selected_and_both_move() {
    let (mut t, b) = doubled();
    t.drag(t.at(10.0, 63), t.at(700.0, 58), json!({}));
    assert_eq!(t.selected(), 2, "the band took the note of each clip");
    t.key("ArrowUp", json!({}));
    assert_eq!((notes_of(&t, t.clip)[0].pitch, notes_of(&t, b)[0].pitch), (61, 61));
}

#[test]
fn r2_a_drag_on_stacked_identical_notes_moves_the_one_it_selected() {
    let (mut t, b) = doubled();
    let grab = mid(t.note_rect(60, 0));
    t.app.keys.audition_tracks.clear();
    t.drag(grab, (grab.0, grab.1 - t.app.keys.layout.key_h * 2.0), json!({}));
    let moved = (notes_of(&t, t.clip)[0].pitch, notes_of(&t, b)[0].pitch);
    assert!(moved == (62, 60) || moved == (60, 62), "exactly one note moved: {moved:?}");
    assert_eq!(t.selected(), 1);
    let sel_clip = t.app.keys.layout.notes.iter().find(|n| n.selected).and_then(|n| n.clip);
    let moved_clip = if moved.0 == 62 { t.clip.0 } else { b.0 };
    assert_eq!(sel_clip, Some(moved_clip), "the selected note is the one that moved");
    let moved_track = t.app.engine.session().find_clip(ClipId(moved_clip)).unwrap().0;
    assert!(t.app.keys.audition_tracks.iter().all(|tr| *tr == moved_track), "the grabbed note sounds on the moved note's track");
}

#[test]
fn r3_dragging_another_clips_note_auditions_on_its_own_track() {
    let (mut t, b) = two();
    let tb = t.app.engine.session().find_clip(b).unwrap().0;
    let grab = mid(t.note_rect(64, 3840));
    t.app.keys.audition_tracks.clear();
    t.drag(grab, (grab.0, grab.1 - t.app.keys.layout.key_h * 2.0), json!({}));
    assert!(!t.app.keys.audition_tracks.is_empty());
    assert!(t.app.keys.audition_tracks.iter().all(|tr| *tr == tb), "{:?} (B is {tb:?})", t.app.keys.audition_tracks);
}

#[test]
fn r4_a_frozen_clip_refuses_the_whole_cross_clip_edit() {
    let (mut t, b) = two();
    let tb = t.app.engine.session().find_clip(b).unwrap().0;
    t.app.run("track.freeze", json!({"tracks": [tb.0]})).unwrap();
    t.frames(3);
    t.click(t.at(2000.0, 62));
    t.key("A", json!({"cmd": true}));
    let before = (notes_of(&t, t.clip), notes_of(&t, b));
    t.key("ArrowUp", json!({}));
    assert_eq!((notes_of(&t, t.clip), notes_of(&t, b)), before, "all or nothing");
    assert!(t.app.status.to_lowercase().contains("frozen"), "{}", t.app.status);
}

#[test]
fn r5_the_bar_drawn_on_top_is_the_one_a_click_brings_forward() {
    let (mut t, b) = doubled();
    let bar = t.app.keys.layout.clips.iter().find(|c| c.clip == b.0).unwrap().bar;
    t.click(mid(bar));
    assert_eq!(t.app.keys.layout.clip, Some(b.0));
}

#[test]
fn r6_ctrl_alt_k_opens_the_current_selection_of_several_not_the_last_clip() {
    let (mut t, b) = two();
    t.key("Escape", json!({}));
    let r = t.app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let tc = r["tracks"][0].as_u64().unwrap();
    let c = ClipId(t.app.run("midi.new_clip", json!({"tracks": [tc], "start": 192_000, "end": 288_000})).unwrap()["clip"].as_u64().unwrap());
    t.app.run("edit.select", json!({"clips": [b.0, c.0]})).unwrap();
    t.frames(2);
    t.app.toggle_keys();
    t.frames(3);
    assert_eq!(t.app.keys.layout.clip, Some(b.0), "the first selected clip is the foreground");
    let open: Vec<u64> = t.app.keys.layout.clips.iter().map(|c| c.clip).collect();
    assert_eq!(open, vec![b.0, c.0]);
}

#[test]
fn r7_when_the_foreground_clip_is_deleted_the_next_one_starts_clean() {
    let (mut t, b) = two();
    t.click(t.at(2000.0, 62));
    assert!(t.app.keys.click_tick.is_some());
    let a = t.clip;
    for tr in &mut t.app.engine.session_mut().tracks {
        if let Some(pl) = tr.playlist_mut() {
            pl.clips.retain(|c| c.id != a);
        }
    }
    t.frames(3);
    assert_eq!(t.app.keys.layout.clip, Some(b.0));
    assert_eq!(t.app.keys.click_tick, None, "A's click position does not carry into B's ticks");
}

// ─── The second refutation review (ADR 0009): each finding fails without its fix ───

/// [`doubled`] with B's C4 960 ticks long and at velocity 110 (A's: 480, 100).
fn doubled_unequal() -> (T, ClipId) {
    let (mut t, b) = doubled();
    t.app.run("midi.note_edit", json!({"clip": b.0, "index": 0, "pitch": 60, "start_ticks": 0, "length_ticks": 960, "velocity": 110})).unwrap();
    t.app.run("midi.note_edit", json!({"clip": t.clip.0, "index": 0, "pitch": 60, "start_ticks": 0, "length_ticks": 480, "velocity": 100})).unwrap();
    t.frames(3);
    (t, b)
}

#[test]
fn s1_a_resize_across_clips_keeps_each_new_length_in_its_own_clip() {
    let (mut t, b) = doubled_unequal();
    // Equal velocities: after A grows to 960 it equals B's old value.
    t.app.run("midi.note_edit", json!({"clip": b.0, "index": 0, "pitch": 60, "start_ticks": 0, "length_ticks": 960, "velocity": 100})).unwrap();
    t.frames(2);
    t.click(t.at(2000.0, 62));
    t.key("A", json!({"cmd": true}));
    assert_eq!(t.selected(), 2);
    let r = t.app.keys.layout.notes.iter().find(|n| n.clip == Some(b.0)).unwrap().rect;
    let edge = (r[2] - 2.0, (r[1] + r[3]) / 2.0);
    let dx = (480.0 * t.app.keys.layout.px_per_tick) as f32;
    t.drag(edge, (edge.0 + dx, edge.1), json!({}));
    assert_eq!((notes_of(&t, t.clip)[0].length, notes_of(&t, b)[0].length), (960, 1440), "each grew by 480 in its own clip");
}

#[test]
fn s1_a_velocity_drag_across_clips_keeps_each_new_velocity_in_its_own_clip() {
    let (mut t, b) = doubled_unequal();
    t.app.run("midi.note_edit", json!({"clip": b.0, "index": 0, "pitch": 60, "start_ticks": 0, "length_ticks": 480, "velocity": 110})).unwrap();
    t.frames(2);
    t.click(t.at(2000.0, 62));
    t.key("A", json!({"cmd": true}));
    let grab = mid(t.app.keys.layout.notes.iter().find(|n| n.clip == Some(t.clip.0)).unwrap().rect);
    t.drag(grab, (grab.0, grab.1 - 15.0), json!({"alt": true}));
    let (a, bv) = (i32::from(notes_of(&t, t.clip)[0].velocity), i32::from(notes_of(&t, b)[0].velocity));
    assert_eq!(bv - a, 10, "the two keep their difference: A {a}, B {bv}");
    assert!(a > 100, "both went up: A {a}, B {bv}");
}

#[test]
fn s2_one_clip_a_band_over_two_identical_notes_selects_one_as_before() {
    let mut t = T::open();
    t.add(60, 0, 480);
    t.add(60, 0, 480);
    t.drag(t.at(10.0, 63), t.at(700.0, 58), json!({}));
    assert_eq!(t.selected(), 1, "as before #329: identical notes in one clip select once");
}

#[test]
fn s3_a_midi_clip_selected_with_an_audio_clip_double_clicks_to_itself_alone() {
    let mut t = T::new();
    let r = t.app.run("track.new", json!({"kind": "audio"})).unwrap();
    let ta = TrackId(r["tracks"][0].as_u64().unwrap());
    let x = t.app.engine.session_mut().new_clip_id();
    let clip = transit_model::Clip::audio(x, "X", transit_model::SourceId(999), 0, 0, 96_000);
    t.app.engine.session_mut().track_mut(ta).unwrap().playlist_mut().unwrap().clips.push(clip);
    t.app.run("edit.select", json!({"clips": [t.clip.0, x.0]})).unwrap();
    t.frames(3);
    let r = t.app.arrange.layout.clips.iter().find(|c| c.id == t.clip.0).unwrap().rect;
    t.click_n(mid(r), 2, json!({}));
    assert!(t.app.keys.layout.visible);
    assert_eq!(t.app.engine.session().edit.selected_clips, vec![t.clip], "the audio clip is not reselected");
}

#[test]
fn s4_an_empty_click_in_another_clips_span_does_not_start_step_entry_past_the_foreground() {
    let (mut t, _) = two();
    t.click(t.at(3840.0 + 1000.0, 62));
    assert!(t.app.keys.click_tick.is_none_or(|at| (0..3840).contains(&at)), "{:?}", t.app.keys.click_tick);
}
