//! The retired one-sample Sampler on the engine's ways in (#455, ADR 0041 amendment 2026-11-11):
//! an opened session, an import of its tracks and a track preset each give it an empty kit and say
//! so; its sample file and pool entry stay; a source held off its stored rate cannot feed a zone.

#![allow(clippy::unwrap_used)]
use crate::Engine;
use serde_json::json;
use std::path::{Path, PathBuf};
use transit_model::legacy_sampler::{RETIRED_PARAMS, SAMPLER_ID, is_one_sample};
use transit_model::{AutoParam, ChannelFormat, Insert, Session, Source, SourceId, TrackKind};

const LINE: &str = "track \"Keys\": the old one-sample Sampler was removed and is now an empty kit; its sample \"hit.wav\" stays in \
                    the session's files and can be loaded onto a pad again; dropped 1 automation lane(s)";
const SUMMARY: &str = "1 track(s) had the old one-sample Sampler and now have an empty Sampler kit";

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("transit-one-sample-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Audio Files")).unwrap();
    dir
}

fn wav(path: &Path, rate: u32) {
    let buf = transit_audio_io::AudioBuffer { sample_rate: rate, channels: vec![(0..4_410).map(|i| ((i % 100) as f32 / 100.0) - 0.5).collect()] };
    std::fs::write(path, transit_audio_io::encode(&buf, &transit_audio_io::EncodeOptions::default()).unwrap()).unwrap();
}

/// An old session as an earlier build saved it, at 48 kHz: track "Keys" with the one-sample
/// Sampler playing `Audio Files/hit.wav` (44.1 kHz, its stored rate), nondefault values for the 19
/// ids and an automation lane on one. Returns the session file and the source id.
fn old_session(dir: &Path) -> (String, SourceId) {
    wav(&dir.join("Audio Files").join("hit.wav"), 44_100);
    let mut s = Session::default();
    let sid = SourceId(s.alloc());
    s.sources.push(Source {
        id: sid,
        name: "hit".into(),
        path: "Audio Files/hit.wav".into(),
        channels: 1,
        frames: 4_410,
        sample_rate: 44_100,
        format: transit_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
        disk: false,
    });
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let mut ins = Insert::new(SAMPLER_ID);
    for (n, id) in RETIRED_PARAMS.iter().enumerate() {
        ins.params.insert((*id).into(), 2.0 + n as f32);
    }
    ins.sample = Some(sid);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(ins);
    tr.lane_mut(&AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "attack".into() }).set_point(0, 0.5);
    let path = dir.join("old.transit").to_string_lossy().into_owned();
    std::fs::write(&path, s.to_json().unwrap()).unwrap();
    (path, sid)
}

fn kit_on(e: &Engine, name: &str) -> Insert {
    e.session().track_by_name(name).and_then(|t| t.instrument.clone()).unwrap()
}

fn is_empty_kit(ins: &Insert) -> bool {
    ins.sampler_patch.as_ref().and_then(transit_model::SamplerPatch::ready).is_some_and(|p| *p == Default::default())
        && ins.sample.is_none()
        && ins.params.is_empty()
}

