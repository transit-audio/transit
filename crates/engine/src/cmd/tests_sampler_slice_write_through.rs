//! The write-through of slice-bound zones (ADR 0041 amendment 2026-11-08, item 1) and the used
//! values in `mix.sampler_slice_state`. After every marker add, move and remove, Clear, Start from
//! Auto's markers, Replace markers and Slices to pads, each zone bound to the edited set stores its
//! slice's derived source, region and loop, in the command's one undo step. The loop's source is
//! generated audio; sets and bindings that a file could hold are set on the patch directly.

use crate::Engine;
use serde_json::{Value, json};
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_model::sampler_fades::Fades;
use transit_model::sampler_loops::derived_region;
use transit_model::sampler_patch::{LoopMode, Marker, Markers, Ownership, PadEntry, PatchV1, PlayMode, SliceBinding, SliceMode, SliceSet, ZoneEntry};
use transit_model::{SourceId, TrackId};

/// An instrument track with an empty patch at pad start note 60 and a loop over a 48,000-frame
/// source of silence (`None` path: audio only in memory), Manual showing, snap off.
fn setup() -> (Engine, TrackId, SourceId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    let rate = e.session().sample_rate.hz();
    let buf = AudioBuffer { sample_rate: rate, channels: vec![vec![0.0; 48_000]] };
    let sid = crate::io::add_source(e.session_mut(), "loop", buf, None, FileFormat::Wav);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.mode = SliceMode::Manual;
        l.snap = false;
    });
    (e, t, sid)
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone()
}

/// Test setup outside the commands (what a saved file or an earlier build could hold).
fn edit(e: &mut Engine, t: TrackId, f: impl FnOnce(&mut PatchV1)) {
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    f(ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap());
}

fn set(next_id: u32, list: &[(u32, u64)]) -> Markers {
    let markers = list.iter().map(|&(id, frame)| Marker { id, frame, ..Marker::default() }).collect();
    Markers { markers, next_id, ..Markers::default() }
}

/// Binds zone `zone` (0-based, source index 0) to slice `id` of `set` with stored region and loop
/// `region`, Gate and looping Forward, on pad `pad` (0-based) when given.
fn bind(e: &mut Engine, t: TrackId, zone: u16, set: SliceSet, id: u32, region: (u64, u64), pad: Option<u8>) {
    edit(e, t, |p| {
        let mut z = ZoneEntry::whole(zone, 0, 48_000);
        (z.start, z.end, z.loop_start, z.loop_end) = (region.0, region.1, region.0, region.1);
        (z.mode, z.loop_mode) = (PlayMode::Gate, LoopMode::Forward);
        z.slice = Some(SliceBinding { set, id, ..SliceBinding::default() });
        p.zones.push(z);
        p.zones.sort_by_key(|z| z.id);
        if let Some(pad) = pad {
            p.set_pad(PadEntry { zone: Some(zone), ..p.pad(pad) });
        }
    });
}

/// Zone `id`'s stored (source, start, end, loop_start, loop_end).
fn stored(p: &PatchV1, id: u16) -> (u16, u64, u64, u64, u64) {
    let z = p.zone(id).unwrap_or_else(|| panic!("zone {id} is empty"));
    (z.source, z.start, z.end, z.loop_start, z.loop_end)
}

/// Asserts every slice-bound zone of `p` stores exactly its derived source, region and loop.
fn all_written(p: &PatchV1, why: &str) {
    for z in p.zones.iter().filter(|z| z.slice.is_some()) {
        let d = derived_region(z, p).unwrap_or_else(|| panic!("{why}: zone {} is bound to no marker", z.id));
        assert_eq!(stored(p, z.id), (d.source, d.start, d.end, d.loop_start, d.loop_end), "{why}: zone {} stores its slice's region", z.id);
    }
}

fn steps(e: &Engine) -> usize {
    e.undo_history().len()
}

/// Runs a command expected to succeed as exactly one undo step: one undo restores the document
/// whole (every stored zone field included), redo reproduces the edit.
fn one_step(e: &mut Engine, cmd: &str, params: Value) -> Value {
    let (before, n) = (e.session_arc(), steps(e));
    let out = e.execute(cmd, &params).unwrap_or_else(|er| panic!("{cmd} {params}: {er}"));
    let after = e.session_arc();
    assert_eq!(steps(e), n + 1, "{cmd} {params}: not exactly one step");
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(*e.session_arc() == *before, "{cmd} {params}: one undo does not restore the document");
    e.execute("edit.redo", &json!({})).unwrap();
    assert!(*e.session_arc() == *after, "{cmd} {params}: redo does not reproduce the edit");
    out
}

fn marker(e: &mut Engine, t: TrackId, params: Value) -> Value {
    let mut p = params;
    p["track"] = json!(t);
    one_step(e, "mix.sampler_slice_marker", p)
}

