//! Track presets with a Sampler kit (#458) over scratch temp folders and generated audio files: no
//! devices, no clock outcome, no pinned hash.

use super::*;
use std::path::PathBuf;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use transit_model::sampler_patch::{PatchV1, SamplerPatch};

struct Dir(transit_test_support::TestDir);

impl Dir {
    fn new() -> Self {
        Self(transit_test_support::TestDir::created("tpreset"))
    }

    fn audio(&self, name: &str, frames: usize, seed: usize) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let samples = (0..frames).map(|i| (((i + seed).wrapping_mul(1664525).wrapping_add(1013904223) >> 16) & 255) as f32 / 256.0 - 0.5).collect();
        let audio = AudioBuffer { sample_rate: 48_000, channels: vec![samples] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, ..EncodeOptions::default() };
        std::fs::write(&path, transit_audio_io::encode(&audio, &opts).unwrap()).unwrap();
        path
    }
}

/// An engine with one instrument track holding a new Sampler kit whose pad 1 plays `sample`.
fn session_with(sample: &std::path::Path) -> (Engine, TrackId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": sample})).unwrap();
    (e, t)
}

fn kit(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().and_then(SamplerPatch::ready).unwrap().clone()
}

/// The stored path of the source pad 1's zone plays on track `t`.
fn pad1_path(e: &Engine, t: TrackId) -> String {
    let p = kit(e, t);
    let z = p.zone(p.pad(0).zone.unwrap()).unwrap();
    e.session().source(p.sources[usize::from(z.source)].source).unwrap().path.clone()
}

fn save(e: &mut Engine, t: TrackId, path: &std::path::Path) -> Value {
    e.execute("edit.select", &json!({"tracks": [t.0]})).unwrap();
    e.execute("track.save_preset", &json!({"path": path.to_string_lossy()})).unwrap()
}

fn load(e: &mut Engine, t: TrackId, path: &str) -> Result<Value> {
    e.execute("edit.select", &json!({"tracks": [t.0]})).unwrap();
    e.execute("track.load_preset", &json!({"path": path}))
}

#[test]
fn a_track_preset_loaded_into_another_session_plays_its_own_samples() {
    let d = Dir::new();
    let kick = d.audio("Kit/Kick.wav", 480, 1);
    let snare = d.audio("Other/Snare.wav", 960, 2);
    let (mut a, t) = session_with(&kick);
    // Session b's own sample takes the same session id the kick has in a.
    let (mut b, u) = session_with(&snare);
    assert_eq!(kit(&a, t).sources[0].source, kit(&b, u).sources[0].source, "the ids collide");
    let saved = save(&mut a, t, &d.0.join("Kit").join("Drums"));
    let text = std::fs::read_to_string(saved["path"].as_str().unwrap()).unwrap();
    let file: Value = serde_json::from_str(&text).unwrap();
    let sources = file["instrument"]["sampler_patch"]["sources"].as_array().unwrap();
    assert!(sources.iter().all(|s| s.get("source").is_none()), "no session id in the preset: {text}");
    assert_eq!(sources[0]["file"]["path"], "Kick.wav", "relative to the preset's folder: {text}");
    b.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let w = b.session().tracks.last().unwrap().id;
    let steps = b.undo_history().len();
    let out = load(&mut b, w, saved["path"].as_str().unwrap()).unwrap();
    assert!(out.get("notice").is_none(), "every sample found: {out}");
    assert!(portable_same(&pad1_path(&b, w), &kick), "the preset's own kick, not session b's snare: {}", pad1_path(&b, w));
    assert_eq!(pad1_path(&b, u), snare.to_string_lossy(), "session b's own kit is untouched");
    assert_eq!(b.undo_history().len(), steps + 1, "one undo step");
    assert!(b.undo());
    assert!(b.session().track(w).unwrap().instrument.as_ref().is_none_or(|i| i.sampler_patch.is_none()), "undo takes the kit back off");
}

fn portable_same(a: &str, b: &std::path::Path) -> bool {
    transit_model::sampler_portable::same_path(a, &transit_model::sampler_portable::strip_verbatim(&b.to_string_lossy()))
}

