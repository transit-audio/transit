//! The pre-count zone's output-path follow-ups (ADR 0052 slice 2b, decisions 10 and 12): the
//! consolidated export starts every stem, MIDI file and the tempo file at the bar holding the
//! session start; the session length is `content_end - start` and `session.inspect` reports the
//! extent; Bounce in Place of a zone clip starts its audio at bar 1. Without a zone every path
//! gives what it gave before, checked against the base expressions recomputed here. No command
//! makes a clip before bar 1 yet, so the sessions are built with one directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::Engine;
use serde_json::json;
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, Insert, MarkerKind, Session, TrackId, TrackKind};
use transit_time::{Range, Samples};

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

/// A clip from beat 3 of bar 0 (half a bar before bar 1) to bar 2, with a beat-long note on its
/// first tick, in the zone, and a marker at bar 2. The zone does not start on a bar line.
fn off_bar_zone_session() -> (Session, TrackId) {
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 0, 960));
    let (mut s, t) = instrument_session(-BAR / 2, BAR + BAR / 2, seq);
    s.add_marker("Verse", MarkerKind::Marker, BAR, BAR);
    (s, t)
}

/// The conductor the base commit wrote: the session's tempo, meter and markers at their own
/// ticks (`session_smf` before this slice).
fn base_conductor(s: &Session) -> transit_midi::Smf {
    transit_midi::Smf {
        format: 1,
        tracks: Vec::new(),
        tempos: s.tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect(),
        meters: s.tempo.meters().iter().map(|m| (m.tick, u8::try_from(m.numerator).unwrap_or(4), u8::try_from(m.denominator).unwrap_or(4))).collect(),
        markers: s.markers.iter().map(|m| (s.tempo.samples_to_ticks(m.start, s.sample_rate), m.name.clone())).collect(),
        key_sigs: Vec::new(),
    }
}

fn consolidated(e: &mut Engine, dir: &std::path::Path) -> serde_json::Value {
    // Without dither, so silence in a stem is exactly zero.
    e.execute("file.bounce_consolidated", &json!({"dir": dir.to_string_lossy(), "dither": false})).unwrap()
}

fn read_wav(path: &str) -> Vec<f32> {
    let (_, audio) = transit_audio_io::decode(&std::fs::read(path).unwrap(), Some("wav")).unwrap();
    audio.channels[0].clone()
}

// ---- consolidated export -------------------------------------------------------------------

