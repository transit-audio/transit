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
    AutoParam, ChannelFormat, Clip, ClipLoop, Control, ControlTarget, Fade, FadeShape, Group, GroupId, Insert, MACRO_SLOT, MacroMapping, MapMode,
    Mapping, MarkerKind, MidiSource, ParamRange, Rack, RackMember, Route, SendSlot, Session, SourceKind, Strip, TrackKind, Zone, macro_param,
};
use transit_midi::{Note, Sequence};
use transit_time::{SampleRate, TICKS_PER_QUARTER};

fn corpus_dir() -> PathBuf {
    std::env::var_os("TRANSIT_CORPUS_DIR").map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/corpus"), PathBuf::from)
}

/// A session saved the way File > Save As does, reduced to its `.transit` file.
fn save_corpus_file(session: Session, name: &str) {
    let version = env!("CARGO_PKG_VERSION");
    let scratch = transit_test_support::TestDir::new("corpus");
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
        t.instrument = Some(Insert::new(transit_model::DEFAULT_INSTRUMENT));
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
    add_v060_features(&mut s, inst, audio[0]);
    add_rack(&mut s);
    add_v070_features(&mut s, audio[1]);
    s
}

/// What v0.7.0 serializes: a Sampler kit (ADR 0041) on its own track, with two managed sources (one
/// at its native 44.1 kHz), pads with names, a stored note, mute, solo and a choke group, a pad start
/// note, keyed zones with every playback field, forward and ping-pong loops with a crossfade, edge
/// fades, a round-robin group and a velocity-layered group with a seed, zones bound to a slice of
/// each marker set, the slice loop with both marker sets (each with a removed id) and every detector
/// and grid field, and slot controls; a user page on the Sampler (ADR 0047); a Custom and a Fit track
/// height; Grid Over Clips on. Transport mappings and Live outputs are per machine and never saved.
fn add_v070_features(s: &mut Session, audio: transit_engine::model::TrackId) {
    use transit_model::sampler_patch::{
        AutoSource, Detector, Division, Fades, GroupEntry, LoopMode, Marker, Markers, Membership, Ownership, PadEntry, PatchState, PatchV1,
        RoundRobin, SliceBinding, SliceLoop, SliceMode, SliceSet, SourceRef, ZoneEntry,
    };
    let sr = s.sample_rate;
    let hit: Vec<f32> = (0..9600).map(|i| (i as f32 * 0.02).sin() * (1.0 - i as f32 / 9600.0)).collect();
    let hit = transit_engine::io::add_source(s, "Kit Hit", AudioBuffer { sample_rate: sr.hz(), channels: vec![hit] }, None, FileFormat::Wav);
    let brk: Vec<f32> = (0..44_100).map(|i| if i % 11_025 < 400 { 0.8 } else { (i as f32 * 0.003).sin() * 0.1 }).collect();
    let brk = transit_engine::io::add_source_audio(
        s,
        "Break",
        std::sync::Arc::new(transit_engine::model::SourceAudio::new(AudioBuffer { sample_rate: 44_100, channels: vec![brk] })),
        None,
        FileFormat::Wav,
    );
    let managed = |source| SourceRef { source, ownership: Ownership::Managed, extra: Default::default() };
    let pad_zone = |id: u16, end: u64, note: u8, label: &str| {
        let mut z = ZoneEntry { label: Some(label.into()), ..ZoneEntry::whole(id, 0, end) };
        z.pad_defaults(note);
        z
    };
    let keyed = ZoneEntry {
        keys: [48, 72],
        velocities: [10, 127],
        root: 60,
        transpose: 12.0,
        cents: -15.0,
        reverse: true,
        attack_ms: 2.0,
        decay_ms: 250.0,
        sustain: 0.6,
        release_ms: 300.0,
        declick_ms: 1.5,
        velocity_sensitivity: 0.5,
        loop_mode: LoopMode::Forward,
        loop_start: 1000,
        loop_end: 8000,
        crossfade: 400,
        fades: Fades { key_lo: 2, key_hi: 3, vel_lo: 4, vel_hi: 0, extra: Default::default() },
        label: Some("Keys".into()),
        ..ZoneEntry::whole(2, 0, 9600)
    };
    let ping_pong = ZoneEntry { keys: [73, 96], loop_mode: LoopMode::PingPong, loop_start: 500, loop_end: 9000, ..ZoneEntry::whole(3, 0, 9600) };
    let member = |id: u16, group: u8, layer: u8, alternative: u8, keys: [u8; 2], velocities: [u8; 2]| ZoneEntry {
        keys,
        velocities,
        membership: Some(Membership { group, layer, alternative, extra: Default::default() }),
        ..ZoneEntry::whole(id, 0, 4800)
    };
    let sliced = |id: u16, set: SliceSet, slice: u32, start: u64, end: u64| {
        let mut z = ZoneEntry {
            id,
            source: 1,
            start,
            end,
            loop_end: end,
            slice: Some(SliceBinding { set, id: slice, extra: Default::default() }),
            ..ZoneEntry::default()
        };
        z.pad_defaults(42);
        z
    };
    let marker = |id: u32, frame: u64, label: Option<&str>| Marker { id, frame, label: label.map(str::to_string), extra: Default::default() };
    let patch = PatchV1 {
        seed: 12_345,
        sources: vec![managed(hit), managed(brk)],
        zones: vec![
            pad_zone(0, 9600, 40, "Kick"),
            pad_zone(1, 4800, 41, "Hat"),
            keyed,
            ping_pong,
            member(4, 1, 0, 0, [36, 47], [1, 127]),
            member(5, 1, 0, 1, [36, 47], [1, 127]),
            member(6, 2, 0, 0, [24, 35], [1, 63]),
            member(7, 2, 1, 0, [24, 35], [64, 127]),
            sliced(8, SliceSet::Auto, 2, 11_025, 22_050),
            sliced(9, SliceSet::Manual, 11, 16_000, 30_000),
        ],
        pad_start_note: 40,
        pads: vec![
            PadEntry { index: 0, zone: Some(0), choke: 1, name: Some("Kick".into()), ..PadEntry::default() },
            PadEntry { index: 1, zone: Some(1), choke: 1, solo: true, name: Some("Hat".into()), ..PadEntry::default() },
            PadEntry { index: 2, zone: Some(8), mute: true, ..PadEntry::default() },
            PadEntry { index: 3, zone: Some(9), note: Some(20), ..PadEntry::default() },
        ],
        groups: vec![
            GroupEntry { id: 1, round_robin: RoundRobin::Sequential, extra: Default::default() },
            GroupEntry { id: 2, round_robin: RoundRobin::Random, extra: Default::default() },
        ],
        slicing: Some(SliceLoop {
            source: 1,
            start: 0,
            end: 44_100,
            mode: SliceMode::Manual,
            snap: false,
            detector: Detector { source: AutoSource::Transients, sensitivity: 0.65, extra: Default::default() },
            division: Division::Eighth,
            bars: Some(1),
            grid_bpm: Some(96.0),
            root: Some(48),
            beat_length: Some(4.0),
            auto: Some(Markers {
                markers: vec![marker(1, 0, None), marker(2, 11_025, None), marker(4, 22_050, None)],
                next_id: 5,
                extra: Default::default(),
            }),
            manual: Some(Markers {
                markers: vec![marker(10, 0, Some("Intro")), marker(11, 16_000, None), marker(13, 30_000, Some("Fill"))],
                next_id: 14,
                extra: Default::default(),
            }),
            markers: None,
            extra: Default::default(),
        }),
        controls: [("zone001_level".to_string(), -6.0), ("zone003_pan".to_string(), -30.0)].into_iter().collect(),
        ..PatchV1::default()
    };
    assert_eq!(patch.validate(), Ok(()), "the corpus kit must load without a note");
    let mut ins = Insert::new("sampler");
    ins.sampler_patch = Some(transit_model::SamplerPatch { state: PatchState::Ready(Box::new(patch)) });
    let mut page = transit_model::UserPage::named("Kit");
    page.slots[0] = Some("zone001_level".into());
    page.slots[1] = Some("zone003_pan".into());
    ins.pages.push(page);
    let kit = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Kit"));
    if let Some(t) = s.track_mut(kit) {
        t.instrument = Some(ins);
        t.height = transit_model::TrackHeight::Custom(200);
    }
    if let Some(t) = s.track_mut(audio) {
        t.height = transit_model::TrackHeight::Fit(120);
    }
    s.edit.grid_over_clips = true;
}