#[test]
fn a_preset_saved_by_session_id_loads_its_zones_missing_and_says_so() {
    let d = Dir::new();
    let snare = d.audio("Snare.wav", 960, 2);
    let (mut b, u) = session_with(&snare);
    let sid = kit(&b, u).sources[0].source;
    // A preset as builds before the portable form wrote it: the saving session's ids.
    let mut patch = serde_json::to_value(b.session().track(u).unwrap().instrument.as_ref().unwrap()).unwrap();
    patch["preset"] = json!("Old");
    let preset = json!({"format": "transit-track-preset", "version": 1, "name": "Bass", "kind": "instrument", "mixer": b.session().track(u).unwrap().mixer, "instrument": patch});
    let path = d.0.join("Bass.tpreset");
    std::fs::write(&path, serde_json::to_string(&preset).unwrap()).unwrap();
    b.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let w = b.session().tracks.last().unwrap().id;
    b.messages.clear();
    let out = load(&mut b, w, path.to_string_lossy().as_ref()).unwrap();
    let line = "track preset \"Bass\": its Sampler samples were saved by session id and cannot be found: load them again.";
    assert_eq!(out["notice"], line);
    assert_eq!(b.messages, vec![line.to_string()]);
    let got = kit(&b, w).sources[0].source;
    assert_ne!(got, sid, "never the loading session's source of that id");
    let src = b.session().source(got).unwrap();
    assert_eq!((src.name.as_str(), src.path.as_str()), (format!("source {}", sid.0).as_str(), ""));
    assert!(!b.session().pool.contains(got), "missing: no audio");
}

#[test]
fn a_missing_sample_loads_in_the_missing_state_named_in_the_status_line() {
    let d = Dir::new();
    let kick = d.audio("Kick.wav", 480, 1);
    let (mut a, t) = session_with(&kick);
    let saved = save(&mut a, t, &d.0.join("Drums"));
    std::fs::remove_file(&kick).unwrap();
    let (mut b, _) = session_with(&d.audio("Elsewhere/Snare.wav", 960, 2));
    b.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let w = b.session().tracks.last().unwrap().id;
    let out = load(&mut b, w, saved["path"].as_str().unwrap()).unwrap();
    let notice = out["notice"].as_str().unwrap();
    assert!(notice.starts_with("track preset \"Drums\": 1 sample was not found and plays silence: Kick.wav ("), "{notice}");
    let src = kit(&b, w).sources[0].source;
    assert!(!b.session().pool.contains(src), "missing");
}

#[test]
fn a_kit_this_version_does_not_read_refuses_the_whole_load() {
    let d = Dir::new();
    let kick = d.audio("Kick.wav", 480, 1);
    let (mut a, t) = session_with(&kick);
    let saved = save(&mut a, t, &d.0.join("Drums"));
    let path = saved["path"].as_str().unwrap().to_string();
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["instrument"]["sampler_patch"]["schema"] = json!(2);
    v["mixer"]["volume_db"] = json!(-12.0);
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    let (mut b, u) = session_with(&d.audio("Snare.wav", 960, 2));
    let before = serde_json::to_value(b.session()).unwrap();
    let err = load(&mut b, u, &path).unwrap_err().to_string();
    assert!(err.contains("saved by a newer transit (Sampler kit schema 2); nothing was loaded"), "{err}");
    assert_eq!(serde_json::to_value(b.session()).unwrap(), before, "nothing applied, the mixer included");
}

