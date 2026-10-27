//! Writes the session compatibility corpus files for this build (#161).
//!
//! `cargo test -p transit-engine --test corpus_generate -- --ignored --nocapture` saves
//! `v<version>-demo.transit` and `v<version>-features.transit` into `crates/model/tests/corpus/`
//! through the same `save_session` the app uses, so the JSON is what this version writes. The audio
//! the save also produces is discarded: a corpus file carries no media. Set `TRANSIT_CORPUS_DIR`
//! to write somewhere else.

use std::path::{Path, PathBuf};
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_engine::Engine;
use transit_engine::model::{
    AutoParam, ChannelFormat, Clip, ClipLoop, Control, ControlTarget, Fade, FadeShape, Group, GroupId, Insert, MapMode, Mapping, MarkerKind,
    MidiSource, Route, SendSlot, Session, SourceKind, Strip, TrackKind,
};
use transit_midi::{Note, Sequence};
use transit_time::{SampleRate, TICKS_PER_QUARTER};

fn corpus_dir() -> PathBuf {
    std::env::var_os("TRANSIT_CORPUS_DIR").map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/corpus"), PathBuf::from)
}

/// A session saved the way File > Save As does, reduced to its `.transit` file.
fn save_corpus_file(session: Session, name: &str) {
    let version = env!("CARGO_PKG_VERSION");
    let scratch = std::env::temp_dir().join(format!("transit-corpus-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let file = format!("v{version}-{name}.transit");
    let mut engine = Engine::new(session);
    transit_engine::io::save_session(&mut engine, scratch.join(&file).to_str().unwrap()).unwrap();
    let out = corpus_dir();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::copy(scratch.join(&file), out.join(&file)).unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
    println!("wrote {}", out.join(&file).display());
}

fn features_session() -> Session {
    let sr = SampleRate::HZ_48000;
    let mut s = Session::new("Features", sr);
    let _ = s.tempo.set_tempo(0, 120.0);
    let q = TICKS_PER_QUARTER;
    let beat = sr.samples(0.5);
    // Two audio tracks sharing a tiny generated source (kept in memory; the save step discards the WAV).
    let tone: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    let src = transit_engine::io::add_source(&mut s, "tone", AudioBuffer { sample_rate: sr.hz(), channels: vec![tone] }, None, FileFormat::Wav);
    let mut audio = Vec::new();
    for name in ["Audio A", "Audio B"] {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        let cid = s.new_clip_id();
        let mut clip = Clip::audio(cid, name, src, 0, beat, 4800);
        clip.fade_in = Fade { len: 480, shape: FadeShape::SCurve, curvature: 0.25 };
        clip.fade_out = Fade { len: 960, shape: FadeShape::SCurve, curvature: 0.0 };
        transit_engine::edit::place_clip(&mut s, t, clip);
        audio.push(t);
    }
    // An instrument track with a looped MIDI clip.
    let inst = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Synth"));
    let mut seq = Sequence::default();
    for (i, pitch) in [60u8, 64, 67, 72].into_iter().enumerate() {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: i as i64 * q, length: q / 2 });
    }
    seq.sort();
    let cid = s.new_clip_id();
    let mut midi = Clip::midi(cid, "Loop", 0, beat * 8, seq);
    midi.set_loop(Some(ClipLoop { start: 0, end: beat * 4 }));
    transit_engine::edit::place_clip(&mut s, inst, midi);
    if let Some(t) = s.track_mut(inst) {
        t.instrument = Some(Insert::new("subtractive_synth"));
    }
    // An External Instrument track.
    let ext = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Hardware Synth"));
    if let Some(t) = s.track_mut(ext) {
        t.instrument = Some(Insert::external("In 1"));
    }
    // An FX channel (aux on a bus) with a send from the first audio track.
    let bus = s.add_bus("FX", ChannelFormat::Stereo);
    let fx = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("FX 1"));
    if let Some(t) = s.track_mut(fx) {
        t.mixer.input = Route::Bus(bus);
        t.mixer.inserts[0] = Some(Insert::new("plate_reverb"));
    }
    if let Some(t) = s.track_mut(audio[0]) {
        let mut send = SendSlot::new(Route::Bus(bus));
        send.level_db = -9.0;
        t.mixer.sends[0] = Some(send);
        // An automation lane with two breakpoints.
        let lane = t.lane_mut(&AutoParam::Volume);
        lane.set_point(0, -20.0);
        lane.set_point(beat * 4, -3.0);
    }
    // A group of the two audio tracks.
    let gid = GroupId(s.alloc());
    let color = s.next_track_color();
    s.groups.push(Group {
        id: gid,
        name: "Audio".into(),
        letter: 'a',
        members: audio.clone(),
        edit: true,
        mix: true,
        active: true,
        color,
        attributes: vec!["volume".into(), "mute".into(), "solo".into(), "record".into()],
    });
    s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    s.add_marker("Start", MarkerKind::Marker, 0, 0);
    // A MIDI mapping: CC 7 on a controller drives the first audio track's volume.
    let source = MidiSource { port: "Controller".into(), channel: Some(0), kind: SourceKind::Cc, number: 7 };
    let mut m = Mapping::new(source, ControlTarget { strip: Strip::Track(audio[0]), control: Control::Volume });
    m.mode = MapMode::Scaled;
    s.midi_mappings.set(m).unwrap();
    s
}

#[test]
#[ignore = "writes files into crates/model/tests/corpus; run it by hand at each release"]
fn write_corpus_files() {
    save_corpus_file(transit_engine::demo::demo_session(), "demo");
    save_corpus_file(features_session(), "features");
}
