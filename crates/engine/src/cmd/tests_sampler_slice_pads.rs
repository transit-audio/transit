//! Slices to pads and Map slices to keys (ADR 0041, amendment 2026-11-08 item 6, #269). The loop's
//! source is generated audio added to the session; marker sets and loose zones are set on the
//! patch directly. Every refusal is checked against the whole document and the undo history.

use crate::Engine;
use serde_json::{Value, json};
use transit_audio_io::{AudioBuffer, FileFormat};
use transit_midi::{Note, Sequence};
use transit_model::sampler_patch::{Marker, Markers, PadEntry, PatchV1, PlayMode, SliceBinding, SliceMode, SliceSet, ZoneEntry};
use transit_model::{Clip, TrackId};
use transit_time::{Range, TICKS_PER_QUARTER};

const LEN: usize = 48_000;

/// The loop source: a slow sine plus a ramp, so every slice region differs from every other.
fn wave(i: usize) -> f32 {
    0.4 * (i as f32 * 0.013).sin() + 0.1 * (i as f32 / LEN as f32)
}

/// An instrument track with an empty patch and a slice loop over a 48,000-frame source.
fn setup() -> (Engine, TrackId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    // A new patch starts its pads at 36; these tests were written for pads from 60.
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    let rate = e.session().sample_rate.hz();
    let buf = AudioBuffer { sample_rate: rate, channels: vec![(0..LEN).map(wave).collect()] };
    let sid = crate::io::add_source(e.session_mut(), "loop", buf, None, FileFormat::Wav);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    (e, t)
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone()
}

/// Test setup outside the commands.
fn edit(e: &mut Engine, t: TrackId, f: impl FnOnce(&mut PatchV1)) {
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    f(ins.sampler_patch.as_mut().unwrap().ready_mut().unwrap());
}

/// `n` evenly spaced slices with ids 1..=n.
fn even(n: u64) -> Markers {
    let markers = (0..n).map(|k| Marker { id: k as u32 + 1, frame: k * LEN as u64 / n, ..Marker::default() }).collect();
    Markers { markers, next_id: n as u32 + 1, ..Markers::default() }
}

/// Manual holds `n` even slices and shows.
fn manual(e: &mut Engine, t: TrackId, n: u64) {
    edit(e, t, |p| {
        let l = p.slicing.as_mut().unwrap();
        l.manual = Some(even(n));
        l.mode = SliceMode::Manual;
    });
}

/// Auto holds `n` even slices (the mode is left as it is).
fn auto(e: &mut Engine, t: TrackId, n: u64) {
    edit(e, t, |p| p.slicing.as_mut().unwrap().auto = Some(even(n)));
}

/// Zone `zone` (0-based) over the source on pad `pad` (0-based), bound to `slice` when given.
fn zone_on_pad(e: &mut Engine, t: TrackId, zone: u16, pad: u8, slice: Option<(SliceSet, u32)>) {
    edit(e, t, |p| {
        let mut z = ZoneEntry::whole(zone, 0, LEN as u64);
        z.slice = slice.map(|(set, id)| SliceBinding { set, id, ..SliceBinding::default() });
        p.zones.push(z);
        p.zones.sort_by_key(|z| z.id);
        p.set_pad(PadEntry { zone: Some(zone), ..p.pad(pad) });
    });
}

fn steps(e: &Engine) -> usize {
    e.undo_history().len()
}

