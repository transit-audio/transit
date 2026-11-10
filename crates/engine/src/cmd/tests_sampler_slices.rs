//! The slicing commands (ADR 0041 and its amendment 2026-11-08, #269). The loop's source is
//! generated audio added to the session; the Auto sets that Detect will write are set on the
//! patch directly. The document is compared whole before and after every refusal.

use super::*;
use crate::Engine;
use serde_json::{Value, json};
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_model::sampler_patch::{Marker, Markers, PadEntry, PatchV1, SliceBinding, SliceLoop, SliceMode, SliceSet, ZoneEntry};
use transit_model::{SourceId, TrackId};

/// The ADR's 48,000-frame loop: positive up to frame 11,999 and negative from 12,000, so the only
/// zero crossing is frame 12,000.
fn step(i: usize) -> f32 {
    if i < 12_000 { 0.5 } else { -0.5 }
}

/// An instrument track with an empty patch at pad start note 60 (stated: these tests were written
/// for pads on 60 and up) and a 48,000-frame source (`None`: no loop yet).
fn bare() -> (Engine, TrackId, SourceId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    let rate = e.session().sample_rate.hz();
    let buf = AudioBuffer { sample_rate: rate, channels: vec![(0..48_000).map(step).collect()] };
    let sid = crate::io::add_source(e.session_mut(), "loop", buf, None, FileFormat::Wav);
    (e, t, sid)
}

/// [`bare`] with the loop made over the whole source.
fn setup() -> (Engine, TrackId, SourceId) {
    let (mut e, t, sid) = bare();
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    (e, t, sid)
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone()
}

fn lp(e: &Engine, t: TrackId) -> SliceLoop {
    patch(e, t).slicing.unwrap()
}

/// Test setup outside the commands (what Detect and Slices to pads will write).
fn edit(e: &mut Engine, t: TrackId, f: impl FnOnce(&mut PatchV1)) {
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    f(ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap());
}

fn set(list: &[(u32, u64, Option<&str>)]) -> Markers {
    let markers = list.iter().map(|&(id, frame, label)| Marker { id, frame, label: label.map(str::to_string), ..Marker::default() }).collect();
    Markers { markers, ..Markers::default() }
}

fn frames(m: &Option<Markers>) -> Vec<u64> {
    m.as_ref().map(|m| m.markers.iter().map(|x| x.frame).collect()).unwrap_or_default()
}

fn ids(m: &Option<Markers>) -> Vec<u32> {
    m.as_ref().map(|m| m.markers.iter().map(|x| x.id).collect()).unwrap_or_default()
}

fn marker(e: &mut Engine, t: TrackId, params: Value) -> crate::Result<Value> {
    let mut p = params;
    p["track"] = json!(t);
    e.execute("mix.sampler_slice_marker", &p)
}

fn err_text(r: crate::Result<Value>) -> String {
    r.expect_err("refused").to_string()
}

fn steps(e: &Engine) -> usize {
    e.undo_history().len()
}

/// Runs a command expected to be refused and checks the document and the history are unchanged.
fn refused(e: &mut Engine, cmd: &str, params: Value) -> String {
    let (before, n) = (e.session_arc(), steps(e));
    let text = err_text(e.execute(cmd, &params));
    assert!(*e.session_arc() == *before, "{cmd} {params}: refused but the document changed");
    assert_eq!(steps(e), n, "{cmd} {params}: refused but a step was recorded");
    text
}

/// Runs a command expected to succeed as one undo step: undo restores the document whole and
/// redo reproduces the edit.
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

/// Auto [S, 24,000 "kick"] and Manual [S id 7 "s", 10,000 id 8 "x"] (next id 9), Auto showing,
/// snap off.
fn two_sets(e: &mut Engine, t: TrackId) {
    edit(e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(set(&[(1, 0, None), (2, 24_000, Some("kick"))]));
        l.manual = Some(Markers { next_id: 9, ..set(&[(7, 0, Some("s")), (8, 10_000, Some("x"))]) });
        l.mode = SliceMode::Auto;
        l.snap = false;
    });
}

