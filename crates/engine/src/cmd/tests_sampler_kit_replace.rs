//! Load Kit Folder's replace option (#465): the caller states it, the engine never guesses. The
//! folder fills pads from A1 as a load into an empty kit does; each filled pad is cleared first as
//! Clear pad clears one; later pads, the slice loop and its markers stay; the replaced pads'
//! sources leave the session only when nothing else names them; one undo step either way.

use super::*;
use transit_audio_io::AudioBuffer;
use transit_model::sampler_patch::{Marker, Markers, SliceBinding, SliceMode, SliceSet};

fn load_with(e: &mut Engine, t: TrackId, folder: &Path, preview: bool, replace: bool) -> Value {
    e.execute(ID, &json!({"track": t, "folder": folder, "preview": preview, "replace": replace})).unwrap()
}

/// A kit with `files` on pads 1..=n (zone n on pad n), each its own referenced source.
fn loaded(files: &[&str]) -> (Engine, TrackId, Dir) {
    let old = Dir::new();
    let (mut e, t) = setup();
    for (i, name) in files.iter().enumerate() {
        let path = old.audio(name, 128, 48_000, 1);
        e.execute("mix.sampler_zone_source", &json!({"track": t, "pad": i + 1, "zone": i + 1, "path": path})).unwrap();
    }
    (e, t, old)
}

/// A folder of `n` new files, `n01.wav`...
fn folder(n: usize) -> Dir {
    let d = Dir::new();
    for i in 0..n {
        d.audio(&format!("n{:02}.wav", i + 1), 64, 48_000, 1);
    }
    d
}

fn source_named(e: &Engine, stem: &str) -> Option<SourceId> {
    e.session().sources.iter().find(|s| Path::new(&s.path).file_stem().and_then(|x| x.to_str()) == Some(stem)).map(|s| s.id)
}

/// The file stem of the sample on pad `pad` (0-based).
fn label(e: &Engine, p: &PatchV1, pad: u8) -> Option<String> {
    let zone = p.pad(pad).zone.and_then(|z| p.zone(z))?;
    let source = e.session().source(p.sources[usize::from(zone.source)].source)?;
    Path::new(&source.path).file_stem().and_then(|x| x.to_str()).map(str::to_string)
}

#[test]
fn without_replace_the_load_fills_free_pads_and_states_nothing_was_replaced() {
    let (mut e, t, _old) = loaded(&["k1.wav", "k2.wav"]);
    let d = folder(2);
    let absent = load(&mut e, t, &d.0, true);
    assert_eq!(load_with(&mut e, t, &d.0, true, false), absent, "replace: false is the default");
    assert_eq!((absent["first_pad"].clone(), absent["last_pad"].clone()), (json!(3), json!(4)), "{absent}");
    assert_eq!((absent["replace"].clone(), absent["replaced"].clone()), (json!(false), json!(0)));
    let out = load(&mut e, t, &d.0, false);
    assert_eq!(out["loaded"], 2);
    let p = patch(&e, t);
    assert_eq!(
        [label(&e, &p, 0), label(&e, &p, 1), label(&e, &p, 2), label(&e, &p, 3)],
        [Some("k1"), Some("k2"), Some("n01"), Some("n02")].map(|x| x.map(str::to_string))
    );
    assert!(source_named(&e, "k1").is_some() && source_named(&e, "k2").is_some(), "nothing pruned without replace");
}

#[test]
fn without_replace_a_folder_larger_than_the_free_pads_is_refused_and_nothing_changes() {
    let (mut e, t, _old) = loaded(&["k1.wav"]);
    let path = e.session().sources[0].path.clone();
    for pad in 2..=62 {
        e.execute("mix.sampler_zone_source", &json!({"track": t, "pad": pad, "zone": pad, "path": path})).unwrap();
    }
    let d = folder(3);
    let out = refused(&mut e, t, &d, 3, "Load kit needs 3 pads; 2 are free.");
    assert_eq!(out["free"], 2);
    // The same folder with replace fits: the three pads from A1 are replaced.
    let out = load_with(&mut e, t, &d.0, false, true);
    assert_eq!((out["loaded"].clone(), out["replaced"].clone()), (json!(3), json!(3)), "{out}");
}

