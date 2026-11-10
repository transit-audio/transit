//! The pad start note (ADR 0041 amendment 2026-11-09): defaults, effective notes, stored notes
//! absolute, the root write-through and refusals (a) to (d).

use super::*;
use crate::SourceId;
use crate::sampler_patch::{Membership, PadEntry, PatchState, SamplerPatch, SliceLoop, SourceRef, ZoneEntry};
use serde_json::json;
use transit_midi::MiddleC;

fn ready(text: &str) -> PatchV1 {
    let p: SamplerPatch = serde_json::from_str(text).unwrap();
    p.ready().cloned().unwrap_or_else(|| panic!("{text} should load ready: {p:?}"))
}

/// A patch at start note `start` with one source and `zones` whole zones (ids 0..zones).
fn patch(start: u8, zones: u16) -> PatchV1 {
    let mut p = PatchV1 { pad_start_note: start, sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }], ..PatchV1::default() };
    for id in 0..zones {
        p.zones.push(ZoneEntry::whole(id, 0, 1000));
    }
    p
}

/// Binds pad `pad` to zone `zone` as a new pad: one-shot, key tracking off, root at its note.
fn bind(p: &mut PatchV1, pad: u8, zone: u16) {
    let note = p.pad_note(pad);
    p.zone_mut(zone).unwrap().pad_defaults(note);
    p.set_pad(PadEntry { zone: Some(zone), ..p.pad(pad) });
}

fn written(p: &PatchV1) -> serde_json::Value {
    serde_json::to_value(SamplerPatch { state: PatchState::Ready(Box::new(p.clone())) }).unwrap()
}