/// Binds zone `zone` (0-based) over the source to slice `id` of `set`, on pad `pad` (0-based).
fn bind(e: &mut Engine, t: TrackId, zone: u16, set: SliceSet, id: u32, pad: u8) {
    edit(e, t, |p| {
        let mut z = ZoneEntry::whole(zone, 0, 48_000);
        z.slice = Some(SliceBinding { set, id, ..SliceBinding::default() });
        p.zones.push(z);
        p.zones.sort_by_key(|z| z.id);
        p.set_pad(PadEntry { zone: Some(zone), ..p.pad(pad) });
    });
}

#[test]
fn slice_commands_are_atomic() {
    let (mut e, t, sid) = setup();
    // A second track with a patch and no loop.
    let (mut f, u, fsid) = bare();
    for (params, why) in [
        (json!({"track": u, "source": 999}), "no source 999"),
        (json!({"track": u, "source": fsid, "start": 10, "end": 10}), "the loop is empty"),
        (json!({"track": u, "source": fsid, "end": 48_001}), "past the source's 48000 frames"),
        (json!({"track": u, "source": fsid, "root": 128}), "`root` must be 0-127"),
    ] {
        assert!(refused(&mut f, "mix.sampler_slice_loop", params.clone()).contains(why), "{params}");
    }
    for cmd in [
        "mix.sampler_slice_mode",
        "mix.sampler_slice_marker",
        "mix.sampler_slice_manual_from_auto",
        "mix.sampler_slice_manual_clear",
        "mix.sampler_slice_set",
    ] {
        let text = refused(&mut f, cmd, json!({"track": u, "mode": "manual", "op": "add", "frame": 5, "snap": true}));
        assert!(text.contains("no slice loop"), "{cmd}: {text}");
    }
    assert!(refused(&mut e, "mix.sampler_slice_loop", json!({"track": t, "source": sid})).contains("one loop per patch"));
    two_sets(&mut e, t);
    let refusals = [
        ("mix.sampler_slice_mode", json!({"mode": "both"}), "`mode` must be auto or manual"),
        ("mix.sampler_slice_marker", json!({"op": "split", "frame": 5}), "`op` must be"),
        ("mix.sampler_slice_marker", json!({"op": "add", "frame": 24_000}), "Cannot add slice: too close to marker 2"),
        ("mix.sampler_slice_marker", json!({"op": "add", "frame": 0}), "Cannot add slice: too close to marker 1"),
        ("mix.sampler_slice_marker", json!({"op": "add", "frame": 48_000}), "Cannot add slice: outside the loop"),
        ("mix.sampler_slice_marker", json!({"op": "add", "frame": -3}), "Cannot add slice: outside the loop"),
        ("mix.sampler_slice_marker", json!({"op": "move", "id": 9, "frame": 5}), "No slice 9 in this set"),
        ("mix.sampler_slice_marker", json!({"op": "remove", "id": 9}), "No slice 9 in this set"),
        ("mix.sampler_slice_marker", json!({"op": "label", "id": 2, "label": "x".repeat(65)}), "at most 64 characters"),
        ("mix.sampler_slice_marker", json!({"op": "label", "id": 2}), "`label` required"),
        ("mix.sampler_slice_set", json!({"bars": 2}), "`bars` travels with Detect"),
        ("mix.sampler_slice_set", json!({"grid_bpm": 96}), "`grid_bpm` travels with Detect"),
        ("mix.sampler_slice_set", json!({"snap": true, "root": 128}), "`root` must be 0-127"),
        ("mix.sampler_slice_set", json!({"beat_length": 0}), "`beat_length` must be"),
    ];
    for (cmd, mut params, why) in refusals {
        params["track"] = json!(t);
        let text = refused(&mut e, cmd, params.clone());
        assert!(text.contains(why), "{cmd} {params}: {text}");
    }
    // Manual showing: S cannot move or go; the snap needs the source.
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    for (params, why) in [
        (json!({"track": t, "op": "move", "id": 7, "frame": 500}), "The loop start cannot move."),
        (json!({"track": t, "op": "remove", "id": 7}), "The loop start cannot be removed."),
    ] {
        assert!(refused(&mut e, "mix.sampler_slice_marker", params.clone()).contains(why), "{params}");
    }
    // Every success is one step.
    let (mut g, v, gsid) = bare();
    one_step(&mut g, "mix.sampler_slice_loop", json!({"track": v, "source": gsid, "start": 100, "end": 40_000, "root": 36}));
    for params in [
        json!({"op": "add", "frame": 30_000}),
        json!({"op": "move", "id": 2, "frame": 20_000}),
        json!({"op": "remove", "id": 2}),
        json!({"op": "label", "id": 2, "label": " snare "}),
        json!({"op": "move", "id": 1, "frame": 20_000}),
    ] {
        // Each from Auto showing, so the ids are Auto's.
        e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "auto"})).unwrap();
        let mut p = params;
        p["track"] = json!(t);
        one_step(&mut e, "mix.sampler_slice_marker", p);
    }
    one_step(&mut e, "mix.sampler_slice_manual_from_auto", json!({"track": t}));
    one_step(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "add", "frame": 40_000}));
    one_step(&mut e, "mix.sampler_slice_manual_clear", json!({"track": t}));
    one_step(&mut e, "mix.sampler_slice_set", json!({"track": t, "snap": true, "root": 48, "beat_length": 8}));
    one_step(&mut e, "mix.sampler_slice_set", json!({"track": t, "beat_length": null}));
}