/// Runs a command expected to be refused: the document and the history are unchanged.
fn refused(e: &mut Engine, cmd: &str, params: Value) -> String {
    let (before, n) = (e.session_arc(), steps(e));
    let text = e.execute(cmd, &params).expect_err("refused").to_string();
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

fn to_pads(e: &mut Engine, t: TrackId, root: Option<u8>) -> Value {
    let mut p = json!({"track": t});
    if let Some(r) = root {
        p["root"] = json!(r);
    }
    one_step(e, "mix.sampler_slices_to_pads", p)
}

/// Every pad with a slice-bound zone: (1-based pad, trigger note, set, slice id), in pad order.
fn bound(e: &Engine, t: TrackId) -> Vec<(u8, u8, SliceSet, u32)> {
    let p = patch(e, t);
    (0..64u8)
        .filter_map(|q| {
            let pad = p.pad(q);
            let b = pad.zone.and_then(|z| p.zone(z)).and_then(|z| z.slice.clone())?;
            Some((q + 1, p.pad_note(q), b.set, b.id))
        })
        .collect()
}

fn run(root: u8, pads: &[u8], set: SliceSet) -> Vec<(u8, u8, SliceSet, u32)> {
    pads.iter().enumerate().map(|(i, q)| (*q, root + i as u8, set, i as u32 + 1)).collect()
}

fn notes_unique(p: &PatchV1) -> bool {
    let mut seen = [false; 128];
    (0..64u8).all(|q| !std::mem::replace(&mut seen[usize::from(p.pad_note(q))], true))
}

fn cell_replaces(out: &Value) -> Vec<Value> {
    out["cells"].as_array().unwrap().iter().map(|c| c["replaces"].clone()).collect()
}

#[test]
fn slices_map_ascending_from_root() {
    for root in [60u8, 36] {
        let (mut e, t) = setup();
        manual(&mut e, t, 8);
        let out = to_pads(&mut e, t, Some(root));
        assert_eq!(bound(&e, t), run(root, &[1, 2, 3, 4, 5, 6, 7, 8], SliceSet::Manual), "root {root}");
        assert_eq!((out["ok"].clone(), out["count"].clone(), out["root"].clone()), (json!(true), json!(8), json!(root)));
        let p = patch(&e, t);
        assert_eq!(p.slicing.as_ref().unwrap().root, Some(root), "the run stores its root");
        for k in 0..8u8 {
            let z = p.zone(p.pad(k).zone.unwrap()).unwrap();
            let (start, end) = (u64::from(k) * 6_000, u64::from(k) * 6_000 + 6_000);
            assert_eq!((z.start, z.end, z.source), (start, end, 0), "a new zone covers its slice");
            assert_eq!((z.mode, z.key_tracking, z.root), (PlayMode::OneShot, false, root + k), "new-pad defaults");
        }
        assert_eq!(super::mapping_status(&p, SliceSet::Manual), super::MappingStatus::Ascending { root });
    }
    // Without a root the run uses the loop's root, so slice 1 lands on pad A1.
    let (mut e, t) = setup();
    manual(&mut e, t, 3);
    to_pads(&mut e, t, None);
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3], SliceSet::Manual));
}

#[test]
fn shortage_refused_not_truncated() {
    let (mut e, t) = setup();
    manual(&mut e, t, 6);
    for q in 0..60u8 {
        zone_on_pad(&mut e, t, u16::from(q), q, None);
    }
    // Notes 120..125: pads A61..A64 hold 120..123 (empty), and no free pad is left for 124, 125.
    let text = refused(&mut e, "mix.sampler_slices_to_pads", json!({"track": t, "root": 120}));
    assert!(text.ends_with("Slices to pads needs 6 pads; 4 are free."), "{text}");
    let before = e.session_arc();
    let plan = e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 120, "preview": true})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()), "a preview edits nothing");
    assert_eq!((plan["ok"].clone(), plan["reason"].clone()), (json!(false), json!("Slices to pads needs 6 pads; 4 are free.")));
    assert_eq!(plan["pads_after"], json!(66), "occupied pads would pass 64");
}

#[test]
fn pad_assignment_follows_the_amendment_steps() {
    // Edited notes: pad A3 plays 40, pad A5 (note 64) is bound to Manual slice 1.
    let edited = || {
        let (mut e, t) = setup();
        manual(&mut e, t, 4);
        e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 3, "note": 40})).unwrap();
        zone_on_pad(&mut e, t, 10, 4, Some((SliceSet::Manual, 1)));
        (e, t)
    };
    // Root 62: no pad holds 62, so slice 1 takes the lowest free pad A1, whose note becomes 62
    // (step 3); A4 (63) and A6 (65) are empty and A5 (64) is slice-bound, so they take slices 2
    // to 4 on their own notes (step 2).
    let (mut e, t) = edited();
    let out = to_pads(&mut e, t, Some(62));
    assert_eq!(
        bound(&e, t),
        vec![(1, 62, SliceSet::Manual, 1), (4, 63, SliceSet::Manual, 2), (5, 64, SliceSet::Manual, 3), (6, 65, SliceSet::Manual, 4)]
    );
    let p = patch(&e, t);
    assert_eq!((p.pad(0).note, p.pad(4).zone), (Some(62), Some(10)), "a step-3 pad takes the note; a taken occupied pad keeps its zone");
    assert!(notes_unique(&p));
    assert_eq!(cell_replaces(&out), [Value::Null, Value::Null, json!({"set": "manual", "slice": 1, "id": 1}), Value::Null]);
    // Root 36: no pad holds 36..39, so the lowest free pads take them (A3 is free: empty, note
    // 40), and A5, bound to the showing set and not taken, is unbound with its note kept.
    let (mut e, t) = edited();
    let out = to_pads(&mut e, t, Some(36));
    assert_eq!(bound(&e, t), run(36, &[1, 2, 3, 4], SliceSet::Manual));
    let p = patch(&e, t);
    assert!(notes_unique(&p));
    assert_eq!((p.pad(4).zone, p.pad_note(4), p.zone(10)), (None, 64, None), "unbound: the pad empties, its note stays");
    assert_eq!(out["unbound"], json!([{"pad": 5, "note": 64, "slice": 1, "id": 1, "zone": 11}]));
    // Default notes, a run without a root (it follows the start note, 60 here): every slice finds
    // its note on an empty pad (step 2), and step 2 leaves the pad's note as it was.
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, None);
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3, 4], SliceSet::Manual));
    assert!((0..4u8).all(|q| patch(&e, t).pad(q).note.is_none()), "step 2 leaves the pad's note as it was");
    // The same run with root 60 stores its root, so every run pad stores its note (absolute).
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, Some(60));
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3, 4], SliceSet::Manual));
    assert!((0..4u8).all(|q| patch(&e, t).pad(q).note == Some(60 + q)), "a run with a root stores every run pad's note");
}

