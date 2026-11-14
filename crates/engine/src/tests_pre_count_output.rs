//! The pre-count zone's output paths (ADR 0052 slice 2, decision 10): a bounce without a range,
//! its stems and the real-time render start at the session start; `bounded_to_track` floors
//! there; a MIDI file export starts at the bar holding the session start, with the conductor
//! initialized at file tick 0, and writes only what sounds. No command makes a clip before bar 1
//! yet, so the sessions here are built with one directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use serde_json::json;
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipId, ClipLoop, Insert, MarkerKind, Session, TrackId, TrackKind};
use transit_time::{Range, SampleRate, Samples, TempoMap};

/// One bar of 4/4 at 120 BPM and 48 kHz.
const BAR: Samples = 96_000;

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

/// An instrument track holding one MIDI clip of `seq` from `start` for `length` samples.
fn instrument_session(start: Samples, length: Samples, seq: Sequence) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    s.track_mut(t).unwrap().instrument = Some(Insert::new(transit_model::DEFAULT_INSTRUMENT));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(id, "m", start, length, seq));
    (s, t)
}

/// A session whose one clip starts a bar before bar 1 and ends at bar 2, with a beat-long note on
/// its first beat (in the zone).
fn zone_session() -> (Session, TrackId) {
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 0, 960));
    instrument_session(-BAR, 2 * BAR, seq)
}

fn peak(ch: &[f32]) -> f32 {
    ch.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

// ---- bounce --------------------------------------------------------------------------------

/// A bounce without a range covers the zone: the file starts at the session start and the zone's
/// note is in it. Without a zone the default range is today's.
#[test]
fn a_bounce_without_a_range_starts_at_the_session_start_and_holds_the_zone() {
    let dir = transit_test_support::TestDir::created("pre-count-bounce");
    let (s, _) = zone_session();
    let mut e = Engine::new(s);
    let path = dir.join("mix.wav").to_string_lossy().into_owned();
    assert_eq!(crate::cmd::bounce_job(&e, &json!({"path": path})).unwrap().range(), Range::new(-BAR, BAR));
    let done = e.execute("file.bounce_mix", &json!({"path": path, "dither": false})).unwrap();
    assert_eq!(done["seconds"], json!(4.0), "{done}");
    let (_, audio) = transit_audio_io::decode(&std::fs::read(&path).unwrap(), Some("wav")).unwrap();
    assert_eq!(audio.channels[0].len() as Samples, 2 * BAR, "the file runs from the zone start to the content end");
    assert!(peak(&audio.channels[0][..BAR as usize / 4]) > 1e-3, "the zone's note is in the file");

    let (s, _) = instrument_session(BAR, BAR, Sequence::default());
    let e = Engine::new(s);
    assert_eq!(crate::cmd::bounce_job(&e, &json!({"path": path})).unwrap().range(), Range::new(0, 2 * BAR), "no zone: from bar 1");
}

/// Stems share the mix's start: the per-path bounce and the stems bounce both start at the
/// session start, so they line up with each other and with the mix.
#[test]
fn stems_without_a_range_share_the_session_start() {
    let dir = transit_test_support::TestDir::created("pre-count-stems");
    let (s, _) = zone_session();
    let mut e = Engine::new(s);
    let job = crate::cmd::bounce_paths_job(&e, &json!({"dir": dir.to_string_lossy()})).unwrap();
    assert_eq!(job.range(), Range::new(-BAR, BAR));
    let done = e.execute("file.bounce_stems", &json!({"dir": dir.to_string_lossy()})).unwrap();
    let file = done["files"].as_array().unwrap().iter().filter_map(|f| f.as_str()).find(|f| f.contains("Keys")).unwrap().to_string();
    let (_, audio) = transit_audio_io::decode(&std::fs::read(&file).unwrap(), Some("wav")).unwrap();
    assert_eq!(audio.channels[0].len() as Samples, 2 * BAR, "{done}");
    assert!(peak(&audio.channels[0][..BAR as usize / 4]) > 1e-3, "the zone's note is in the stem");
}

/// The real-time render's default range starts at the session start too.
#[test]
fn a_real_time_render_without_a_range_starts_at_the_session_start() {
    let dir = transit_test_support::TestDir::created("pre-count-rt");
    let (s, t) = zone_session();
    let mut e = Engine::new(s);
    e.set_io_map(transit_model::IoMap::for_device("Test", "Interface", 2, 2));
    let port = json!({"backend": "fake", "name": "Synth"});
    e.execute("instrument.external", &json!({"track": t.0, "port": port, "return": "In 1", "manual_ms": 20.0})).unwrap();
    let path = dir.join("mix.wav").to_string_lossy().into_owned();
    let plan = crate::cmd::realtime_plan(&e, &json!({"path": path})).unwrap();
    assert_eq!(plan.range, Range::new(-BAR, BAR));
}

/// `bounded_to_track` reaches the zone and floors at the session start; without a zone, at 0.
#[test]
fn bounded_to_track_reaches_the_zone_and_floors_at_the_session_start() {
    let (s, t) = zone_session();
    assert_eq!(crate::io::bounded_to_track(&s, t, Range::new(-3 * BAR, BAR)), Range::new(-BAR, BAR));
    assert_eq!(crate::io::bounded_to_track(&s, t, Range::new(-BAR / 2, BAR / 2)), Range::new(-BAR / 2, BAR / 2));
    let (s, t) = instrument_session(BAR, BAR, Sequence::default());
    assert_eq!(crate::io::bounded_to_track(&s, t, Range::new(-BAR, BAR)), Range::new(0, BAR));
}

// ---- MIDI file export ----------------------------------------------------------------------

/// Tempos as (tick, BPM to a thousandth): a file stores microseconds per quarter.
fn tempos_of(smf: &transit_midi::Smf) -> Vec<(i64, i64)> {
    smf.tempos.iter().map(|(t, bpm)| (*t, (bpm * 1000.0).round() as i64)).collect()
}

fn exported(s: Session) -> transit_midi::Smf {
    transit_midi::read_smf(&crate::io::export_midi(&Engine::new(s), None).unwrap()).unwrap()
}

/// Notes of the export's track `i` as (pitch, start, length).
fn notes_of(smf: &transit_midi::Smf, i: usize) -> Vec<(u8, i64, i64)> {
    smf.tracks[i].sequence.notes.iter().map(|n| (n.pitch, n.start, n.length)).collect()
}

/// ADR 0052 decision 10's fixture: a 90 BPM, 3/4 session with a one-bar zone. The file starts at
/// the zone's bar: tempo and meter at file tick 0, the later changes and the marker shifted by one
/// 3/4 bar, and the pickup at its tick from the zone's bar line with its length.
#[test]
fn a_midi_export_starts_at_the_zone_bar_with_the_conductor_at_file_tick_0() {
    const BAR34: i64 = 2880;
    let mut s = Session { sample_rate: SampleRate::new(48_000).unwrap(), tempo: TempoMap::new(90.0, 3, 4).unwrap(), ..Session::default() };
    s.tempo.set_tempo(2 * BAR34, 120.0).unwrap();
    s.tempo.set_meter(2 * BAR34, 4, 4).unwrap();
    let at = |s: &Session, tick: i64| s.tempo.tick_to_samples(tick, s.sample_rate);
    let bar2 = at(&s, BAR34);
    s.add_marker("Verse", MarkerKind::Marker, bar2, bar2);
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 960, 480)); // the pickup: beat 2 of bar 0
    seq.notes.push(note(64, BAR34 + 100, 240)); // in bar 1
    let (start, end) = (at(&s, -BAR34), at(&s, BAR34));
    assert_eq!(start, -96_000);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(ClipId(1), "m", start, end - start, seq));
    let smf = exported(s);
    assert_eq!(notes_of(&smf, 0), [(60, 960, 480), (64, BAR34 + 100, 240)]);
    assert_eq!(tempos_of(&smf), [(0, 90_000), (3 * BAR34, 120_000)]);
    assert_eq!(smf.meters, [(0, 3, 4), (3 * BAR34, 4, 4)]);
    assert_eq!(smf.markers, [(2 * BAR34, "Verse".to_string())]);
}