#[test]
fn auto_edit_copies_to_manual_in_one_undo() {
    let (mut e, t, _) = setup();
    two_sets(&mut e, t);
    let before = lp(&e, t);
    let out = one_step(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "add", "frame": 30_000}));
    assert_eq!((out["copied_auto"].clone(), out["mode"].clone(), out["fixed"].clone()), (json!(true), json!("manual"), json!(false)));
    let after = lp(&e, t);
    assert_eq!(after.mode, SliceMode::Manual);
    assert_eq!(frames(&after.manual), [0, 24_000, 30_000], "Manual is the copy plus the marker");
    assert_eq!(after.manual.as_ref().unwrap().markers[1].label.as_deref(), Some("kick"), "labels travel with the copy");
    assert_eq!(after.auto, before.auto, "Auto never changes");
    e.execute("edit.undo", &json!({})).unwrap();
    let undone = lp(&e, t);
    assert_eq!((undone.mode, &undone.manual), (SliceMode::Auto, &before.manual), "one undo: Auto showing, Manual exactly as before");
    e.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(lp(&e, t), after);
}

#[test]
fn touching_s_in_auto_copies_and_switches_but_s_stays() {
    for (op, params) in [("move", json!({"op": "move", "id": 1, "frame": 5_000})), ("remove", json!({"op": "remove", "id": 1}))] {
        let (mut e, t, _) = setup();
        two_sets(&mut e, t);
        let auto = lp(&e, t).auto;
        let mut p = params;
        p["track"] = json!(t);
        let out = one_step(&mut e, "mix.sampler_slice_marker", p);
        assert_eq!((out["fixed"].clone(), out["copied_auto"].clone()), (json!(true), json!(true)), "{op}: {out}");
        let l = lp(&e, t);
        assert_eq!((l.mode, frames(&l.manual)), (SliceMode::Manual, vec![0, 24_000]), "{op}: copied and switched, S stays at the loop start");
        assert_eq!(l.auto, auto, "{op}");
        assert_eq!(out["id"], json!(l.manual.as_ref().unwrap().markers[0].id), "{op}: the result names S's Manual copy");
    }
}

#[test]
fn touching_s_in_manual_is_refused_unchanged() {
    let (mut e, t, _) = setup();
    two_sets(&mut e, t);
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    let text = refused(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "move", "id": 7, "frame": 500}));
    assert!(text.contains("The loop start cannot move."), "{text}");
    let text = refused(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "remove", "id": 7}));
    assert!(text.contains("The loop start cannot be removed."), "{text}");
}

