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
