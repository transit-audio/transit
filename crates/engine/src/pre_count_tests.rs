//! The pre-count zone's engine floors (ADR 0052 slice 1, decisions 5 to 7 and 12): transport,
//! loop, selection, scroll, clip moves, pastes and copies floor at the session start instead of
//! at sample 0; musical addressing takes a signed clip origin; ADR 0019's tick grid and the
//! retime hold across bar 1.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Engine, TransportRequest};
use serde_json::{Value, json};
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, ClipLoop, EditMode, Session, Source, SourceId, TrackId, TrackKind};
use transit_time::{Range, Samples, TICKS_PER_QUARTER as Q};

/// One bar of 4/4 at 120 BPM and 48 kHz.
const BAR: Samples = 96_000;

fn run(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute_with_mode(id, &p, EditMode::Slip).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

fn note(pitch: u8, start: i64, length: i64) -> Note {
    Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start, length }
}

fn source() -> Source {
    Source {
        id: SourceId(7),
        name: "src".into(),
        path: "src.wav".into(),
        channels: 1,
        frames: 10_000_000,
        sample_rate: 48_000,
        format: transit_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
        disk: false,
    }
}

/// A session with one MIDI track; each `(start, length)` is a MIDI clip on it with one note.
fn midi_session(clips: &[(Samples, Samples)]) -> (Session, TrackId, Vec<ClipId>) {
    let mut s = Session::default();
    s.sources.push(source());
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let mut ids = Vec::new();
    for &(start, length) in clips {
        let id = s.new_clip_id();
        let mut seq = Sequence::default();
        seq.notes.push(note(60, 0, Q));
        crate::edit::place_clip(&mut s, t, Clip::midi(id, "m", start, length, seq));
        ids.push(id);
    }
    (s, t, ids)
}

/// An audio track on `s` with one clip at `start`.
fn add_audio(s: &mut Session, start: Samples, length: Samples) -> (TrackId, ClipId) {
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let id = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::audio(id, "a", SourceId(7), 0, start, length));
    (t, id)
}

fn start_of(e: &Engine, id: ClipId) -> Samples {
    e.session().find_clip(id).unwrap().1.start
}

fn last_request(e: &Engine) -> TransportRequest {
    e.transport_requests.last().cloned().unwrap()
}

// ---- edits ---------------------------------------------------------------------------------

#[test]
fn a_zone_clip_moved_less_than_its_distance_to_bar_1_stays_in_the_zone() {
    let (s, _, ids) = midi_session(&[(-BAR, BAR / 4)]);
    let mut e = Engine::new(s);
    run(&mut e, "edit.move_clips", json!({"clips": [ids[0].0], "by": BAR / 2}));
    assert_eq!(start_of(&e, ids[0]), -BAR / 2);
    assert_eq!(e.session().start(), -BAR / 2, "the zone shrinks with its clip");
}

#[test]
fn a_move_earlier_stops_at_the_session_start_and_never_grows_the_zone() {
    let (s, _, ids) = midi_session(&[(-BAR, BAR / 4), (BAR, BAR / 4)]);
    let mut e = Engine::new(s);
    // The later clip alone: it stops at the session start.
    run(&mut e, "edit.move_clips", json!({"clips": [ids[1].0], "by": -3 * BAR}));
    assert_eq!(start_of(&e, ids[1]), -BAR);
    assert_eq!(e.session().start(), -BAR);
    run(&mut e, "edit.undo", json!({}));
    // Both: the zone clip is already at the start, so neither moves; their spacing is kept.
    run(&mut e, "edit.move_clips", json!({"clips": [ids[0].0, ids[1].0], "by": -BAR / 2}));
    assert_eq!((start_of(&e, ids[0]), start_of(&e, ids[1])), (-BAR, BAR));
}

/// The one behavior change of slice 1 in a session without a zone: a move of several clips that
/// reaches bar 1 stops for all of them, keeping their spacing, where each clip clamped on its own.
#[test]
fn a_move_of_several_clips_that_reaches_bar_1_stops_for_all_of_them() {
    let (s, _, ids) = midi_session(&[(10_000, 5_000), (30_000, 5_000)]);
    let mut e = Engine::new(s);
    run(&mut e, "edit.move_clips", json!({"clips": [ids[0].0, ids[1].0], "by": -20_000}));
    assert_eq!((start_of(&e, ids[0]), start_of(&e, ids[1])), (0, 20_000), "stopped by 10,000 at bar 1, spacing kept");
}