/// Without a zone the file starts at bar 1, as before.
#[test]
fn a_midi_export_without_a_zone_starts_at_bar_1() {
    let mut s = Session::default();
    s.tempo.set_tempo(3840, 100.0).unwrap();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 120, 480));
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(ClipId(1), "m", BAR, BAR, seq));
    let smf = exported(s);
    assert_eq!(tempos_of(&smf), [(0, 120_000), (3840, 100_000)]);
    assert_eq!(notes_of(&smf, 0), [(60, 3840 + 120, 480)]);
}

/// Only what sounds is written: a note-off cut at the clip's end, nothing that starts past it, and
/// a looped clip's repeats written out.
#[test]
fn a_midi_export_writes_only_what_sounds() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 0, 480));
    seq.notes.push(note(62, 3000, 2000)); // crosses the clip end at tick 3840
    seq.notes.push(note(64, 4000, 480)); // starts past it
    seq.ctrls.push(CtrlEvent { tick: 100, channel: 0, kind: CtrlKind::Cc(7), value: 90 });
    seq.ctrls.push(CtrlEvent { tick: 3900, channel: 0, kind: CtrlKind::Cc(7), value: 10 }); // past the end
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(ClipId(1), "m", 0, BAR, seq));
    let l = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Loop"));
    let mut seq = Sequence::default();
    seq.notes.push(note(48, 0, 240));
    // From bar 2 for one bar, looping its first beat.
    let mut c = Clip::midi(ClipId(2), "l", BAR, BAR, seq);
    c.set_loop(Some(ClipLoop { start: 0, end: BAR / 4 }));
    s.track_mut(l).unwrap().playlist_mut().unwrap().clips.push(c);
    let smf = exported(s);
    assert_eq!(notes_of(&smf, 0), [(60, 0, 480), (62, 3000, 840)]);
    let ctrls: Vec<(i64, i32)> = smf.tracks[0].sequence.ctrls.iter().map(|c| (c.tick, c.value)).collect();
    assert_eq!(ctrls, [(100, 90)]);
    assert_eq!(notes_of(&smf, 1), [(48, 3840, 240), (48, 4800, 240), (48, 5760, 240), (48, 6720, 240)]);
}
