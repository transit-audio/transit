//! Kit presets (#467) over scratch temp folders and generated audio files: no devices, no clock
//! outcome, no pinned hash.

use super::*;
use std::path::Path;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use transit_model::sampler_patch::ZoneEntry;
use transit_model::{ChannelFormat, Clip, Frozen, TrackId, TrackKind};

pub(super) struct Dir(pub(super) PathBuf);

impl Dir {
    pub(super) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // The stamp only names fresh storage; no test outcome depends on elapsed time.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("transit-tkit-{}-{id}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub(super) fn audio(&self, name: &str, frames: usize, rate: u32) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let format = if name.ends_with(".flac") { FileFormat::Flac } else { FileFormat::Wav };
        let samples = (0..frames).map(|i| ((i.wrapping_mul(1664525).wrapping_add(1013904223) >> 16) & 255) as f32 / 256.0 - 0.5).collect();
        let audio = AudioBuffer { sample_rate: rate, channels: vec![samples] };
        let opts = EncodeOptions { format, bit_depth: BitDepth::Int16, ..EncodeOptions::default() };
        std::fs::write(&path, transit_audio_io::encode(&audio, &opts).unwrap()).unwrap();
        path
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(super) fn setup() -> (Engine, TrackId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    (e, t)
}

pub(super) fn kit(e: &Engine, t: TrackId) -> PatchV1 {
    patch_of(e.session(), LOAD, t).unwrap()
}

pub(super) fn save_kit(e: &mut Engine, t: TrackId, path: &Path) -> Value {
    e.execute(SAVE, &json!({"track": t, "path": path})).unwrap()
}

pub(super) fn recall(e: &mut Engine, t: TrackId, path: &Path, replace: bool) -> Value {
    e.execute(LOAD, &json!({"track": t, "path": path, "replace": replace})).unwrap()
}

/// `p` with every source id cleared and every source referenced: what a recall keeps.
fn normalized(p: &PatchV1) -> PatchV1 {
    let mut p = p.clone();
    for r in &mut p.sources {
        r.source = SourceId(0);
        r.ownership = Ownership::Referenced;
    }
    p
}

/// A kit over `d`'s Kick.wav, Snare.wav and Hat.flac: two pads (a name, a choke group, mute), a
/// key-mapped zone, a control, a seed and a pad start note away from the defaults.
fn build(e: &mut Engine, t: TrackId, d: &Dir) {
    let kick = d.audio("Kick.wav", 128, 48_000);
    let snare = d.audio("Snare.wav", 256, 44_100);
    let hat = d.audio("Hat.flac", 96, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": kick})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 2, "path": snare})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 3, "path": hat, "playback": {"keys": [70, 80], "root": 75}})).unwrap();
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 2, "name": "Snare", "choke": 3, "mute": true})).unwrap();
    e.execute("mix.sampler_control", &json!({"track": t, "id": "zone001_level", "value": -6.0})).unwrap();
    e.execute("mix.sampler_seed", &json!({"track": t, "seed": 7})).unwrap();
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 40})).unwrap();
}

