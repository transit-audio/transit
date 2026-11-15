//! Recording into the pre-count zone (ADR 0052 decision 8, slice 7): the take builder and its
//! placement with the count-in length the plan carries. The default session: 120 bpm, 4/4,
//! 48 kHz, so a beat is 24,000 samples and 960 ticks, and a tick is 25 samples.

#![allow(clippy::unwrap_used)]

use crate::Engine;
use crate::io::Recording;
use crate::midi_take::{Cut, Msg, RecEvent, build, build_wrapping, place_midi_recording, played_before_count_in};
use serde_json::json;
use transit_midi::{CtrlEvent, CtrlKind};
use transit_model::{ClipContent, ClipId, TrackId};
use transit_time::{Range, Samples};

const BEAT: Samples = 24_000;
const BAR: Samples = 4 * BEAT;

fn ev(t: TrackId, offset: Samples, msg: Msg) -> RecEvent {
    RecEvent { track: t, channel: 0, offset, msg }
}
fn on(t: TrackId, offset: Samples, pitch: u8) -> RecEvent {
    ev(t, offset, Msg::NoteOn { pitch, velocity: 90 })
}
fn off(t: TrackId, offset: Samples, pitch: u8) -> RecEvent {
    ev(t, offset, Msg::NoteOff { pitch, velocity: 0 })
}
fn pedal(t: TrackId, offset: Samples, value: u8) -> RecEvent {
    ev(t, offset, Msg::Cc { cc: 64, value })
}
fn cut(start: Samples, stop: Samples, count_in: Samples) -> Cut {
    Cut { start, pass: None, punch: None, stop, count_in: Some(count_in) }
}
fn plan(start: Samples, count_in: Samples) -> Recording {
    Recording { count_in: Some(count_in), rate: 48_000, ..Recording::new(start) }
}
fn engine() -> (Engine, TrackId) {
    let mut e = Engine::default();
    let t = TrackId(e.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    (e, t)
}
fn clip(e: &Engine, id: ClipId) -> transit_model::Clip {
    e.session().find_clip(id).unwrap().1.clone()
}
fn sequence(e: &Engine, id: ClipId) -> transit_midi::Sequence {
    match clip(e, id).content {
        ClipContent::Midi { sequence } => sequence,
        _ => panic!("not MIDI"),
    }
}
const T: TrackId = TrackId(1);

/// Recorded from bar 1 with a one-bar count-in: the take starts a bar before bar 1, the zone
/// appears, and Return to Zero goes there.
#[test]
fn a_take_from_bar_1_with_a_pickup_makes_the_zone_and_return_to_zero_reaches_it() {
    let (mut e, t) = engine();
    let events = [on(t, -BEAT, 59), off(t, -BEAT / 2, 59), on(t, 0, 60), off(t, BEAT, 60)];
    let ids = place_midi_recording(&mut e, &plan(0, BAR), &events, 2 * BEAT);
    assert_eq!(ids.len(), 1);
    assert_eq!((clip(&e, ids[0]).start, clip(&e, ids[0]).end()), (-BAR, 2 * BEAT));
    assert_eq!(e.session().start(), -BAR, "the pickup made the zone");
    let notes: Vec<_> = sequence(&e, ids[0]).notes.iter().map(|n| (n.pitch, n.start, n.length)).collect();
    assert_eq!(notes, vec![(59, 3 * 960, 480), (60, 4 * 960, 960)]);
    e.execute("transport.rtz", &json!({})).unwrap();
    assert_eq!(e.session().edit.playhead, -BAR);
}

/// A record start inside the zone (a cursor before bar 1) counts in further back; a pickup there
/// grows the zone to the new clip's start.
#[test]
fn a_record_start_in_the_zone_grows_the_zone() {
    let (mut e, t) = engine();
    place_midi_recording(&mut e, &plan(0, BAR), &[on(t, -BEAT, 59), off(t, -1, 59), on(t, 0, 60), off(t, BEAT, 60)], BEAT);
    assert_eq!(e.session().start(), -BAR);
    // From two beats before bar 1, another one-bar count-in, on a second track.
    let t2 = TrackId(e.execute("track.new", &json!({"kind": "midi"})).unwrap()["tracks"][0].as_u64().unwrap());
    let ids = place_midi_recording(&mut e, &plan(-2 * BEAT, BAR), &[on(t2, -3 * BEAT, 48), off(t2, -2 * BEAT, 48)], BEAT);
    assert_eq!(clip(&e, ids[0]).start, -2 * BEAT - BAR);
    assert_eq!(e.session().start(), -2 * BEAT - BAR, "the zone grew to the new take");
    assert_eq!(sequence(&e, ids[0]).notes[0].start, 960, "the pickup a beat into the count-in");
}

/// With a pickup, every event from the count-in start is in the clip and sounds: a pedal pressed
/// before the first note sits at its place, at a non-negative tick.
#[test]
fn with_a_pickup_the_count_in_controllers_are_visible() {
    let s = transit_model::Session::default();
    let events = [pedal(T, -90_000, 127), on(T, -60_000, 55), off(T, -50_000, 55), pedal(T, 1_000, 0), on(T, 0, 60), off(T, BEAT, 60)];
    let built = build(&s.tempo, s.sample_rate, &events, &cut(0, BEAT, BAR));
    assert_eq!(built[0].range, Range::new(-BAR, BEAT));
    let ctrls: Vec<_> = built[0].sequence.ctrls.iter().map(|c| (c.tick, c.value)).collect();
    // 6,000 samples into the count-in is 240 ticks; 1,000 samples after the latch is 3,880.
    assert_eq!(ctrls, vec![(240, 127), (3_880, 0)]);
}

/// A pass with captured controllers and nothing else gives a silent clip whose left edge stays at
/// the latch, the controllers hidden before it; a pass with no retained event gives no clip.
#[test]
fn a_controller_only_count_in_gives_a_silent_clip_at_the_latch_and_nothing_retained_gives_none() {
    let (mut e, t) = engine();
    let ids = place_midi_recording(&mut e, &plan(BAR, BAR), &[pedal(t, -2 * BEAT, 127)], 2 * BEAT);
    assert_eq!(ids.len(), 1);
    let c = clip(&e, ids[0]);
    assert_eq!((c.start, c.end()), (BAR, BAR + 2 * BEAT), "the edge stays at the latch");
    assert_eq!(e.session().start(), 0, "no zone");
    assert_eq!(sequence(&e, ids[0]).ctrls, vec![CtrlEvent { tick: -2 * 960, channel: 0, kind: CtrlKind::Cc(64), value: 127 }]);
    assert!(sequence(&e, ids[0]).notes.is_empty());
    assert!(e.session().needs().contains(&"midi_hidden_notes"), "{:?}", e.session().needs());
    let (mut e, t) = engine();
    assert!(place_midi_recording(&mut e, &plan(BAR, BAR), &[off(t, -BEAT, 50)], 2 * BEAT).is_empty(), "a stray release is no clip");
    assert!(e.session().track(t).unwrap().clips().is_empty());
}

/// With the count-in length known, a note-on before the count-in start is no pickup: it is left
/// out, and the status line's count says so.
#[test]
fn a_note_before_the_count_in_start_is_no_pickup_and_is_counted() {
    let s = transit_model::Session::default();
    let events = [on(T, -BAR - 100, 50), off(T, -BAR + 100, 50), on(T, 0, 60), off(T, BEAT, 60)];
    let built = build(&s.tempo, s.sample_rate, &events, &cut(BAR, BEAT, BAR));
    assert_eq!(built[0].range, Range::new(BAR, BAR + BEAT), "no pickup: the clip starts at the latch");
    assert_eq!(built[0].sequence.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60]);
    assert_eq!(played_before_count_in(&plan(BAR, BAR), &events), 1);
    assert_eq!(played_before_count_in(&plan(BAR, BAR + 200), &events), 0, "inside a longer count-in it is a pickup");
    assert_eq!(played_before_count_in(&Recording { count_in: None, ..plan(BAR, BAR) }, &events), 0, "an old journal keeps every pickup");
    let punch = Recording { punch: Some(Range::new(BAR, 2 * BAR)), ..plan(BAR, BAR) };
    assert_eq!(played_before_count_in(&punch, &events), 0, "a punch keeps its range");
}