#[test]
fn replace_with_fewer_files_than_loaded_pads_fills_from_a1_and_leaves_the_later_pads() {
    let (mut e, t, _old) = loaded(&["k1.wav", "k2.wav", "k3.wav", "k4.wav", "k5.wav"]);
    let before = patch(&e, t);
    let d = folder(2);
    let preview = load_with(&mut e, t, &d.0, true, true);
    assert_eq!((preview["first_pad"].clone(), preview["last_pad"].clone()), (json!(1), json!(2)), "{preview}");
    assert_eq!((preview["replace"].clone(), preview["replaced"].clone(), preview["replaced_slices"].clone()), (json!(true), json!(2), json!(0)));
    let out = load_with(&mut e, t, &d.0, false, true);
    assert_eq!(out, preview, "the preview is the plan applied");
    let p = patch(&e, t);
    assert_eq!([label(&e, &p, 0), label(&e, &p, 1)], [Some("n01".to_string()), Some("n02".to_string())]);
    for pad in 2..5u8 {
        assert_eq!(p.pad(pad), before.pad(pad), "pad {} is past the folder's files: left as it was", pad + 1);
        // The source indices renumber when the replaced sources drop out; the source is the same.
        let (mut now, was) = (p.zone(u16::from(pad)).unwrap().clone(), before.zone(u16::from(pad)).unwrap());
        assert_eq!(p.sources[usize::from(now.source)].source, before.sources[usize::from(was.source)].source);
        now.source = was.source;
        assert_eq!(&now, was);
    }
    assert_eq!(p.zones.len(), 5);
}

#[test]
fn a_replaced_pad_is_cleared_first_so_no_setting_of_the_old_sample_carries_over() {
    let (mut e, t, _old) = loaded(&["k1.wav", "k2.wav"]);
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 1, "name": "Old", "choke": 3, "mute": true, "solo": true, "note": 110})).unwrap();
    e.execute("mix.sampler_control", &json!({"track": t, "id": "zone001_level", "value": -12.0})).unwrap();
    e.execute("mix.sampler_control", &json!({"track": t, "id": "zone002_pan", "value": 40.0})).unwrap();
    // A free pad in the range with a name and a choke group of its own.
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 3, "name": "Spare", "choke": 2})).unwrap();
    {
        let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
        let p = ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap();
        p.zone_mut(0).unwrap().transpose = 7.0;
        p.zone_mut(0).unwrap().attack_ms = 200.0;
    }
    let d = folder(3);
    let out = load_with(&mut e, t, &d.0, false, true);
    assert_eq!((out["loaded"].clone(), out["replaced"].clone()), (json!(3), json!(2)), "{out}");
    let p = patch(&e, t);
    let pad = p.pad(0);
    assert_eq!((pad.name.clone(), pad.choke, pad.mute, pad.solo), (None, 0, false, false), "the pad is cleared as Clear pad clears it");
    assert_eq!(p.pad_note(0), 110, "the trigger note stays, as Clear pad keeps it");
    let spare = p.pad(2);
    assert_eq!((spare.name.clone(), spare.choke), (None, 0), "every pad the folder fills is cleared");
    let zone = p.zone(pad.zone.unwrap()).unwrap();
    let mut fresh = ZoneEntry::whole(zone.id, zone.source, 64);
    fresh.pad_defaults(110);
    fresh.label = Some("n01".into());
    assert_eq!(zone, &fresh, "the new zone has the new-pad defaults, nothing of the old zone");
    assert!(p.controls.keys().all(|id| !id.starts_with("zone001_") && !id.starts_with("zone002_")), "{:?}", p.controls);
}