#[test]
fn a_new_patch_stores_36_and_an_absent_field_reads_60() {
    let new = PatchV1::default();
    assert_eq!(new.pad_start_note, 36);
    assert_eq!(SamplerPatch::new().ready().unwrap().pad_start_note, 36);
    assert_eq!(written(&new), json!({"schema": 1, "pad_start_note": 36}), "a new patch's 36 is written");
    assert_eq!((new.pad_note(0), new.pad_note(15), new.pad_note(16), new.pad_note(63)), (36, 51, 52, 99));
    let old = ready(r#"{"schema":1}"#);
    assert_eq!(old.pad_start_note, 60, "a patch saved without the field reads 60");
    assert_eq!((old.pad_note(0), old.pad_note(63)), (60, 123));
    assert_eq!(written(&old), json!({"schema": 1}), "60 is not written");
    let explicit = ready(r#"{"schema":1,"pad_start_note":12}"#);
    assert_eq!((explicit.pad_start_note, explicit.pad_note(5)), (12, 17));
}

#[test]
fn an_old_patch_round_trips_byte_identically_and_its_pads_keep_60_plus_index() {
    // The golden texts were written by the base tree (before the pad start note) from the same
    // input, on a fresh copy of it in its own target.
    for (input, golden) in [(OLD_PATCH, GOLDEN), (OLD_PATCH_NO_ROOT, GOLDEN_NO_ROOT)] {
        let p: SamplerPatch = serde_json::from_str(input).unwrap();
        assert!(p.ready().is_some(), "{p:?}");
        let text = serde_json::to_string(&p).unwrap();
        assert_eq!(text, golden, "the base wrote these bytes");
        let again: SamplerPatch = serde_json::from_str(&text).unwrap();
        assert_eq!(serde_json::to_string(&again).unwrap(), text, "and they read back to the same bytes");
    }
    let p = ready(OLD_PATCH);
    for i in 0..64u8 {
        let want = if i == 5 { 20 } else { 60 + i };
        assert_eq!(p.pad_note(i), want, "pad {i}");
        assert_eq!(p.pad_note_stored(i), i == 5);
    }
    assert_eq!(p.slicing.as_ref().unwrap().root, Some(62), "a stored loop root is kept");
    assert_eq!(p.slice_root(), 62);
    let p = ready(OLD_PATCH_NO_ROOT);
    assert_eq!((p.slicing.as_ref().unwrap().root, p.slice_root()), (None, 60), "no stored root: the start note, 60");
}

#[test]
fn a_start_change_moves_only_defaulted_pads_and_keeps_stored_notes_absolute() {
    let mut p = patch(60, 0);
    p.set_pad(PadEntry { index: 2, note: Some(100), ..PadEntry::default() });
    p.set_pad(PadEntry { index: 7, mute: true, ..PadEntry::default() });
    let before: Vec<u8> = (0..64).map(|i| p.pad_note(i)).collect();
    let change = p.set_pad_start(36).unwrap();
    assert_eq!((change.from, change.to, change.semitones()), (60, 36, -24));
    for i in 0..64u8 {
        let want = if i == 2 { 100 } else { 36 + i };
        assert_eq!(p.pad_note(i), want, "pad {i}");
    }
    assert_ne!(before[7], p.pad_note(7), "a stored entry without a note moves");
    assert_eq!(p.validate(), Ok(()));
    // An unchanged start note changes nothing.
    let same = p.clone();
    assert_eq!(p.set_pad_start(36).unwrap().semitones(), 0);
    assert_eq!(p, same);
}

#[test]
fn a_stored_note_equal_to_its_current_default_survives_a_later_edit_and_a_start_change() {
    let mut p = patch(36, 0);
    // Pad A4 stores 39, which is also its default at start 36.
    p.set_pad(PadEntry { index: 3, note: Some(39), ..PadEntry::default() });
    assert!(p.pad_note_stored(3), "setting a note equal to the default stores it");
    // A later mute edit of the same pad, then its unmute: the entry is pruned only of what is plain.
    p.set_pad(PadEntry { mute: true, ..p.pad(3) });
    p.set_pad(PadEntry { mute: false, ..p.pad(3) });
    assert_eq!(p.pad(3).note, Some(39), "the stored note survives the pruning");
    assert_eq!(written(&p)["pads"], json!([{"index": 3, "note": 39}]));
    p.set_pad_start(48).unwrap();
    assert_eq!((p.pad_note(3), p.pad_note(2), p.pad_note(4)), (39, 50, 52), "it stays while its neighbors move");
}

#[test]
fn the_root_write_through_keeps_each_moved_pad_distance_and_nothing_else() {
    let mut p = patch(60, 4);
    // Pad A1 defaulted, key-tracked, with a user root a fifth below its note.
    bind(&mut p, 0, 0);
    let z = p.zone_mut(0).unwrap();
    z.key_tracking = true;
    z.root = 53;
    // Pad A2 defaulted with the new-pad root (its note).
    bind(&mut p, 1, 1);
    // Pad A3 with a stored note and a stored root.
    p.set_pad(PadEntry { index: 2, note: Some(20), ..PadEntry::default() });
    bind(&mut p, 2, 2);
    p.zone_mut(2).unwrap().root = 70;
    // Zone 4 unbound, root 66.
    p.zone_mut(3).unwrap().root = 66;
    let before = p.clone();
    p.set_pad_start(36).unwrap();
    let root = |p: &PatchV1, z: u16| p.zone(z).unwrap().root;
    assert_eq!((root(&p, 0), root(&p, 1)), (29, 37), "each moved pad keeps its note-to-root distance");
    assert_eq!(i16::from(p.pad_note(0)) - i16::from(root(&p, 0)), i16::from(before.pad_note(0)) - i16::from(root(&before, 0)));
    assert_eq!((root(&p, 2), root(&p, 3)), (70, 66), "a stored-note pad's zone and an unbound zone keep their roots");
    for z in 0..4 {
        let (a, b) = (p.zone(z).unwrap(), before.zone(z).unwrap());
        assert_eq!(ZoneEntry { root: 0, ..a.clone() }, ZoneEntry { root: 0, ..b.clone() }, "zone {z}: only the root moves");
    }
}

#[test]
fn refusals_a_to_d_leave_the_patch_identical_and_name_pad_and_note() {
    // (a)
    let mut p = patch(60, 0);
    let before = p.clone();
    assert_eq!(p.set_pad_start(65), Err(PadRefusal::StartRange));
    assert_eq!(PadRefusal::StartRange.text(MiddleC::C3), "note must be an integer from 0 to 64");
    assert_eq!(p, before);
    // (b) Pad B1 stores 36; a start of 36 would put pad A1 there.
    p.set_pad(PadEntry { index: 16, note: Some(36), ..PadEntry::default() });
    let before = p.clone();
    let r = p.set_pad_start(36).unwrap_err();
    assert_eq!(r, PadRefusal::Taken { stored: 16, note: 36, start: 36, defaulted: 0 });
    assert_eq!(r.text(MiddleC::C3), "Pad B1 plays C1: a start note of C1 would put pad A1 there.");
    assert_eq!(r.text(MiddleC::C4), "Pad B1 plays C2: a start note of C2 would put pad A1 there.");
    assert_eq!(p, before);
    // (c) Seven unbound zones on E3 (64) at every velocity, and an eighth at velocities 1 to 10:
    // pad A5 bound and moved onto 64 would be a ninth voice at velocity 5.
    let mut p = patch(36, 9);
    for z in 0..7 {
        p.zone_mut(z).unwrap().keys = [60, 70];
    }
    let partial = p.zone_mut(7).unwrap();
    partial.keys = [64, 64];
    partial.velocities = [1, 10];
    bind(&mut p, 4, 8);
    let before = p.clone();
    let r = p.set_pad_start(60).unwrap_err();
    assert_eq!(r, PadRefusal::NinthLayer { pad: 4, note: 64 });
    assert_eq!(r.text(MiddleC::C3), "Pad A5 would be a ninth layer on E3.");
    assert_eq!(p, before);
    // A layer's alternative 1 is not counted: as alternative 1 the eighth zone lets the move pass.
    p.zone_mut(7).unwrap().membership = Some(Membership { group: 1, layer: 0, alternative: 1, ..Membership::default() });
    p.zone_mut(6).unwrap().membership = Some(Membership { group: 1, layer: 0, alternative: 0, ..Membership::default() });
    p.zone_mut(6).unwrap().keys = [64, 64];
    p.zone_mut(6).unwrap().velocities = [1, 10];
    assert!(p.set_pad_start(60).is_ok(), "pad, six plain zones and alternative 0 are eight");
    // (d) Pad A3's zone root is 120: up 8 would be 128.
    let mut p = patch(36, 1);
    bind(&mut p, 2, 0);
    p.zone_mut(0).unwrap().root = 120;
    let before = p.clone();
    let r = p.set_pad_start(44).unwrap_err();
    assert_eq!(r, PadRefusal::RootRange { pad: 2 });
    assert_eq!(r.text(MiddleC::C3), "Pad A3's root would leave 0 to 127.");
    assert_eq!(p, before);
}

#[test]
fn the_notice_names_the_start_note_and_the_semitone_distance() {
    let down = StartChange { from: 60, to: 36 };
    assert_eq!(down.notice(MiddleC::C3), "Pad start note C1 (down 24 semitones): transpose pad clips by -24 to keep them on the same pads.");
    assert_eq!(down.notice(MiddleC::C4), "Pad start note C2 (down 24 semitones): transpose pad clips by -24 to keep them on the same pads.");
    let up = StartChange { from: 36, to: 37 };
    assert_eq!(up.notice(MiddleC::C3), "Pad start note C#1 (up 1 semitone): transpose pad clips by +1 to keep them on the same pads.");
    assert_eq!(StartChange { from: 36, to: 36 }.notice(MiddleC::C3), "Pad start note C1 (unchanged).");
}

#[test]
fn a_loaded_start_note_past_64_is_opaque_and_unique_notes_are_effective() {
    let p: SamplerPatch = serde_json::from_str(r#"{"schema":1,"pad_start_note":65}"#).unwrap();
    let PatchState::Invalid { reason, .. } = &p.state else { panic!("65 should load opaque: {p:?}") };
    assert!(reason.contains("pad start note 65 is past 64"), "{reason}");
    assert!(ready(r#"{"schema":1,"pad_start_note":64}"#).validate().is_ok());
    // Pad A1 stores 41: at start 36 pad A6 defaults to 41 too.
    let p: SamplerPatch = serde_json::from_str(r#"{"schema":1,"pad_start_note":36,"pads":[{"index":0,"note":41}]}"#).unwrap();
    assert!(matches!(p.state, PatchState::Invalid { .. }), "{p:?}");
    assert!(ready(r#"{"schema":1,"pads":[{"index":0,"note":41}]}"#).validate().is_ok(), "at the saved 60 it is unique");
}

#[test]
fn an_explicit_loop_root_stays_put_and_an_absent_one_follows_the_start_note() {
    let mut p = patch(60, 0);
    p.slicing = Some(SliceLoop { end: 100, ..SliceLoop::default() });
    assert_eq!(p.slice_root(), 60);
    p.set_pad_start(12).unwrap();
    assert_eq!(p.slice_root(), 12, "it follows the start note");
    assert!(written(&p)["loop"].get("root").is_none());
    p.slicing.as_mut().unwrap().root = Some(60);
    assert_eq!(written(&p)["loop"]["root"], 60, "a stored 60 is written");
    p.set_pad_start(36).unwrap();
    assert_eq!(p.slice_root(), 60, "a stored root is absolute");
}

#[test]
fn pads_are_named_by_bank_and_slot() {
    assert_eq!([0, 4, 15, 16, 28, 63].map(pad_label), ["A1", "A5", "A16", "B1", "B13", "D16"].map(String::from));
}

const OLD_PATCH: &str = r#"{"schema":1,"seed":7,"sources":[{"source":1},{"source":2}],"zones":[{"id":0,"source":0,"end":48000,"loop_end":48000,"mode":"one_shot","key_tracking":false,"root":60},{"id":1,"source":1,"start":100,"end":30000,"loop_mode":"forward","loop_start":200,"loop_end":20000,"keys":[40,72],"root":64,"transpose":2.0},{"id":2,"source":0,"end":1000,"root":61,"key_tracking":false,"mode":"one_shot"},{"id":3,"source":1,"end":2000,"keys":[0,50]}],"pads":[{"index":0,"zone":0},{"index":5,"zone":2,"note":20,"choke":3},{"index":9,"mute":true}],"loop":{"source":1,"end":40000,"root":62,"manual":{"markers":[{"id":1,"frame":0},{"id":2,"frame":20000}]}}}"#;
const OLD_PATCH_NO_ROOT: &str = r#"{"schema":1,"sources":[{"source":1}],"pads":[{"index":3,"note":10}],"loop":{"source":0,"end":40000}}"#;
const GOLDEN: &str = r#"{"schema":1,"seed":7,"sources":[{"source":1},{"source":2}],"zones":[{"id":0,"source":0,"start":0,"end":48000,"loop_mode":"off","loop_start":0,"loop_end":48000,"keys":[0,127],"velocities":[1,127],"root":60,"transpose":0.0,"cents":0.0,"reverse":false,"mode":"one_shot","key_tracking":false,"attack_ms":1.0,"decay_ms":300.0,"sustain":1.0,"release_ms":150.0,"declick_ms":0.0,"velocity_sensitivity":0.7},{"id":1,"source":1,"start":100,"end":30000,"loop_mode":"forward","loop_start":200,"loop_end":20000,"keys":[40,72],"velocities":[1,127],"root":64,"transpose":2.0,"cents":0.0,"reverse":false,"mode":"gate","key_tracking":true,"attack_ms":1.0,"decay_ms":300.0,"sustain":1.0,"release_ms":150.0,"declick_ms":0.0,"velocity_sensitivity":0.7},{"id":2,"source":0,"start":0,"end":1000,"loop_mode":"off","loop_start":0,"loop_end":0,"keys":[0,127],"velocities":[1,127],"root":61,"transpose":0.0,"cents":0.0,"reverse":false,"mode":"one_shot","key_tracking":false,"attack_ms":1.0,"decay_ms":300.0,"sustain":1.0,"release_ms":150.0,"declick_ms":0.0,"velocity_sensitivity":0.7},{"id":3,"source":1,"start":0,"end":2000,"loop_mode":"off","loop_start":0,"loop_end":0,"keys":[0,50],"velocities":[1,127],"root":60,"transpose":0.0,"cents":0.0,"reverse":false,"mode":"gate","key_tracking":true,"attack_ms":1.0,"decay_ms":300.0,"sustain":1.0,"release_ms":150.0,"declick_ms":0.0,"velocity_sensitivity":0.7}],"pads":[{"index":0,"zone":0},{"index":5,"zone":2,"note":20,"choke":3},{"index":9,"mute":true}],"loop":{"source":1,"start":0,"end":40000,"mode":"auto","root":62,"manual":{"markers":[{"id":1,"frame":0},{"id":2,"frame":20000}]}}}"#;
const GOLDEN_NO_ROOT: &str =
    r#"{"schema":1,"sources":[{"source":1}],"pads":[{"index":3,"note":10}],"loop":{"source":0,"start":0,"end":40000,"mode":"auto"}}"#;
