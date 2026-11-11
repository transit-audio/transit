//! DAWproject export (#261, slice 2): schema-shaped XML and round trips through our importer.

use super::*;
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_model::{Clip, MarkerKind};

fn archive(xml: &str) -> std::io::Cursor<Vec<u8>> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    w.start_file("project.xml", zip::write::SimpleFileOptions::default()).unwrap();
    w.write_all(xml.as_bytes()).unwrap();
    let mut c = w.finish().unwrap();
    c.set_position(0);
    c
}

fn read(xml: &str) -> (Session, Vec<String>) {
    read_dawproject(archive(xml), "round trip").unwrap()
}

fn only_clip(s: &Session) -> &Clip {
    let clips: Vec<_> = s.tracks.iter().flat_map(|t| t.clips()).collect();
    assert_eq!(clips.len(), 1, "{clips:?}");
    clips[0]
}

fn sequence(c: &Clip) -> &Sequence {
    match &c.content {
        ClipContent::Midi { sequence } => sequence,
        other => panic!("not MIDI: {other:?}"),
    }
}

#[test]
fn tracks_maps_markers_notes_expression_and_stem_clips_round_trip() {
    let mut s = Session::new("A & <B>", transit_time::SampleRate::HZ_48000);
    s.tempo.set_tempo(0, 80.0).unwrap();
    s.tempo.set_tempo(960, 140.0).unwrap();
    s.tempo.set_meter(0, 3, 4).unwrap();
    s.tempo.set_meter(2880, 6, 8).unwrap();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Piano & <Lead> \"quoted\""));
    let t = s.track_mut(track).unwrap();
    t.mixer.volume_db = -6.0;
    t.mixer.pan = vec![-1.0, 0.0];
    t.mixer.mute = true;
    t.mixer.solo = true;
    let start = s.tempo.tick_to_samples(960, s.sample_rate);
    let end = s.tempo.tick_to_samples(3840, s.sample_rate);
    let mut clip = Clip::midi(
        s.new_clip_id(),
        "Verse & < chorus >",
        start,
        end - start,
        Sequence {
            notes: vec![Note { pitch: 60, velocity: 64, release_velocity: 32, channel: 1, start: 480, length: 960 }],
            ctrls: vec![
                CtrlEvent { tick: 0, channel: 1, kind: CtrlKind::Cc(64), value: 127 },
                CtrlEvent { tick: 480, channel: 1, kind: CtrlKind::PitchBend, value: -8192 },
                CtrlEvent { tick: 960, channel: 1, kind: CtrlKind::ChannelPressure, value: 90 },
                CtrlEvent { tick: 1440, channel: 1, kind: CtrlKind::PolyPressure(60), value: 108 },
                CtrlEvent { tick: 1920, channel: 1, kind: CtrlKind::Program, value: 5 },
            ],
        },
    );
    clip.muted = true;
    clip.color = Some([0x12, 0xab, 0xef]);
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
    let at = s.tempo.tick_to_samples(1920, s.sample_rate);
    s.add_marker("B & <section>", MarkerKind::Marker, at, at);

    let stem = Stem { track, path: "audio/track-0.wav".into(), disk: PathBuf::new(), channels: 2 };
    let xml = project_xml(&s, &[stem]);
    let doc = roxmltree::Document::parse(&xml).unwrap();
    let clip = doc.descendants().find(|n| n.has_tag_name("Clip") && n.attribute("name") == Some("Verse & < chorus >")).unwrap();
    let children: Vec<_> = clip.children().filter(roxmltree::Node::is_element).map(|n| n.tag_name().name()).collect();
    assert_eq!(children, ["Lanes"], "Project.xsd gives Clip one timeline child; Notes and Points share a Lanes");
    for tag in ["TempoAutomation", "TimeSignatureAutomation"] {
        let points = doc.descendants().find(|n| n.has_tag_name(tag)).unwrap();
        let first = points.children().find(roxmltree::Node::is_element).map(|n| n.tag_name().name());
        assert_eq!(first, Some("Target"), "Project.xsd requires every Points timeline to start with Target: {tag}");
    }
    let (got, report) = read(&xml);
    assert_eq!(report, vec!["Not imported from the DAWproject: 1 audio clips"]);
    assert_eq!(got.tempo.tempos(), s.tempo.tempos());
    assert_eq!(got.tempo.meters(), s.tempo.meters());
    assert_eq!(got.tracks.len(), 1);
    let t = &got.tracks[0];
    assert_eq!((t.kind, t.name.as_str()), (TrackKind::Instrument, "Piano & <Lead> \"quoted\""));
    assert!((t.mixer.volume_db - -6.0).abs() < 1e-6);
    assert_eq!(t.mixer.pan, vec![-1.0, 0.0]);
    assert!(t.mixer.mute && t.mixer.solo);
    let c = only_clip(&got);
    assert_eq!((c.name.as_str(), c.start, c.length, c.muted), ("Verse & < chorus >", start, end - start, true));
    assert_eq!(sequence(c).notes, vec![Note { pitch: 60, velocity: 64, release_velocity: 32, channel: 1, start: 480, length: 960 }]);
    for event in [
        CtrlEvent { tick: 0, channel: 1, kind: CtrlKind::Cc(64), value: 127 },
        CtrlEvent { tick: 480, channel: 1, kind: CtrlKind::PitchBend, value: -8192 },
        CtrlEvent { tick: 960, channel: 1, kind: CtrlKind::ChannelPressure, value: 90 },
        CtrlEvent { tick: 1440, channel: 1, kind: CtrlKind::PolyPressure(60), value: 108 },
        CtrlEvent { tick: 1920, channel: 1, kind: CtrlKind::Program, value: 5 },
    ] {
        assert!(sequence(c).ctrls.contains(&event), "missing {event:?}: {:?}", sequence(c).ctrls);
    }
    assert_eq!(got.markers.len(), 1);
    assert_eq!((got.markers[0].name.as_str(), got.markers[0].start), ("B & <section>", at));
    assert!(xml.contains("<Application name=\"transit\" version=\""));
    assert!(xml.contains("color=\"#12abef\""));
}

