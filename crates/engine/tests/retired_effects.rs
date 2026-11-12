//! A session saved by a build that had the inherited effects retired in stage 1 (#264, ADR 0040)
//! opens with those slots emptied, reports each track, slot and device by name, says the count in
//! one status line, reads as modified, and leaves the rest of the track as it was.

use serde_json::json;
use std::path::PathBuf;
use transit_engine::Engine;
use transit_model::{
    AutoParam, ChannelFormat, Control, ControlTarget, Insert, MacroMapping, Mapping, MidiSource, Rack, Session, SourceKind, Strip, TrackKind,
};
use transit_time::Range;

const STATUS: &str = "3 insert(s) used retired built-in effects and were removed";

/// The session as an old build saved it: a Vox track with a kept Compressor, a Chorus and a
/// Maximizer, a mapping, a lane and a rack macro on the Chorus (and a mapping, a lane and a macro
/// on the Compressor, which must stay), and a Gtr track with a Phaser.
fn old_session() -> String {
    let mut s = Session::default();
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vox"));
    let gtr = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    let host = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Rack"));
    let mut comp = Insert::new("compressor");
    comp.params.insert("ratio".into(), 8.0);
    {
        let t = s.track_mut(vox).unwrap();
        t.mixer.inserts[0] = Some(comp);
        t.mixer.inserts[1] = Some(Insert::new("chorus"));
        t.mixer.inserts[2] = Some(Insert::new("maximizer"));
        t.mixer.volume_db = -4.5;
        t.lane_mut(&AutoParam::Plugin { slot: 1, param: "rate".into() }).set_point(0, 1.0);
        t.lane_mut(&AutoParam::Plugin { slot: 0, param: "ratio".into() }).set_point(0, 1.0);
        t.lane_mut(&AutoParam::Volume).set_point(0, -3.0);
    }
    s.track_mut(gtr).unwrap().mixer.inserts[4] = Some(Insert::new("phaser"));
    let map = |t, slot: u8, param: &str, cc| {
        let source = MidiSource { port: "kb".into(), channel: Some(0), kind: SourceKind::Cc, number: cc };
        Mapping::new(source, ControlTarget { strip: Strip::Track(t), control: Control::Plugin { slot, param: param.into() } })
    };
    s.midi_mappings.set(map(vox, 1, "rate", 1)).unwrap();
    s.midi_mappings.set(map(vox, 0, "ratio", 2)).unwrap();
    let mut rack = Rack::default();
    for slot in [1, 0] {
        rack.macros[0].mappings.push(MacroMapping { track: vox, slot, param: "p".into(), ..MacroMapping::default() });
    }
    s.track_mut(host).unwrap().rack = Some(rack);
    s.to_json().unwrap()
}

fn open(text: &str, tag: &str) -> (Engine, PathBuf) {
    let dir = transit_test_support::TestDir::new(&format!("retired-effects-{tag}"));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Old.transit");
    std::fs::write(&path, text).unwrap();
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    (e, path)
}

fn plugins(e: &Engine, track: &str) -> Vec<Option<String>> {
    e.session().track_by_name(track).unwrap().mixer.inserts.iter().map(|i| i.as_ref().map(|i| i.plugin.clone())).collect()
}

#[test]
fn a_session_with_retired_inserts_opens_emptied_reported_counted_and_modified() {
    let (mut e, path) = open(&old_session(), "load");
    assert_eq!(plugins(&e, "Vox")[..3], [Some("compressor".into()), None, None]);
    assert_eq!(plugins(&e, "Gtr")[4], None);

    let lines: Vec<&str> = e.messages.iter().map(String::as_str).collect();
    assert_eq!(
        lines,
        [
            "track \"Vox\", insert B: the retired Chorus was removed; dropped 1 automation lane(s), 1 MIDI mapping(s), 1 rack macro target(s)",
            "track \"Vox\", insert C: the retired Maximizer was removed",
            "track \"Gtr\", insert E: the retired Phaser was removed",
            STATUS,
        ]
    );
    assert!(e.is_dirty(), "the removal shows as a modified session");

    // Nothing else on the track changed: the Compressor and its lane, mapping and macro target, the fader and the other lane.
    let vox = e.session().track_by_name("Vox").unwrap();
    let comp = vox.mixer.inserts[0].as_ref().unwrap();
    assert_eq!((comp.plugin.as_str(), comp.params.get("ratio")), ("compressor", Some(&8.0)));
    assert_eq!(vox.mixer.volume_db, -4.5);
    assert!(vox.lane(&AutoParam::Plugin { slot: 0, param: "ratio".into() }).is_some());
    assert!(vox.lane(&AutoParam::Plugin { slot: 1, param: "rate".into() }).is_none());
    assert!(vox.lane(&AutoParam::Volume).is_some());
    assert_eq!(e.session().midi_mappings.len(), 1);
    let macros = &e.session().track_by_name("Rack").unwrap().rack.as_ref().unwrap().macros[0].mappings;
    assert_eq!(macros.iter().map(|m| m.slot).collect::<Vec<_>>(), [0]);

    // It plays, and saved it names no retired device and opens clean and unmodified.
    let out = transit_mix::render_range(e.session(), Range::new(0, 4800), 512);
    assert!(out.iter().flatten().all(|x| x.is_finite()));
    e.execute("session.save", &json!({})).unwrap();
    assert!(!e.is_dirty());
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(!saved.contains("\"chorus\"") && !saved.contains("\"maximizer\"") && !saved.contains("\"phaser\""));
    let mut again = Engine::default();
    again.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert!(again.messages.iter().all(|m| !m.contains("retired")), "{:?}", again.messages);
    assert!(!again.is_dirty());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn opening_from_bytes_removes_and_reads_as_modified_too() {
    let mut e = Engine::default();
    transit_engine::io::open_session_bytes(&mut e, "Old.transit", old_session().as_bytes()).unwrap();
    assert_eq!(plugins(&e, "Vox")[..3], [Some("compressor".into()), None, None]);
    assert!(e.messages.iter().any(|m| m == STATUS), "{:?}", e.messages);
    assert!(e.is_dirty());
}

#[test]
fn a_retired_device_cannot_be_added() {
    let mut e = Engine::default();
    let a = e.execute("track.new", &json!({"kind": "audio", "name": "A"})).unwrap()["tracks"][0].as_u64().unwrap();
    for id in ["chorus", "maximizer", "pitch_shifter", "time_shift", "lofi"] {
        assert!(e.execute("mix.insert", &json!({"track": a, "plugin": id})).is_err(), "{id}");
    }
    let i = e.execute("track.new", &json!({"kind": "instrument", "name": "I"})).unwrap()["tracks"][0].as_u64().unwrap();
    assert!(e.execute("mix.instrument", &json!({"track": i, "plugin": "drum_synth"})).is_err());
}