#[test]
fn reapplication_at_a_new_root_unbinds_unused_pads_and_reports_them() {
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, Some(60));
    let zones: Vec<u16> = (0..2u8).map(|q| patch(&e, t).pad(q).zone.unwrap()).collect();
    let out = to_pads(&mut e, t, Some(62));
    assert_eq!(bound(&e, t), run(62, &[3, 4, 5, 6], SliceSet::Manual));
    assert_eq!(
        out["unbound"],
        json!([{"pad": 1, "note": 60, "slice": 1, "id": 1, "zone": zones[0] + 1}, {"pad": 2, "note": 61, "slice": 2, "id": 2, "zone": zones[1] + 1}])
    );
    let p = patch(&e, t);
    assert_eq!((p.pad(0).zone, p.pad_note(0), p.pad(1).zone, p.pad_note(1)), (None, 60, None, 61));
    // Their zones are emptied first, so the two new pads' zones take those lowest free slots.
    assert_eq!(zones.iter().map(|z| p.pad_of(*z)).collect::<Vec<_>>(), [Some(4), Some(5)], "freed zone slots are reused by the new pads");
    assert_eq!(p.zones.len(), 4);
    assert_eq!(out["pads_after"], json!(4));
}

#[test]
fn hidden_set_pad_taken_is_reported_as_replaced() {
    let (mut e, t) = setup();
    auto(&mut e, t, 2);
    manual(&mut e, t, 4);
    zone_on_pad(&mut e, t, 20, 0, Some((SliceSet::Auto, 2)));
    edit(&mut e, t, |p| {
        let z = p.zone_mut(20).unwrap();
        z.attack_ms = 7.0;
        z.label = Some("hat".into());
    });
    let out = to_pads(&mut e, t, Some(60));
    assert_eq!(cell_replaces(&out)[0], json!({"set": "auto", "slice": 2, "id": 2}));
    let p = patch(&e, t);
    let z = p.zone(20).unwrap();
    assert_eq!(p.pad(0).zone, Some(20), "the pad keeps its zone");
    assert_eq!((z.attack_ms, z.label.as_deref()), (7.0, Some("hat")), "and the zone keeps its settings");
    assert_eq!(z.slice.as_ref().map(|b| (b.set, b.id)), Some((SliceSet::Manual, 1)));
}

#[test]
fn pads_after_counts_a_taken_hidden_set_pad_once() {
    // An Auto-bound pad on note 60 is taken by step 2 of a four-slice Manual run: four pads are
    // occupied after the run, as the preview and the answer say.
    let (mut e, t) = setup();
    auto(&mut e, t, 2);
    manual(&mut e, t, 4);
    zone_on_pad(&mut e, t, 20, 0, Some((SliceSet::Auto, 2)));
    let plan = e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 60, "preview": true})).unwrap();
    assert_eq!(plan["pads_after"], json!(4));
    let out = to_pads(&mut e, t, Some(60));
    assert_eq!(out["pads_after"], json!(4));
    assert_eq!((0..64u8).filter(|q| patch(&e, t).pad(*q).zone.is_some()).count(), 4);
}

#[test]
fn the_whole_run_is_one_undo_step() {
    // A run that unbinds two pads, rebinds two and makes two zones with new notes.
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, Some(60));
    let n = steps(&e);
    to_pads(&mut e, t, Some(62));
    assert_eq!(steps(&e), n + 1);
    assert_eq!(e.undo_history().last().map(String::as_str), Some("Slices to Pads"));
}