/// Manual [7 at 0, 8 at 10,000, 9 at 20,000] (next id 10) with zone 0 on slice 7 (pad A1) and
/// zone 1 on slice 8 (pad A2), both looping over their slices as stored.
fn three_slices(e: &mut Engine, t: TrackId) {
    edit(e, t, |p| p.slicing.as_mut().unwrap().manual = Some(set(10, &[(7, 0), (8, 10_000), (9, 20_000)])));
    bind(e, t, 0, SliceSet::Manual, 7, (0, 10_000), Some(0));
    bind(e, t, 1, SliceSet::Manual, 8, (10_000, 20_000), Some(1));
}

#[test]
fn marker_move_writes_through_to_a_looping_slice_zone_in_the_same_undo_step() {
    let (mut e, t, _) = setup();
    three_slices(&mut e, t);
    all_written(&patch(&e, t), "the setup");
    // Move: slice 8 now ends at 15,000, so zone 1's region and loop end there.
    marker(&mut e, t, json!({"op": "move", "id": 9, "frame": 15_000}));
    let p = patch(&e, t);
    assert_eq!(stored(&p, 1), (0, 10_000, 15_000, 10_000, 15_000), "move: zone 1 follows its slice's end");
    assert_eq!(p.loop_used(1), Some(LoopMode::Forward), "move: zone 1 still loops");
    all_written(&p, "move");
    // Add: a marker at 5,000 splits slice 7, so zone 0 now ends at 5,000.
    let new = marker(&mut e, t, json!({"op": "add", "frame": 5_000}))["id"].as_u64().unwrap();
    let p = patch(&e, t);
    assert_eq!(stored(&p, 0), (0, 0, 5_000, 0, 5_000), "add: zone 0 ends at the new marker");
    all_written(&p, "add");
    // Remove: the new marker goes, slice 7 merges back, so zone 0 ends at 10,000 again.
    let out = marker(&mut e, t, json!({"op": "remove", "id": new}));
    assert_eq!(out["freed_zones"], json!([]), "remove: no zone was bound to the removed slice");
    let p = patch(&e, t);
    assert_eq!(stored(&p, 0), (0, 0, 10_000, 0, 10_000), "remove: zone 0 covers the merged slice");
    assert_eq!(stored(&p, 1), (0, 10_000, 15_000, 10_000, 15_000), "remove: zone 1 is unchanged");
    all_written(&p, "remove");
}

#[test]
fn replace_markers_writes_through_a_kept_slice_whose_next_boundary_moves() {
    let (mut e, t, _) = setup();
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(set(4, &[(1, 0), (2, 6_000), (3, 9_000)]));
        l.mode = SliceMode::Auto;
    });
    bind(&mut e, t, 0, SliceSet::Auto, 2, (6_000, 9_000), Some(0));
    // A 120 BPM 1/16 grid: 0, 6,000 ... 42,000. Id 2 stays at 6,000; its next boundary moves from
    // 9,000 (id 3, removed) to 12,000.
    let cid =
        e.execute("mix.sampler_slice_detect", &json!({"track": t, "source": "grid", "grid_bpm": 120})).unwrap()["candidate_id"].as_u64().unwrap();
    let out = one_step(&mut e, "mix.sampler_slice_replace", json!({"track": t, "candidate_id": cid}));
    assert_eq!((out["kept"].clone(), out["removed"].clone()), (json!([1, 2]), json!([3])));
    let p = patch(&e, t);
    assert_eq!(stored(&p, 0), (0, 6_000, 12_000, 6_000, 12_000), "the kept slice's zone follows its new next boundary");
    assert_eq!(p.loop_used(0), Some(LoopMode::Forward));
    all_written(&p, "replace");
}