#[test]
fn a_sample_with_no_file_refuses_the_save() {
    let d = Dir::new();
    let (mut e, t) = session_with(&d.audio("Kick.wav", 480, 1));
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 64]] };
    let take = crate::io::add_source(e.session_mut(), "Take", buf, None, FileFormat::Wav);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 2, "source": take})).unwrap();
    e.execute("edit.select", &json!({"tracks": [t.0]})).unwrap();
    let path = d.0.join("Drums");
    let err = e.execute("track.save_preset", &json!({"path": path.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("has no file yet"), "{err}");
    assert!(!d.0.join("Drums.tpreset").exists(), "nothing written");
}

/// #487 with #458: a dotted preset name keeps its dots, the samples are stored relative to that
/// file's folder, and the preset loads into another session with its samples found.
#[test]
fn a_dotted_preset_name_round_trips_with_its_samples() {
    let d = Dir::new();
    let kick = d.audio("Kit/Kick.wav", 480, 1);
    let (mut a, t) = session_with(&kick);
    let saved = save(&mut a, t, &d.0.join("Kit").join("My.Kit v2"));
    let written = d.0.join("Kit").join("My.Kit v2.tpreset");
    assert_eq!(saved["path"], written.to_string_lossy().as_ref(), "the whole name, then .tpreset");
    assert!(!d.0.join("Kit").join("My.tpreset").exists(), "the name never loses its tail");
    let file: Value = serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
    assert_eq!(file["instrument"]["sampler_patch"]["sources"][0]["file"]["path"], "Kick.wav", "relative to the preset's folder");
    let (mut b, _) = session_with(&d.audio("Other/Snare.wav", 960, 2));
    b.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let w = b.session().tracks.last().unwrap().id;
    let out = load(&mut b, w, written.to_string_lossy().as_ref()).unwrap();
    assert!(out.get("notice").is_none(), "every sample found: {out}");
    assert!(portable_same(&pad1_path(&b, w), &kick), "the preset's own kick: {}", pad1_path(&b, w));
}

/// #487 with #458: a save under a dotted name is refused for a sample with no file yet, and the
/// preset already saved under that name is left as it was (no file under a cut name either).
#[test]
fn a_refused_save_under_a_dotted_name_leaves_the_earlier_preset() {
    let d = Dir::new();
    let (mut e, t) = session_with(&d.audio("Kick.wav", 480, 1));
    let saved = save(&mut e, t, &d.0.join("Drums v1.5"));
    let written = d.0.join("Drums v1.5.tpreset");
    assert_eq!(saved["path"], written.to_string_lossy().as_ref());
    let before = std::fs::read_to_string(&written).unwrap();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 64]] };
    let take = crate::io::add_source(e.session_mut(), "Take", buf, None, FileFormat::Wav);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 2, "source": take})).unwrap();
    e.execute("edit.select", &json!({"tracks": [t.0]})).unwrap();
    let err = e.execute("track.save_preset", &json!({"path": d.0.join("Drums v1.5").to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("has no file yet"), "{err}");
    assert_eq!(std::fs::read_to_string(&written).unwrap(), before, "the earlier preset is untouched");
    assert!(!d.0.join("Drums v1.tpreset").exists(), "nothing written under a cut name");
}

#[test]
fn a_preset_with_the_retired_subtractive_synth_gets_a_macro_oscillator_as_a_session_load_does() {
    let d = Dir::new();
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    let mut old = transit_model::Insert::new(transit_model::legacy_synth::RETIRED_SYNTH_ID);
    old.params.insert("cutoff".into(), 300.0);
    let preset = json!({"format": "transit-track-preset", "version": 1, "name": "Lead", "kind": "instrument", "mixer": e.session().track(t).unwrap().mixer, "instrument": old});
    let path = d.0.join("Lead.tpreset");
    std::fs::write(&path, serde_json::to_string(&preset).unwrap()).unwrap();
    e.messages.clear();
    load(&mut e, t, path.to_string_lossy().as_ref()).unwrap();
    let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
    assert_eq!(ins.plugin, transit_model::legacy_synth::DEFAULT_INSTRUMENT);
    assert_eq!(ins.params.get("level"), Some(&transit_model::legacy_synth::SUBSTITUTE_LEVEL_DB), "the session load's substitute");
    assert!(!ins.params.contains_key("cutoff"));
    assert_eq!(e.messages, vec![format!("1 track(s) {}", transit_model::legacy_synth::RETIRED_SYNTH_NOTE)]);
}

#[test]
fn an_inserts_sampler_patch_never_travels_in_a_preset() {
    let d = Dir::new();
    let kick = d.audio("Kick.wav", 480, 1);
    let (mut e, t) = session_with(&kick);
    let patch = e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.clone();
    let mut ins = transit_model::Insert::new("sampler");
    ins.sampler_patch = patch;
    e.session_mut().track_mut(t).unwrap().mixer.inserts[0] = Some(ins);
    let saved = save(&mut e, t, &d.0.join("Drums"));
    let text = std::fs::read_to_string(saved["path"].as_str().unwrap()).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    assert!(v["mixer"]["inserts"][0].get("sampler_patch").is_none(), "{text}");
}