#[test]
fn loose_pad_collision_refused() {
    let (mut e, t) = setup();
    manual(&mut e, t, 8);
    // A loose sample on pad A5 (note 64), and another on A10 (note 69, outside the run).
    zone_on_pad(&mut e, t, 0, 4, None);
    zone_on_pad(&mut e, t, 1, 9, None);
    let text = refused(&mut e, "mix.sampler_slices_to_pads", json!({"track": t, "root": 60}));
    assert!(text.ends_with("Pad A5 already plays E3"), "{text}");
    let plan = e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 60, "preview": true})).unwrap();
    assert_eq!(
        (plan["ok"].clone(), plan["reason"].clone(), plan["loose_collisions"].clone()),
        (json!(false), json!("Pad A5 already plays E3"), json!([{"pad": 5, "note": 64}]))
    );
    // Clear of the loose pads' notes, the same run is accepted and leaves both loose pads alone.
    let before = patch(&e, t);
    to_pads(&mut e, t, Some(70));
    let p = patch(&e, t);
    assert_eq!((p.pad(4), p.pad(9)), (before.pad(4), before.pad(9)));
    assert_eq!((p.zone(0), p.zone(1)), (before.zone(0), before.zone(1)));
}

/// ADR 0051 decision 5: the refusal names the note in the engine's message convention, while the
/// parameters, the result fields and the document are the same under both settings.
#[test]
fn collision_note_names_follow_the_message_middle_c() {
    let run = |convention: transit_midi::MiddleC| {
        let (mut e, t) = setup();
        e.set_message_middle_c(convention);
        manual(&mut e, t, 8);
        zone_on_pad(&mut e, t, 0, 4, None);
        zone_on_pad(&mut e, t, 1, 9, None);
        let pads = refused(&mut e, "mix.sampler_slices_to_pads", json!({"track": t, "root": 60}));
        let mut plan = e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 60, "preview": true})).unwrap();
        let reason = plan.as_object_mut().unwrap().remove("reason").unwrap();
        to_pads(&mut e, t, Some(70));
        let map = refused(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 69}));
        (pads, reason, map, plan, serde_json::to_value(e.session()).unwrap())
    };
    let (c3, c4) = (run(transit_midi::MiddleC::C3), run(transit_midi::MiddleC::C4));
    assert!(c3.0.ends_with("Pad A5 already plays E3"), "{}", c3.0);
    assert!(c4.0.ends_with("Pad A5 already plays E4"), "{}", c4.0);
    assert_eq!((c3.1.clone(), c4.1.clone()), (json!("Pad A5 already plays E3"), json!("Pad A5 already plays E4")));
    assert!(c3.2.ends_with("Pad A10 already plays A3"), "{}", c3.2);
    assert!(c4.2.ends_with("Pad A10 already plays A4"), "{}", c4.2);
    assert_eq!(c3.3, c4.3, "the plan's fields are MIDI numbers under either setting");
    assert_eq!(c3.3["loose_collisions"], json!([{"pad": 5, "note": 64}]));
    assert_eq!(c3.4, c4.4, "the document is the same under either setting");
}

#[test]
fn root_overflow_refused() {
    let (mut e, t) = setup();
    manual(&mut e, t, 8);
    let text = refused(&mut e, "mix.sampler_slices_to_pads", json!({"track": t, "root": 121}));
    assert!(text.ends_with("Root 121 is too high for 8 slices: the highest root is 120."), "{text}");
    let plan = e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 121, "preview": true})).unwrap();
    assert_eq!((plan["ok"].clone(), plan["cells"].clone()), (json!(false), json!([])));
    let text = refused(&mut e, "mix.sampler_slices_to_pads", json!({"track": t, "root": 128}));
    assert!(text.contains("`root` must be an integer from 0 to 127"), "{text}");
    let text = refused(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 121}));
    assert!(text.ends_with("Root 121 is too high for 8 slices: the highest root is 120."), "{text}");
    // The highest root fits: 120..123 are pads A61..A64, 124..127 go to A1..A4.
    to_pads(&mut e, t, Some(120));
    let p = patch(&e, t);
    let keys: Vec<(u8, u8)> = (0..64u8).filter_map(|q| p.pad(q).zone.map(|_| (q + 1, p.pad_note(q)))).collect();
    assert_eq!(keys, [(1, 124), (2, 125), (3, 126), (4, 127), (61, 120), (62, 121), (63, 122), (64, 123)]);
    assert!(notes_unique(&p));
}

#[test]
fn preview_edits_nothing_and_matches_the_applied_result() {
    let (mut e, t) = setup();
    auto(&mut e, t, 2);
    manual(&mut e, t, 4);
    zone_on_pad(&mut e, t, 30, 5, Some((SliceSet::Auto, 1)));
    to_pads(&mut e, t, Some(60));
    let (before, n) = (e.session_arc(), steps(&e));
    let plan = e.execute("mix.sampler_slices_to_pads", &json!({"track": t, "root": 62, "preview": true})).unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()), "a preview edits nothing");
    assert_eq!(steps(&e), n, "and records no step");
    let applied = to_pads(&mut e, t, Some(62));
    assert_eq!(plan, applied, "the preview is the applied plan");
    assert_eq!(plan["ok"], json!(true));
    // The plan names the hidden-set pad it replaces (A6, Auto slice 1) and the pads it unbinds.
    assert_eq!(cell_replaces(&plan)[3], json!({"set": "auto", "slice": 1, "id": 1}));
    assert_eq!(plan["unbound"].as_array().unwrap().len(), 2);
}