#[test]
fn save_then_recall_into_an_empty_sampler_equals_the_original_kit() {
    let d = Dir::new();
    let (mut a, t) = setup();
    build(&mut a, t, &d);
    let file = d.0.join("808");
    let saved = save_kit(&mut a, t, &file);
    assert_eq!(saved["path"], d.0.join("808.tkit").to_string_lossy().as_ref());
    assert_eq!((saved["pads"].as_u64(), saved["zones"].as_u64(), saved["sources"].as_u64()), (Some(2), Some(3), Some(3)));
    assert_eq!((saved["relative"].as_u64(), saved["absolute"].as_u64()), (Some(3), Some(0)), "{saved}");
    let text = std::fs::read_to_string(d.0.join("808.tkit")).unwrap();
    assert!(text.contains("\"format\": \"transit-sampler-kit\""), "pretty printed: {text}");
    let file: Value = serde_json::from_str(&text).unwrap();
    assert!(file["kit"]["sources"].as_array().unwrap().iter().all(|s| s.get("source").is_none() && s["file"]["path"].is_string()), "{text}");
    let (mut b, u) = setup();
    let steps = b.undo_history().len();
    let out = recall(&mut b, u, &d.0.join("808.tkit"), false);
    assert_eq!(out["missing"], json!([]), "{out}");
    // A new Sampler's pad start note is 36; the kit's is 40, and the notice says so.
    assert_eq!(
        out["notice"],
        "Kit \"808\" loaded: 2 pads, 3 zones. Pad start note E1 (up 4 semitones): transpose pad clips by +4 to keep them on the same pads."
    );
    assert_eq!(normalized(&kit(&b, u)), normalized(&kit(&a, t)));
    assert_eq!(b.undo_history().len(), steps + 1);
    assert_eq!(b.undo_label(), Some("Load Sampler Kit"));
    assert_eq!(b.session().track(u).unwrap().instrument.as_ref().unwrap().preset, "808");
    // Native audio, as the original holds it.
    for (x, y) in kit(&a, t).sources.iter().zip(&kit(&b, u).sources) {
        assert_eq!(a.session().pool.get(x.source).unwrap().buffer.channels, b.session().pool.get(y.source).unwrap().buffer.channels);
        assert_eq!(b.session().source(y.source).unwrap().sample_rate, a.session().source(x.source).unwrap().sample_rate);
    }
}

#[test]
fn a_loaded_kit_is_replaced_only_with_replace_in_one_undo_step() {
    let d = Dir::new();
    let other = Dir::new();
    let (mut e, t) = setup();
    build(&mut e, t, &d);
    save_kit(&mut e, t, &d.0.join("808"));
    e.execute("mix.sampler_zone_clear", &json!({"track": t, "zone": 3})).unwrap();
    let ride = other.audio("Ride.wav", 64, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 9, "pad": 5, "path": ride})).unwrap();
    let before = e.session_arc();
    let steps = e.undo_history().len();
    let asked = recall(&mut e, t, &d.0.join("808.tkit"), false);
    assert_eq!(asked, json!({"track": t, "kit": "808", "loaded": 0, "replace_needed": true, "pads": [1, 2, 5], "zones": [1, 2, 9]}));
    assert!(Arc::ptr_eq(&before, &e.session_arc()), "nothing changed");
    assert_eq!(e.undo_history().len(), steps);
    let old_doc = serde_json::to_value(e.session()).unwrap();
    let old_pool: Vec<SourceId> = e.session().pool.ids().collect();
    let out = recall(&mut e, t, &d.0.join("808.tkit"), true);
    assert_eq!(out["zones"], 3, "{out}");
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(kit(&e, t).zone(8).is_none(), "the whole kit is replaced");
    assert!(!e.session().sources.iter().any(|s| s.path.ends_with("Ride.wav")), "the replaced kit's unused source leaves");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(serde_json::to_value(e.session()).unwrap(), old_doc, "the old kit and its sources");
    assert_eq!(e.session().pool.ids().collect::<Vec<_>>(), old_pool, "and its pool audio");
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(kit(&e, t).zones.len(), 3);
}

#[test]
fn a_missing_sample_takes_the_missing_state_and_the_rest_loads() {
    let d = Dir::new();
    let (mut a, t) = setup();
    build(&mut a, t, &d);
    save_kit(&mut a, t, &d.0.join("808"));
    std::fs::remove_file(d.0.join("Snare.wav")).unwrap();
    let (mut b, u) = setup();
    let out = recall(&mut b, u, &d.0.join("808.tkit"), false);
    let looked = d.0.join("Snare.wav").to_string_lossy().into_owned();
    assert_eq!(out["missing"], json!([{"zone": 2, "pad": 2, "path": looked}]), "{out}");
    assert!(
        out["notice"].as_str().unwrap().starts_with(&format!("Kit \"808\": 1 sample was not found and plays silence: Snare.wav ({looked}). ")),
        "{out}"
    );
    let p = kit(&b, u);
    let zone = p.zone(1).unwrap();
    let sid = p.sources[usize::from(zone.source)].source;
    let src = b.session().source(sid).unwrap();
    assert_eq!((src.path.as_str(), src.frames, src.sample_rate, src.channels, src.unsaved), (looked.as_str(), 256, 44_100, 1, false));
    assert!(b.session().pool.get(sid).is_none(), "no audio: the missing state");
    for z in [0u16, 2] {
        let s = p.sources[usize::from(p.zone(z).unwrap().source)].source;
        assert!(b.session().pool.get(s).is_some(), "the rest of the kit loads");
    }
}