/// What v0.6.0 serializes beyond the rack: a MIDI effect slot with a Random Sequencer ahead of the
/// instrument, a Resonator and a Modal Voice track, a Filter, a Time Stretcher and a Tuner insert,
/// and a Touch return ramp that is not the default. System Audio stays off (the default, nothing
/// saved). A Texture insert is left out: its frozen recording is a WAV beside the session.
fn add_v060_features(s: &mut Session, synth: transit_engine::model::TrackId, audio: transit_engine::model::TrackId) {
    if let Some(t) = s.track_mut(synth) {
        t.midi_effects.push(transit_engine::model::MidiEffectSlot::new("midi_transpose"));
        t.midi_effects.push(transit_engine::model::MidiEffectSlot::new("random_sequencer"));
    }
    for (name, id) in [("Resonator", "resonator"), ("Modal Voice", "modal_voice")] {
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(name));
        if let Some(tr) = s.track_mut(t) {
            tr.instrument = Some(Insert::new(id));
        }
    }
    if let Some(t) = s.track_mut(audio) {
        t.mixer.inserts[1] = Some(Insert::new("filter"));
        t.mixer.inserts[2] = Some(Insert::new("time_stretcher"));
        t.mixer.inserts[3] = Some(Insert::new("tuner"));
    }
    s.touch_return_ms = 350;
}