#[test]
fn uses_the_showing_set_not_the_hidden_one() {
    let (mut e, t) = setup();
    auto(&mut e, t, 3);
    manual(&mut e, t, 5);
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "auto"})).unwrap();
    to_pads(&mut e, t, Some(60));
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3], SliceSet::Auto));
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "manual"})).unwrap();
    to_pads(&mut e, t, Some(60));
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3, 4, 5], SliceSet::Manual));
    // An undelivered candidate (eight grid slices) is never used: the accepted Auto set is.
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("mix.sampler_slice_mode", &json!({"track": t, "mode": "auto"})).unwrap();
    let c = e.execute("mix.sampler_slice_detect", &json!({"track": t, "source": "grid", "division": "1/16", "grid_bpm": 120.0})).unwrap();
    let ready = e.execute("mix.sampler_slice_candidate", &json!({"track": t})).unwrap();
    assert_eq!((ready["candidate"]["candidate_id"].clone(), ready["candidate"]["count"].clone()), (c["candidate_id"].clone(), json!(8)));
    let out = to_pads(&mut e, t, Some(60));
    assert_eq!(out["count"], json!(3));
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3], SliceSet::Auto));
}

#[test]
fn reapplication_replaces_the_previous_run_and_reports_it() {
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    e.execute("mix.sampler_slice_set", &json!({"track": t, "snap": false})).unwrap();
    to_pads(&mut e, t, Some(60));
    // A new marker inside slice 1 (id 5): the slices are now ids 1, 5, 2, 3, 4.
    e.execute("mix.sampler_slice_marker", &json!({"track": t, "op": "add", "frame": 6_000})).unwrap();
    let out = to_pads(&mut e, t, Some(60));
    let ids: Vec<u32> = bound(&e, t).iter().map(|b| b.3).collect();
    let pads: Vec<(u8, u8)> = bound(&e, t).iter().map(|b| (b.0, b.1)).collect();
    assert_eq!(ids, [1, 5, 2, 3, 4]);
    assert_eq!(pads, [(1, 60), (2, 61), (3, 62), (4, 63), (5, 64)]);
    assert_eq!(
        cell_replaces(&out),
        [
            Value::Null,
            json!({"set": "manual", "slice": 3, "id": 2}),
            json!({"set": "manual", "slice": 4, "id": 3}),
            json!({"set": "manual", "slice": 5, "id": 4}),
            Value::Null
        ]
    );
    assert_eq!(out["unbound"], json!([]));
}

#[test]
fn hidden_set_bindings_survive_unless_their_pad_is_taken() {
    let (mut e, t) = setup();
    auto(&mut e, t, 2);
    manual(&mut e, t, 4);
    zone_on_pad(&mut e, t, 30, 0, Some((SliceSet::Auto, 1)));
    zone_on_pad(&mut e, t, 31, 9, Some((SliceSet::Auto, 2)));
    let (pad, zone) = (patch(&e, t).pad(9), patch(&e, t).zone(31).cloned());
    let out = to_pads(&mut e, t, Some(60));
    let p = patch(&e, t);
    assert_eq!((p.pad(9), p.zone(31).cloned()), (pad, zone), "a hidden-set pad the run does not take is untouched");
    assert_eq!(p.zone(30).and_then(|z| z.slice.clone()).map(|b| (b.set, b.id)), Some((SliceSet::Manual, 1)), "the taken one is rebound");
    assert_eq!(cell_replaces(&out)[0], json!({"set": "auto", "slice": 1, "id": 1}));
    assert_eq!(out["unbound"], json!([]), "hidden-set pads are never unbound");
}