/// No catch range (decision 1.10): a note played 10 ticks before the downbeat lands 10 ticks
/// before it.
#[test]
fn a_note_ten_ticks_early_lands_ten_ticks_before_the_downbeat() {
    let s = transit_model::Session::default();
    let built = build(&s.tempo, s.sample_rate, &[on(T, -250, 60), off(T, BEAT, 60)], &cut(BAR, BEAT, BAR));
    assert_eq!(built[0].range.start, 0, "the clip opens to the count-in start");
    assert_eq!(built[0].sequence.notes[0].start, 4 * 960 - 10);
}

/// A take merged into an existing clip that starts two bars earlier: the take's count-in pedal,
/// hidden in the take, sounds in the merged clip (decision 8, accepted).
#[test]
fn a_merge_into_a_clip_two_bars_earlier_sounds_the_takes_count_in_controller() {
    let (mut e, t) = engine();
    let first = place_midi_recording(&mut e, &plan(BAR, 0), &[on(t, 0, 60), off(t, 4 * BAR, 60)], 4 * BAR)[0];
    let ids = place_midi_recording(&mut e, &plan(3 * BAR, BEAT), &[pedal(t, -BEAT / 2, 127), on(t, 0, 64), off(t, BEAT, 64)], BEAT);
    assert_eq!(ids, vec![first], "merged");
    let c = clip(&e, first);
    assert_eq!(c.start, BAR);
    let ctrls = sequence(&e, first).ctrls;
    // Half a beat before bar 4 is two bars less half a beat into the clip, which starts at bar 2.
    assert_eq!(ctrls, vec![CtrlEvent { tick: 8 * 960 - 480, channel: 0, kind: CtrlKind::Cc(64), value: 127 }]);
}

/// A record start in the zone that runs into a cycle in the zone still cuts its laps (`laps` used
/// to need a start at or after bar 1).
#[test]
fn a_take_latched_in_the_zone_splits_at_the_wrap() {
    let s = transit_model::Session::default();
    let wrap = Range::new(-BAR, -2 * BEAT);
    let events = [on(T, 1_000, 60), off(T, 2_000, 60), on(T, 40_000, 62), off(T, 41_000, 62)];
    let built = build_wrapping(&s.tempo, s.sample_rate, &events, &cut(-BAR + BEAT / 2, 0, 0), Some(wrap));
    assert_eq!(built.iter().map(|b| (b.pass, b.range)).collect::<Vec<_>>(), vec![(0, Range::new(-BAR + BEAT / 2, -2 * BEAT)), (1, wrap)]);
    assert_eq!(crate::take_laps::completed(-BAR + BEAT / 2, None, Some(wrap), 40_000), 1);
}
