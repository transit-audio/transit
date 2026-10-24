use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_midi::Note;
use soundcraft_model::{ChannelFormat, ClipContent, ClipId, TrackKind};

fn fixture() -> (Engine, ClipId) {
    let mut e = Engine::default();
    let track = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let clip = ClipId(e.execute("midi.new_clip", &json!({"track": track.0})).unwrap()["clip"].as_u64().unwrap());
    for (pitch, start, length) in [(60, 100, 220), (62, 480, 330), (64, 990, 140)] {
        e.execute(
            "midi.note_add",
            &json!({"clip": clip.0, "pitch": pitch, "start_ticks": start, "length_ticks": length, "velocity": 73, "channel": 3}),
        )
        .unwrap();
    }
    (e, clip)
}

fn notes(e: &Engine, clip: ClipId) -> Vec<Note> {
    match &e.session().find_clip(clip).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.clone(),
        _ => panic!("fixture must be MIDI"),
    }
}

#[test]
fn quantize_strength_selection_and_ends_are_one_atomic_undo() {
    for (strength, ends, start, length) in [(0, true, 100, 220), (50, false, 50, 220), (50, true, 50, 230), (100, true, 0, 240)] {
        let (mut e, clip) = fixture();
        let before = notes(&e, clip);
        let undo = e.undo_history().len();
        let result = e
            .execute("event.quantize", &json!({"clip": clip.0, "indices": [0, 0], "grid": 240, "strength": strength, "quantize_ends": ends}))
            .unwrap();
        let after = notes(&e, clip);
        assert_eq!(after[0], Note { start, length, ..before[0] });
        assert_eq!(&after[1..], &before[1..], "unselected notes stay unchanged");
        assert_eq!(result["notes"], 1, "duplicate indices apply only once");
        assert_eq!(serde_json::from_value::<Vec<Note>>(result["selected_notes"].clone()).unwrap(), vec![after[0]]);
        assert_eq!(e.undo_history().len(), undo + usize::from(strength != 0));
        if strength != 0 {
            e.execute("edit.undo", &json!({})).unwrap();
            assert_eq!(notes(&e, clip), before);
            e.execute("edit.redo", &json!({})).unwrap();
            assert_eq!(notes(&e, clip), after);
        }
    }
}

#[test]
fn quantize_eighth_swing_delays_offbeats_by_stated_fraction() {
    for (strength, swing, expected) in [(100, 100, 640), (100, 50, 560), (50, 100, 560), (0, 100, 480)] {
        let (mut e, clip) = fixture();
        let before = notes(&e, clip);
        e.execute("event.quantize", &json!({"clip": clip.0, "indices": [1], "grid": "1/8", "strength": strength, "swing": swing})).unwrap();
        let after = notes(&e, clip);
        assert_eq!(after[1], Note { start: expected, ..before[1] });
        assert_eq!(after[0], before[0]);
        assert_eq!(after[2], before[2]);
    }
}

#[test]
fn quantize_rejects_bad_selection_before_changing_any_note() {
    let (mut e, clip) = fixture();
    let before = notes(&e, clip);
    let undo = e.undo_history().len();
    for indices in [json!([0, 99]), json!([0, -1]), json!([0, 0.5]), json!(null), json!("all")] {
        assert!(e.execute("event.quantize", &json!({"clip": clip.0, "indices": indices})).is_err());
        assert_eq!(notes(&e, clip), before);
    }
    assert!(e.execute("event.quantize", &json!({"clips": [clip.0, clip.0], "indices": [0]})).is_err());
    assert!(e.execute("event.quantize", &json!({"clip": clip.0, "quantize_ends": "yes"})).is_err());
    e.execute("event.quantize", &json!({"clip": clip.0, "indices": []})).unwrap();
    assert_eq!(notes(&e, clip), before);
    assert_eq!(e.undo_history().len(), undo);
}

#[test]
fn quantize_whole_clip_keeps_legacy_contract_and_supports_note_ends() {
    let (mut e, clip) = fixture();
    e.execute("event.quantize", &json!({"clip": clip.0, "grid": 240, "quantize_ends": true})).unwrap();
    let after = notes(&e, clip);
    assert_eq!((after[0].start, after[0].length), (0, 240));
    assert_eq!((after[1].start, after[1].length), (480, 240));
    assert_eq!((after[2].start, after[2].length), (960, 240));
}