#[test]
fn map_slices_rewrites_only_the_run() {
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, Some(60));
    zone_on_pad(&mut e, t, 40, 9, None);
    // Swap slices 1 and 2 between pads A1 and A2 by the pad commands.
    let (za, zb) = (patch(&e, t).pad(0).zone.unwrap(), patch(&e, t).pad(1).zone.unwrap());
    for (cmd, pad, zone) in [
        ("mix.sampler_pad_unbind", 1, None),
        ("mix.sampler_pad_unbind", 2, None),
        ("mix.sampler_pad_bind", 1, Some(zb)),
        ("mix.sampler_pad_bind", 2, Some(za)),
    ] {
        let mut p = json!({"track": t, "pad": pad});
        if let Some(z) = zone {
            p["zone"] = json!(z + 1);
        }
        e.execute(cmd, &p).unwrap();
    }
    let state = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    assert_eq!(state["loop"]["mapping"], json!("stale"));
    assert_eq!(state["loop"]["out_of_order"], json!([{"slice": 1, "id": 1, "pad": 2, "key": 61}, {"slice": 2, "id": 2, "pad": 1, "key": 60}]));
    let others = |p: &PatchV1| (4..64u8).map(|q| p.pad(q)).collect::<Vec<_>>();
    let before = patch(&e, t);
    let out = one_step(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 60}));
    assert_eq!(
        out["pads"],
        json!([{"pad": 2, "note": 60, "slice": 1, "id": 1}, {"pad": 1, "note": 61, "slice": 2, "id": 2}, {"pad": 3, "note": 62, "slice": 3, "id": 3}, {"pad": 4, "note": 63, "slice": 4, "id": 4}])
    );
    let p = patch(&e, t);
    assert_eq!(others(&p), others(&before), "every other pad, the loose one included, is untouched");
    assert_eq!(p.zones, before.zones, "no zone changes");
    let state = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    assert_eq!((state["loop"]["mapping"].clone(), state["loop"]["mapping_root"].clone()), (json!("ascending"), json!(60)));
    // The whole run moves to a new root in one step; a loose pad on one of its notes refuses.
    one_step(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 36}));
    assert_eq!(patch(&e, t).slicing.unwrap().root, Some(36), "the loop stores the root");
    let text = refused(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 69}));
    assert!(text.ends_with("Pad A10 already plays A3"), "{text}");
    // A slice without a pad cannot be mapped.
    e.execute("mix.sampler_pad_unbind", &json!({"track": t, "pad": 3})).unwrap();
    let text = refused(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 60}));
    assert!(text.ends_with("Slice 3 has no pad: use Slices to pads."), "{text}");
}

/// Slice 2 moved from pad A2 to the empty pad A9 by the pad commands: the mapping is stale and
/// the empty A2 still holds note 61, one of the run's notes at root 60.
fn slice_two_moved_to_a9() -> (Engine, TrackId) {
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, Some(60));
    let z = patch(&e, t).pad(1).zone.unwrap();
    e.execute("mix.sampler_pad_unbind", &json!({"track": t, "pad": 2})).unwrap();
    e.execute("mix.sampler_pad_bind", &json!({"track": t, "pad": 9, "zone": z + 1})).unwrap();
    let state = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    assert_eq!(state["loop"]["mapping"], json!("stale"));
    (e, t)
}

#[test]
fn map_slices_moves_an_empty_pad_off_the_run() {
    let (mut e, t) = slice_two_moved_to_a9();
    let before = patch(&e, t);
    let out = one_step(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 60}));
    // A9 takes 61; the empty A2 takes the note A9 vacated (68); nothing else moves.
    assert_eq!(out["moved"], json!([{"pad": 2, "from": 61, "to": 68}]));
    assert_eq!(out["pads"].as_array().unwrap().iter().map(|x| x["pad"].as_u64().unwrap()).collect::<Vec<_>>(), [1, 9, 3, 4]);
    assert_eq!(
        bound(&e, t),
        vec![(1, 60, SliceSet::Manual, 1), (3, 62, SliceSet::Manual, 3), (4, 63, SliceSet::Manual, 4), (9, 61, SliceSet::Manual, 2)]
    );
    let p = patch(&e, t);
    assert_eq!((p.pad(1).zone, p.pad_note(1)), (None, 68), "the empty pad stays empty on the vacated note");
    assert!(notes_unique(&p), "no two pads share a note");
    assert_eq!(p.zones, before.zones, "no zone changes");
    let others = |p: &PatchV1| (0..64u8).filter(|q| ![0, 1, 2, 3, 8].contains(q)).map(|q| p.pad(q)).collect::<Vec<_>>();
    assert_eq!(others(&p), others(&before), "every other pad is untouched");
    let state = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
    assert_eq!((state["loop"]["mapping"].clone(), state["loop"]["mapping_root"].clone()), (json!("ascending"), json!(60)));
}

#[test]
fn map_slices_still_refuses_a_sounding_pad_on_a_run_note() {
    // As above, but the pad A2 left holds a zone bound to the hidden Auto set: it sounds.
    let (mut e, t) = slice_two_moved_to_a9();
    auto(&mut e, t, 2);
    zone_on_pad(&mut e, t, 30, 1, Some((SliceSet::Auto, 1)));
    let text = refused(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 60}));
    assert!(text.ends_with("Pad A2 already plays C#3"), "{text}");
}

#[test]
fn a_set_without_markers_is_refused() {
    let (mut e, t) = setup();
    let text = refused(&mut e, "mix.sampler_slices_to_pads", json!({"track": t}));
    assert!(text.ends_with("The Auto set has no slice markers yet: detect or add slices first."), "{text}");
}