#[test]
fn refused_add_in_auto_changes_nothing() {
    let (mut e, t, _) = setup();
    two_sets(&mut e, t);
    for (params, why) in [
        (json!({"track": t, "op": "add", "frame": 24_000}), "too close to marker 2"),
        (json!({"track": t, "op": "add", "frame": 50_000}), "outside the loop"),
    ] {
        assert!(refused(&mut e, "mix.sampler_slice_marker", params.clone()).contains(why), "{params}");
    }
    // The snap's source is missing: refused, no copy, no switch.
    let gone = patch(&e, t).sources[0].source;
    e.session_mut().pool.remove(gone);
    let text = refused(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "add", "frame": 30_000, "snap": true}));
    assert!(text.contains("The source audio is not available to snap to."), "{text}");
    assert_eq!(lp(&e, t).mode, SliceMode::Auto);
}

#[test]
fn auto_marker_gesture_applies_to_the_manual_copy() {
    let (mut e, t, _) = setup();
    two_sets(&mut e, t);
    let out = marker(&mut e, t, json!({"op": "move", "id": 2, "frame": 30_000})).unwrap();
    let l = lp(&e, t);
    let copy = &l.manual.as_ref().unwrap().markers[1];
    assert_eq!((copy.frame, copy.label.as_deref()), (30_000, Some("kick")), "the Manual copy of Auto's marker 2 moved");
    assert_eq!(out["id"], json!(copy.id), "the result names the Manual id");
    assert_ne!(copy.id, 2, "the copy has a new Manual id");
    assert_eq!(frames(&l.auto), [0, 24_000], "Auto's own marker did not move");
    // In Manual now: a label aimed at that Manual id edits it.
    marker(&mut e, t, json!({"op": "label", "id": copy.id, "label": null})).unwrap();
    assert_eq!(lp(&e, t).manual.unwrap().markers[1].label, None);
}

#[test]
fn grid_bpm_round_trips_and_is_range_checked() {
    let (mut e, t, _) = setup();
    let raw = |p: &PatchV1| serde_json::to_value(p).unwrap();
    assert!(raw(&patch(&e, t))["loop"].get("grid_bpm").is_none(), "absent when unset");
    for ok in [5.0, 96.5, 1000.0] {
        edit(&mut e, t, |p| p.slicing.as_mut().unwrap().grid_bpm = Some(ok));
        let v = raw(&patch(&e, t));
        assert_eq!(v["loop"]["grid_bpm"], json!(ok));
        let back = transit_model::SamplerPatch::from_value(v);
        assert_eq!(back.ready().and_then(|p| p.slicing.as_ref()).and_then(|l| l.grid_bpm), Some(ok), "{ok} round-trips");
        assert!(transit_mix::check_sampler_patch(&patch(&e, t), e.session()).is_ok());
    }
    for bad_bpm in [4.9, 1000.1, f64::NAN] {
        let mut p = patch(&e, t);
        p.slicing.as_mut().unwrap().grid_bpm = Some(bad_bpm);
        assert!(transit_mix::check_sampler_patch(&p, e.session()).unwrap_err().contains("grid tempo"), "{bad_bpm} refused on store");
        let mut v = raw(&patch(&e, t));
        v["loop"]["grid_bpm"] = json!(bad_bpm);
        if bad_bpm.is_finite() {
            assert!(transit_model::SamplerPatch::from_value(v).ready().is_none(), "{bad_bpm} is kept opaque on load");
        }
    }
    // The state reports it.
    let st = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    assert_eq!(st["loop"]["grid_bpm"], json!(1000.0));
    assert_eq!(st["loop"]["grid_tempo"], json!({"bpm": 1000.0, "origin": "grid_bpm"}));
}