#[test]
fn a_changed_or_undecodable_sample_takes_the_missing_state_named_in_the_notice() {
    let d = Dir::new();
    let (mut a, t) = setup();
    build(&mut a, t, &d);
    save_kit(&mut a, t, &d.0.join("808"));
    d.audio("Kick.wav", 200, 48_000); // another frame count, seen in the header
    d.audio("Hat.flac", 96, 44_100); // another rate, seen after decoding
    std::fs::write(d.0.join("Snare.wav"), b"RIFF not audio").unwrap();
    let (mut b, u) = setup();
    let out = recall(&mut b, u, &d.0.join("808.tkit"), false);
    let notice = out["notice"].as_str().unwrap();
    assert!(notice.contains("Kick.wav changed since the kit was saved: 48000 Hz, 200 frames; the kit expects 48000 Hz, 128 frames"), "{notice}");
    assert!(notice.contains("Hat.flac changed since the kit was saved: 44100 Hz"), "{notice}");
    assert!(notice.contains("Snare.wav could not be read"), "{notice}");
    assert_eq!(out["missing"].as_array().unwrap().len(), 3);
    let p = kit(&b, u);
    assert_eq!(p.zone(0).unwrap().end, 128, "the kit's region, never rescaled");
    assert!(p.sources.iter().all(|r| b.session().pool.get(r.source).is_none()), "no new audio decoded into the kit");
    assert_eq!(b.session().source(p.sources[0].source).unwrap().frames, 128, "the kit's recorded facts");
}

#[test]
fn a_relative_sample_resolves_against_the_kit_file_and_follows_a_moved_folder() {
    let d = Dir::new();
    let (mut a, t) = setup();
    let lib = Dir::new();
    let kick = lib.audio("Kit/Kick.wav", 128, 48_000);
    a.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": kick})).unwrap();
    save_kit(&mut a, t, &lib.0.join("Kit").join("Mine"));
    // The whole folder moves: found.
    let moved = d.0.join("Moved");
    std::fs::rename(lib.0.join("Kit"), &moved).unwrap();
    let (mut b, u) = setup();
    // The session's folder holds a same-named sample: never used for a kit's relative path.
    let decoy = Dir::new();
    decoy.audio("Kick.wav", 64, 48_000);
    b.session_mut().folder = Some(decoy.0.clone());
    let out = recall(&mut b, u, &moved.join("Mine.tkit"), false);
    assert_eq!(out["missing"], json!([]), "{out}");
    let p = kit(&b, u);
    assert_eq!(b.session().source(p.sources[0].source).unwrap().path, moved.join("Kick.wav").to_string_lossy().as_ref());
    // The kit file alone moves: its relative sample is missing, named where it looked.
    std::fs::rename(moved.join("Mine.tkit"), d.0.join("Mine.tkit")).unwrap();
    let (mut c, w) = setup();
    c.session_mut().folder = Some(moved.clone());
    let out = recall(&mut c, w, &d.0.join("Mine.tkit"), false);
    assert_eq!(out["missing"][0]["path"], d.0.join("Kick.wav").to_string_lossy().as_ref(), "{out}");
}

