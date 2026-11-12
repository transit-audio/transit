//! #327: notes played during the record count-in (pickups) join the take, and the clip starts at
//! the first of them (Logic's behavior). A synthetic pass stands in for the audio thread's
//! capture, as in `midi_record_tests` (120 bpm, 4/4, 48 kHz: a beat is 24,000 samples and 960
//! ticks); the player's stamping is tested in `transit-playback`.

use crate::{Services, StudioApp};
use serde_json::json;
use transit_engine::Engine;
use transit_midi::Sequence;
use transit_mix::{Message, NoteId};
use transit_model::{ClipContent, TrackId};
use transit_playback::MidiTake;
use transit_playback::midi_capture::CapturedEvent;
use transit_playback::record::start::Point;
use transit_playback::timeline_map::Position;

const BEAT: i64 = 24_000;
const B: u64 = 24_000;
const TICKS: i64 = transit_time::TICKS_PER_QUARTER;
/// The device frame the synthetic latch sits at, far enough in for a count-in before it.
const LATCH: u64 = 1_000_000;

fn app() -> (StudioApp, TrackId) {
    let mut app = StudioApp::new(Engine::default(), None, Services::default());
    let t = TrackId(app.run("track.new", json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    app.run("mix.record_arm", json!({"tracks": [t.0], "value": true})).unwrap();
    (app, t)
}

fn on(frame: u64, track: TrackId, key: u8) -> CapturedEvent {
    CapturedEvent { frame, track, channel: 0, message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE }, approximate: false }
}

fn off(frame: u64, track: TrackId, key: u8) -> CapturedEvent {
    CapturedEvent { message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE }, ..on(frame, track, key) }
}

/// A finished pass latched at session sample `sample` (device frame [`LATCH`]).
fn pass(track: TrackId, sample: i64, events: Vec<CapturedEvent>) -> MidiTake {
    let start = Some(Point { host_ns: 0, output_frame: LATCH, position: Position { sample, fraction: 0, denominator: 1 } });
    MidiTake { tracks: vec![track], start, events, dropped: 0, device_rate: 48_000 }
}

fn plan(start: i64, pass: Option<i64>) -> transit_engine::io::Recording {
    transit_engine::io::Recording { start, pass, punch: None, rate: 48_000, takes: Vec::new(), midi_journal: None, midi_placed: false, cycle: None }
}

/// Record from `start` to `stop` with `take` as the capture, then Stop through the host.
fn record(app: &mut StudioApp, start: i64, pass_len: Option<i64>, stop: i64, take: MidiTake) {
    app.record_start = start;
    app.take_plan = Some(plan(start, pass_len));
    app.engine.transport.recording = true;
    app.start_play(stop, None, None);
    app.finished_test_midi = Some(take);
    app.stop_play();
}

fn notes(app: &StudioApp, id: transit_model::ClipId) -> Vec<(u8, i64, i64)> {
    let s = app.engine.session_arc();
    let clip = s.tracks.iter().flat_map(|t| t.playlists.iter()).flat_map(|p| p.clips.iter()).find(|c| c.id == id).unwrap();
    let ClipContent::Midi { sequence: Sequence { notes, .. } } = &clip.content else { panic!("not MIDI") };
    notes.iter().map(|n| (n.pitch, n.start, n.length)).collect()
}

fn spans(app: &StudioApp, t: TrackId) -> Vec<(i64, i64)> {
    app.engine.session().track(t).unwrap().clips().iter().map(|c| (c.start, c.length)).collect()
}

#[test]
fn a_pickup_one_beat_early_starts_the_clip_a_beat_before_the_record_start_in_one_undo_step() {
    let (mut app, t) = app();
    let before = app.engine.session_arc();
    let undo = app.engine.undo_history().len();
    // Played on the last beat of the count-in, then on the downbeat.
    let events = vec![on(LATCH - B, t, 59), off(LATCH - B / 2, t, 59), on(LATCH, t, 60), off(LATCH + B, t, 60)];
    record(&mut app, 4 * BEAT, None, 8 * BEAT, pass(t, 4 * BEAT, events));
    assert_eq!(spans(&app, t), [(3 * BEAT, 5 * BEAT)], "the clip starts at the pickup");
    let id = app.engine.session().track(t).unwrap().clips()[0].id;
    // Both notes at their absolute positions: the pickup at beat 3, the downbeat at beat 4.
    assert_eq!(notes(&app, id), [(59, 0, TICKS / 2), (60, TICKS, TICKS)]);
    assert!(app.status.starts_with("Recorded 1 MIDI clip (2 notes)"), "the count includes the pickup: {}", app.status);
    assert_eq!(app.engine.undo_history().len(), undo + 1);
    let placed = app.engine.session_arc();
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(*app.engine.session_arc(), *before, "one undo step restores the document");
    app.run("edit.redo", json!({})).unwrap();
    assert_eq!(*app.engine.session_arc(), *placed, "redo places it again");
}

#[test]
fn a_pass_with_no_pickup_places_exactly_as_before() {
    let take = |t: TrackId, stray: bool| {
        let mut events = vec![on(LATCH + B * 3 / 2, t, 60), off(LATCH + B * 5 / 2, t, 60), on(LATCH + 3 * B, t, 64), off(LATCH + B * 7 / 2, t, 64)];
        if stray {
            // A release of a key held from before the count-in is not a pickup.
            events.insert(0, off(LATCH - B, t, 48));
        }
        pass(t, 2 * BEAT, events)
    };
    let (mut plain, t) = app();
    record(&mut plain, 2 * BEAT, None, 6 * BEAT, take(t, false));
    let (mut stray, t2) = app();
    record(&mut stray, 2 * BEAT, None, 6 * BEAT, take(t2, true));
    assert_eq!(spans(&plain, t), [(2 * BEAT, 4 * BEAT)], "the clip starts at the record start, as before");
    let session = |app: &StudioApp| serde_json::to_string(&*app.engine.session_arc()).unwrap();
    assert_eq!(session(&stray), session(&plain), "byte-identical session");
}