#[test]
fn a_mixed_midi_and_audio_move_stops_at_the_audio_clips_bar_1_floor() {
    let (mut s, _, ids) = midi_session(&[(-BAR, BAR / 4), (10_000, 5_000)]);
    let (_, audio) = add_audio(&mut s, 30_000, 5_000);
    let mut e = Engine::new(s);
    run(&mut e, "edit.move_clips", json!({"clips": [ids[1].0, audio.0], "by": -50_000}));
    // The MIDI clip could go back to -96,000; the audio clip stops at bar 1 and holds it.
    assert_eq!((start_of(&e, ids[1]), start_of(&e, audio)), (-20_000, 0));
}

#[test]
fn a_copy_of_a_zone_clip_lands_in_the_zone_and_stops_at_the_floors() {
    let (mut s, _, ids) = midi_session(&[(-BAR, BAR / 8)]);
    let (_, audio) = add_audio(&mut s, 20_000, 1_000);
    let mut e = Engine::new(s);
    let out = run(&mut e, "edit.copy_clips", json!({"clips": [ids[0].0], "by": 20_000}));
    let copy = ClipId(out["clips"][0].as_u64().unwrap());
    assert_eq!(start_of(&e, copy), -BAR + 20_000);
    // Earlier than the session start: the copy stops there, which is on the original, so it is
    // refused rather than lifted to bar 1.
    let err = e.execute("edit.copy_clips", &json!({"clips": [ids[0].0], "by": -BAR})).unwrap_err().to_string();
    assert!(err.contains("would land on an original clip"), "{err}");
    // A copy of the audio clip stops at bar 1.
    let out = run(&mut e, "edit.copy_clips", json!({"clips": [audio.0], "by": -BAR}));
    assert_eq!(out["by"], -20_000);
}

#[test]
fn a_paste_stops_at_the_floor() {
    let (s, t, ids) = midi_session(&[(-BAR, BAR / 8)]);
    let mut e = Engine::new(s);
    run(&mut e, "edit.copy", json!({"tracks": [t.0], "start": -BAR, "end": -BAR + BAR / 8}));
    run(&mut e, "edit.clear", json!({"tracks": [t.0], "clips": [ids[0].0], "start": 0, "end": 0}));
    assert_eq!(e.session().start(), 0);
    run(&mut e, "edit.paste", json!({"tracks": [t.0], "at": -BAR}));
    assert_eq!(e.session().start(), 0, "the paste stops at bar 1: no zone is made by a paste");
    assert_eq!(e.session().track(t).unwrap().clips()[0].start, 0);
}

#[test]
fn deleting_the_zone_clip_returns_the_start_to_0_and_undo_restores_it() {
    let (s, t, ids) = midi_session(&[(-BAR, BAR / 4), (BAR, BAR)]);
    let mut e = Engine::new(s);
    run(&mut e, "transport.loop", json!({"start": -BAR, "end": -BAR / 2}));
    run(&mut e, "edit.select", json!({"start": -BAR / 2}));
    run(&mut e, "edit.clear", json!({"tracks": [t.0], "clips": [ids[0].0], "start": -BAR / 2, "end": -BAR / 2}));
    assert_eq!(e.session().start(), 0);
    // Stored positions are not rewritten; they floor at the session start when read.
    assert_eq!(e.session().edit.loop_range, Range::new(-BAR, -BAR / 2));
    assert_eq!(e.session().edit.selection, Range::point(-BAR / 2));
    let v = run(&mut e, "transport.rewind", json!({"seconds": 0}));
    assert_eq!(v["at"], 0, "the stored cursor before the session start reads as the start");
    run(&mut e, "edit.undo", json!({}));
    assert_eq!(e.session().start(), -BAR);
    assert_eq!(start_of(&e, ids[0]), -BAR);
}

#[test]
fn a_shuffle_ripple_in_the_zone_closes_up_to_the_session_start() {
    let (s, t, ids) = midi_session(&[(-BAR, 10_000), (-BAR / 2, 10_000)]);
    let mut e = Engine::new(s);
    e.execute_with_mode("edit.clear", &json!({"tracks": [t.0], "start": -80_000, "end": -60_000}), EditMode::Shuffle).unwrap();
    assert_eq!(start_of(&e, ids[1]), -BAR / 2 - 20_000, "the later zone clip closes the gap");
    assert_eq!(start_of(&e, ids[0]), -BAR);
}