#[test]
fn grid_tempo_follows_the_priority_chain() {
    let base = SliceLoop { end: 48_000, ..SliceLoop::default() };
    let at = Some(AtTempo { bpm: 120.0, meter: (4, 4) });
    // Bars wins and reads the meter at the placement: 2 bars of 4/4 over one second is 480 BPM,
    // the same derived tempo as the model's grid.
    let bars = SliceLoop { bars: Some(2), grid_bpm: Some(96.0), ..base.clone() };
    let grid =
        transit_model::slicing::grid_boundaries(0, 48_000, 48_000, 1.0, transit_model::sampler_patch::Division::Quarter, (4, 4), Some(2)).unwrap();
    assert_eq!(grid_tempo(&bars, 48_000, Some(100.0), at), Ok((grid.bpm, TempoOrigin::Bars)));
    assert_eq!(grid_tempo(&bars, 48_000, Some(100.0), None), Err(NEEDS_AT));
    let bpm = SliceLoop { grid_bpm: Some(96.0), ..base.clone() };
    assert_eq!(grid_tempo(&bpm, 48_000, Some(100.0), None), Ok((96.0, TempoOrigin::GridBpm)));
    assert_eq!(grid_tempo(&base, 48_000, Some(100.0), None), Ok((100.0, TempoOrigin::Acid)));
    for unusable in [Some(4.0), Some(1000.5), Some(f64::NAN), None] {
        assert_eq!(grid_tempo(&base, 48_000, unusable, at), Ok((120.0, TempoOrigin::Session)), "{unusable:?}");
        assert_eq!(grid_tempo(&base, 48_000, unusable, None), Err(NEEDS_AT), "{unusable:?}");
    }
    // The default beat length ignores Bars and grid_bpm: the loop's second at acid 120 is two
    // quarter notes, at a session 60 one.
    let both = SliceLoop { bars: Some(2), grid_bpm: Some(96.0), ..base };
    assert_eq!(default_beat_length(&both, 48_000, Some(120.0), None), Some(2.0));
    assert_eq!(default_beat_length(&both, 48_000, None, Some(AtTempo { bpm: 60.0, meter: (4, 4) })), Some(1.0));
    assert_eq!(default_beat_length(&both, 48_000, None, None), None);
}

#[test]
fn marker_ids_are_stable_across_moves_and_never_reused() {
    let (mut e, t, _) = setup();
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    e.execute("mix.sampler_slice_set", &json!({"track": t, "snap": false})).unwrap();
    let mut added = Vec::new();
    for f in [10_000, 20_000, 30_000] {
        added.push(marker(&mut e, t, json!({"op": "add", "frame": f})).unwrap()["id"].as_u64().unwrap() as u32);
    }
    let before = ids(&lp(&e, t).manual);
    let out = marker(&mut e, t, json!({"op": "move", "id": added[1], "frame": 25_000})).unwrap();
    assert_eq!(out["id"], json!(added[1]));
    assert_eq!(ids(&lp(&e, t).manual), before, "a move keeps every id");
    // A move past a neighbor is clamped one frame short of it and says which.
    let out = marker(&mut e, t, json!({"op": "move", "id": added[1], "frame": 45_000})).unwrap();
    assert_eq!((out["frame"].clone(), out["clamped"]["at_marker"].clone()), (json!(29_999), json!(added[2])));
    assert_eq!(out["clamped"]["text"], json!("clamped at marker 4"));
    let out = marker(&mut e, t, json!({"op": "move", "id": added[2], "frame": 60_000})).unwrap();
    assert_eq!((out["frame"].clone(), out["clamped"]["at_end"].clone()), (json!(47_999), json!(true)));
    let top = *added.iter().max().unwrap();
    marker(&mut e, t, json!({"op": "remove", "id": top})).unwrap();
    let again = marker(&mut e, t, json!({"op": "add", "frame": 40_000})).unwrap()["id"].as_u64().unwrap() as u32;
    assert!(again > top, "a removed id never returns ({again} after removing {top})");
}

#[test]
fn copy_creates_new_manual_ids_and_keeps_labels() {
    let (mut e, t, _) = setup();
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(set(&[(1, 0, Some("a")), (2, 10_000, Some("b"))]));
        l.manual = Some(Markers { next_id: 4, ..set(&[(1, 0, None), (3, 5_000, None)]) });
    });
    let out = one_step(&mut e, "mix.sampler_slice_manual_from_auto", json!({"track": t}));
    let l = lp(&e, t);
    let m = l.manual.as_ref().unwrap();
    assert_eq!(frames(&l.manual), frames(&l.auto));
    assert_eq!(m.markers.iter().map(|x| x.label.as_deref()).collect::<Vec<_>>(), [Some("a"), Some("b")]);
    assert!(m.markers.iter().all(|x| x.id >= 4), "new Manual ids, never an old one: {:?}", ids(&l.manual));
    assert_eq!((l.mode, out["mode"].clone()), (SliceMode::Manual, json!("manual")));
}