#[test]
fn cycle_recording_puts_the_pickup_in_the_first_take_and_leaves_the_second_unchanged() {
    let (mut app, t) = app();
    let lap = 2 * B;
    let events = vec![
        on(LATCH - B / 2, t, 59),
        off(LATCH - B / 4, t, 59),
        on(LATCH, t, 60),
        off(LATCH + 1_000, t, 60),
        on(LATCH + lap + B, t, 62),
        off(LATCH + lap + B + 1_000, t, 62),
    ];
    record(&mut app, 4 * BEAT, Some(2 * BEAT), 4 * BEAT, pass(t, 4 * BEAT, events));
    let tr = app.engine.session().track(t).unwrap().clone();
    assert_eq!(tr.playlists.len(), 2, "a take per lap");
    assert_eq!(tr.active_playlist, 0);
    let clips = |k: usize| -> Vec<_> { tr.playlists[k].clips.iter().map(|c| (c.start, c.length, c.take, notes(&app, c.id))).collect() };
    // Take 1 was placed from the pickup (84,000) to the lap end. ADR 0039 then puts the newest
    // take on top: take 2 covers the lap and plays; the part of take 1 nothing newer covers, the
    // pickup, stays in play beside it (`takes::activate_clip`, today's rule); the rest of take 1
    // is the other take of the stack.
    assert_eq!(
        clips(0),
        [(4 * BEAT - BEAT / 2, BEAT / 2, Some(1), vec![(59, 0, TICKS / 4)]), (4 * BEAT, 2 * BEAT, Some(2), vec![(62, TICKS, 40)])],
        "the pickup of the first take plays before the second take, which is one lap as before"
    );
    assert_eq!(clips(1), [(4 * BEAT, 2 * BEAT, Some(1), vec![(60, 0, 40)])]);
}

#[test]
fn a_pickup_before_the_session_start_is_clamped_at_sample_zero() {
    let (mut app, t) = app();
    // Record start half a beat into the session: a pickup two beats early would land before
    // sample 0.
    let events = vec![on(LATCH - 2 * B, t, 59), off(LATCH - B / 4, t, 59), on(LATCH, t, 60), off(LATCH + B, t, 60)];
    record(&mut app, BEAT / 2, None, 2 * BEAT, pass(t, BEAT / 2, events));
    assert_eq!(spans(&app, t)[0].0, 0);
    let id = app.engine.session().track(t).unwrap().clips()[0].id;
    assert_eq!(notes(&app, id)[0], (59, 0, TICKS / 4), "starts at sample 0, ends where it was released");
}

/// #440: recorded from the session start, count-in notes released before it are left out of the
/// take and the status line says so once; a recording with none left out says nothing about it.
#[test]
fn count_in_notes_left_out_at_the_session_start_are_reported_on_the_status_line() {
    let (mut app, t) = app();
    let events = vec![
        on(LATCH - 3 * B, t, 55),
        off(LATCH - 2 * B, t, 55),
        on(LATCH - 2 * B, t, 57),
        off(LATCH - B, t, 57),
        on(LATCH - B / 2, t, 59),
        off(LATCH + B / 2, t, 59),
        on(LATCH + B, t, 60),
        off(LATCH + 2 * B, t, 60),
    ];
    record(&mut app, 0, None, 3 * BEAT, pass(t, 0, events));
    let id = app.engine.session().track(t).unwrap().clips()[0].id;
    assert_eq!(notes(&app, id), [(59, 0, TICKS / 2), (60, TICKS, TICKS)], "the held note from the session start, then the played one");
    assert!(app.status.starts_with("Recorded 1 MIDI clip (2 notes) on "), "{}", app.status);
    assert!(app.status.ends_with("; 2 count-in notes before the session start were left out"), "{}", app.status);
    assert_eq!(app.status.matches("left out").count(), 1, "said once: {}", app.status);
    // The same count-in from bar 2: every note is a pickup and nothing is left out.
    let (mut later, t) = self::app();
    let events = vec![on(LATCH - 3 * B, t, 55), off(LATCH - 2 * B, t, 55), on(LATCH + B, t, 60), off(LATCH + 2 * B, t, 60)];
    record(&mut later, 4 * BEAT, None, 7 * BEAT, pass(t, 4 * BEAT, events));
    assert!(later.status.starts_with("Recorded 1 MIDI clip (2 notes) on "), "{}", later.status);
    assert!(!later.status.contains("left out"), "{}", later.status);
}

#[test]
fn the_inspected_pass_reports_where_capture_began() {
    let (mut app, _) = app();
    assert!(app.recording_pass_inspect().is_null(), "no pass, nothing to report");
    app.take_plan = Some(plan(8 * BEAT, None));
    app.count_in_plan = Some(transit_playback::CountIn { frames: 4 * BEAT, beat: BEAT, per_bar: 4 });
    assert_eq!(app.recording_pass_inspect(), json!({"record_start": 8 * BEAT, "count_in": 4 * BEAT, "capture_start": 4 * BEAT}));
    app.take_plan = Some(plan(BEAT, None));
    assert_eq!(app.recording_pass_inspect()["capture_start"], 0, "never before the session start");
    app.count_in_plan = None;
    assert_eq!(app.recording_pass_inspect()["capture_start"], BEAT, "no count-in: capture begins at the record start");
}