#[test]
fn move_song_start_refuses_while_the_session_has_a_zone() {
    let (s, _, _) = midi_session(&[(-BAR, BAR / 4)]);
    let mut e = Engine::new(s);
    let err = e.execute("event.move_song_start", &json!({"to": BAR})).unwrap_err().to_string();
    assert!(err.contains("pre-count zone"), "{err}");
    let (s, _, ids) = midi_session(&[(0, BAR / 4)]);
    let mut e = Engine::new(s);
    run(&mut e, "event.move_song_start", json!({"to": BAR}));
    assert_eq!(start_of(&e, ids[0]), BAR, "without a zone it works as before");
}

// ---- transport, loop, selection, scroll ----------------------------------------------------

#[test]
fn return_to_zero_goes_to_the_session_start() {
    let (s, _, _) = midi_session(&[(-BAR, BAR / 4)]);
    let mut e = Engine::new(s);
    assert_eq!(run(&mut e, "transport.rtz", json!({}))["at"], -BAR);
    assert_eq!(last_request(&e), TransportRequest::Locate(-BAR));
    let mut e = Engine::new(midi_session(&[(BAR, BAR)]).0);
    assert_eq!(run(&mut e, "transport.rtz", json!({}))["at"], 0, "without a zone: bar 1");
}

#[test]
fn locate_play_rewind_and_the_loop_reach_the_zone_and_floor_at_its_start() {
    let (s, _, _) = midi_session(&[(-BAR, BAR / 4)]);
    let mut e = Engine::new(s);
    assert_eq!(run(&mut e, "transport.locate", json!({"at": -BAR / 2}))["at"], -BAR / 2);
    assert_eq!(run(&mut e, "transport.locate", json!({"at": -3 * BAR}))["at"], -BAR);
    assert_eq!(run(&mut e, "transport.locate", json!({"at": "bars_beats:0|1|000"}))["at"], -BAR, "bar 0 is the zone's bar");
    run(&mut e, "transport.play", json!({"from": -BAR / 4}));
    assert_eq!(last_request(&e), TransportRequest::PlayExact(-BAR / 4));
    run(&mut e, "transport.play", json!({"from": -5 * BAR}));
    assert_eq!(last_request(&e), TransportRequest::PlayExact(-BAR));
    run(&mut e, "transport.locate", json!({"at": -BAR / 2}));
    assert_eq!(run(&mut e, "transport.rewind", json!({"seconds": 10}))["at"], -BAR);
    let v = run(&mut e, "transport.loop", json!({"start": -2 * BAR, "end": -BAR / 2}));
    assert_eq!((v["start"].as_i64(), v["end"].as_i64()), (Some(-BAR), Some(-BAR / 2)));
    // Without a zone: today's floor at 0.
    let mut e = Engine::new(midi_session(&[(BAR, BAR)]).0);
    assert_eq!(run(&mut e, "transport.locate", json!({"at": -BAR / 2}))["at"], 0);
    run(&mut e, "transport.play", json!({"from": -BAR / 4}));
    assert_eq!(last_request(&e), TransportRequest::PlayExact(0));
}

#[test]
fn cycle_on_without_a_region_makes_four_bars_floored_at_the_session_start() {
    let (s, _, _) = midi_session(&[(-12_000, BAR)]);
    let mut e = Engine::new(s);
    run(&mut e, "options.loop_playback", json!({"value": true, "at": -50_000}));
    assert_eq!(e.session().edit.loop_range, Range::new(-12_000, BAR - 12_000), "from the session start to the content end");
}