#[test]
fn clear_keeps_s_only_and_leaves_auto() {
    let (mut e, t, _) = setup();
    two_sets(&mut e, t);
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    let before = lp(&e, t);
    one_step(&mut e, "mix.sampler_slice_manual_clear", json!({"track": t}));
    let l = lp(&e, t);
    let m = l.manual.as_ref().unwrap();
    assert_eq!(
        (m.markers.len(), m.markers[0].id, m.markers[0].frame, m.markers[0].label.as_deref()),
        (1, 7, 0, Some("s")),
        "S keeps its id and label"
    );
    assert_eq!((l.auto, l.mode), (before.auto, SliceMode::Manual));
    // Clearing a set that is S only changes nothing and records no step.
    let n = steps(&e);
    e.execute("mix.sampler_slice_manual_clear", &json!({"track": t})).unwrap();
    assert_eq!(steps(&e), n);
}

#[test]
fn sixty_fifth_slice_refused_with_exact_text_and_set_unchanged() {
    let (mut e, t, _) = setup();
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    e.execute("mix.sampler_slice_set", &json!({"track": t, "snap": false})).unwrap();
    for k in 1..64u64 {
        marker(&mut e, t, json!({"op": "add", "frame": k * 100})).unwrap();
    }
    assert_eq!(lp(&e, t).manual.unwrap().markers.len(), 64);
    let text = refused(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "add", "frame": 30_000}));
    assert!(text.ends_with("Cannot add slice: 64 slices per loop"), "{text}");
    // A 64-slice Auto copies whole, and the 65th add on the copy refuses the same way.
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(Markers {
            markers: (0..64u64).map(|k| Marker { id: k as u32 + 1, frame: k * 500, ..Marker::default() }).collect(),
            ..Markers::default()
        });
        l.mode = SliceMode::Auto;
    });
    let text = refused(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "add", "frame": 40_000}));
    assert!(text.ends_with("Cannot add slice: 64 slices per loop"), "{text}: refused in Auto with no copy");
    e.execute("mix.sampler_slice_manual_from_auto", &json!({"track": t})).unwrap();
    assert_eq!(lp(&e, t).manual.unwrap().markers.len(), 64);
    let text = refused(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "add", "frame": 40_000}));
    assert!(text.ends_with("Cannot add slice: 64 slices per loop"), "{text}");
}

#[test]
fn remove_frees_only_that_sets_bindings() {
    let (mut e, t, _) = setup();
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.auto = Some(set(&[(1, 0, None), (2, 24_000, None)]));
        l.manual = Some(set(&[(1, 0, None), (2, 12_000, None)]));
        l.mode = SliceMode::Manual;
        l.snap = false;
    });
    bind(&mut e, t, 0, SliceSet::Auto, 2, 0);
    bind(&mut e, t, 1, SliceSet::Manual, 2, 1);
    let out = one_step(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "remove", "id": 2}));
    assert_eq!((out["freed_pads"].clone(), out["freed_zones"].clone(), out["merged_into"].clone()), (json!([2]), json!([2]), json!(1)));
    let p = patch(&e, t);
    assert!(p.zone(1).is_none() && p.pad(1).zone.is_none(), "the Manual slice's zone and pad are freed");
    assert_eq!(p.zone(0).and_then(|z| z.slice.clone()).map(|b| (b.set, b.id)), Some((SliceSet::Auto, 2)), "the Auto binding with the same id stays");
    assert_eq!(p.pad(0).zone, Some(0));
    e.execute("edit.undo", &json!({})).unwrap();
    // In Auto, removing Auto's marker 2 edits its Manual copy: Auto and its binding stay; the
    // replaced Manual set's binding is freed and reported.
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "auto"})).unwrap();
    let out = one_step(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "remove", "id": 2}));
    assert_eq!(out["freed_zones"], json!([2]));
    let p = patch(&e, t);
    assert_eq!(frames(&p.slicing.as_ref().unwrap().auto), [0, 24_000]);
    assert_eq!(p.zone(0).and_then(|z| z.slice.clone()).map(|b| (b.set, b.id)), Some((SliceSet::Auto, 2)));
}

