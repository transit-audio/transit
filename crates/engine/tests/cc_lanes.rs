use serde_json::json;
use transit_engine::Engine;
use transit_midi::{CtrlEvent, CtrlKind};
use transit_model::{ClipContent, ClipId};

fn setup() -> (Engine, ClipId) {
    let mut e = Engine::default();
    let track = e.execute("track.new", &json!({"kind":"instrument"})).unwrap()["tracks"][0].clone();
    let id = e.execute("midi.new_clip", &json!({"tracks":[track], "start":0, "end":96000})).unwrap()["clip"].as_u64().unwrap();
    (e, ClipId(id))
}
fn points(e: &Engine, clip: ClipId) -> Vec<CtrlEvent> {
    let ClipContent::Midi { sequence } = &e.session().find_clip(clip).unwrap().1.content else { panic!() };
    sequence.ctrls.clone()
}
#[test]
fn cc_commands_sort_and_undo_without_touching_other_channels() {
    let (mut e, clip) = setup();
    for (tick, channel, controller, value) in [(960, 0, 64, 0), (0, 0, 64, 99), (480, 2, 11, 37)] {
        e.execute("midi.cc_add", &json!({"clip":clip.0,"tick":tick,"channel":channel,"controller":controller,"value":value})).unwrap();
    }
    let original = points(&e, clip);
    assert_eq!(original.iter().map(|c| c.tick).collect::<Vec<_>>(), [0, 480, 960]);
    assert_eq!(original[0].value, 99);
    let before = e.undo_pushes();
    e.execute("midi.cc_edit", &json!({"clip":clip.0,"index":0,"tick":700,"value":73})).unwrap();
    let edited = points(&e, clip);
    assert_eq!(edited[0], original[1]);
    assert_eq!(edited[1], CtrlEvent { tick: 700, value: 73, ..original[0] });
    assert_eq!(e.undo_pushes(), before + 1);
    assert!(e.undo());
    assert_eq!(points(&e, clip), original);
    assert!(e.redo());
    assert_eq!(points(&e, clip), edited);
    e.execute("midi.cc_delete", &json!({"clip":clip.0,"index":1})).unwrap();
    assert_eq!(points(&e, clip), vec![original[1], original[2]]);
    assert!(e.undo());
    assert_eq!(points(&e, clip), edited);
}
#[test]
fn invalid_cc_edits_are_atomic_and_do_not_delete_other_event_types() {
    let (mut e, clip) = setup();
    e.execute("midi.cc_add", &json!({"clip":clip.0,"tick":0,"controller":64,"value":127})).unwrap();
    if let ClipContent::Midi { sequence } = &mut e.session_mut().find_clip_mut(clip).unwrap().content {
        sequence.ctrls.push(CtrlEvent { tick: 480, channel: 0, kind: CtrlKind::PitchBend, value: -1234 });
    }
    let original = points(&e, clip);
    let undo = e.undo_pushes();
    for (command, params) in [
        ("midi.cc_add", json!({"tick":0,"controller":128,"value":1})),
        ("midi.cc_add", json!({"tick":0,"controller":1,"value":1,"channel":16})),
        ("midi.cc_edit", json!({"index":0,"tick":120,"value":128})),
        ("midi.cc_edit", json!({"index":0,"tick":-1})),
        ("midi.cc_edit", json!({"index":0,"value":"127"})),
        ("midi.cc_edit", json!({"index":1,"value":0})),
        ("midi.cc_delete", json!({"index":1})),
        ("midi.cc_delete", json!({"index":999})),
    ] {
        let mut p = params;
        p["clip"] = json!(clip.0);
        assert!(e.execute(command, &p).is_err());
        assert_eq!(points(&e, clip), original);
        assert_eq!(e.undo_pushes(), undo);
    }
}