#[test]
fn slices_to_pads_at_a_new_root_after_a_marker_add_writes_the_rebound_zones() {
    let (mut e, t, _) = setup();
    edit(&mut e, t, |p| p.slicing.as_mut().unwrap().manual = Some(set(10, &[(7, 0), (8, 12_000), (9, 24_000)])));
    one_step(&mut e, "mix.sampler_slices_to_pads", json!({"track": t, "root": 60}));
    // Pad A2 plays slice 8; it loops.
    let z = patch(&e, t).pad(1).zone.unwrap();
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": z + 1, "playback": {"mode": "gate"}, "region": {"loop_mode": "forward"}}))
        .unwrap();
    assert_eq!(stored(&patch(&e, t), z), (0, 12_000, 24_000, 12_000, 24_000), "A2's zone stores slice 8 before the run");
    let added = marker(&mut e, t, json!({"op": "add", "frame": 6_000}))["id"].as_u64().unwrap() as u32;
    // At root 61, slice 1 (id 7) lands on A2, which already plays 61: A2 is taken while occupied
    // and changes slice, keeping its zone and settings.
    let out = one_step(&mut e, "mix.sampler_slices_to_pads", json!({"track": t, "root": 61}));
    assert_eq!(out["cells"][0]["pad"], json!(2), "slice 1 is on pad A2: {out}");
    let p = patch(&e, t);
    assert_eq!(p.pad(1).zone, Some(z), "A2 keeps its zone");
    let zone = p.zone(z).unwrap();
    assert_eq!(zone.slice.as_ref().map(|b| (b.set, b.id)), Some((SliceSet::Manual, 7)));
    assert_eq!(stored(&p, z), (0, 0, 6_000, 0, 6_000), "A2's zone stores slice 1's region and loop");
    assert_eq!((zone.mode, zone.loop_mode, p.loop_used(z)), (PlayMode::Gate, LoopMode::Forward, Some(LoopMode::Forward)), "and still loops");
    // A3 plays the added slice 2.
    let z3 = p.pad(2).zone.unwrap();
    assert_eq!(p.zone(z3).unwrap().slice.as_ref().map(|b| b.id), Some(added));
    assert_eq!(stored(&p, z3), (0, 6_000, 12_000, 6_000, 12_000), "A3's zone stores slice 2's region");
    all_written(&p, "re-application");
}

#[test]
fn slices_to_pads_hidden_set_takeover_writes_the_source_region_and_loop() {
    let (mut e, t, _) = setup();
    let rate = e.session().sample_rate.hz();
    let other =
        crate::io::add_source(e.session_mut(), "other", AudioBuffer { sample_rate: rate, channels: vec![vec![0.0; 30_000]] }, None, FileFormat::Wav);
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(set(3, &[(1, 0), (2, 24_000)]));
        l.manual = Some(set(9, &[(7, 0), (8, 10_000)]));
    });
    // Zone 0 on pad A1 plays Auto's slice 2; it stores another source (as an older file could)
    // and the Auto slice's region.
    bind(&mut e, t, 0, SliceSet::Auto, 2, (24_000, 48_000), Some(0));
    edit(&mut e, t, |p| {
        let i = p.source_index(other, Ownership::Managed) as u16;
        p.zone_mut(0).unwrap().source = i;
    });
    let before = patch(&e, t);
    assert_eq!(before.sources.len(), 2);
    assert_eq!(stored(&before, 0), (1, 24_000, 48_000, 24_000, 48_000));
    // Manual showing at root 60: A1 already plays 60, so Manual slice 1 takes it from Auto.
    let out = one_step(&mut e, "mix.sampler_slices_to_pads", json!({"track": t}));
    assert_eq!(out["cells"][0]["replaces"]["set"], json!("auto"), "{out}");
    let p = patch(&e, t);
    assert_eq!(p.zone(0).unwrap().slice.as_ref().map(|b| (b.set, b.id)), Some((SliceSet::Manual, 7)));
    let loop_source = p.slicing.as_ref().unwrap().source;
    assert_eq!(stored(&p, 0), (loop_source, 0, 10_000, 0, 10_000), "the source, region and loop of Manual's slice 1");
    assert_eq!(p.sources.len(), 1, "the source no zone uses any more is dropped");
    assert_eq!(p.sources[usize::from(loop_source)].source, before.sources[0].source, "the loop's source stays");
    all_written(&p, "takeover");
}

#[test]
fn clear_writes_the_start_slice_through_and_frees_the_rest() {
    let (mut e, t, _) = setup();
    three_slices(&mut e, t);
    let out = one_step(&mut e, "mix.sampler_slice_manual_clear", json!({"track": t}));
    // The body: a removed marker frees the zones bound to its slice.
    assert_eq!((out["freed_pads"].clone(), out["freed_zones"].clone()), (json!([2]), json!([2])));
    let p = patch(&e, t);
    assert!(p.zone(1).is_none(), "zone 1's slice is gone, so it is freed");
    assert_eq!(stored(&p, 0), (0, 0, 48_000, 0, 48_000), "slice 7 is now the whole loop, and zone 0 stores it");
    all_written(&p, "clear");
}