#[test]
fn the_selection_and_the_scroll_floor_at_the_session_start() {
    let (s, _, _) = midi_session(&[(-BAR, BAR / 4)]);
    let mut e = Engine::new(s);
    run(&mut e, "edit.select", json!({"start": -BAR / 2, "end": -1_000}));
    assert_eq!(e.session().edit.selection, Range::new(-BAR / 2, -1_000));
    run(&mut e, "edit.select", json!({"start": -2 * BAR}));
    assert_eq!(e.session().edit.selection, Range::point(-BAR));
    assert_eq!(run(&mut e, "view.scroll", json!({"to": -BAR / 2}))["scroll"], -BAR / 2);
    assert_eq!(run(&mut e, "view.scroll", json!({"to": -5 * BAR}))["scroll"], -BAR);
    let mut e = Engine::new(midi_session(&[(BAR, BAR)]).0);
    run(&mut e, "edit.select", json!({"start": -BAR / 2}));
    assert_eq!(e.session().edit.selection, Range::point(0));
    assert_eq!(run(&mut e, "view.scroll", json!({"to": -BAR / 2}))["scroll"], 0);
}

#[test]
fn an_opened_session_reads_its_stored_cursor_floored_at_its_start() {
    let (mut s, _, _) = midi_session(&[(-BAR, BAR / 4)]);
    s.edit.selection = Range::point(-BAR / 2);
    let mut e = Engine::default();
    e.replace_session(s.clone());
    assert_eq!(e.transport.position, -BAR / 2);
    s.edit.selection = Range::point(-3 * BAR);
    e.replace_session(s);
    assert_eq!(e.transport.position, -BAR);
}

// ---- MIDI clip creation and musical addressing ---------------------------------------------

/// 4/4 at tick 0, 3/4 from tick 3840, and a MIDI clip starting at tick -3840 (bar 0).
fn zone_clip_with_a_meter_change() -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    s.tempo.set_meter(3840, 3, 4).unwrap();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let start = s.tempo.tick_to_samples(-3840, s.sample_rate);
    let id = s.new_clip_id();
    crate::edit::place_clip(&mut s, t, Clip::midi(id, "m", start, 3 * BAR, Sequence::default()));
    (Engine::new(s), t, id)
}

#[test]
fn a_zone_clip_takes_musical_note_starts_and_lengths_from_its_signed_origin() {
    let (mut e, _, id) = zone_clip_with_a_meter_change();
    run(&mut e, "midi.note_add", json!({"clip": id.0, "pitch": 60, "start": "3|1", "length": "1|2"}));
    let (_, c) = e.session().find_clip(id).unwrap();
    let ClipContent::Midi { sequence } = &c.content else { panic!() };
    assert_eq!((sequence.notes[0].start, sequence.notes[0].length), (7680, 960), "bar 0, bar 1, then the first 3/4 bar");
}

#[test]
fn midi_clip_creation_floors_at_the_session_start() {
    let (mut e, _, _) = zone_clip_with_a_meter_change();
    let zone = e.session().start();
    let t = TrackId(run(&mut e, "track.new", json!({"kind": "midi"}))["tracks"][0].as_u64().unwrap());
    let out = run(&mut e, "midi.new_clip", json!({"track": t.0, "start": zone - BAR, "end": zone + 1_000}));
    assert_eq!(out["start"], zone);
    let t = TrackId(run(&mut e, "track.new", json!({"kind": "midi"}))["tracks"][0].as_u64().unwrap());
    let out = run(&mut e, "event.step_input", json!({"track": t.0, "at": -10_000, "pitch": 62}));
    assert_eq!(start_of(&e, ClipId(out["clip"].as_u64().unwrap())), zone, "a step before the zone lands at its start");
    // Without a zone, bar 1.
    let (s, t, _) = midi_session(&[]);
    let mut e = Engine::new(s);
    let out = run(&mut e, "midi.new_clip", json!({"track": t.0, "start": -BAR, "end": BAR}));
    assert_eq!((out["start"].as_i64(), out["end"].as_i64()), (Some(0), Some(BAR)));
}

// ---- ADR 0019 across bar 1 and the retime --------------------------------------------------