#[test]
fn snap_uses_the_source_and_refuses_without_it() {
    let (mut e, t, _) = setup();
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    let out = marker(&mut e, t, json!({"op": "add", "frame": 6_000, "snap": false})).unwrap();
    let id = out["id"].clone();
    assert_eq!((out["frame"].clone(), out["snapped"].clone()), (json!(6_000), json!(false)));
    let out = marker(&mut e, t, json!({"op": "move", "id": id, "frame": 12_100})).unwrap();
    assert_eq!(
        (out["id"].clone(), out["frame"].clone(), out["requested"].clone(), out["snapped"].clone()),
        (id.clone(), json!(12_000), json!(12_100), json!(true))
    );
    let out = marker(&mut e, t, json!({"op": "move", "id": id, "frame": 12_100, "snap": false})).unwrap();
    assert_eq!(out["frame"], json!(12_100));
    let out = marker(&mut e, t, json!({"op": "move", "id": id, "frame": 12_241})).unwrap();
    assert_eq!(out["frame"], json!(12_241), "no crossing within 5 ms: the requested frame");
    let out = marker(&mut e, t, json!({"op": "add", "frame": 11_900})).unwrap();
    assert_eq!(out["frame"], json!(12_000), "an add snaps too");
    let gone = patch(&e, t).sources[0].source;
    e.session_mut().pool.remove(gone);
    let text = refused(&mut e, "mix.sampler_slice_marker", json!({"track": t, "op": "move", "id": id, "frame": 20_000}));
    assert!(text.contains("The source audio is not available to snap to."), "{text}");
    assert_eq!(
        marker(&mut e, t, json!({"op": "move", "id": id, "frame": 20_000, "snap": false})).unwrap()["frame"],
        json!(20_000),
        "snap off needs no audio"
    );
}

/// A scratch folder per test.
struct Dir(std::path::PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("sc-sampler-slices-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reopen_restores_both_sets() {
    let d = Dir::new("reopen");
    let (mut e, t, _) = setup();
    // A new loop writes only its region and mode: every setting is absent at its default.
    let v = serde_json::to_value(patch(&e, t)).unwrap();
    let mut keys: Vec<&String> = v["loop"].as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["end", "mode", "source", "start"]);
    two_sets(&mut e, t);
    marker(&mut e, t, json!({"op": "add", "frame": 30_000})).unwrap();
    let s_id = lp(&e, t).manual.unwrap().markers[0].id;
    marker(&mut e, t, json!({"op": "label", "id": s_id, "label": "start"})).unwrap();
    edit(&mut e, t, |p| p.slicing.as_mut().unwrap().grid_bpm = Some(87.5));
    let want = lp(&e, t);
    let file = d.path("song.transit");
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": file})).unwrap();
    assert_eq!(lp(&back, t), want, "both sets, ids, labels, mode and settings, as saved");
}

#[test]
fn old_single_marker_set_still_loads_as_manual() {
    let d = Dir::new("old");
    let (mut e, t, _) = setup();
    edit(&mut e, t, |p| p.slicing.as_mut().unwrap().manual = Some(set(&[(4, 0, None), (6, 9_000, Some("snare"))])));
    bind(&mut e, t, 0, SliceSet::Manual, 6, 0);
    let file = d.path("old.transit");
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    // Rewrite the loop as a build without the two sets wrote it: one `markers` list.
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let mut found = false;
    let mut stack = vec![&mut v];
    while let Some(x) = stack.pop() {
        if let Some(l) = x.get_mut("sampler_patch").and_then(|p| p.get_mut("loop")).and_then(Value::as_object_mut) {
            let manual = l.remove("manual").unwrap();
            l.insert("markers".into(), manual["markers"].clone());
            l.remove("mode");
            found = true;
            break;
        }
        match x {
            Value::Object(o) => stack.extend(o.values_mut()),
            Value::Array(a) => stack.extend(a.iter_mut()),
            _ => {}
        }
    }
    assert!(found);
    std::fs::write(&file, serde_json::to_string(&v).unwrap()).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": file})).unwrap();
    let p = patch(&back, t);
    let l = p.slicing.as_ref().unwrap();
    assert_eq!((l.mode, ids(&l.manual), frames(&l.manual), l.auto.is_none()), (SliceMode::Manual, vec![4, 6], vec![0, 9_000], true));
    assert_eq!(p.zone(0).and_then(|z| z.slice.clone()).map(|b| (b.set, b.id)), Some((SliceSet::Manual, 6)), "the binding is intact");
    let st = back.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    assert_eq!(st["loop"]["manual"][1]["pad"], json!(1));
}