/// A rack (ADR 0034, #197): two instruments in a routing folder with its bus, overlapping key
/// zones with fades and a velocity zone, a MIDI clip on the rack, macro 1 mapped with a range and a
/// curve and automated, macro 2 learned to a controller.
fn add_rack(s: &mut Session) {
    let sr = s.sample_rate;
    let beat = sr.samples(0.5);
    let bus = s.add_bus("Rack Bus", ChannelFormat::Stereo);
    let folder = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some("Rack"));
    let mut members = Vec::new();
    for (name, key_zone, vel_zone) in [
        ("Low Layer", Zone { low: 0, fade_low: 0, fade_high: 60, high: 64 }, Zone::FULL),
        ("High Layer", Zone { low: 60, fade_low: 64, fade_high: 127, high: 127 }, Zone { low: 1, fade_low: 1, fade_high: 100, high: 127 }),
    ] {
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(name));
        if let Some(tr) = s.track_mut(t) {
            tr.instrument = Some(Insert::new(transit_model::DEFAULT_INSTRUMENT));
            tr.folder = Some(folder);
            tr.mixer.output = Route::Bus(bus);
        }
        members.push(RackMember { track: t, key_zone, vel_zone });
    }
    let low = members[0].track;
    let mut gain = Insert::new("gain");
    gain.params.insert("gain".into(), 0.0);
    if let Some(t) = s.track_mut(low) {
        t.mixer.inserts[0] = Some(gain);
    }
    let mut rack = Rack { members, ..Rack::default() };
    rack.macros[0].name = "Tone".into();
    rack.macros[0].value = 0.5;
    rack.macros[0].mappings.push(MacroMapping {
        track: low,
        slot: 0,
        param: "gain".into(),
        min: 0.25,
        max: 0.75,
        curve: 0.5,
        range: ParamRange { lo: -96.0, hi: 24.0, log: false, step: false },
    });
    rack.macros[1].midi = Some(MidiSource { port: "Controller".into(), channel: Some(0), kind: SourceKind::Cc, number: 21 });
    let mut seq = Sequence::default();
    for (i, pitch) in [48u8, 62, 72].into_iter().enumerate() {
        seq.notes.push(Note {
            pitch,
            velocity: 100,
            release_velocity: 64,
            channel: 0,
            start: i as i64 * TICKS_PER_QUARTER,
            length: TICKS_PER_QUARTER,
        });
    }
    let cid = s.new_clip_id();
    let clip = Clip::midi(cid, "Layers", 0, beat * 4, seq);
    if let Some(t) = s.track_mut(folder) {
        t.mixer.input = Route::Bus(bus);
        t.rack = Some(rack);
        if let Some(p) = t.playlist_mut() {
            p.clips.push(clip);
        }
        let lane = t.lane_mut(&AutoParam::Plugin { slot: MACRO_SLOT, param: macro_param(0) });
        lane.set_point(0, 0.25);
        lane.set_point(beat * 4, 0.75);
    }
}

