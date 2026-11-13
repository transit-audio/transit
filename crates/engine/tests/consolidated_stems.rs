//! Consolidated track stems and their editable MIDI (#261 slice 3).

use std::path::{Path, PathBuf};
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_engine::Engine;
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, Session, TrackKind};
use transit_time::SampleRate;

fn scratch(name: &str) -> transit_test_support::TestDir {
    transit_test_support::TestDir::created(name)
}

fn tone(s: &mut Session) -> transit_model::SourceId {
    transit_engine::io::add_source(
        s,
        "tone",
        AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800], vec![0.25; 4800]] },
        None,
        FileFormat::Wav,
    )
}

fn run(e: &mut Engine, dir: &Path) -> serde_json::Value {
    e.execute("file.bounce_consolidated", &serde_json::json!({"dir": dir.to_string_lossy(), "format": "wav", "bit_depth": "32f", "dither": false}))
        .unwrap()
}

fn paths(result: &serde_json::Value, key: &str) -> Vec<PathBuf> {
    result[key].as_array().unwrap().iter().map(|v| PathBuf::from(v.as_str().unwrap())).collect()
}

#[test]
fn consolidated_stems_start_at_zero_and_each_midi_track_carries_the_tempo_map() {
    let dir = scratch("consolidated-stems");
    let mut s = Session::new("Interchange", SampleRate::HZ_48000);
    s.tempo.set_tempo(0, 90.0).unwrap();
    s.tempo.set_tempo(960, 140.0).unwrap();
    let source = tone(&mut s);
    let audio = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Audio"));
    let audio_clip = s.new_clip_id();
    s.track_mut(audio).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(audio_clip, "tone", source, 0, 4800, 4800));
    let keys = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let sequence =
        Sequence { notes: vec![Note { pitch: 60, velocity: 100, release_velocity: 32, channel: 0, start: 0, length: 480 }], ctrls: Vec::new() };
    let keys_clip = s.new_clip_id();
    s.track_mut(keys).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(keys_clip, "phrase", 4800, 4800, sequence));
    let mut e = Engine::new(s);

    let result = run(&mut e, &dir);
    let stems = paths(&result, "files");
    let midi = paths(&result, "midi_files");
    assert_eq!(stems.len(), 2, "{result}");
    assert_eq!(midi.len(), 1, "{result}");
    for stem in stems {
        let audio_track = stem.to_string_lossy().contains("Audio");
        let (_, decoded) = transit_audio_io::decode(&std::fs::read(&stem).unwrap(), Some("wav")).unwrap();
        assert_eq!(decoded.frames(), 9600, "every stem includes the leading session silence: {}", stem.display());
        if audio_track {
            assert!(decoded.channels.iter().all(|c| c[..4800].iter().all(|v| *v == 0.0)), "the clip stays at its session position");
            assert_eq!((decoded.channels[0][4800], decoded.channels[1][4800]), (0.5, 0.25));
        }
    }
    let smf = transit_midi::read_smf(&std::fs::read(&midi[0]).unwrap()).unwrap();
    assert_eq!(smf.tempos.len(), 2);
    assert_eq!((smf.tempos[0].0, smf.tempos[1].0), (0, 960));
    assert!(
        (smf.tempos[0].1 - 90.0).abs() < 0.0002 && (smf.tempos[1].1 - 140.0).abs() < 0.0002,
        "SMF tempo uses integer microseconds per quarter note: {:?}",
        smf.tempos
    );
    assert_eq!(smf.tracks.len(), 1);
    assert_eq!(smf.tracks[0].name, "Keys");
    assert_eq!(smf.tracks[0].sequence.notes[0].start, 144, "0.1 seconds at 90 BPM and 960 PPQ");
    assert_eq!(smf.tracks[0].sequence.notes[0].release_velocity, 32, "recorded release velocity survives");
    let tempo = transit_midi::read_smf(&std::fs::read(result["tempo_file"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(tempo.tempos, smf.tempos);
    assert!(tempo.tracks.iter().all(|t| t.sequence.notes.is_empty() && t.sequence.ctrls.is_empty()), "the tempo file also covers audio-only exports");
    assert!(!e.is_dirty(), "export does not edit the session");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn audio_only_tracks_get_distinct_stems_and_a_tempo_file() {
    let dir = scratch("consolidated-audio-only");
    let mut s = Session::default();
    s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Drums"));
    s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Bass"));
    let mut e = Engine::new(s);
    let result = run(&mut e, &dir);
    assert_eq!(result["files"].as_array().unwrap().len(), 2);
    assert_eq!(result["midi_files"].as_array().unwrap().len(), 0);
    let tempo = transit_midi::read_smf(&std::fs::read(result["tempo_file"].as_str().unwrap()).unwrap()).unwrap();
    assert!(!tempo.tempos.is_empty() && tempo.tracks.iter().all(|t| t.sequence.notes.is_empty() && t.sequence.ctrls.is_empty()));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn muted_inactive_and_solo_safe_tracks_do_not_change_the_isolated_stem() {
    let dir = scratch("consolidated-isolation");
    let mut s = Session::default();
    let source = tone(&mut s);
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Source"));
    s.track_mut(track).unwrap().mixer.mute = true;
    s.track_mut(track).unwrap().inactive = true;
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", source, 0, 0, 4800));
    let other = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Solo safe"));
    s.track_mut(other).unwrap().mixer.solo_safe = true;
    let clip = s.new_clip_id();
    s.track_mut(other).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "other", source, 0, 0, 4800));
    let bus1 = s.add_bus("Bus 1", ChannelFormat::Stereo);
    let bus2 = s.add_bus("Bus 2", ChannelFormat::Stereo);
    let aux1 = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Aux 1"));
    let aux2 = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Aux 2"));
    s.track_mut(track).unwrap().mixer.output = transit_model::Route::Bus(bus1);
    s.track_mut(aux1).unwrap().mixer.input = transit_model::Route::Bus(bus1);
    s.track_mut(aux1).unwrap().mixer.output = transit_model::Route::Bus(bus2);
    s.track_mut(aux2).unwrap().mixer.input = transit_model::Route::Bus(bus2);
    let mut gain = transit_model::Insert::new("gain");
    gain.params.insert("gain".into(), -6.0);
    s.track_mut(aux2).unwrap().mixer.inserts[0] = Some(gain);
    let mut e = Engine::new(s);

    let result = run(&mut e, &dir);
    let stems = paths(&result, "files");
    assert_eq!(stems.len(), 2, "muted and inactive tracks still get their material exported");
    let (_, decoded) = transit_audio_io::decode(&std::fs::read(&stems[0]).unwrap(), Some("wav")).unwrap();
    let want = 0.5 * transit_dsp::db_to_gain(-6.0);
    assert!((decoded.channels[0][0] - want).abs() < 1e-4, "nested aux processing, not solo-safe bleed: {} against {want}", decoded.channels[0][0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn looped_midi_is_written_as_it_sounds_and_empty_instruments_get_an_smf() {
    let dir = scratch("consolidated-loops");
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Loop"));
    let mut clip = Clip::midi(
        s.new_clip_id(),
        "Loop",
        0,
        48_000,
        Sequence { notes: vec![Note { pitch: 64, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 }], ctrls: Vec::new() },
    );
    clip.looping = Some(transit_model::ClipLoop { start: 0, end: 24_000 });
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
    let empty = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Empty"));
    s.track_mut(empty).unwrap().inactive = true;
    let mut e = Engine::new(s);

    let result = run(&mut e, &dir);
    let midi = paths(&result, "midi_files");
    assert_eq!(midi.len(), 2, "even an empty instrument keeps an editable track");
    let smf = transit_midi::read_smf(&std::fs::read(&midi[0]).unwrap()).unwrap();
    let starts: Vec<i64> = smf.tracks[0].sequence.notes.iter().map(|n| n.start).collect();
    assert_eq!(starts, [0, 960], "the loop is unrolled at the session tempo");
    let empty_smf = transit_midi::read_smf(&std::fs::read(&midi[1]).unwrap()).unwrap();
    assert_eq!(empty_smf.tracks.len(), 1);
    assert!(empty_smf.tracks[0].sequence.notes.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rack_members_midi_contains_the_rack_clips() {
    let dir = scratch("consolidated-rack");
    let mut e = Engine::default();
    let member = e.execute("track.new", &serde_json::json!({"kind": "instrument", "name": "Member"})).unwrap()["tracks"][0].as_u64().unwrap();
    let rack = e.execute("rack.new", &serde_json::json!({"tracks": [member]})).unwrap()["rack"].as_u64().unwrap();
    let clip = e.session_mut().new_clip_id();
    let sequence = Sequence {
        notes: vec![
            Note { pitch: 60, velocity: 110, release_velocity: 64, channel: 0, start: 0, length: 480 },
            Note { pitch: 67, velocity: 110, release_velocity: 64, channel: 0, start: 0, length: 480 },
        ],
        ctrls: Vec::new(),
    };
    e.session_mut().track_mut(transit_model::TrackId(rack)).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(
        clip,
        "rack notes",
        0,
        4800,
        sequence,
    ));
    let own_clip = e.session_mut().new_clip_id();
    let own_sequence =
        Sequence { notes: vec![Note { pitch: 60, velocity: 110, release_velocity: 32, channel: 0, start: 0, length: 960 }], ctrls: Vec::new() };
    e.session_mut().track_mut(transit_model::TrackId(member)).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(
        own_clip,
        "member notes",
        0,
        48_000,
        own_sequence,
    ));
    e.execute(
        "rack.zone",
        &serde_json::json!({
            "track": rack,
            "member": member,
            "key": {"low": 64, "high": 67, "fade_low": 64, "fade_high": 67},
            "velocity": {"low": 0, "high": 127, "fade_low": 0, "fade_high": 100}
        }),
    )
    .unwrap();
    e.execute("event.midi_track_offsets", &serde_json::json!({"track": member, "offset": 4800})).unwrap();

    let result = run(&mut e, &dir);
    let midi = paths(&result, "midi_files");
    assert_eq!(midi.len(), 1, "the member gets the MIDI file; the rack folder gets none");
    let smf = transit_midi::read_smf(&std::fs::read(&midi[0]).unwrap()).unwrap();
    assert_eq!(smf.tracks[0].sequence.notes.len(), 2, "the key zone excludes only the rack's C4; the member's own C4 remains");
    let own = smf.tracks[0].sequence.notes.iter().find(|n| n.pitch == 60).unwrap();
    assert_eq!((own.length, own.release_velocity), (960, 32), "an excluded rack off cannot terminate the member's own note");
    let rack_note = smf.tracks[0].sequence.notes.iter().find(|n| n.pitch == 67).unwrap();
    assert_eq!(rack_note.velocity, 71, "the velocity fade matches playback");
    let stems = paths(&result, "files");
    let (_, decoded) = transit_audio_io::decode(&std::fs::read(&stems[0]).unwrap(), Some("wav")).unwrap();
    assert_eq!(decoded.frames(), 52_800, "the member's offset also extends its rack clip's tail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn track_rtp_and_offset_are_written_into_the_exported_notes() {
    let dir = scratch("consolidated-rtp");
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Timing"));
    let sequence =
        Sequence { notes: vec![Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 }], ctrls: Vec::new() };
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(clip, "phrase", 4800, 48_000, sequence));
    let mut e = Engine::new(s);
    e.execute("event.midi_rtp", &serde_json::json!({"track": track.0, "velocity": 10, "transpose": 12, "duration": 200, "delay": 480})).unwrap();
    e.execute("event.midi_track_offsets", &serde_json::json!({"track": track.0, "offset": 4800})).unwrap();

    let result = run(&mut e, &dir);
    let midi = paths(&result, "midi_files");
    let smf = transit_midi::read_smf(&std::fs::read(&midi[0]).unwrap()).unwrap();
    let note = smf.tracks[0].sequence.notes[0];
    assert_eq!((note.pitch, note.velocity, note.start, note.length), (72, 110, 864, 960));
    let stems = paths(&result, "files");
    let (_, decoded) = transit_audio_io::decode(&std::fs::read(&stems[0]).unwrap(), Some("wav")).unwrap();
    assert_eq!(decoded.frames(), 69_600, "RTP delay and offset extend the rendered tail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn playback_content_end_converts_delay_across_tempo_changes() {
    let mut s = Session::default();
    s.tempo.set_tempo(0, 600.0).unwrap();
    s.tempo.set_tempo(960, 60.0).unwrap();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Tempo"));
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(clip, "phrase", 0, 48_000, Sequence::default()));
    let mut e = Engine::new(s);
    e.execute("event.midi_rtp", &serde_json::json!({"track": track.0, "delay": 1920})).unwrap();
    assert_eq!(transit_mix::playback_content_end(e.session()), 100_800, "960 ticks at 600 BPM plus 960 at 60 BPM extend the tail");
}

#[test]
fn an_inactive_external_instrument_is_refused_then_named_after_confirmation() {
    let dir = scratch("consolidated-inactive-external");
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Hardware"));
    s.track_mut(track).unwrap().instrument = Some(transit_model::Insert::external("In 1"));
    s.track_mut(track).unwrap().inactive = true;
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(clip, "phrase", 0, 4800, Sequence::default()));
    let mut e = Engine::new(s);

    let err = e.execute("file.bounce_consolidated", &serde_json::json!({"dir": dir.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("Hardware"), "the inactive device is named in the refusal: {err}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "nothing is written before confirmation");
    let result = e.execute("file.bounce_consolidated", &serde_json::json!({"dir": dir.to_string_lossy(), "without_external": true})).unwrap();
    assert_eq!(result["left_out"], serde_json::json!(["Hardware"]));
    assert_eq!(result["files"].as_array().unwrap().len(), 0);
    assert_eq!(result["midi_files"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