#[test]
fn a_loop_keeps_its_content_window_and_a_plain_midi_track_needs_no_stem() {
    let mut s = Session::new("Loops", transit_time::SampleRate::HZ_48000);
    let track = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("MIDI"));
    let mut clip = Clip::midi(
        s.new_clip_id(),
        "Loop",
        0,
        48_000,
        Sequence { notes: vec![Note { pitch: 64, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 }], ctrls: Vec::new() },
    );
    clip.looping = Some(transit_model::ClipLoop { start: 0, end: 24_000 });
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
    let xml = project_xml(&s, &[]);
    assert!(xml.contains("contentType=\"notes\""), "{xml}");
    assert!(xml.contains("playStart=\"0.000000000\" loopStart=\"0.000000000\" loopEnd=\"1.000000000\""), "{xml}");
    let (got, report) = read(&xml);
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(got.tracks[0].kind, TrackKind::Instrument, "DAWproject note tracks import as instrument tracks");
    let notes = &sequence(only_clip(&got)).notes;
    assert_eq!(notes.len(), 2);
    assert_eq!(notes.iter().map(|n| n.start).collect::<Vec<_>>(), vec![0, 960]);
}

fn temp_dir() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("transit-dawproject-export-{}-{stamp}-{serial}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    dir
}

fn zip_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Vec<u8> {
    use std::io::Read as _;
    let mut entry = archive.by_name(name).unwrap();
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn the_command_writes_a_complete_archive_and_replaces_it_only_when_asked() {
    let dir = temp_dir();
    let mut s = Session::new("Tone", transit_time::SampleRate::HZ_48000);
    let source =
        add_source(&mut s, "tone", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800], vec![0.25; 4800]] }, None, FileFormat::Wav);
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Tone"));
    s.track_mut(track).unwrap().mixer.volume_db = -6.0;
    s.track_mut(track).unwrap().mixer.pan = vec![-1.0, 0.0];
    let mut volume = transit_model::AutomationLane::new(transit_model::AutoParam::Volume);
    volume.set_point(0, -6.0);
    s.track_mut(track).unwrap().automation.push(volume);
    let mut pan = transit_model::AutomationLane::new(transit_model::AutoParam::Pan(1));
    pan.set_point(0, 0.0);
    s.track_mut(track).unwrap().automation.push(pan);
    let clip = Clip::audio(s.new_clip_id(), "tone", source, 0, 0, 4800);
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(clip);
    let other = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Solo safe"));
    s.track_mut(other).unwrap().mixer.solo_safe = true;
    let clip = Clip::audio(s.new_clip_id(), "other", source, 0, 0, 4800);
    s.track_mut(other).unwrap().playlist_mut().unwrap().clips.push(clip);
    let mut e = Engine::new(s);
    let requested = dir.join("tone");
    let path = dir.join("tone.dawproject");
    let result = e.execute("file.export_dawproject", &serde_json::json!({"path": requested.to_string_lossy()})).unwrap();
    assert_eq!(result["tracks"], serde_json::json!(2));
    assert_eq!(result["clips"], serde_json::json!(0));
    assert_eq!(result["stems"], serde_json::json!(2));
    assert_eq!(result["path"], serde_json::json!(path.to_string_lossy()));
    assert!(result["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("full-length WAV stems")));
    assert!(result["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("volume/pan automation is omitted")));
    assert!(!e.is_dirty(), "export does not edit the session");
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(archive.len(), 3);
    let xml = String::from_utf8(zip_entry(&mut archive, "project.xml")).unwrap();
    assert!(xml.contains("<File path=\"audio/track-0.wav\"/>"), "{xml}");
    let wav = zip_entry(&mut archive, "audio/track-0.wav");
    let (info, audio) = transit_audio_io::decode(&wav, Some("wav")).unwrap();
    assert_eq!((info.frames, audio.frames(), audio.num_channels()), (4800, 4800, 2));
    assert_eq!((audio.channels[0][0], audio.channels[1][0]), (0.5, 0.25));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "temporary stems and part files are cleaned up");

    let before = std::fs::read(&path).unwrap();
    let err = e.execute("file.export_dawproject", &serde_json::json!({"path": path.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("replace: true"), "{err}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "a refused replacement leaves the old archive alone");
    e.execute("file.export_dawproject", &serde_json::json!({"path": path.to_string_lossy(), "replace": true})).unwrap();
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn export_respects_the_automation_write_root() {
    let dir = temp_dir();
    std::fs::create_dir(dir.join("inside")).unwrap();
    let roots = crate::file_access::AutomationRoots::new(None, Some(&dir.join("inside"))).unwrap().unwrap();
    let mut e = Engine::default();
    let inside = dir.join("inside/ok.dawproject");
    let outside = dir.join("outside/no.dawproject");
    crate::file_access::confine(Some(&roots), || {
        e.execute("file.export_dawproject", &serde_json::json!({"path": inside.to_string_lossy()})).unwrap();
        let err = e.execute("file.export_dawproject", &serde_json::json!({"path": outside.to_string_lossy()})).unwrap_err().to_string();
        assert!(err.contains("outside the write root"), "{err}");
    });
    assert!(inside.is_file() && !outside.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsupported_structure_and_external_audio_are_named_in_the_report() {
    let mut s = Session::default();
    s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Reverb"));
    let external = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Hardware"));
    s.track_mut(external).unwrap().instrument = Some(transit_model::Insert::external("In 1"));
    s.add_marker("Selection", MarkerKind::Selection, 0, 100);
    let notes = export_notes(&s, 0, &["Hardware".into()]);
    assert!(notes.iter().any(|n| n.contains("1 aux, VCA and folder tracks")));
    assert!(notes.iter().any(|n| n.contains("External Instrument audio")));
    assert!(notes.iter().any(|n| n.contains("1 stored selections")));
}

#[test]
fn xml_attribute_whitespace_and_invalid_controls_survive_safely() {
    let mut s = Session::default();
    s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("One\nTwo\tThree\rFour\u{1}"));
    let (got, report) = read(&project_xml(&s, &[]));
    assert!(report.is_empty(), "{report:?}");
    assert_eq!(got.tracks[0].name, "One\nTwo\tThree\rFour\u{fffd}");
}

#[test]
fn stereo_balance_inverts_both_sides_of_the_importers_mapping() {
    for (pan, want) in [(&[-1.0, 1.0][..], 0.5), (&[-1.0, 0.0][..], 0.25), (&[0.0, 1.0][..], 0.75)] {
        let (value, exact) = balance(pan);
        assert!(exact);
        assert!((value - want).abs() < 1e-6, "{pan:?}: {value}");
    }
    assert!(!balance(&[0.0, 0.0]).1, "independent dual pan is not one balance");
    assert!(!balance(&[0.0, 0.0, 0.0]).1, "surround is not one balance");
}

#[test]
fn an_unrelated_part_file_is_never_truncated_or_removed() {
    let dir = temp_dir();
    let path = dir.join("song.dawproject");
    let part = dir.join("song.dawproject.part");
    std::fs::write(&part, b"not an export").unwrap();
    let mut e = Engine::default();
    let err = e.execute("file.export_dawproject", &serde_json::json!({"path": path.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("part"), "{err}");
    assert_eq!(std::fs::read(&part).unwrap(), b"not an export");
    assert!(!path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_nested_aux_chain_renders_into_the_source_stem() {
    let dir = temp_dir();
    let mut s = Session::default();
    let source = add_source(&mut s, "tone", AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4800]; 2] }, None, FileFormat::Wav);
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Source"));
    let clip = s.new_clip_id();
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(clip, "tone", source, 0, 0, 4800));
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
    let path = dir.join("nested.dawproject");
    e.execute("file.export_dawproject", &serde_json::json!({"path": path.to_string_lossy()})).unwrap();
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let wav = zip_entry(&mut archive, "audio/track-0.wav");
    let (_, audio) = transit_audio_io::decode(&wav, Some("wav")).unwrap();
    let want = 0.5 * transit_dsp::db_to_gain(-6.0);
    assert!((audio.channels[0][0] - want).abs() < 1e-4, "nested aux processing reached the stem: {} against {want}", audio.channels[0][0]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inactive_tracks_are_omitted_and_solo_safe_is_translated() {
    let mut s = Session::default();
    let solo = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Solo"));
    let controller = s.add_track(TrackKind::Vca, ChannelFormat::Stereo, Some("VCA solo"));
    s.track_mut(controller).unwrap().mixer.solo = true;
    let safe = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Safe"));
    s.track_mut(safe).unwrap().mixer.solo_safe = true;
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Inactive master"));
    s.track_mut(master).unwrap().inactive = true;
    s.track_mut(master).unwrap().mixer.mute = true;
    let inactive = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Inactive MIDI"));
    s.track_mut(inactive).unwrap().inactive = true;
    let stems = [
        Stem { track: solo, path: "audio/solo.wav".into(), disk: PathBuf::new(), channels: 2 },
        Stem { track: safe, path: "audio/safe.wav".into(), disk: PathBuf::new(), channels: 2 },
    ];
    let xml = project_xml(&s, &stems);
    let doc = roxmltree::Document::parse(&xml).unwrap();
    assert!(!xml.contains("Inactive master") && !xml.contains("Inactive MIDI"), "{xml}");
    for (name, expected) in [("Solo", "false"), ("Safe", "true")] {
        let track = doc.descendants().find(|n| n.has_tag_name("Track") && n.attribute("name") == Some(name)).unwrap();
        let channel = track.children().find(|n| n.has_tag_name("Channel")).unwrap();
        assert_eq!(channel.attribute("solo"), Some(expected), "{name} under a VCA-only solo");
    }
    let notes = export_notes(&s, 2, &[]);
    assert!(notes.iter().any(|n| n.contains("2 inactive tracks")), "{notes:?}");
}