#[test]
fn an_opened_old_session_keeps_the_file_and_the_pool_entry_and_reads_modified() {
    let dir = scratch("open");
    let (path, sid) = old_session(&dir);
    let file = dir.join("Audio Files").join("hit.wav");
    let bytes = std::fs::read(&file).unwrap();
    let mut e = Engine::default();
    crate::io::open_session(&mut e, &path).unwrap();
    assert!(is_empty_kit(&kit_on(&e, "Keys")));
    assert!(e.messages.iter().any(|m| m == LINE), "{:?}", e.messages);
    assert!(e.messages.iter().any(|m| m == SUMMARY), "{:?}", e.messages);
    assert!(e.dirty, "the open reads modified");
    assert!(is_one_sample(e.saved.track_by_name("Keys").unwrap().instrument.as_ref().unwrap()), "the saved state is the file's");
    let src = e.session().source(sid).expect("the source stays");
    assert_eq!((src.sample_rate, src.frames), (44_100, 4_410), "the source still describes the file");
    let audio = e.session().pool.get(sid).expect("its audio is in the pool");
    assert_eq!(audio.buffer.sample_rate, 48_000, "decoded at the session rate, like timeline audio");
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "the file is untouched by the open");

    crate::io::save_session(&mut e, &path).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "the file is untouched by the save");
    let text = std::fs::read_to_string(&path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert!(v["needs"].as_array().unwrap().contains(&json!("sampler_patch")));
    assert!(v["sources"].as_array().unwrap().iter().any(|s| s["path"] == "Audio Files/hit.wav"), "the saved session still lists the file");

    let mut again = Engine::default();
    crate::io::open_session(&mut again, &path).unwrap();
    assert!(!again.messages.iter().any(|m| m.contains("one-sample")), "{:?}", again.messages);
    assert!(!again.dirty);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_held_off_its_stored_rate_cannot_feed_a_zone() {
    let dir = scratch("rate");
    let (path, sid) = old_session(&dir);
    let mut e = Engine::default();
    crate::io::open_session(&mut e, &path).unwrap();
    let before = e.session_arc();
    let err = e.execute("mix.sampler_zone_source", &json!({"track": "Keys", "zone": 1, "source": sid.0})).unwrap_err().to_string();
    assert!(err.contains("load it from its file") && err.contains("44100") && err.contains("48000"), "{err}");
    assert_eq!(e.session(), before.as_ref(), "refused whole");
    // The same file loaded from its path imports at its own rate as a new source, which a zone takes,
    // and so does a session source whose rates match.
    let r =
        e.execute("mix.sampler_zone_source", &json!({"track": "Keys", "zone": 1, "path": dir.join("Audio Files").join("hit.wav").to_string_lossy()}));
    let fresh = SourceId(r.unwrap()["source"].as_u64().unwrap());
    assert_ne!(fresh, sid);
    let src = e.session().source(fresh).unwrap();
    assert_eq!((src.sample_rate, e.session().pool.get(fresh).unwrap().buffer.sample_rate), (44_100, 44_100));
    e.execute("mix.sampler_zone_source", &json!({"track": "Keys", "zone": 2, "source": fresh.0})).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn importing_an_old_track_gives_it_an_empty_kit_and_posts_the_donors_line() {
    let dir = scratch("import");
    let (path, _) = old_session(&dir);
    let mut e = crate::demo::demo_engine();
    let sources = e.session().sources.len();
    e.messages.clear();
    e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Keys"]})).unwrap();
    assert!(e.messages.iter().any(|m| m == LINE), "the donor's line, under the donor's name: {:?}", e.messages);
    assert!(e.messages.iter().any(|m| m == SUMMARY), "{:?}", e.messages);
    let keys = e.session().tracks.iter().rev().find(|t| t.name.starts_with("Keys")).unwrap();
    assert!(is_empty_kit(keys.instrument.as_ref().unwrap()), "the demo's Keys keeps its name; the import is renamed");
    assert!(keys.automation.is_empty(), "the lane on the old id did not come along");
    assert_eq!(e.session().sources.len(), sources, "the sample file does not come along");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_track_preset_with_the_old_sampler_loads_as_an_empty_kit_and_keeps_the_targets_lanes() {
    let dir = scratch("preset");
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 2, "kind": "instrument"})).unwrap();
    let (from, to) = (e.session().tracks[0].id, e.session().tracks[1].id);
    let mut old = Insert::new(SAMPLER_ID);
    old.params.insert("root_note".into(), 48.0);
    old.sample = Some(SourceId(1));
    old.preset = "Plucky".into();
    let s = e.session_mut();
    s.track_mut(from).unwrap().instrument = Some(old);
    s.track_mut(to).unwrap().name = "Target".into();
    s.track_mut(to).unwrap().lane_mut(&AutoParam::Plugin { slot: AutoParam::INSTRUMENT_SLOT, param: "level".into() }).set_point(0, 0.5);
    e.execute("edit.select", &json!({"tracks": [from.0]})).unwrap();
    let saved = e.execute("track.save_preset", &json!({"path": dir.join("old").to_string_lossy()})).unwrap();
    e.execute("edit.select", &json!({"tracks": [to.0]})).unwrap();
    e.messages.clear();
    e.execute("track.load_preset", &json!({"path": saved["path"]})).unwrap();
    let ins = e.session().track(to).unwrap().instrument.clone().unwrap();
    assert!(is_empty_kit(&ins), "{ins:?}");
    assert_eq!(ins.preset, "<factory default>", "the preset label is reset");
    assert_eq!(e.messages, vec!["track \"Target\": the old one-sample Sampler was removed and is now an empty kit".to_string()]);
    assert_eq!(e.session().track(to).unwrap().automation.len(), 1, "the target track's lanes stay, as for any preset");
    let _ = std::fs::remove_dir_all(&dir);
}