/// A looped MIDI clip from bar 0 to bar 2 sounds every repeat on `tick_to_samples(t + k·C)`,
/// through bar 1, at two rates and four tempos, in one block and in blocks of 512.
#[test]
fn a_looped_zone_clip_repeats_on_its_tick_grid_across_bar_1() {
    const C: i64 = 1440;
    let (base, total) = (-3840, 7680);
    for hz in [44_100u32, 48_000] {
        for bpm in [93.7, 102.5, 120.0, 140.0] {
            let mut s = Session {
                sample_rate: transit_time::SampleRate::new(hz).unwrap(),
                tempo: transit_time::TempoMap::new(bpm, 4, 4).unwrap(),
                ..Session::default()
            };
            let (sr, tempo) = (s.sample_rate, s.tempo.clone());
            let at = |tick: i64| tempo.tick_to_samples(tick, sr);
            let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
            let mut seq = Sequence::default();
            seq.notes.push(note(60, 0, 200));
            seq.notes.push(note(64, 500, 200));
            let (start, end) = (at(base), at(base + total));
            let mut c = Clip::midi(ClipId(1), "l", start, end - start, seq);
            c.set_loop(Some(ClipLoop { start: 0, end: at(base + C) - start }));
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
            let mut want: Vec<(Samples, u8)> = (0..6)
                .flat_map(|k| [(base + k * C, 60), (base + 500 + k * C, 64)])
                .filter(|(tick, _)| *tick < base + total)
                .map(|(tick, key)| (at(tick), key))
                .collect();
            want.sort_unstable();
            assert!(want.iter().any(|(a, _)| *a < 0) && want.iter().any(|(a, _)| *a > 0), "repeats on both sides of bar 1");
            let (from, frames) = (start - 100, (end - start + 200) as usize);
            assert_eq!(transit_mix::clip_note_ons(&s, t, from, frames), want, "{hz} Hz {bpm} BPM");
            let mut blocks = Vec::new();
            let mut pos = from;
            while pos < from + frames as Samples {
                blocks.extend(transit_mix::clip_note_ons(&s, t, pos, 512));
                pos += 512;
            }
            assert_eq!(blocks, want, "{hz} Hz {bpm} BPM, blocks of 512");
        }
    }
}

#[test]
fn a_split_at_bar_1_of_a_clip_spanning_it_sounds_what_the_unsplit_clip_sounded() {
    for (hz, bpm) in [(48_000u32, 120.0), (44_100, 102.5)] {
        let mut s = Session {
            sample_rate: transit_time::SampleRate::new(hz).unwrap(),
            tempo: transit_time::TempoMap::new(bpm, 4, 4).unwrap(),
            ..Session::default()
        };
        let sr = s.sample_rate;
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let mut seq = Sequence::default();
        for (k, tick) in [0, 1000, 2500, 3840, 5000, 7000].into_iter().enumerate() {
            seq.notes.push(note(60 + k as u8, tick, 300));
        }
        let (start, end) = (s.tempo.tick_to_samples(-3840, sr), s.tempo.tick_to_samples(3840, sr));
        crate::edit::place_clip(&mut s, t, Clip::midi(ClipId(1), "m", start, end - start, seq));
        let mut e = Engine::new(s);
        let (from, frames) = (start - 100, (end - start + 200) as usize);
        let before = transit_mix::clip_note_ons(e.session(), t, from, frames);
        assert_eq!(before.len(), 6);
        run(&mut e, "edit.separate", json!({"tracks": [t.0], "at": 0}));
        assert_eq!(e.session().track(t).unwrap().clips().len(), 2, "split at bar 1");
        assert_eq!(transit_mix::clip_note_ons(e.session(), t, from, frames), before, "{hz} Hz {bpm} BPM");
    }
}

#[test]
fn a_tempo_change_at_bar_1_keeps_a_zone_clips_notes_on_their_ticks() {
    let (s, t, ids) = midi_session(&[(-BAR, BAR)]);
    let mut e = Engine::new(s);
    run(&mut e, "midi.note_add", json!({"clip": ids[0].0, "pitch": 64, "start_ticks": 1920, "length_ticks": 960}));
    let notes = |e: &Engine| match &e.session().find_clip(ids[0]).unwrap().1.content {
        ClipContent::Midi { sequence } => sequence.notes.clone(),
        ClipContent::Audio { .. } => panic!(),
    };
    let before = notes(&e);
    run(&mut e, "event.tempo", json!({"bpm": 60, "at": 0}));
    let s = e.session();
    let c = &s.track(t).unwrap().clips()[0];
    assert_eq!(s.tempo.samples_to_ticks(c.start, s.sample_rate), -3840, "the clip starts on bar 0's tick");
    assert_eq!((c.start, c.length), (-2 * BAR, 2 * BAR), "its samples follow the new tempo");
    assert_eq!(notes(&e), before, "the notes keep their ticks");
    assert_eq!(s.start(), -2 * BAR);
}
