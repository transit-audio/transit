//! A session source a zone takes by id keeps its own kind (#471): a referenced file stays
//! referenced, so a kit recall releases it when nothing else names it and a session save keeps its
//! path; managed audio stays managed. Scratch temp folders and generated audio only.

use super::tests::{Dir, kit, recall, save_kit, setup};
use super::*;
use transit_audio_io::{AudioBuffer, FileFormat};

/// A second Sampler track with an empty patch.
fn second(e: &mut Engine) -> TrackId {
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let u = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": u})).unwrap();
    u
}

/// A kit file `Hat.tkit` in `d` over its own Hat.wav, which names nothing of the test's session.
fn other_kit(d: &Dir) -> PathBuf {
    let (mut b, u) = setup();
    let hat = d.audio("Hat.wav", 64, 48_000);
    b.execute("mix.sampler_zone_source", &json!({"track": u, "zone": 1, "path": hat})).unwrap();
    save_kit(&mut b, u, &d.0.join("Hat"));
    d.0.join("Hat.tkit")
}

/// Track `t` loads Kick.wav from `d` by path (referenced) and track `u` takes that source by id;
/// then `t`'s zone is cleared, so only `u` names it. The source's id.
fn by_id(e: &mut Engine, d: &Dir, t: TrackId, u: TrackId) -> (SourceId, PathBuf) {
    let kick = d.audio("Kick.wav", 128, 48_000);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": kick})).unwrap();
    let sid = kit(e, t).sources[0].source;
    assert_eq!(kit(e, t).sources[0].ownership, Ownership::Referenced);
    e.execute("mix.sampler_zone_source", &json!({"track": u, "zone": 1, "pad": 1, "source": sid})).unwrap();
    e.execute("mix.sampler_zone_clear", &json!({"track": t, "zone": 1})).unwrap();
    (sid, kick)
}

#[test]
fn a_referenced_source_assigned_by_id_stays_referenced_and_leaves_with_its_kit() {
    let (d, other) = (Dir::new(), Dir::new());
    let (mut e, t) = setup();
    let u = second(&mut e);
    let (sid, kick) = by_id(&mut e, &d, t, u);
    assert_eq!(kit(&e, u).sources[0].ownership, Ownership::Referenced, "the zone keeps the source's own kind");
    assert!(e.session().source(sid).is_some(), "still named by track u's zone");
    let file = other_kit(&other);
    let undone = serde_json::to_value(e.session()).unwrap();
    recall(&mut e, u, &file, true);
    assert!(e.session().source(sid).is_none() && e.session().pool.get(sid).is_none(), "nothing names it any more: it leaves the session");
    assert!(kick.is_file(), "the file is never touched");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(serde_json::to_value(e.session()).unwrap(), undone, "undo brings it back");
}

#[test]
fn a_session_save_keeps_the_path_of_a_referenced_source_assigned_by_id() {
    let (d, out) = (Dir::new(), Dir::new());
    let (mut e, t) = setup();
    let u = second(&mut e);
    let (sid, kick) = by_id(&mut e, &d, t, u);
    let song = out.0.join("song").join("song.transit");
    e.execute("session.save_as", &json!({"path": song})).unwrap();
    assert_eq!(e.session().source(sid).unwrap().path, kick.to_string_lossy(), "the file is referenced where it is");
    let audio = out.0.join("song").join("Audio Files");
    let written: Vec<_> = std::fs::read_dir(&audio).map(|r| r.flatten().map(|f| f.file_name()).collect()).unwrap_or_default();
    assert!(written.is_empty(), "no copy is written into the session: {written:?}");
}

#[test]
fn managed_audio_assigned_by_id_stays_managed_and_stays_after_a_recall() {
    let (d, other) = (Dir::new(), Dir::new());
    let (mut e, t) = setup();
    let rate = e.session().sample_rate.hz();
    let buf = || AudioBuffer { sample_rate: rate, channels: vec![(0..96).map(|i| i as f32 / 200.0).collect()] };
    // The session's own audio, and a timeline import a save has still to write (its original
    // path is absolute, but it is unsaved: the save copies it into Audio Files).
    let own = crate::io::add_source(e.session_mut(), "own", buf(), None, FileFormat::Wav);
    let pending = d.audio("Import.wav", 96, rate);
    let import = crate::io::add_source(e.session_mut(), "import", buf(), Some(pending.to_string_lossy().as_ref()), FileFormat::Wav);
    assert!(e.session().source(import).unwrap().unsaved);
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "source": own})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "source": import})).unwrap();
    let p = kit(&e, t);
    assert!(p.sources.iter().all(|r| r.ownership == Ownership::Managed), "{:?}", p.sources);
    recall(&mut e, t, &other_kit(&other), true);
    assert!(e.session().source(own).is_some() && e.session().source(import).is_some(), "managed sources stay: they may be the only copy");
}

#[test]
fn kit_save_writes_a_by_id_referenced_source_by_its_file_path() {
    let (d, elsewhere) = (Dir::new(), Dir::new());
    let (mut e, t) = setup();
    let u = second(&mut e);
    let (_, kick) = by_id(&mut e, &d, t, u);
    // Beside the sample: relative, and no note that the kit depends on a session folder.
    let out = save_kit(&mut e, u, &d.0.join("K"));
    assert_eq!((out["relative"].as_u64(), out["absolute"].as_u64(), &out["notes"]), (Some(1), Some(0), &json!([])), "{out}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(d.0.join("K.tkit")).unwrap()).unwrap();
    assert_eq!(v["kit"]["sources"][0]["file"]["path"], "Kick.wav");
    assert!(v["kit"]["sources"][0].get("ownership").is_none(), "{v}");
    // Elsewhere: the absolute path as the session holds it.
    let out = save_kit(&mut e, u, &elsewhere.0.join("K"));
    assert_eq!((out["relative"].as_u64(), out["absolute"].as_u64(), &out["notes"]), (Some(0), Some(1), &json!([])), "{out}");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(elsewhere.0.join("K.tkit")).unwrap()).unwrap();
    assert_eq!(v["kit"]["sources"][0]["file"]["path"], kick.to_string_lossy().as_ref());
}