/// A session with one rack and nothing else: the corpus fixture of #197.
fn rack_session() -> Session {
    let mut s = Session::new("Rack", SampleRate::HZ_48000);
    let _ = s.tempo.set_tempo(0, 120.0);
    add_rack(&mut s);
    s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    s
}

#[test]
#[ignore = "writes files into crates/model/tests/corpus; run it by hand at each release"]
fn write_corpus_files() {
    save_corpus_file(transit_engine::demo::demo_session(), "demo");
    save_corpus_file(features_session(), "features");
}

#[test]
#[ignore = "writes the rack fixture (#197) into crates/model/tests/corpus; run it by hand"]
fn write_rack_fixture() {
    save_corpus_file(rack_session(), "rack");
}

/// An audio track and a MIDI track, each with a take stack of three takes on three playlists
/// (#25, ADR 0039): the audio track plays take 2 over half the stack with take 3 around it, the
/// MIDI track plays take 3; a clip outside the stacks plays on the audio track.
fn takes_session() -> Session {
    let sr = SampleRate::HZ_48000;
    let mut s = Session::new("Takes", sr);
    let _ = s.tempo.set_tempo(0, 120.0);
    let tone: Vec<f32> = (0..96_000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
    let src = transit_engine::io::add_source(&mut s, "Vox_01", AudioBuffer { sample_rate: sr.hz(), channels: vec![tone] }, None, FileFormat::Wav);
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let mut lists: Vec<Vec<Clip>> = vec![Vec::new(), Vec::new(), Vec::new()];
    let take = |s: &mut Session, n: u32, offset: i64, start: i64, len: i64| {
        let mut c = Clip::audio(s.new_clip_id(), format!("Vox_01.{n}"), src, offset, start, len);
        c.take = Some(n);
        c
    };
    // Playing: take 3, then take 2 over the second half (a shorter take chosen inside a longer one).
    lists[0].push(take(&mut s, 3, 64_000, 0, 24_000));
    lists[0].push(take(&mut s, 2, 56_000, 24_000, 8_000));
    lists[0].push(Clip::audio(s.new_clip_id(), "Later", src, 0, 192_000, 24_000));
    lists[1].push(take(&mut s, 1, 0, 0, 32_000));
    lists[2].push(take(&mut s, 2, 32_000, 0, 24_000));
    lists[2].push(take(&mut s, 3, 88_000, 24_000, 8_000));
    if let Some(t) = s.track_mut(vox) {
        t.playlists = lists.into_iter().enumerate().map(|(i, clips)| transit_model::Playlist { name: format!("Vox.{:02}", i + 1), clips }).collect();
        t.active_playlist = 0;
    }
    let keys = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
    let q = TICKS_PER_QUARTER;
    let mut midi = Vec::new();
    for (n, pitch) in [(1u32, 60u8), (2, 62), (3, 64)] {
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: q });
        let mut c = Clip::midi(s.new_clip_id(), format!("Keys_{n:02}"), 0, 48_000, seq);
        c.take = Some(n);
        midi.push(c);
    }
    if let Some(t) = s.track_mut(keys) {
        let mut it = midi.into_iter();
        let (a, b, c) = (it.next(), it.next(), it.next());
        t.playlists = [c, a, b]
            .into_iter()
            .enumerate()
            .map(|(i, clip)| transit_model::Playlist { name: format!("Keys.{:02}", i + 1), clips: clip.into_iter().collect() })
            .collect();
        t.active_playlist = 0;
    }
    s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    s
}

#[test]
#[ignore = "writes the take stack fixture (#25) into crates/model/tests/corpus; run it by hand"]
fn write_takes_fixture() {
    save_corpus_file(takes_session(), "takes");
}