#[test]
fn refusals_on_the_budget_and_the_eight_voices_rule_leave_the_old_kit() {
    let d = Dir::new();
    // Sparse PCM files: their headers alone pass 128 MiB; nothing that large is decoded.
    let mut sources = Vec::new();
    let mut zones = Vec::new();
    for i in 0..12u16 {
        let name = format!("{i:02}.wav");
        let header = transit_audio_io::wav_float_header(2, 48_000, 1_440_000).unwrap();
        std::fs::write(d.0.join(&name), &header).unwrap();
        std::fs::OpenOptions::new().write(true).open(d.0.join(&name)).unwrap().set_len(header.len() as u64 + 1_440_000 * 8).unwrap();
        sources.push(
            json!({"file": {"path": name, "name": format!("{i:02}"), "frames": 1_440_000, "channels": 2, "sample_rate": 48_000, "format": "Wav"}}),
        );
        zones.push(json!({"id": i, "source": i, "end": 1_440_000, "keys": [i, i]}));
    }
    let big = d.0.join("Big.tkit");
    std::fs::write(&big, serde_json::to_string(&portable::file(json!({"schema": 1, "sources": sources, "zones": zones}))).unwrap()).unwrap();
    let (mut e, t) = setup();
    let kick = d.audio("Kick.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": kick})).unwrap();
    // Nine voices on every note, set directly: a map loaded over the bound.
    {
        let p = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch.as_mut().unwrap().ready_mut().unwrap();
        for id in 1..9 {
            p.zones.push(ZoneEntry { id, ..p.zones[0].clone() });
        }
    }
    save_kit(&mut e, t, &d.0.join("Nine"));
    let before = e.session_arc();
    let steps = e.undo_history().len();
    let out = recall(&mut e, t, &big, true);
    assert!(out["notice"].as_str().unwrap().contains("128 MiB"), "{out}");
    assert_eq!(out["loaded"], 0);
    assert!(Arc::ptr_eq(&before, &e.session_arc()) && e.undo_history().len() == steps, "nothing changed");
    // The kit's own overloads count: judged against an empty patch, not the kit it replaces.
    let out = recall(&mut e, t, &d.0.join("Nine.tkit"), true);
    assert!(out["notice"].as_str().unwrap().contains("ninth"), "{out}");
    assert!(Arc::ptr_eq(&before, &e.session_arc()) && e.undo_history().len() == steps, "nothing changed, no source added");
}

#[test]
fn a_newer_kit_is_refused_and_never_overwritten() {
    let d = Dir::new();
    let (mut e, t) = setup();
    let kick = d.audio("Kick.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": kick})).unwrap();
    for (name, text, says) in [
        ("Schema.tkit", r#"{"format": "transit-sampler-kit", "version": 1, "kit": {"schema": 2, "zones": []}}"#, "Sampler kit schema 2"),
        ("Version.tkit", r#"{"format": "transit-sampler-kit", "version": 2, "kit": {}}"#, "format version 2"),
    ] {
        let path = d.0.join(name);
        std::fs::write(&path, text).unwrap();
        let before = e.session_arc();
        let out = recall(&mut e, t, &path, true);
        assert!(out["notice"].as_str().unwrap().contains(says), "{out}");
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        let err = e.execute(SAVE, &json!({"track": t, "path": path})).unwrap_err().to_string();
        assert!(err.contains("was saved by a newer transit: Save As under another name"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "byte-identical");
    }
}

#[test]
fn the_old_kits_source_stays_while_a_clip_names_it_and_leaves_when_nothing_does() {
    let d = Dir::new();
    let (mut e, t) = setup();
    let kept = d.audio("Kept.wav", 128, 48_000);
    let gone = d.audio("Gone.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": kept})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "path": gone})).unwrap();
    let p = kit(&e, t);
    let (kept_id, gone_id) = (p.sources[0].source, p.sources[1].source);
    let audio = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let s = e.session_mut();
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, audio, Clip::audio(cid, "Kept", kept_id, 0, 0, 128));
    s.edit.selected_sources = vec![kept_id, gone_id];
    // The replacement kit names neither.
    let other = Dir::new();
    let (mut b, u) = setup();
    let hat = other.audio("Hat.wav", 64, 48_000);
    b.execute("mix.sampler_zone_source", &json!({"track": u, "zone": 1, "path": hat})).unwrap();
    save_kit(&mut b, u, &other.0.join("Hat"));
    let undone = serde_json::to_value(e.session()).unwrap();
    recall(&mut e, t, &other.0.join("Hat.tkit"), true);
    let s = e.session();
    assert!(s.source(kept_id).is_some() && s.pool.get(kept_id).is_some(), "a clip names it");
    assert!(s.source(gone_id).is_none() && s.pool.get(gone_id).is_none(), "nothing names it");
    assert_eq!(s.edit.selected_sources, vec![kept_id]);
    assert!(Path::new(&gone).is_file(), "the file is never touched");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(serde_json::to_value(e.session()).unwrap(), undone);
    assert!(e.session().pool.get(gone_id).is_some(), "undo brings its audio back");
}