#[test]
fn recorded_sustain_survives_edit_undo_disk_roundtrip_and_audio_playback() {
    use transit_engine::{
        io,
        midi_take::{self, Msg, RecEvent},
    };
    use transit_model::TrackId;
    use transit_time::Range;
    let mut e = Engine::default();
    let track =
        TrackId(e.execute("track.new", &json!({"kind":"instrument","instrument":"subtractive_synth"})).unwrap()["tracks"][0].as_u64().unwrap());
    let events = [
        (0, Msg::NoteOn { pitch: 60, velocity: 100 }),
        (6000, Msg::Cc { cc: 64, value: 101 }),
        (12000, Msg::NoteOff { pitch: 60, velocity: 64 }),
        (36000, Msg::Cc { cc: 64, value: 12 }),
        (40000, Msg::Cc { cc: 11, value: 43 }),
    ]
    .map(|(offset, msg)| RecEvent { track, channel: 0, offset, msg });
    let plan = io::Recording { start: 0, pass: None, punch: None, rate: 48000, takes: vec![], midi_journal: None, midi_placed: false };
    let clip = midi_take::place_midi_recording(&mut e, &plan, &events, 48000)[0];
    let original = points(&e, clip);
    assert_eq!(original.iter().map(|c| c.value).collect::<Vec<_>>(), [101, 12, 43]);
    assert_eq!(
        original.iter().map(|c| c.tick).collect::<Vec<_>>(),
        [6000, 36000, 40000].map(|sample| e.session().tempo.samples_to_ticks(sample, e.session().sample_rate))
    );
    let render = |e: &Engine| transit_mix::render_range(e.session(), Range::new(0, 60000), 256);
    let recorded = render(&e);
    assert!(recorded.iter().flatten().any(|x| x.abs() > 1e-5));
    e.execute("midi.cc_edit", &json!({"clip":clip.0,"index":0,"value":0})).unwrap();
    assert_ne!(render(&e), recorded, "sustain must affect audible playback");
    assert!(e.undo());
    assert_eq!(points(&e, clip), original);
    assert_eq!(render(&e), recorded);
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("transit-cc-{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("sustain.transit");
    e.execute("session.save_as", &json!({"path":file})).unwrap();
    let mut loaded = Engine::default();
    loaded.execute("session.open", &json!({"path":file})).unwrap();
    assert_eq!(points(&loaded, clip), original);
    assert_eq!(render(&loaded), recorded);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn controller_only_recording_overdubs_and_undo_preserves_the_existing_notes() {
    use transit_engine::{
        io,
        midi_take::{self, Msg, RecEvent},
    };
    use transit_model::TrackId;
    let (mut e, clip) = setup();
    let track: TrackId = e.session().find_clip(clip).unwrap().0;
    e.execute("midi.note_add", &json!({"clip":clip.0,"pitch":60,"start_ticks":0,"length_ticks":480})).unwrap();
    let plan = io::Recording { start: 0, pass: None, punch: None, rate: 48000, takes: vec![], midi_journal: None, midi_placed: false };
    let events = [(6000, 127), (36000, 0)].map(|(offset, value)| RecEvent { track, channel: 3, offset, msg: Msg::Cc { cc: 64, value } });
    let before = e.session().clone();
    let undo = e.undo_pushes();
    let clips = midi_take::place_midi_recording(&mut e, &plan, &events, 48000);
    assert_eq!(clips, vec![clip], "a pedal-only overdub must reach the existing clip");
    let recorded = points(&e, clip);
    let ClipContent::Midi { sequence: before_sequence } = &before.find_clip(clip).unwrap().1.content else { panic!() };
    let ClipContent::Midi { sequence: after_sequence } = &e.session().find_clip(clip).unwrap().1.content else { panic!() };
    assert_eq!(after_sequence.notes, before_sequence.notes, "pedal-only overdub must preserve existing notes");
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        recorded.iter().map(|c| c.tick).collect::<Vec<_>>(),
        [6000, 36000].map(|sample| e.session().tempo.samples_to_ticks(sample, e.session().sample_rate))
    );
    assert_eq!(recorded.iter().map(|c| (c.channel, c.value)).collect::<Vec<_>>(), [(3, 127), (3, 0)]);
    assert_eq!(e.undo_pushes(), undo + 1);
    let serialized = e.session().to_json().unwrap();
    let loaded = transit_model::Session::from_json(&serialized).unwrap();
    assert_eq!(loaded.find_clip(clip).unwrap().1.content, e.session().find_clip(clip).unwrap().1.content);
    assert!(e.undo());
    assert_eq!(e.session().find_clip(clip).unwrap().1.content, before.find_clip(clip).unwrap().1.content);
    assert!(e.redo());
    assert_eq!(points(&e, clip), recorded);
}
