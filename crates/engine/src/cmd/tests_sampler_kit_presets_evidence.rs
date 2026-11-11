//! The rest of the kit presets amendment's evidence (ADR 0041, amendment 2026-11-11, item 12) and
//! the review's follow-ups: what keeps the replaced kit's source, the read root, `..`, network and
//! extended-length paths, another system's absolute path, unknown fields, nothing decoded on a
//! budget refusal, and a failed worker. Scratch temp folders and generated audio only.

use super::tests::{Dir, kit, recall, save_kit, setup};
use super::*;
use std::cell::Cell;
use std::sync::Mutex;
use transit_model::{ChannelFormat, Clip, Frozen, SamplerPatch, TrackKind};

/// Kit files whose recall worker fails in this test binary ([`fail_worker_for`]).
static FAIL: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The recall worker's failure seam: staging `file` panics when a test asked for it.
pub(super) fn fail_worker_for(file: &Path) {
    if FAIL.lock().is_ok_and(|v| v.iter().any(|f| f == file)) {
        panic!("the test's worker failure");
    }
}

/// A hand-written kit entry naming `path` with the record of a mono 16-bit 48 kHz file.
fn entry(path: &str, frames: u64) -> Value {
    json!({"file": {"path": path, "name": "Hit", "frames": frames, "channels": 1, "sample_rate": 48_000, "format": "Wav"}})
}

/// A hand-written kit file at `file`: one zone per entry, on keys 60 up.
fn hand_kit(file: &Path, entries: &[Value]) {
    let zones: Vec<Value> = (0..entries.len()).map(|i| json!({"id": i, "source": i, "end": 128, "keys": [60 + i, 60 + i]})).collect();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, serde_json::to_string(&portable::file(json!({"schema": 1, "sources": entries, "zones": zones}))).unwrap()).unwrap();
}

/// The kit file's entries for [`prepare`], as `load` reads them.
fn read_refs(file: &Path) -> (Value, Vec<FileRef>) {
    let v = files::read(file).unwrap();
    let (kit, entries) = portable::from_portable(v["kit"].clone()).unwrap();
    let refs = entries
        .into_iter()
        .map(|e| match e {
            Entry::File(f) => f,
            Entry::Session(_) => panic!("a file entry"),
        })
        .collect();
    (kit, refs)
}

fn whole_reads() -> usize {
    crate::io::WHOLE_READS.with(Cell::get)
}

/// What else names the replaced kit's source in [`the_old_kits_source_stays_while_anything_else_names_it`].
#[derive(Debug, Clone, Copy, PartialEq)]
enum Holder {
    Nothing,
    Freeze,
    AnotherSampler,
    NewKit,
    OpaquePatch,
    Clipboard,
}

fn instrument(e: &mut Engine) -> TrackId {
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let u = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": u})).unwrap();
    u
}

fn set_patch(e: &mut Engine, t: TrackId, patch: SamplerPatch) {
    e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch = Some(patch);
}