#[test]
fn mode_switch_changes_neither_set() {
    let (mut e, t, _) = setup();
    // The first selection of Manual starts it with S only (absent, nothing written).
    one_step(&mut e, "mix.sampler_slice_mode", json!({"track": t, "mode": "manual"}));
    let l = lp(&e, t);
    assert_eq!((l.mode, l.manual.clone(), l.auto.clone()), (SliceMode::Manual, None, None));
    let st = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    assert_eq!(st["loop"]["manual"], json!([{"id": null, "frame": 0, "end": 48_000, "label": null, "pad": null, "key": null, "zones": []}]));
    two_sets(&mut e, t);
    let before = lp(&e, t);
    for mode in ["manual", "auto", "manual"] {
        one_step(&mut e, "mix.sampler_slice_mode", json!({"track": t, "mode": mode}));
        let l = lp(&e, t);
        assert_eq!((&l.auto, &l.manual), (&before.auto, &before.manual), "{mode}");
    }
    let n = steps(&e);
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    assert_eq!(steps(&e), n, "the mode already showing records no step");
}

#[test]
fn state_reports_mapping_status() {
    let (mut e, t, sid) = setup();
    let map = |e: &mut Engine| e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap()["loop"].clone();
    edit(&mut e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.manual = Some(set(&[(1, 0, None), (2, 16_000, None), (3, 32_000, Some("hat"))]));
        l.mode = SliceMode::Manual;
    });
    assert_eq!(map(&mut e)["mapping"], json!("none"));
    for (zone, id) in [(0u16, 1u32), (1, 2), (2, 3)] {
        bind(&mut e, t, zone, SliceSet::Manual, id, zone as u8);
    }
    let st = map(&mut e);
    assert_eq!((st["mapping"].clone(), st["mapping_root"].clone()), (json!("ascending"), json!(60)));
    assert_eq!(st["manual"][2], json!({"id": 3, "frame": 32_000, "end": 48_000, "label": "hat", "pad": 3, "key": 62, "zones": [3]}));
    assert_eq!((st["source"].clone(), st["mode"].clone(), st["root"].clone()), (json!(sid), json!("manual"), json!(60)));
    // A manual pad swap through the pad commands breaks the order.
    for (cmd, params) in [
        ("mix.sampler_pad_unbind", json!({"track": t, "pad": 1})),
        ("mix.sampler_pad_unbind", json!({"track": t, "pad": 2})),
        ("mix.sampler_pad_bind", json!({"track": t, "pad": 1, "zone": 2})),
        ("mix.sampler_pad_bind", json!({"track": t, "pad": 2, "zone": 1})),
    ] {
        e.execute(cmd, &params).unwrap();
    }
    let st = map(&mut e);
    assert_eq!(st["mapping"], json!("stale"));
    assert_eq!(st["out_of_order"], json!([{"slice": 1, "id": 1, "pad": 2, "key": 61}, {"slice": 2, "id": 2, "pad": 1, "key": 60}]));
    // The grid tempo and the default beat length read the session at the placement only.
    assert_eq!((st["grid_tempo"].clone(), st["default_beat_length"].clone()), (Value::Null, Value::Null));
    let at = e.execute("mix.sampler_slice_state", &json!({"track": t, "at": 0})).unwrap()["loop"].clone();
    let bpm = e.session().tempo.tempo_at_tick(0);
    assert_eq!(at["grid_tempo"], json!({"bpm": bpm, "origin": "session"}));
    let rate = f64::from(e.session().sample_rate.hz());
    assert_eq!(at["default_beat_length"], json!(48_000.0 / rate * bpm / 60.0));
}