fn notes(e: &mut Engine, t: TrackId, list: &[(u8, i64)]) {
    let q = TICKS_PER_QUARTER;
    let mut seq = Sequence::default();
    for &(pitch, start) in list {
        seq.notes.push(Note { pitch, velocity: 127, release_velocity: 64, channel: 0, start: start * q, length: q });
    }
    let s = e.session_mut();
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::midi(cid, "notes", 0, 48_000 * 8, seq));
}

#[test]
fn slice_pad_renders_region_at_original_pitch() {
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, Some(60));
    // Slice 3 is frames 24,000..36,000 on pad A3 (note 62). A neutral envelope and velocity
    // response, so the voice is the region itself.
    let zone = patch(&e, t).pad(2).zone.unwrap();
    e.execute(
        "mix.sampler_zone_edit",
        &json!({"track": t, "zone": zone + 1, "playback": {"attack_ms": 0.0, "declick_ms": 0.0, "velocity_sensitivity": 0.0}}),
    )
    .unwrap();
    notes(&mut e, t, &[(62, 0)]);
    let out = transit_mix::render_range(e.session(), Range::new(0, 24_000), 512);
    let want: Vec<f32> = (24_000..36_000).map(wave).collect();
    let got = &out[0][..12_000];
    let first = got.iter().zip(&want).position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None, "first difference at {first:?}: {:?} vs {:?}", first.map(|i| got[i]), first.map(|i| want[i]));
    assert!(out[0][12_000..].iter().all(|x| *x == 0.0), "the one-shot ends with its slice");
}

/// FNV-1a over the bits of every sample.
fn hash(out: &[Vec<f32>]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for ch in out {
        for x in ch {
            for b in x.to_bits().to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    h
}

struct Dir(std::path::PathBuf);

impl Dir {
    fn new(tag: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("sc-sampler-slice-pads-{tag}-{}", std::process::id()));
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

/// Integer noise for a golden: a 32-bit LCG, its top 24 bits as a signed fraction at half scale.
/// Every step (integer arithmetic, a 24-bit integer to f32, division by a power of two) is exact
/// on every platform, unlike `f32::sin`, which the platform C runtime computes.
fn noise(n: usize) -> Vec<f32> {
    let mut x: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((x >> 8) as i32 - (1 << 23)) as f32 / (1 << 24) as f32
        })
        .collect()
}

/// A session as builds before Slices to pads wrote it: a loose pad, a Manual loop of three slices
/// and a slice-bound pad zone over slice 2, with notes for both pads, over an integer-noise source.
/// The render path is unity, unshaped, untransposed pads at 0 dB with key tracking off, so every
/// value is IEEE-exact arithmetic and the hash is the same on every platform. Captured on the tree
/// before slices D to F (a `git archive` copy in its own fresh build); never regenerated from new
/// code.
const OLD_SESSION_GOLDEN: u64 = 0xc355_437f_a49e_8e1d;

#[test]
fn old_sessions_open_and_render_with_unchanged_hashes() {
    let d = Dir::new("golden");
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    let rate = e.session().sample_rate.hz();
    let sid = crate::io::add_source(e.session_mut(), "loop", AudioBuffer { sample_rate: rate, channels: vec![noise(LEN)] }, None, FileFormat::Wav);
    e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": sid})).unwrap();
    edit(&mut e, t, |p| {
        // A patch saved before the pad start note reads 60 (the field is written only when it is not
        // 60, so the saved bytes are those an older build wrote).
        p.pad_start_note = 60;
        let l = p.slicing.as_mut().unwrap();
        l.manual = Some(even(3));
        l.mode = SliceMode::Manual;
        let mut loose = ZoneEntry::whole(0, 0, 12_000);
        loose.pad_defaults(60);
        let mut sliced = ZoneEntry { id: 1, source: 0, start: 16_000, end: 32_000, loop_start: 16_000, loop_end: 32_000, ..ZoneEntry::default() };
        sliced.pad_defaults(61);
        sliced.slice = Some(SliceBinding { set: SliceSet::Manual, id: 2, ..SliceBinding::default() });
        p.zones = vec![loose, sliced];
        p.set_pad(PadEntry { zone: Some(0), ..p.pad(0) });
        p.set_pad(PadEntry { zone: Some(1), ..p.pad(1) });
    });
    notes(&mut e, t, &[(60, 0), (61, 1)]);
    let file = d.path("old.transit");
    e.execute("session.save_as", &json!({"path": file})).unwrap();
    let mut back = Engine::default();
    back.execute("session.open", &json!({"path": file})).unwrap();
    let h = hash(&transit_mix::render_range(back.session(), Range::new(0, 72_000), 512));
    assert_eq!(h, OLD_SESSION_GOLDEN, "an old session renders as it did");
    assert_eq!(serde_json::to_value(patch(&back, t)).unwrap(), serde_json::to_value(patch(&e, t)).unwrap(), "and opens with its patch as saved");
}

// The pad start note (ADR 0041 amendment 2026-11-09 item 7) through the slice commands.

/// Sets the pad start note by the command.
fn start(e: &mut Engine, t: TrackId, note: u8) {
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": note})).unwrap();
}

/// Slice to MIDI's first and last key for the showing set, from a preview (nothing is edited).
fn midi_keys(e: &mut Engine, t: TrackId) -> (Value, Value) {
    let out = e.execute("mix.slice_to_midi", &json!({"track": t, "timing": "fit", "at": 0, "preview": true})).unwrap();
    (out["first_key"].clone(), out["last_key"].clone())
}

#[test]
fn a_default_run_starts_at_the_pad_start_note_and_stores_no_root() {
    for s in [36u8, 12] {
        let (mut e, t) = setup();
        start(&mut e, t, s);
        manual(&mut e, t, 8);
        let out = to_pads(&mut e, t, None);
        assert_eq!(out["root"], json!(s), "start {s}: the run's root is the start note");
        assert_eq!(bound(&e, t), run(s, &[1, 2, 3, 4, 5, 6, 7, 8], SliceSet::Manual), "start {s}: slice 1 on pad A1");
        let p = patch(&e, t);
        assert_eq!(p.slicing.as_ref().unwrap().root, None, "start {s}: a run without a root stores none");
        assert!((0..8u8).all(|q| !p.pad_note_stored(q)), "start {s}: pads on their default notes store no note");
        assert_eq!(p.slice_root(), s);
    }
}

#[test]
fn slice_to_midi_keys_follow_the_start_note_after_a_default_run() {
    let (mut e, t) = setup();
    start(&mut e, t, 36);
    manual(&mut e, t, 4);
    to_pads(&mut e, t, None);
    assert_eq!(midi_keys(&mut e, t), (json!(36), json!(39)), "keys from the start note");
    // The run follows a start change as a whole, and Slice to MIDI follows it.
    start(&mut e, t, 12);
    assert_eq!(bound(&e, t), run(12, &[1, 2, 3, 4], SliceSet::Manual), "the run moved with the start note");
    assert_eq!(midi_keys(&mut e, t), (json!(12), json!(15)), "keys follow the start note");
}

#[test]
fn an_explicit_map_root_survives_a_start_change() {
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, None);
    // Map at the run's own root: every run pad sits on its default note before it.
    one_step(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 60}));
    let p = patch(&e, t);
    assert_eq!(p.slicing.as_ref().unwrap().root, Some(60), "Map stores its root");
    assert!((0..4u8).all(|q| p.pad_note_stored(q)), "every run pad stores its note");
    start(&mut e, t, 64);
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3, 4], SliceSet::Manual), "the run stays where Map put it");
    assert_eq!(patch(&e, t).slice_root(), 60);
    assert_eq!(midi_keys(&mut e, t), (json!(60), json!(63)));
}