#[test]
fn the_old_kits_source_stays_while_anything_else_names_it() {
    let lib = Dir::new();
    let old = lib.audio("Old.wav", 128, 48_000);
    // The kit every case recalls over the old one: another sample.
    let other = Dir::new();
    let (mut h, ht) = setup();
    let hat = other.audio("Hat.wav", 64, 48_000);
    h.execute("mix.sampler_zone_source", &json!({"track": ht, "zone": 1, "path": hat})).unwrap();
    save_kit(&mut h, ht, &other.0.join("Hat"));
    for holder in [Holder::Nothing, Holder::Freeze, Holder::AnotherSampler, Holder::NewKit, Holder::OpaquePatch, Holder::Clipboard] {
        let (mut e, t) = setup();
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": old})).unwrap();
        let sid = kit(&e, t).sources[0].source;
        let mut file = other.0.join("Hat.tkit");
        match holder {
            Holder::Nothing => {}
            Holder::Freeze => {
                let a = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
                e.session_mut().track_mut(a).unwrap().freeze = Some(Frozen { source: sid, start: 0, length: 128, hash: 0, revision: 0, latency: 0 });
            }
            Holder::AnotherSampler => {
                let u = instrument(&mut e);
                let patch = e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.clone().unwrap();
                set_patch(&mut e, u, patch);
            }
            Holder::NewKit => {
                // The new kit names the same file: the source is reused, so the new patch names it.
                file = lib.0.join("Same.tkit");
                save_kit(&mut e, t, &file);
                e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 2, "path": hat})).unwrap();
            }
            Holder::OpaquePatch => {
                let u = instrument(&mut e);
                set_patch(&mut e, u, SamplerPatch::from_value(json!({"schema": 9, "sources": [{"source": sid.0}]})));
            }
            Holder::Clipboard => {
                let cid = e.session_mut().new_clip_id();
                e.clipboard.tracks = vec![vec![Clip::audio(cid, "Old", sid, 0, 0, 128)]];
            }
        }
        let out = recall(&mut e, t, &file, true);
        assert!(out.get("zones").is_some(), "{holder:?}: {out}");
        let s = e.session();
        let kept = s.source(sid).is_some() && s.pool.get(sid).is_some();
        assert_eq!(kept, holder != Holder::Nothing, "{holder:?}: the old source stays exactly while something else names it");
        if !kept {
            assert!(s.source(sid).is_none() && s.pool.get(sid).is_none(), "gone from the sources and the pool");
        }
        assert!(Path::new(&old).is_file(), "the file is never touched");
    }
}

#[test]
fn a_sample_outside_the_read_root_is_missing_and_never_read() {
    let d = Dir::new();
    let inside = d.audio("Kit/Inside.wav", 64, 48_000);
    let outside = d.audio("Outside/Out.wav", 96, 48_000);
    let (mut a, t) = setup();
    a.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": inside})).unwrap();
    a.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 2, "path": outside})).unwrap();
    let saved = save_kit(&mut a, t, &d.0.join("Kit").join("K"));
    assert_eq!((saved["relative"].as_u64(), saved["absolute"].as_u64()), (Some(1), Some(1)), "{saved}");
    let file = d.0.join("Kit").join("K.tkit");
    let roots = crate::file_access::AutomationRoots::new(Some(&d.0.join("Kit")), None).unwrap();
    let (mut b, u) = setup();
    let out = crate::file_access::confine(roots.as_ref(), || recall(&mut b, u, &file, false));
    let missing = out["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 1, "{out}");
    assert_eq!((missing[0]["zone"].as_u64(), missing[0]["pad"].as_u64()), (Some(2), Some(2)));
    assert!(missing[0]["path"].as_str().unwrap().ends_with("Out.wav"), "{out}");
    let p = kit(&b, u);
    assert!(b.session().pool.get(p.sources[usize::from(p.zone(0).unwrap().source)].source).is_some(), "the sample inside the root loads");
    // Staged on this thread, where whole-file reads are counted: only the sample inside is read.
    let (kit_value, refs) = read_refs(&file);
    let (mut c, w) = setup();
    let reads = whole_reads();
    let staged = crate::file_access::confine(roots.as_ref(), || prepare(&mut c, w, &file, kit_value, &refs));
    let staged = staged.unwrap_or_else(|why| panic!("{why}"));
    assert_eq!(whole_reads() - reads, 1, "Out.wav is never read");
    assert_eq!(staged.missing.len(), 1);
}

