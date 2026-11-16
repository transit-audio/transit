//! Notes played during the record count-in (pickups, #327) join the take, and the clip then covers
//! the whole count-in, before bar 1 too (ADR 0052 decision 8, Logic's behavior). A synthetic pass stands in for the audio thread's
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

pub(crate) const BEAT: i64 = 24_000;
pub(crate) const B: u64 = 24_000;
const TICKS: i64 = transit_time::TICKS_PER_QUARTER;
/// The device frame the synthetic latch sits at, far enough in for a count-in before it.
pub(crate) const LATCH: u64 = 1_000_000;

pub(crate) fn app() -> (StudioApp, TrackId) {
    let mut app = StudioApp::new(Engine::default(), None, Services::default());
    let t = TrackId(app.run("track.new", json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    app.run("mix.record_arm", json!({"tracks": [t.0], "value": true})).unwrap();
    (app, t)
}

pub(crate) fn on(frame: u64, track: TrackId, key: u8) -> CapturedEvent {
    CapturedEvent { frame, track, channel: 0, message: Message::NoteOn { key, velocity: 100, id: NoteId::NONE }, approximate: false }
}

pub(crate) fn off(frame: u64, track: TrackId, key: u8) -> CapturedEvent {
    CapturedEvent { message: Message::NoteOff { key, velocity: 0, id: NoteId::NONE }, ..on(frame, track, key) }
}

/// A finished pass latched at session sample `sample` (device frame [`LATCH`]).
pub(crate) fn pass(track: TrackId, sample: i64, events: Vec<CapturedEvent>) -> MidiTake {
    let start = Some(Point { host_ns: 0, output_frame: LATCH, position: Position { sample, fraction: 0, denominator: 1 } });
    MidiTake { tracks: vec![track], start, events, dropped: 0, device_rate: 48_000 }
}

fn plan(start: i64, pass: Option<i64>, count_in: i64) -> transit_engine::io::Recording {
    transit_engine::io::Recording {
        start,
        pass,
        punch: None,
        rate: 48_000,
        takes: Vec::new(),
        midi_journal: None,
        midi_placed: false,
        cycle: None,
        end: None,
        count_in: Some(count_in),
    }
}

/// Record from `start` after a count-in of `count_in` samples to `stop` with `take` as the
/// capture, then Stop through the host.
pub(crate) fn record(app: &mut StudioApp, start: i64, count_in: i64, pass_len: Option<i64>, stop: i64, take: MidiTake) {
    app.record_start = start;
    app.take_plan = Some(plan(start, pass_len, count_in));
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
fn a_pickup_one_beat_early_starts_the_clip_at_the_count_in_start_in_one_undo_step() {
    let (mut app, t) = app();
    let before = app.engine.session_arc();
    let undo = app.engine.undo_history().len();
    // A two-beat count-in: played on its last beat, then on the downbeat.
    let events = vec![on(LATCH - B, t, 59), off(LATCH - B / 2, t, 59), on(LATCH, t, 60), off(LATCH + B, t, 60)];
    record(&mut app, 4 * BEAT, 2 * BEAT, None, 8 * BEAT, pass(t, 4 * BEAT, events));
    assert_eq!(spans(&app, t), [(2 * BEAT, 6 * BEAT)], "the clip covers the whole count-in");
    let id = app.engine.session().track(t).unwrap().clips()[0].id;
    // Both notes at their absolute positions: the pickup at beat 3, the downbeat at beat 4.
    assert_eq!(notes(&app, id), [(59, TICKS, TICKS / 2), (60, 2 * TICKS, TICKS)]);
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
    record(&mut plain, 2 * BEAT, 2 * BEAT, None, 6 * BEAT, take(t, false));
    let (mut stray, t2) = app();
    record(&mut stray, 2 * BEAT, 2 * BEAT, None, 6 * BEAT, take(t2, true));
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
    record(&mut app, 4 * BEAT, BEAT, Some(2 * BEAT), 4 * BEAT, pass(t, 4 * BEAT, events));
    let tr = app.engine.session().track(t).unwrap().clone();
    assert_eq!(tr.playlists.len(), 2, "a take per lap");
    assert_eq!(tr.active_playlist, 0);
    let clips = |k: usize| -> Vec<_> { tr.playlists[k].clips.iter().map(|c| (c.start, c.length, c.take, notes(&app, c.id))).collect() };
    // Take 1 was placed from the one-beat count-in's start (72,000) to the lap end. ADR 0039 then
    // puts the newest take on top: take 2 covers the lap and plays; the part of take 1 nothing
    // newer covers, the count-in with its pickup, stays in play beside it (`takes::activate_clip`,
    // today's rule); the rest of take 1 is the other take of the stack.
    assert_eq!(
        clips(0),
        [(3 * BEAT, BEAT, Some(1), vec![(59, TICKS / 2, TICKS / 4)]), (4 * BEAT, 2 * BEAT, Some(2), vec![(62, TICKS, 40)])],
        "the count-in of the first take plays before the second take, which is one lap as before"
    );
    assert_eq!(clips(1), [(4 * BEAT, 2 * BEAT, Some(1), vec![(60, 0, 40)])]);
}

/// ADR 0052: recorded half a beat into the session after a two-beat count-in, the take starts a
/// beat and a half before bar 1, where the count-in started; the pickup sits where it was played.
#[test]
fn a_pickup_before_bar_1_lands_in_the_zone_where_it_was_played() {
    let (mut app, t) = app();
    let events = vec![on(LATCH - B, t, 59), off(LATCH - B / 4, t, 59), on(LATCH, t, 60), off(LATCH + B, t, 60)];
    record(&mut app, BEAT / 2, 2 * BEAT, None, 2 * BEAT, pass(t, BEAT / 2, events));
    assert_eq!(spans(&app, t)[0].0, -3 * BEAT / 2);
    assert_eq!(app.engine.session().start(), -3 * BEAT / 2, "the take made the pre-count zone");
    let id = app.engine.session().track(t).unwrap().clips()[0].id;
    assert_eq!(notes(&app, id), [(59, TICKS, TICKS * 3 / 4), (60, 2 * TICKS, TICKS)]);
}

/// The owner's case (#441, replacing #440's stopgap): recorded from the session start after a
/// one-bar count-in, every note played during the count-in is in the take, before bar 1, with its
/// length; the status line counts them and says nothing is left out.
#[test]
fn count_in_notes_at_the_session_start_are_kept_before_bar_1() {
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
    record(&mut app, 0, 4 * BEAT, None, 3 * BEAT, pass(t, 0, events));
    assert_eq!(spans(&app, t), [(-4 * BEAT, 7 * BEAT)], "the clip covers the count-in bar before bar 1");
    let id = app.engine.session().track(t).unwrap().clips()[0].id;
    assert_eq!(notes(&app, id), [(55, TICKS, TICKS), (57, 2 * TICKS, TICKS), (59, TICKS * 7 / 2, TICKS), (60, 5 * TICKS, TICKS)]);
    assert!(app.status.starts_with("Recorded 1 MIDI clip (4 notes) on "), "{}", app.status);
    assert!(!app.status.contains("left out") && !app.status.contains("not recorded"), "{}", app.status);
    assert_eq!(app.engine.session().start(), -4 * BEAT);
}

/// A note played before the count-in started is not in the take, and the status line says so
/// once: nothing recorded is dropped without a word.
#[test]
fn a_note_before_the_count_in_started_is_reported_on_the_status_line() {
    let (mut app, t) = app();
    let events = vec![on(LATCH - 4 * B - 10, t, 50), off(LATCH - 4 * B + 10, t, 50), on(LATCH + B, t, 60), off(LATCH + 2 * B, t, 60)];
    record(&mut app, 4 * BEAT, 4 * BEAT, None, 7 * BEAT, pass(t, 4 * BEAT, events));
    assert_eq!(spans(&app, t), [(4 * BEAT, 3 * BEAT)], "no pickup in the count-in: from the record start");
    assert!(app.status.starts_with("Recorded 1 MIDI clip (1 note) on "), "{}", app.status);
    assert!(app.status.ends_with("; 1 note played before recording began was not recorded"), "{}", app.status);
}

#[test]
fn the_inspected_pass_reports_where_capture_began() {
    let (mut app, _) = app();
    assert!(app.recording_pass_inspect().is_null(), "no pass, nothing to report");
    app.take_plan = Some(plan(8 * BEAT, None, 4 * BEAT));
    assert_eq!(app.recording_pass_inspect(), json!({"record_start": 8 * BEAT, "count_in": 4 * BEAT, "capture_start": 4 * BEAT}));
    app.take_plan = Some(plan(BEAT, None, 4 * BEAT));
    assert_eq!(app.recording_pass_inspect()["capture_start"], -3 * BEAT, "before bar 1 when the count-in reaches there");
    app.take_plan = Some(plan(BEAT, None, 0));
    assert_eq!(app.recording_pass_inspect()["capture_start"], BEAT, "no count-in: capture begins at the record start");
}