#[test]
fn a_run_with_a_root_stores_every_run_pad_note_and_survives_a_start_change() {
    let (mut e, t) = setup();
    manual(&mut e, t, 4);
    to_pads(&mut e, t, Some(60));
    assert!((0..4u8).all(|q| patch(&e, t).pad_note_stored(q)), "every run pad stores its note");
    start(&mut e, t, 64);
    assert_eq!(bound(&e, t), run(60, &[1, 2, 3, 4], SliceSet::Manual), "the run is absolute like its root");
    assert_eq!(midi_keys(&mut e, t), (json!(60), json!(63)));
}

#[test]
fn a_stored_note_on_an_empty_pad_refuses_a_start_change_truthfully() {
    // Map moves the empty A2 to note 68 (stored); a start note of 64 would put the defaulted A5
    // on 68.
    let (mut e, t) = slice_two_moved_to_a9();
    one_step(&mut e, "mix.sampler_map_slices", json!({"track": t, "root": 60}));
    let state = e.execute("mix.sampler_state", &json!({"track": t})).unwrap();
    let a2 = &state["pads"][1];
    assert_eq!((a2["note"].clone(), a2["note_stored"].clone(), a2["zone"].clone()), (json!(68), json!(true), Value::Null), "A2 is empty on 68");
    let a5 = &state["pads"][4];
    assert_eq!((a5["note"].clone(), a5["note_stored"].clone()), (json!(64), json!(false)), "A5 is defaulted: start 60 plus 4");
    let text = refused(&mut e, "mix.sampler_pad_start", json!({"track": t, "note": 64}));
    assert!(text.ends_with("Pad A2 plays G#3: a start note of E3 would put pad A5 there."), "{text}");
}
