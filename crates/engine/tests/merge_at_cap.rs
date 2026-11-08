//! Merged gestures and `squash_undo` at and below `history_limit`.

use serde_json::json;
use transit_engine::Engine;
use transit_model::{ChannelFormat, TrackId, TrackKind};

fn setup(fill: usize) -> (Engine, TrackId) {
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    for i in 0..fill {
        let pan = if i % 2 == 0 { 0.5 } else { -0.5 };
        e.execute("mix.pan", &json!({"track": t.0, "pan": pan})).unwrap();
    }
    (e, t)
}

fn vol(e: &Engine, t: TrackId) -> f32 {
    e.session().track(t).unwrap().mixer.volume_db
}

fn gesture(e: &mut Engine, t: TrackId, n: usize) {
    for i in 0..n {
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -0.1 * (i as f64 + 1.0)}), "fader").unwrap();
    }
    e.end_merge();
}

fn check(fill: usize, n: usize, limit: usize) {
    let (mut e, t) = setup(0);
    e.history_limit = limit;
    for i in 0..fill {
        e.execute("mix.pan", &json!({"track": t.0, "pan": if i % 2 == 0 { 0.5 } else { -0.5 }})).unwrap();
    }
    let before = e.undo_history().len();
    let last = (-0.1 * n as f64) as f32;
    gesture(&mut e, t, n);
    assert_eq!(e.undo_history().len(), (before + 1).min(limit), "fill {fill} n {n}");
    assert!((vol(&e, t) - last).abs() < 1e-4);
    assert!(e.undo());
    assert_eq!(vol(&e, t), 0.0, "one undo restores the pre-gesture state");
    assert!(e.redo());
    assert!((vol(&e, t) - last).abs() < 1e-4);
}

#[test]
fn gesture_at_the_cap_is_one_step() {
    check(200, 50, 200);
}

#[test]
fn gesture_below_the_cap_is_one_step() {
    check(10, 50, 200);
}

#[test]
fn gesture_longer_than_the_limit_is_still_one_step() {
    // The first command of a gesture may push the oldest step off a full history, but the merged
    // commands after it never do, so the pre-gesture snapshot survives however long the drag runs.
    check(200, 250, 200);
}

#[test]
fn squash_undo_at_the_cap_folds_the_right_steps() {
    let (mut e, t) = setup(200);
    assert_eq!(e.undo_history().len(), 200);
    for db in [-1.0, -2.0, -3.0] {
        e.execute("mix.volume", &json!({"track": t.0, "db": db})).unwrap();
    }
    assert_eq!(e.undo_history().len(), 200);
    e.execute("edit.squash_undo", &json!({"n": 3, "label": "Take"})).unwrap();
    assert_eq!((e.undo_history().len(), e.undo_label()), (198, Some("Take")));
    e.undo();
    assert_eq!(vol(&e, t), 0.0);
}

/// A merged gesture whose first command changes nothing (a quantize of notes already on the grid)
/// still adds exactly one step, and leaves the step before it alone (#329: cross-clip edits run one
/// command per clip under one key).
#[test]
fn a_gesture_whose_first_command_changes_nothing_is_still_one_step_of_its_own() {
    let mut e = Engine::default();
    let tracks = e.execute("track.new", &json!({"kind": "instrument", "count": 2})).unwrap();
    let ids: Vec<u64> = tracks["tracks"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    let clip = |e: &mut Engine, t: u64| e.execute("midi.new_clip", &json!({"tracks": [t], "start": 0, "end": 0})).unwrap()["clip"].as_u64().unwrap();
    let (a, b) = (clip(&mut e, ids[0]), clip(&mut e, ids[1]));
    e.execute("midi.note_add", &json!({"clip": a, "pitch": 60, "start_ticks": 0, "length_ticks": 240})).unwrap();
    // The step before the gesture: an off-grid note in B.
    e.execute("midi.note_add", &json!({"clip": b, "pitch": 64, "start_ticks": 100, "length_ticks": 240})).unwrap();
    e.execute("edit.select", &json!({"clips": [a, b]})).unwrap();
    let steps = e.undo_history().len();
    let quantize = |clip: u64| json!({"clip": clip, "indices": [0], "grid": 240, "strength": 100, "swing": 0, "quantize_ends": false});
    e.execute_merged("event.quantize", &quantize(a), "q").unwrap();
    e.execute_merged("event.quantize", &quantize(b), "q").unwrap();
    e.end_merge();
    let b_note = |e: &Engine| match &e.session().find_clip(transit_model::ClipId(b)).unwrap().1.content {
        transit_model::ClipContent::Midi { sequence } => sequence.notes.iter().map(|n| n.start).collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    assert_eq!(b_note(&e), vec![0], "B's note quantized");
    assert_eq!(e.undo_history().len(), steps + 1, "the gesture is one step of its own");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(b_note(&e), vec![100], "one undo reverts only the quantize; B's note is still there");
}