/// With a zone that starts half a bar into bar 0, the stem, the track's MIDI file and the tempo
/// file all start at bar 0's first sample and tick: the zone's note sits half a bar into the stem
/// and at tick 1920 of the MIDI file (the same instant at 120 BPM), and bar 1 falls one bar in.
#[test]
fn a_consolidated_export_starts_every_file_at_the_bar_holding_the_session_start() {
    let dir = transit_test_support::TestDir::created("pre-count-consolidated");
    let (s, _) = off_bar_zone_session();
    let mut e = Engine::new(s);
    assert_eq!(crate::io::consolidated_range(e.session()), Range::new(-BAR, BAR));
    let done = consolidated(&mut e, &dir);
    assert_eq!(done["seconds"], json!(4.0), "{done}");

    let stem = read_wav(done["files"][0].as_str().unwrap());
    assert_eq!(stem.len() as Samples, 2 * BAR, "the stem runs from bar 0 to the content end");
    let half = (BAR / 2) as usize;
    assert_eq!(stem[..half].iter().position(|x| *x != 0.0), None, "nothing sounds before the zone's clip");
    assert!(stem[half..half + half / 2].iter().any(|x| *x != 0.0), "the zone's note is half a bar into the stem");

    let midi = transit_midi::read_smf(&std::fs::read(done["midi_files"][0].as_str().unwrap()).unwrap()).unwrap();
    let notes: Vec<(u8, i64, i64)> = midi.tracks.iter().flat_map(|t| &t.sequence.notes).map(|n| (n.pitch, n.start, n.length)).collect();
    assert_eq!(notes, [(60, 1920, 960)], "the zone's note half a bar (two beats) after the file's first bar line");
    assert_eq!(midi.tempos.iter().map(|t| t.0).collect::<Vec<_>>(), [0]);
    assert_eq!(midi.meters, [(0, 4, 4)]);
    assert_eq!(midi.markers, [(2 * 3840, "Verse".to_string())], "the marker at bar 2 moved one bar later");

    let tempo = transit_midi::read_smf(&std::fs::read(done["tempo_file"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(
        (tempo.tempos.iter().map(|t| t.0).collect::<Vec<_>>(), tempo.meters, tempo.markers),
        (vec![0], vec![(0, 4, 4)], vec![(2 * 3840, "Verse".to_string())])
    );
}

/// Without a zone the consolidated export is byte for byte what the base commit wrote: the range
/// is `0..playback_content_end`, each MIDI file is the base conductor plus the track as played
/// over that range, and the tempo file is the base conductor.
#[test]
fn a_consolidated_export_without_a_zone_is_unchanged() {
    let dir = transit_test_support::TestDir::created("pre-count-consolidated-none");
    let mut seq = Sequence::default();
    seq.notes.push(note(60, 120, 480));
    seq.notes.push(note(64, 3000, 2000));
    seq.ctrls.push(CtrlEvent { tick: 100, channel: 0, kind: CtrlKind::Cc(64), value: 127 });
    let (mut s, t) = instrument_session(BAR, 2 * BAR, seq);
    s.tempo.set_tempo(7680, 100.0).unwrap();
    s.add_marker("Chorus", MarkerKind::Marker, 2 * BAR, 2 * BAR);
    let base_range = Range::new(0, transit_mix::playback_content_end(&s));
    let mut e = Engine::new(s);
    assert_eq!(crate::io::consolidated_range(e.session()), base_range);
    let done = consolidated(&mut e, &dir);
    assert_eq!(done["seconds"], json!(e.session().sample_rate.seconds(base_range.len())));
    assert_eq!(read_wav(done["files"][0].as_str().unwrap()).len() as Samples, base_range.len());

    let s = e.session();
    let mut want = base_conductor(s);
    let track = s.track(t).unwrap();
    want.tracks.push(transit_midi::SmfTrack { name: track.name.clone(), sequence: transit_mix::sequence_as_played(s, track, base_range).unwrap() });
    assert!(!want.tracks[0].sequence.notes.is_empty());
    let midi = std::fs::read(done["midi_files"][0].as_str().unwrap()).unwrap();
    assert!(midi == transit_midi::write_smf(&want).unwrap(), "the MIDI file differs from the base commit's");
    let tempo = std::fs::read(done["tempo_file"].as_str().unwrap()).unwrap();
    assert!(tempo == transit_midi::write_smf(&base_conductor(s)).unwrap(), "the tempo file differs from the base commit's");
}

// ---- session length and extent -------------------------------------------------------------

/// The session length is `content_end - start`: the open line's duration and `session.inspect`'s
/// `length_samples` count the zone, and the inspect reply reports the start beside the end.
#[test]
fn the_session_length_counts_the_zone_and_the_extent_reports_the_start() {
    let (s, _) = off_bar_zone_session();
    assert_eq!((s.start(), s.content_end(), s.length()), (-BAR / 2, BAR, BAR + BAR / 2));
    let line = crate::io::session_open_line("a.transit", &s, 0, std::time::Duration::ZERO);
    assert!(line.contains(" duration=3.0 s "), "{line}");
    let mut e = Engine::new(s);
    let v = e.execute("session.inspect", &json!({})).unwrap();
    assert_eq!((&v["start_samples"], &v["end_samples"], &v["length_samples"]), (&json!(-BAR / 2), &json!(BAR), &json!(BAR + BAR / 2)), "{v}");
    let info = e.execute("file.get_info", &json!({})).unwrap();
    assert_eq!(info["length_samples"], json!(BAR + BAR / 2));
}

/// Without a zone the length is the base commit's `content_end().max(0)`, in the open line and in
/// `length_samples`, and the start is 0.
#[test]
fn the_session_length_without_a_zone_is_unchanged() {
    let empty = Session::default();
    let (one, _) = instrument_session(BAR, BAR, Sequence::default());
    for s in [empty, one, crate::demo::demo_engine().session().clone()] {
        let base_seconds = s.content_end().max(0) as f64 / f64::from(s.sample_rate.hz().max(1));
        let line = crate::io::session_open_line("a.transit", &s, 0, std::time::Duration::ZERO);
        assert!(line.contains(&format!(" duration={base_seconds:.1} s ")), "{line}");
        let base_length = s.content_end();
        let mut e = Engine::new(s);
        let v = e.execute("session.inspect", &json!({})).unwrap();
        assert_eq!((&v["start_samples"], &v["length_samples"], &v["end_samples"]), (&json!(0), &json!(base_length), &json!(base_length)));
    }
}

// ---- Bounce in Place -----------------------------------------------------------------------

/// A track with one constant-level audio clip at each of `spans`, pushed straight onto the
/// playlist (no command places audio before bar 1).
fn audio_track(e: &mut Engine, level: f32, spans: &[(i64, i64)]) -> (TrackId, Vec<ClipId>) {
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Keys"));
    let mut ids = Vec::new();
    for (a, b) in spans {
        let buf = AudioBuffer { sample_rate: s.sample_rate.hz(), channels: vec![vec![level; (b - a) as usize]] };
        let src = crate::io::add_source(s, "Keys", buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "Keys", src, 0, *a, b - a));
        ids.push(id);
    }
    (t, ids)
}

/// The printed clip: its start, its offset into the print, its length and the print's samples.
fn print_of(e: &Engine, t: TrackId) -> (i64, i64, i64, Vec<f32>) {
    let c = &e.session().track(t).unwrap().clips()[0];
    let ClipContent::Audio { source, offset, .. } = c.content else { panic!("an audio clip") };
    (c.start, offset, c.length, e.session().pool.get(source).unwrap().buffer.channels[0].clone())
}

/// Bounce in Place of a clip that starts in the zone: the span is rendered whole and the printed
/// clip starts at bar 1, its left edge hiding the zone's audio, so from bar 1 on it plays what the
/// source played. A clip at or after bar 1 still prints at its own start.
#[test]
fn bounce_in_place_of_a_zone_clip_starts_its_audio_at_bar_1() {
    let tail = 96_000; // two seconds at 48 kHz
    let mut e = Engine::default();
    let (_, c) = audio_track(&mut e, 0.25, &[(-BAR / 2, BAR / 2)]);
    let r = e.execute("clip.bounce", &json!({"clips": [c[0].0]})).unwrap();
    let d = TrackId(r["tracks"][0].as_u64().unwrap());
    let (start, offset, length, print) = print_of(&e, d);
    assert_eq!((start, offset, length), (0, BAR / 2, BAR / 2 + tail), "the print starts at bar 1, the zone hidden before its edge");
    let want: Vec<f32> = (0..BAR + tail).map(|i| if i < BAR { 0.25 } else { 0.0 }).collect();
    assert_eq!(print.len(), want.len());
    assert_eq!(print.iter().zip(&want).position(|(g, w)| g.to_bits() != w.to_bits()), None, "the file holds the whole span");

    // No zone: placed at the span's start with no offset, as the base commit placed it.
    let mut e = Engine::default();
    let (_, c) = audio_track(&mut e, 0.25, &[(10_000, 20_000)]);
    let r = e.execute("clip.bounce", &json!({"clips": [c[0].0]})).unwrap();
    let (start, offset, length, print) = print_of(&e, TrackId(r["tracks"][0].as_u64().unwrap()));
    assert_eq!((start, offset, length, print.len() as i64), (10_000, 0, 10_000 + tail, 10_000 + tail));
}

/// Clips wholly before bar 1, tail included, would print nothing that sounds from bar 1: the
/// command is refused before anything renders and the session is unchanged.
#[test]
fn bounce_in_place_of_clips_that_end_before_bar_1_is_refused_and_changes_nothing() {
    let mut e = Engine::default();
    let (_, c) = audio_track(&mut e, 0.25, &[(-4 * BAR, -3 * BAR)]);
    let before = e.session().to_json().unwrap();
    let err = e.execute("clip.bounce", &json!({"clips": [c[0].0]})).unwrap_err().to_string();
    assert!(err.contains("end before bar 1"), "{err}");
    assert!(e.session().to_json().unwrap() == before, "a refused print changes nothing");
}

/// When the print hides audio before bar 1 and the source is muted, deactivated or hidden, that part
/// stops playing, and the reply says so. Not with Leave (the source still plays it), and not for a
/// clip with nothing before bar 1.
#[test]
fn bounce_in_place_says_when_the_part_before_bar_1_stops_playing() {
    let notice = |zone: bool, source: &str| {
        let mut e = Engine::default();
        let span = if zone { (-BAR / 2, BAR / 2) } else { (10_000, 20_000) };
        let (_, c) = audio_track(&mut e, 0.25, &[span]);
        let r = e.execute("clip.bounce", &json!({"clips": [c[0].0], "source": source})).unwrap();
        assert_eq!(r["tracks"].as_array().unwrap().len(), 1, "{r}");
        r.get("notice").cloned()
    };
    let said = Some(json!(crate::cmd::clip_bounce::ZONE_SILENCED_NOTICE));
    for source in ["mute", "mute_deactivate", "hide"] {
        assert_eq!(notice(true, source), said, "{source}");
        assert_eq!(notice(false, source), None, "{source} without a zone");
    }
    assert_eq!(notice(true, "leave"), None, "Leave keeps the source playing the part before bar 1");
}
