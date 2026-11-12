//! Generated plain-file fixtures, no devices or clock outcomes.

use super::*;
use std::path::Path;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use transit_model::sampler_patch::PlayMode;

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // The stamp only names fresh storage; no test outcome depends on elapsed time.
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("transit-kit-{}-{id}-{stamp}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn audio(&self, name: &str, frames: usize, rate: u32, channels: usize) -> PathBuf {
        let path = self.0.join(name);
        let format = match path.extension().unwrap().to_str().unwrap() {
            "flac" => FileFormat::Flac,
            "aif" => FileFormat::Aiff,
            _ => FileFormat::Wav,
        };
        let samples = (0..frames).map(|i| ((i.wrapping_mul(1664525).wrapping_add(1013904223) >> 16) & 255) as f32 / 256.0 - 0.5).collect();
        let audio = AudioBuffer { sample_rate: rate, channels: vec![samples; channels] };
        let opts = EncodeOptions { format, bit_depth: BitDepth::Int16, ..EncodeOptions::default() };
        std::fs::write(&path, transit_audio_io::encode(&audio, &opts).unwrap()).unwrap();
        path
    }
}

fn setup() -> (Engine, TrackId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    (e, t)
}

fn load(e: &mut Engine, t: TrackId, folder: &Path, preview: bool) -> Value {
    e.execute(ID, &json!({"track": t, "folder": folder, "preview": preview})).unwrap()
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    patch_of(e.session(), ID, t).unwrap()
}

fn refused(e: &mut Engine, t: TrackId, d: &Dir, count: usize, needle: &str) -> Value {
    let before = e.session_arc();
    let steps = e.undo_history().len();
    let mut results = Vec::new();
    for preview in [true, false] {
        let out = load(e, t, &d.0, preview);
        assert_eq!(out["loaded"], 0, "{out}");
        assert_eq!(out["skipped"], count, "{out}");
        assert_eq!(out["needed"], count);
        assert!(out["notice"].as_str().unwrap().contains(needle), "{out}");
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        assert_eq!(e.undo_history().len(), steps);
        results.push(out);
    }
    assert_eq!(results[0], results[1]);
    results.remove(0)
}

#[test]
fn kit_fills_free_pads_in_case_folded_name_order() {
    // A case-insensitive filesystem cannot hold both spellings. Exercise the tie directly,
    // then the real folder command with filenames distinct on every supported filesystem.
    let mut paths: Vec<PathBuf> = ["b.wav", "A.wav", "a.wav", "C.flac", "d.aif"].into_iter().map(PathBuf::from).collect();
    files::sort_files(&mut paths);
    assert_eq!(paths, ["A.wav", "a.wav", "b.wav", "C.flac", "d.aif"].map(PathBuf::from));
    let d = Dir::new();
    for name in ["b.wav", "A.wav", "C.flac", "d.aif"] {
        d.audio(name, 128, 48_000, 1);
    }
    let (mut e, t) = setup();
    let out = load(&mut e, t, &d.0, false);
    assert_eq!(out["loaded"], 4, "{out}");
    let p = patch(&e, t);
    for (i, name) in ["A.wav", "b.wav", "C.flac", "d.aif"].iter().enumerate() {
        assert_eq!(out["pads"][i], json!({"pad": i+1, "zone": i+1, "file": name, "frames":128, "channels":1}));
        let z = p.zone(i as u16).unwrap();
        assert_eq!(p.pad(i as u8).zone, Some(i as u16));
        assert_eq!(z.root, 36 + i as u8);
        assert_eq!(z.mode, PlayMode::OneShot);
        assert!(!z.key_tracking);
        assert_eq!(z.label.as_deref(), Path::new(name).file_stem().and_then(|x| x.to_str()));
    }
}