#[test]
fn start_from_autos_markers_frees_manual_zones_and_leaves_auto_zones_as_stored() {
    let (mut e, t, _) = setup();
    three_slices(&mut e, t);
    edit(&mut e, t, |p| p.slicing.as_mut().unwrap().auto = Some(set(3, &[(1, 0), (2, 30_000)])));
    // An Auto zone whose stored fields disagree with its slice (a loaded file): Start from Auto's
    // markers changes no Auto boundary, so it is not an affected zone and stays as stored.
    bind(&mut e, t, 2, SliceSet::Auto, 2, (31_000, 40_000), Some(2));
    let out = one_step(&mut e, "mix.sampler_slice_manual_from_auto", json!({"track": t}));
    assert_eq!((out["freed_pads"].clone(), out["freed_zones"].clone()), (json!([1, 2]), json!([1, 2])), "every Manual binding is freed");
    let p = patch(&e, t);
    assert!(p.zones.iter().all(|z| z.slice.as_ref().is_none_or(|b| b.set == SliceSet::Auto)), "no zone is bound to the new Manual set");
    assert_eq!(stored(&p, 2), (0, 31_000, 40_000, 31_000, 40_000), "the Auto zone is untouched");
}

#[test]
fn a_saved_and_reopened_session_stores_the_written_regions() {
    let d = std::env::temp_dir().join(format!("sc-sampler-write-through-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let (mut e, t, _) = setup();
    three_slices(&mut e, t);
    marker(&mut e, t, json!({"op": "move", "id": 8, "frame": 4_000}));
    marker(&mut e, t, json!({"op": "add", "frame": 30_000}));
    let file = d.join("song.transit").to_string_lossy().into_owned();
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": file})).unwrap();
    let p = patch(&back, t);
    let _ = std::fs::remove_dir_all(&d);
    assert_eq!(stored(&p, 0), (0, 0, 4_000, 0, 4_000), "zone 0 as saved");
    assert_eq!(stored(&p, 1), (0, 4_000, 20_000, 4_000, 20_000), "zone 1 as saved");
    all_written(&p, "reopened");
}

#[test]
fn slice_state_reports_the_used_values_beside_the_stored_ones() {
    let (mut e, t, _) = setup();
    three_slices(&mut e, t);
    edit(&mut e, t, |p| {
        // Zone 0 (pad A1, slice 7): a crossfade that a shorter slice will clamp, and fades that a
        // pad's zone never plays.
        let z = p.zone_mut(0).unwrap();
        (z.crossfade, z.fades) = (4_000, Fades::of([1, 1, 1, 1]));
        // Zone 1 (slice 8) without a pad: fades and a crossfade that play as stored.
        let z = p.zone_mut(1).unwrap();
        (z.crossfade, z.fades) = (1_000, Fades::of([1, 2, 3, 4]));
        p.set_pad(PadEntry { zone: None, ..p.pad(1) });
    });
    // Zone 2 (pad A3, slice 9): a stored loop on a one-shot plays unlooped.
    bind(&mut e, t, 2, SliceSet::Manual, 9, (20_000, 48_000), Some(2));
    edit(&mut e, t, |p| {
        let z = p.zone_mut(2).unwrap();
        (z.mode, z.crossfade) = (PlayMode::OneShot, 10);
    });
    // Slice 7 shrinks to 6,000 frames, so 4,000 frames of crossfade play as 3,000.
    marker(&mut e, t, json!({"op": "move", "id": 8, "frame": 6_000}));
    let state = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    let zones = state["loop"]["zones"].as_array().unwrap_or_else(|| panic!("no zones: {state}")).clone();
    let of = |n: u64| zones.iter().find(|z| z["zone"] == json!(n)).unwrap_or_else(|| panic!("no zone {n}: {state}")).clone();
    let w = |f: [u8; 4]| json!({"key_lo": f[0], "key_hi": f[1], "vel_lo": f[2], "vel_hi": f[3]});
    let pick =
        |z: &Value| json!([z["loop_mode"], z["loop_used"], z["crossfade"], z["crossfade_used"], z["fades"], z["fades_used"], z["slice"]["id"]]);
    assert_eq!(pick(&of(1)), json!(["forward", "forward", 4_000, 3_000, w([1, 1, 1, 1]), w([0, 0, 0, 0]), 7]), "zone 1 (0-based 0): clamped");
    assert_eq!(pick(&of(2)), json!(["forward", "forward", 1_000, 1_000, w([1, 2, 3, 4]), w([1, 2, 3, 4]), 8]), "zone 2: as stored");
    assert_eq!(pick(&of(3)), json!(["forward", "off", 10, 0, w([0, 0, 0, 0]), w([0, 0, 0, 0]), 9]), "zone 3: a one-shot does not loop");
    assert_eq!(of(1)["region"], json!({"source": 0, "start": 0, "end": 6_000, "loop_start": 0, "loop_end": 6_000}));
    // The two queries agree, key for key, on every slice-bound zone.
    let full = e.execute("mix.sampler_state", &json!({"track": t})).unwrap();
    let all = full["zones"].as_array().unwrap();
    assert_eq!(zones.len(), 3);
    for z in &zones {
        assert_eq!(Some(z), all.iter().find(|x| x["zone"] == z["zone"]), "mix.sampler_slice_state and mix.sampler_state differ");
    }
}