#[test]
fn a_slice_bound_pad_in_range_is_replaced_and_the_loop_and_markers_stay() {
    let (mut e, t, _old) = loaded(&[]);
    let rate = e.session().sample_rate.hz();
    let buf = AudioBuffer { sample_rate: rate, channels: vec![(0..4800).map(|i| (i as f32 * 0.01).sin() * 0.3).collect()] };
    let sid = crate::io::add_source(e.session_mut(), "loop", buf, None, transit_audio_io::FileFormat::Wav);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    {
        let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
        let p = ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap();
        let l = p.slicing.as_mut().unwrap();
        let markers = (0..4u32).map(|k| Marker { id: k + 1, frame: u64::from(k) * 1200, ..Marker::default() }).collect();
        l.manual = Some(Markers { markers, next_id: 5, ..Markers::default() });
        l.mode = SliceMode::Manual;
        for (zone, slice) in [(0u16, 1u32), (1, 2)] {
            let mut z = ZoneEntry::whole(zone, 0, 4800);
            z.slice = Some(SliceBinding { set: SliceSet::Manual, id: slice, ..SliceBinding::default() });
            z.start = u64::from(slice - 1) * 1200;
            z.end = u64::from(slice) * 1200;
            z.loop_start = z.start;
            z.loop_end = z.end;
            p.zones.push(z);
            p.set_pad(PadEntry { zone: Some(zone), ..p.pad(zone as u8) });
        }
    }
    let before = patch(&e, t);
    let d = folder(1);
    let preview = load_with(&mut e, t, &d.0, true, true);
    assert_eq!((preview["replaced"].clone(), preview["replaced_slices"].clone()), (json!(1), json!(1)), "{preview}");
    load_with(&mut e, t, &d.0, false, true);
    let p = patch(&e, t);
    assert_eq!(label(&e, &p, 0).as_deref(), Some("n01"), "the slice-bound pad is replaced like any other");
    assert!(p.zone(p.pad(0).zone.unwrap()).unwrap().slice.is_none());
    assert_eq!(p.pad(1), before.pad(1), "the slice pad past the folder's files stays");
    assert_eq!(p.zone(1), before.zone(1));
    let (a, b) = (p.slicing.as_ref().unwrap(), before.slicing.as_ref().unwrap());
    assert_eq!((a.manual.clone(), a.auto.clone(), a.mode), (b.manual.clone(), b.auto.clone(), b.mode), "the markers stay");
    assert_eq!(p.sources[usize::from(a.source)].source, sid, "the loop keeps its source");
    assert!(e.session().source(sid).is_some());
}

#[test]
fn replace_is_one_undo_step_that_restores_everything_including_the_sources() {
    let (mut e, t, _old) = loaded(&["k1.wav", "k2.wav", "k3.wav"]);
    let before = serde_json::to_value(e.session()).unwrap();
    let pool = e.session().pool.len();
    let steps = e.undo_history().len();
    let d = folder(2);
    load_with(&mut e, t, &d.0, false, true);
    assert_eq!(e.undo_history().len(), steps + 1);
    assert!(source_named(&e, "k1").is_none(), "replaced and unnamed: gone");
    let after = serde_json::to_value(e.session()).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(serde_json::to_value(e.session()).unwrap(), before, "one undo restores the kit and its sources");
    assert_eq!(e.session().pool.len(), pool);
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(serde_json::to_value(e.session()).unwrap(), after);
}

#[test]
fn a_replaced_pads_source_leaves_only_when_nothing_else_names_it() {
    let (mut e, t, old) = loaded(&["shared.wav", "only.wav"]);
    let _ = old;
    let ids = (source_named(&e, "shared").unwrap(), source_named(&e, "only").unwrap());
    // Pad 5 plays the same session source as pad 1.
    e.execute("mix.sampler_zone_source", &json!({"track": t, "pad": 5, "zone": 5, "source": ids.0})).unwrap();
    let p = patch(&e, t);
    assert_eq!(p.sources[usize::from(p.zone(4).unwrap().source)].source, ids.0, "pad 5 names the shared source");
    let d = folder(2);
    load_with(&mut e, t, &d.0, false, true);
    assert!(e.session().source(ids.0).is_some() && e.session().pool.get(ids.0).is_some(), "pad 5 still names it: kept");
    assert!(e.session().source(ids.1).is_none() && e.session().pool.get(ids.1).is_none(), "nothing names it: gone");
}

#[test]
fn a_refused_replace_changes_nothing() {
    let (mut e, t, _old) = loaded(&["k1.wav"]);
    let d = folder(1);
    std::fs::write(d.0.join("n02.wav"), b"not audio").unwrap();
    let before = e.session_arc();
    let steps = e.undo_history().len();
    for preview in [true, false] {
        let out = load_with(&mut e, t, &d.0, preview, true);
        assert_eq!(out["loaded"], 0, "{out}");
        assert_eq!(out["offender"], "n02.wav");
        assert!(Arc::ptr_eq(&before, &e.session_arc()));
        assert_eq!(e.undo_history().len(), steps);
    }
    let big = folder(65);
    let out = load_with(&mut e, t, &big.0, false, true);
    assert_eq!(out["notice"], "Load kit needs 65 pads; a Sampler has 64.");
    assert!(Arc::ptr_eq(&before, &e.session_arc()));
}