#[test]
fn kit_starts_at_the_first_free_pad_and_never_overwrites() {
    let d = Dir::new();
    let old = Dir::new();
    let path = old.audio("old.wav", 128, 48_000, 1);
    let (mut e, t) = setup();
    for pad in [1, 2, 5] {
        e.execute("mix.sampler_zone_source", &json!({"track":t,"pad":pad,"zone":pad,"path":path})).unwrap();
    }
    e.execute("mix.sampler_pad_set", &json!({"track":t,"pad":3,"note":12})).unwrap();
    for name in ["1.wav", "2.wav", "3.wav"] {
        d.audio(name, 128, 48_000, 1);
    }
    let before = patch(&e, t);
    let out = load(&mut e, t, &d.0, false);
    assert_eq!(out["loaded"], 3, "{out}");
    assert_eq!(out["pads"].as_array().unwrap().iter().map(|p| p["pad"].as_u64().unwrap()).collect::<Vec<_>>(), [3, 4, 6]);
    let after = patch(&e, t);
    for pad in [0, 1, 4] {
        assert_eq!(before.pad(pad), after.pad(pad));
        assert_eq!(before.zone(u16::from(pad)), after.zone(u16::from(pad)));
    }
    assert_eq!(after.zone(2).unwrap().root, 12);
    assert!(after.pad_note_stored(2));
}

#[test]
fn kit_is_not_recursive_and_ignores_other_files() {
    let d = Dir::new();
    d.audio("hit.wav", 128, 48_000, 1);
    d.audio(".hidden.wav", 128, 48_000, 1);
    std::fs::create_dir(d.0.join("nested")).unwrap();
    std::fs::copy(d.0.join("hit.wav"), d.0.join("nested/hit.wav")).unwrap();
    for name in ["mapping.sfz", "notes.txt"] {
        std::fs::write(d.0.join(name), b"not executed").unwrap();
    }
    let (mut e, t) = setup();
    let out = load(&mut e, t, &d.0, false);
    assert_eq!(out["loaded"], 1);
    assert_eq!(out["ignored"], json!([".hidden.wav", "mapping.sfz", "nested", "notes.txt"]));
}

#[test]
fn kit_is_one_undo_step() {
    let d = Dir::new();
    for name in ["one.wav", "two.wav"] {
        d.audio(name, 128, 48_000, 1);
    }
    let (mut e, t) = setup();
    let before = serde_json::to_value(e.session()).unwrap();
    let steps = e.undo_history().len();
    assert_eq!(load(&mut e, t, &d.0, false)["loaded"], 2);
    assert_eq!(e.undo_history().len(), steps + 1);
    let after = serde_json::to_value(e.session()).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(serde_json::to_value(e.session()).unwrap(), before);
    assert_eq!(e.session().pool.len(), 0);
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(serde_json::to_value(e.session()).unwrap(), after);
    assert_eq!(e.session().pool.len(), 2);
}

#[test]
fn kit_over_capacity_refused_not_truncated() {
    let d = Dir::new();
    for i in 0..20 {
        d.audio(&format!("{i}.wav"), 32, 48_000, 1);
    }
    let (mut e, t) = setup();
    let path = d.0.join("0.wav");
    for pad in 1..=50 {
        e.execute("mix.sampler_zone_source", &json!({"track":t,"pad":pad,"zone":pad,"path":path})).unwrap();
    }
    let out = refused(&mut e, t, &d, 20, "Load kit needs 20 pads; 14 are free.");
    assert_eq!(out["free"], 14);
}

#[test]
fn kit_with_an_overlong_file_refuses_the_whole_kit_naming_it() {
    let d = Dir::new();
    d.audio("a.wav", 128, 48_000, 1);
    d.audio("long.wav", 329_600, 8_000, 1);
    let (mut e, t) = setup();
    let out = refused(&mut e, t, &d, 2, "41.2 s");
    assert_eq!(out["offender"], "long.wav");
    assert_eq!(out["notice"], "Load kit: long.wav is 41.2 s long, more than the 30 s a Sampler holds in memory; nothing was loaded (0 of 2 files).");
}

#[test]
fn kit_over_the_patch_budget_refuses_whole() {
    let d = Dir::new();
    // Sparse PCM files: the header alone reaches the aggregate bound; no giant decode.
    for i in 0..12 {
        let path = d.0.join(format!("{i:02}.wav"));
        let header = transit_audio_io::wav_float_header(2, 48_000, 1_440_000).unwrap();
        std::fs::write(&path, &header).unwrap();
        std::fs::OpenOptions::new().write(true).open(path).unwrap().set_len(header.len() as u64 + 1_440_000 * 8).unwrap();
    }
    let (mut e, t) = setup();
    let out = refused(&mut e, t, &d, 12, "128 MiB");
    assert_eq!(out["offender"], "11.wav");
}