#[test]
fn a_referenced_sampler_source_is_reused_only_when_it_matches() {
    let d = Dir::new();
    let (mut e, t) = setup();
    let kick = d.audio("Kick.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": kick})).unwrap();
    save_kit(&mut e, t, &d.0.join("K"));
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let u = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": u})).unwrap();
    let n = e.session().sources.len();
    recall(&mut e, u, &d.0.join("K.tkit"), false);
    assert_eq!(e.session().sources.len(), n, "the same file at the same rate and length: reused");
    assert_eq!(kit(&e, u).sources[0].source, kit(&e, t).sources[0].source);
    // A record that no longer matches the held audio is read as a new source (here: missing).
    let text = std::fs::read_to_string(d.0.join("K.tkit")).unwrap().replace("\"frames\": 128", "\"frames\": 129");
    std::fs::write(d.0.join("K2.tkit"), text).unwrap();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let w = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": w})).unwrap();
    let out = recall(&mut e, w, &d.0.join("K2.tkit"), false);
    assert_eq!(e.session().sources.len(), n + 1, "{out}");
    assert_ne!(kit(&e, w).sources[0].source, kit(&e, t).sources[0].source);
}

#[test]
fn opaque_and_frozen_targets_are_refused() {
    let d = Dir::new();
    let (mut e, t) = setup();
    let kick = d.audio("Kick.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": kick})).unwrap();
    save_kit(&mut e, t, &d.0.join("K"));
    let file = d.0.join("K.tkit");
    e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch =
        Some(transit_model::SamplerPatch::from_value(json!({"schema": 9})));
    let before = e.session_arc();
    let out = recall(&mut e, t, &file, true);
    assert!(out["notice"].as_str().unwrap().contains("cannot edit the track's Sampler patch"), "{out}");
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    let err = e.execute(SAVE, &json!({"track": t, "path": d.0.join("Opaque")})).unwrap_err().to_string();
    assert!(err.contains("cannot save the track's Sampler kit (it is kept as saved)"), "{err}");
    let (mut f, u) = setup();
    f.session_mut().track_mut(u).unwrap().freeze = Some(Frozen { source: SourceId(99), start: 0, length: 1, hash: 0, revision: 0, latency: 0 });
    let before = f.session_arc();
    let out = recall(&mut f, u, &file, true);
    assert!(out["notice"].as_str().unwrap().contains("frozen"), "{out}");
    assert!(Arc::ptr_eq(&before, &f.session_arc()));
}

#[test]
fn save_appends_the_extension_and_refuses_a_sample_with_no_file() {
    let d = Dir::new();
    let (mut e, t) = setup();
    let kick = d.audio("Kick.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": kick})).unwrap();
    for (given, written) in [("808 v1.5", "808 v1.5.tkit"), ("Kit.1", "Kit.1.tkit"), ("Kit.2", "Kit.2.tkit"), ("Up.TKIT", "Up.TKIT")] {
        let out = save_kit(&mut e, t, &d.0.join(given));
        assert_eq!(out["path"], d.0.join(written).to_string_lossy().as_ref());
        assert!(d.0.join(written).is_file());
    }
    assert!(!d.0.join("808 v1.tkit").exists(), "never replacing another extension");
    // Dropped bytes are the session's own audio with no file yet: nothing is written.
    let bytes = std::fs::read(&kick).unwrap();
    crate::io::load_zone_bytes(&mut e, t, 1, "Hit.wav", &bytes).unwrap();
    e.execute("mix.sampler_pad_bind", &json!({"track": t, "pad": 3, "zone": 2})).unwrap();
    let err = e.execute(SAVE, &json!({"track": t, "path": d.0.join("NoFile")})).unwrap_err().to_string();
    assert!(err.contains("Pad A3's sample \"Hit.wav\" has no file yet"), "{err}");
    assert!(!d.0.join("NoFile.tkit").exists() && !d.0.join("NoFile.tkit.tmp").exists());
}
