//! Markers from foreign sessions before the export origin (#701).

use serde_json::{Value, json};
use std::io::Read as _;
use transit_engine::Engine;
use transit_model::{ChannelFormat, Clip, MarkerKind, Session, TrackKind};

const BAR: i64 = 96_000;
const NOTICE: &str = "2 markers before the export start were moved to the start.";

fn session(zone: bool, early: bool) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
    let start = if zone { -BAR / 2 } else { 0 };
    let c = Clip::midi(s.new_clip_id(), "Notes", start, BAR, transit_midi::Sequence::default());
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
    let origin = if zone { -BAR } else { 0 };
    for (name, at) in [("Early", origin - BAR), ("Just early", origin - 1), ("At start", origin), ("Later", BAR)] {
        // Set directly: ordinary marker placement does not allow this foreign-file case.
        let id = s.add_marker(name, MarkerKind::Marker, 0, 0);
        let m = s.markers.iter_mut().find(|m| m.id == id).unwrap();
        m.start = if early { at } else { at.max(origin) };
        m.end = m.start;
    }
    s
}

fn midi_markers(bytes: &[u8]) {
    let smf = transit_midi::read_smf(bytes).unwrap();
    assert_eq!(smf.markers.len(), 4);
    assert!(smf.markers.iter().all(|(tick, _)| *tick >= 0));
    for name in ["Early", "Just early", "At start"] {
        assert_eq!(smf.markers.iter().find(|(_, n)| n == name).unwrap().0, 0);
    }
}

fn export(e: &mut Engine, command: &str, path: &std::path::Path) -> Value {
    e.execute(command, &json!({"path": path, "dir": path, "dither": false})).unwrap()
}

#[test]
fn midi_export_clamps_early_markers_and_reports_the_count() {
    let dir = transit_test_support::TestDir::created("early-marker-midi");
    for zone in [false, true] {
        let path = dir.join(format!("{zone}.mid"));
        let reply = export(&mut Engine::new(session(zone, true)), "file.export_midi", &path);
        let bytes = std::fs::read(&path).unwrap();
        midi_markers(&bytes);
        assert_eq!(reply["notice"], NOTICE);
        let reply = export(&mut Engine::new(session(zone, false)), "file.export_midi", &path);
        assert!(reply.get("notice").is_none());
        assert_eq!(bytes, std::fs::read(path).unwrap());
    }
}

#[test]
fn valid_midi_export_bytes_match_the_original_conductor() {
    let s = session(false, false);
    let mut original = transit_midi::Smf {
        format: 1,
        tracks: Vec::new(),
        tempos: s.tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect(),
        meters: vec![(0, 4, 4)],
        markers: s.markers.iter().map(|m| (s.tempo.samples_to_ticks(m.start, s.sample_rate), m.name.clone())).collect(),
        key_sigs: Vec::new(),
    };
    original.tracks.push(transit_midi::SmfTrack { name: "Keys".into(), sequence: Default::default() });
    assert_eq!(transit_engine::io::export_midi(&Engine::new(s), None).unwrap(), transit_midi::write_smf(&original).unwrap());
}

#[test]
fn consolidated_export_clamps_early_markers_and_reports_the_count() {
    let dir = transit_test_support::TestDir::created("early-marker-consolidated");
    for zone in [false, true] {
        let a = export(&mut Engine::new(session(zone, true)), "file.bounce_consolidated", &dir.join(format!("early-{zone}")));
        let b = export(&mut Engine::new(session(zone, false)), "file.bounce_consolidated", &dir.join(format!("valid-{zone}")));
        for key in ["tempo_file", "midi_files"] {
            let path = |r: &Value| if key == "tempo_file" { r[key].as_str().unwrap().to_string() } else { r[key][0].as_str().unwrap().to_string() };
            let bytes = std::fs::read(path(&a)).unwrap();
            midi_markers(&bytes);
            assert_eq!(bytes, std::fs::read(path(&b)).unwrap());
        }
        assert_eq!(a["notice"], NOTICE);
        assert!(b.get("notice").is_none());
    }
}

#[test]
fn dawproject_export_clamps_early_markers_and_reports_the_count() {
    let dir = transit_test_support::TestDir::created("early-marker-project");
    for zone in [false, true] {
        let path = dir.join(format!("early-{zone}.dawproject"));
        let reply = export(&mut Engine::new(session(zone, true)), "file.export_dawproject", &path);
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut xml = String::new();
        archive.by_name("project.xml").unwrap().read_to_string(&mut xml).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let times: Vec<f64> = doc.descendants().filter(|n| n.has_tag_name("Marker")).map(|n| n.attribute("time").unwrap().parse().unwrap()).collect();
        assert_eq!(times.len(), 4);
        assert!(times.iter().all(|t| *t >= 0.0), "{times:?}");
        assert_eq!(&times[..3], &[0.0; 3]);
        assert_eq!(reply["notice"], NOTICE);
        let valid = dir.join(format!("valid-{zone}.dawproject"));
        let reply = export(&mut Engine::new(session(zone, false)), "file.export_dawproject", &valid);
        assert!(reply.get("notice").is_none());
        assert_eq!(std::fs::read(path).unwrap(), std::fs::read(valid).unwrap());
    }
}