#[test]
fn kit_with_an_unreadable_file_refuses_whole_naming_it() {
    let d = Dir::new();
    d.audio("a.wav", 128, 48_000, 1);
    std::fs::write(d.0.join("broken.wav"), b"not audio").unwrap();
    let (mut e, t) = setup();
    assert_eq!(refused(&mut e, t, &d, 2, "broken.wav")["offender"], "broken.wav");
}

#[test]
fn kit_with_a_surround_file_refused_with_the_conversion_message() {
    let d = Dir::new();
    d.audio("wide.wav", 128, 48_000, 6);
    let (mut e, t) = setup();
    let out = refused(&mut e, t, &d, 1, "mono or stereo");
    assert!(out["notice"].as_str().unwrap().contains("convert it first"));
}

#[test]
fn preview_matches_the_applied_plan_and_changes_nothing() {
    let d = Dir::new();
    for name in ["hit.wav", "hit.aif", "hit.flac"] {
        d.audio(name, 441, 44_100, 2);
    }
    let (mut e, t) = setup();
    let before = e.session_arc();
    let steps = e.undo_history().len();
    let preview = load(&mut e, t, &d.0, true);
    assert_eq!(preview["loaded"], 3, "{preview}");
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert_eq!(e.undo_history().len(), steps);
    assert_eq!(load(&mut e, t, &d.0, false), preview);
    assert_eq!(preview["pads"][0]["frames"], 441, "native frames retained (ADR 0041)");
}

#[test]
fn kit_references_files_by_path_and_saves_and_reloads() {
    let d = Dir::new();
    let save = Dir::new();
    d.audio("hit.wav", 128, 48_000, 1);
    let (mut e, t) = setup();
    assert_eq!(load(&mut e, t, &d.0, false)["loaded"], 1);
    let before = patch(&e, t);
    assert!(before.sources.iter().all(|s| s.ownership == Ownership::Referenced));
    let path = save.0.join("song.transit");
    e.execute("session.save_as", &json!({"path":path})).unwrap();
    let mut back = Engine::default();
    let opened = back.execute("session.open", &json!({"path":path})).unwrap();
    assert_eq!(patch(&back, t), before);
    for source in &before.sources {
        let audio = back.session().pool.get(source.source).unwrap_or_else(|| panic!("reopened kit audio is missing: {opened}"));
        assert_eq!(audio.buffer.channels, e.session().pool.get(source.source).unwrap().buffer.channels);
    }
    assert!(back.session().sources.iter().all(|s| !s.unsaved && Path::new(&s.path).is_absolute()));
    assert!(!save.0.join("Audio Files/hit.wav").exists());
}

#[test]
fn kit_uses_checked_store_and_refuses_a_ninth_layer() {
    let d = Dir::new();
    let path = d.audio("hit.wav", 128, 48_000, 1);
    let (mut e, t) = setup();
    for zone in 1..=8 {
        e.execute("mix.sampler_zone_source", &json!({"track":t,"zone":zone,"path":path})).unwrap();
    }
    refused(&mut e, t, &d, 1, "ninth layer");
}

#[test]
fn kit_empty_missing_and_relative_folders_are_checked() {
    let d = Dir::new();
    let (mut e, t) = setup();
    refused(&mut e, t, &d, 0, "No audio files");
    let before = e.session_arc();
    assert_eq!(load(&mut e, t, &d.0.join("absent"), false)["loaded"], 0);
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    d.audio("hit.wav", 128, 48_000, 1);
    e.session_mut().folder = Some(d.0.clone());
    assert_eq!(load(&mut e, t, Path::new("../").join(d.0.file_name().unwrap()).as_path(), false)["loaded"], 1);
}

#[test]
fn kit_audio_matches_the_existing_pad_source_path_in_process() {
    let d = Dir::new();
    let path = d.audio("hit.wav", 441, 44_100, 2);
    let (mut kit, t) = setup();
    let (mut pad, u) = setup();
    assert_eq!(load(&mut kit, t, &d.0, false)["loaded"], 1);
    pad.execute("mix.sampler_zone_source", &json!({"track":u,"pad":1,"zone":1,"path":path})).unwrap();
    let a = patch(&kit, t);
    let b = patch(&pad, u);
    let left = &kit.session().pool.get(a.sources[0].source).unwrap().buffer;
    let right = &pad.session().pool.get(b.sources[0].source).unwrap().buffer;
    assert_eq!(left.sample_rate, right.sample_rate);
    assert_eq!(left.channels, right.channels);
    let mut za = a.zone(0).unwrap().clone();
    za.label = None;
    assert_eq!(&za, b.zone(0).unwrap());
}