#[test]
fn a_relative_entry_climbing_with_dotdot_resolves_from_the_kit_folder() {
    let d = Dir::new();
    d.audio("Samples/Kick.wav", 128, 48_000);
    let file = d.0.join("Kits").join("Up.tkit");
    // The second climbs above the root: refused by the path check, so missing.
    let climb = format!("{}Nowhere.wav", "../".repeat(64));
    hand_kit(&file, &[entry("../Samples/Kick.wav", 128), entry(&climb, 128)]);
    let (mut b, u) = setup();
    let out = recall(&mut b, u, &file, false);
    let missing = out["missing"].as_array().unwrap();
    assert_eq!(missing.len(), 1, "{out}");
    assert_eq!(missing[0]["zone"], 2, "{out}");
    let p = kit(&b, u);
    let sid = p.sources[0].source;
    assert!(b.session().pool.get(sid).is_some(), "found one folder up: {out}");
    let path = b.session().source(sid).unwrap().path.clone();
    assert!(portable::same_path(&path, &d.0.join("Samples").join("Kick.wav").to_string_lossy()), "{path}");
    assert!(!path.contains(".."), "stored folded: {path}");
}

#[test]
fn a_network_sample_on_another_share_is_saved_absolute_and_missing_on_recall() {
    let d = Dir::new();
    let (mut a, t) = setup();
    let kick = d.audio("Kick.wav", 128, 48_000);
    a.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": kick})).unwrap();
    let sid = kit(&a, t).sources[0].source;
    let share = r"\\nas\samples\Kick.wav";
    a.session_mut().sources.iter_mut().find(|s| s.id == sid).unwrap().path = share.into();
    let saved = save_kit(&mut a, t, &d.0.join("Net"));
    assert_eq!((saved["relative"].as_u64(), saved["absolute"].as_u64()), (Some(0), Some(1)), "{saved}");
    assert!(saved["notes"].to_string().contains("another network share"), "{saved}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(d.0.join("Net.tkit")).unwrap()).unwrap();
    assert_eq!(v["kit"]["sources"][0]["file"]["path"], share, "absolute, never relative");
    let (mut b, u) = setup();
    let out = recall(&mut b, u, &d.0.join("Net.tkit"), false);
    assert_eq!(out["missing"], json!([{"zone": 1, "pad": 1, "path": share}]), "{out}");
    let p = kit(&b, u);
    assert_eq!(b.session().source(p.sources[0].source).unwrap().path, share);
    assert!(b.session().pool.get(p.sources[0].source).is_none());
}

#[test]
fn an_extended_length_source_path_is_saved_without_the_prefix_and_found_again() {
    let d = Dir::new();
    let lib = Dir::new();
    let kick = lib.audio("Kick.wav", 128, 48_000);
    let (mut a, t) = setup();
    a.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": kick})).unwrap();
    let sid = kit(&a, t).sources[0].source;
    let plain = a.session().source(sid).unwrap().path.clone();
    a.session_mut().sources.iter_mut().find(|s| s.id == sid).unwrap().path = format!(r"\\?\{plain}");
    save_kit(&mut a, t, &d.0.join("Away"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(d.0.join("Away.tkit")).unwrap()).unwrap();
    assert_eq!(v["kit"]["sources"][0]["file"]["path"], plain.as_str(), "written without the prefix");
    let beside = save_kit(&mut a, t, &lib.0.join("Beside"));
    assert_eq!(beside["relative"], 1, "the prefix is removed before the relative test: {beside}");
    let (mut b, u) = setup();
    let out = recall(&mut b, u, &d.0.join("Away.tkit"), false);
    assert_eq!(out["missing"], json!([]), "{out}");
    let p = kit(&b, u);
    assert_eq!(b.session().source(p.sources[0].source).unwrap().path, plain);
}

#[test]
fn another_systems_absolute_path_is_missing_and_never_mapped_onto_the_kits_drive() {
    let d = Dir::new();
    let kick = d.audio("Kick.wav", 128, 48_000);
    let text = kick.to_string_lossy().into_owned();
    // The same file named the other system's way: on Windows the macOS or Linux form (no drive,
    // `/`), which the path check would put on the kit file's drive; elsewhere a drive path.
    let other = if cfg!(windows) { text[2..].replace('\\', "/") } else { format!(r"C:{}", text.replace('/', "\\")) };
    let file = d.0.join("Other.tkit");
    hand_kit(&file, &[entry(&other, 128)]);
    let (mut b, u) = setup();
    let out = recall(&mut b, u, &file, false);
    assert_eq!(out["missing"], json!([{"zone": 1, "path": other}]), "{out}");
    assert!(b.session().pool.get(kit(&b, u).sources[0].source).is_none(), "nothing loaded from {text}");
}

#[test]
fn unknown_fields_survive_recall_and_save_except_inside_a_file_entry() {
    let d = Dir::new();
    d.audio("Kick.wav", 128, 48_000);
    let file = d.0.join("Future.tkit");
    let mut source = entry("Kick.wav", 128);
    source["file"]["future_file"] = json!(2);
    source["future_source"] = json!(3);
    let kit_json = json!({
        "schema": 1,
        "future_top": 1,
        "sources": [source],
        "zones": [{"id": 0, "source": 0, "end": 128, "keys": [60, 60], "future_zone": 4}],
    });
    std::fs::write(&file, serde_json::to_string(&portable::file(kit_json)).unwrap()).unwrap();
    let (mut b, u) = setup();
    let out = recall(&mut b, u, &file, false);
    assert_eq!(out["missing"], json!([]), "{out}");
    save_kit(&mut b, u, &d.0.join("Again"));
    let v: Value = serde_json::from_str(&std::fs::read_to_string(d.0.join("Again.tkit")).unwrap()).unwrap();
    let k = &v["kit"];
    assert_eq!((&k["future_top"], &k["sources"][0]["future_source"], &k["zones"][0]["future_zone"]), (&json!(1), &json!(3), &json!(4)), "{v}");
    assert!(k["sources"][0]["file"].get("future_file").is_none(), "unknown fields inside `file` are not kept: {v}");
}

#[test]
fn a_kit_over_the_budget_is_refused_from_headers_with_nothing_decoded() {
    let d = Dir::new();
    let mut entries = Vec::new();
    for i in 0..12u16 {
        let name = format!("{i:02}.wav");
        let header = transit_audio_io::wav_float_header(2, 48_000, 1_440_000).unwrap();
        std::fs::write(d.0.join(&name), &header).unwrap();
        std::fs::OpenOptions::new().write(true).open(d.0.join(&name)).unwrap().set_len(header.len() as u64 + 1_440_000 * 8).unwrap();
        let mut e = entry(&name, 1_440_000);
        e["file"]["channels"] = json!(2);
        entries.push(e);
    }
    let file = d.0.join("Big.tkit");
    hand_kit(&file, &entries);
    let (kit_value, refs) = read_refs(&file);
    let (mut e, t) = setup();
    let reads = whole_reads();
    let refused = prepare(&mut e, t, &file, kit_value, &refs).err().expect("refused");
    assert!(refused.contains("128 MiB"), "{refused}");
    assert_eq!(whole_reads(), reads, "refused from the headers: no file was read whole, nothing decoded");
}

#[test]
fn a_failed_recall_worker_answers_data_and_changes_nothing() {
    let d = Dir::new();
    let (mut e, t) = setup();
    let kick = d.audio("Kick.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "path": kick})).unwrap();
    save_kit(&mut e, t, &d.0.join("K"));
    let file = d.0.join("K.tkit");
    FAIL.lock().unwrap().push(file.clone());
    let before = e.session_arc();
    let steps = e.undo_history().len();
    let out = e.execute(LOAD, &json!({"track": t, "path": file, "replace": true}));
    FAIL.lock().unwrap().retain(|f| f != &file);
    let out = out.expect("a failed worker is a refusal answered as data");
    assert_eq!(out, json!({"track": t, "kit": "K", "loaded": 0, "notice": "Kit \"K\": import worker failed; nothing was loaded."}));
    assert!(Arc::ptr_eq(&before, &e.session_arc()) && e.undo_history().len() == steps, "nothing changed");
}