#[test]
fn preview_reads_headers_without_reading_pcm_bodies() {
    use std::io::{Cursor, Read, Seek, SeekFrom};
    struct HeaderOnly {
        data: Cursor<Vec<u8>>,
        end: u64,
    }
    impl Read for HeaderOnly {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            assert!(self.data.position() + out.len() as u64 <= self.end, "probe tried to read audio");
            self.data.read(out)
        }
    }
    impl Seek for HeaderOnly {
        fn seek(&mut self, p: SeekFrom) -> std::io::Result<u64> {
            self.data.seek(p)
        }
    }
    let d = Dir::new();
    // The WAV encoder reserves a 36-byte JUNK chunk for an eventual RF64 upgrade.
    for (name, end) in [("hit.wav", 80), ("hit.aif", 54), ("hit.flac", 42)] {
        let path = d.audio(name, 8192, 48_000, 1);
        let mut reader = HeaderOnly { data: Cursor::new(std::fs::read(path).unwrap()), end };
        let info = transit_audio_io::probe_metadata(&mut reader).unwrap();
        assert_eq!((info.frames, info.channels, info.sample_rate), (8192, 1, 48_000));
    }
}

#[test]
fn kit_preserves_checked_roots_on_the_import_worker() {
    let allowed = Dir::new();
    let outside = Dir::new();
    outside.audio("hit.wav", 128, 48_000, 1);
    let roots = crate::file_access::AutomationRoots::new(Some(&allowed.0), None).unwrap();
    let (mut e, t) = setup();
    crate::file_access::confine(roots.as_ref(), || {
        refused(&mut e, t, &outside, 0, "outside the read root");
    });
}

#[test]
fn kit_refuses_zone_capacity_and_streaming_threshold() {
    let d = Dir::new();
    let path = d.audio("hit.wav", 48_000, 48_000, 1);
    let (mut e, t) = setup();
    e.session_mut().edit.values.insert(transit_model::disk::STREAM_MINUTES_KEY.into(), 0.001);
    refused(&mut e, t, &d, 1, "streaming threshold");
    e.session_mut().edit.values.remove(transit_model::disk::STREAM_MINUTES_KEY);
    e.execute("mix.sampler_zone_source", &json!({"track":t,"pad":1,"zone":1,"path":path})).unwrap();
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    let p = ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap();
    for id in 1..256 {
        let mut zone = p.zones[0].clone();
        zone.id = id;
        p.zones.push(zone);
    }
    refused(&mut e, t, &d, 1, "256 zones");
}

#[test]
fn preview_keeps_native_lengths_like_pad_import() {
    let d = Dir::new();
    d.audio("hit.wav", 3, 44_100, 1);
    let (mut e, t) = setup();
    let preview = load(&mut e, t, &d.0, true);
    assert_eq!(preview["loaded"], 1, "{preview}");
    assert_eq!(preview["pads"][0]["frames"], 3);
    assert_eq!(load(&mut e, t, &d.0, false), preview);
}

#[test]
fn kit_decode_failure_after_header_preview_discards_all_staged_sources() {
    let d = Dir::new();
    d.audio("a.wav", 128, 48_000, 1);
    let path = d.audio("broken.flac", 8192, 48_000, 1);
    // Keep STREAMINFO and remove every compressed packet: a header cannot certify the body.
    std::fs::OpenOptions::new().write(true).open(&path).unwrap().set_len(42).unwrap();
    let (mut e, t) = setup();
    let before = e.session_arc();
    let steps = e.undo_history().len();
    assert_eq!(load(&mut e, t, &d.0, true)["loaded"], 2);
    let out = load(&mut e, t, &d.0, false);
    assert_eq!(out["loaded"], 0, "{out}");
    assert_eq!(out["offender"], "broken.flac");
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
    assert_eq!(e.undo_history().len(), steps);
}

#[path = "tests_sampler_kit_replace.rs"]
mod replace;
