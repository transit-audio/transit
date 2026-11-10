//! Key-map group shape and the voices one note starts (ADR 0041 body and amendment 2026-11-09,
//! refusal (c)).

use super::*;
use crate::SourceId;
use crate::sampler_patch::{Membership, PadEntry, PatchState, SamplerPatch, SourceRef, ZoneEntry};
use serde_json::json;

fn member(id: u16, group: u8, layer: u8, alternative: u8, keys: [u8; 2], velocities: [u8; 2]) -> ZoneEntry {
    let mut z = ZoneEntry::whole(id, 0, 1000);
    z.keys = keys;
    z.velocities = velocities;
    z.membership = Some(Membership { group, layer, alternative, ..Membership::default() });
    z
}

fn with(zones: Vec<ZoneEntry>) -> PatchV1 {
    PatchV1 { sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }], zones, pad_start_note: 60, ..PatchV1::default() }
}

/// Group 3: layer 0 (velocities 1..63) and layer 1 (64..127), three alternatives each, keys 40..60.
fn valid() -> PatchV1 {
    let mut zones = Vec::new();
    for layer in 0..2u8 {
        for a in 0..3u8 {
            let v = if layer == 0 { [1, 63] } else { [64, 127] };
            zones.push(member(u16::from(layer * 3 + a), 3, layer, a, [40, 60], v));
        }
    }
    with(zones)
}

#[test]
fn group_shape_admission() {
    assert_eq!(valid().check_groups(), Ok(()));
    assert_eq!(valid().validate(), Ok(()), "a 2-layer, 3-alternative group is accepted");
    let cases: Vec<(&str, PatchV1, &str)> = vec![
        (
            "nine layers",
            {
                let mut p = valid();
                p.zones.push(member(10, 3, 8, 0, [40, 60], [1, 5]));
                p
            },
            "group 3 would have 9 layers (at most 8)",
        ),
        (
            "nine alternatives",
            {
                let mut p = valid();
                for a in 3..9u8 {
                    p.zones.push(member(10 + u16::from(a), 3, 0, a, [40, 60], [1, 63]));
                }
                p
            },
            "group 3 layer 0 would have a ninth alternative (at most 8)",
        ),
        (
            "another key range in the group",
            {
                let mut p = valid();
                p.zones[4].keys = [40, 61];
                p
            },
            "group 3: zone 5 has another key range than zone 1 (a group has one key range)",
        ),
        (
            "unequal velocity ranges across a layer",
            {
                let mut p = valid();
                p.zones[2].velocities = [1, 62];
                p
            },
            "group 3 layer 0: zone 3 has another velocity range than zone 1 (a layer's alternatives share one)",
        ),
        (
            "a gap in the alternatives",
            {
                let mut p = valid();
                p.zones.retain(|z| z.id != 4);
                p
            },
            "group 3 layer 1 has no alternative 1 (alternatives count up from 0 without a gap)",
        ),
        (
            "a duplicate triple",
            {
                let mut p = valid();
                p.zones.push(member(20, 3, 1, 2, [40, 60], [64, 127]));
                p
            },
            "zones 6 and 21 are both group 3 layer 1 alternative 2",
        ),
        (
            "a pad-bound member",
            {
                let mut p = valid();
                p.set_pad(PadEntry { index: 2, zone: Some(1), ..PadEntry::default() });
                p
            },
            "zone 2 is pad A3's zone: a pad's zone cannot be a member of group 3",
        ),
    ];
    for (what, p, text) in cases {
        let r = p.check_groups().expect_err(what);
        assert_eq!(r.to_string(), text, "{what}");
        assert_eq!(p.validate().map_err(|e| e.0), Err(text.to_string()), "{what}: the validator refuses it with the same text");
    }
    // Different groups may have different shapes; a group may skip a layer.
    let mut p = valid();
    p.zones.push(member(30, 4, 5, 0, [0, 10], [1, 127]));
    assert_eq!(p.check_groups(), Ok(()));
}

#[test]
fn a_badly_shaped_group_loads_opaque_with_a_note_and_a_good_one_loads_ready() {
    // Group 0 with only alternative 1: it would play silent, so it loads opaque with a note.
    let raw = json!({
        "schema": 1, "sources": [{"source": 1}],
        "zones": [{"id": 0, "end": 100, "membership": {"group": 0, "layer": 0, "alternative": 1}}],
    });
    let p = SamplerPatch::from_value(raw.clone());
    assert!(
        matches!(&p.state, PatchState::Invalid { reason, .. } if reason == "group 0 layer 0 has no alternative 0 (alternatives count up from 0 without a gap)"),
        "{:?}",
        p.state
    );
    assert!(p.note("Keys").is_some_and(|n| n.contains("no alternative 0")));
    assert_eq!(serde_json::to_value(&p).unwrap(), raw, "kept as saved");
    let good = json!({
        "schema": 1, "sources": [{"source": 1}],
        "zones": [{"id": 0, "end": 100, "membership": {"group": 0, "layer": 0, "alternative": 0}}],
    });
    assert!(SamplerPatch::from_value(good).ready().is_some());
}

/// Eight plain zones on keys 60..64: zone i over velocities 1..127, except zone 7 over 1..10.
fn eight() -> PatchV1 {
    let mut zones = Vec::new();
    for i in 0..8u16 {
        let mut z = ZoneEntry::whole(i, 0, 1000);
        z.keys = [60, 64];
        if i == 7 {
            z.velocities = [1, 10];
        }
        zones.push(z);
    }
    with(zones)
}

#[test]
fn a_note_counts_a_bound_pad_each_plain_zone_and_one_voice_per_group_layer() {
    let mut p = eight();
    assert_eq!(p.note_voices(62)[5], 8);
    assert_eq!(p.note_voices(62)[11], 7, "zone 8 stops at velocity 10");
    assert_eq!(p.note_voices(65)[5], 0);
    // A group layer of three alternatives is one voice; a second layer at those velocities another.
    for a in 0..3u8 {
        p.zones.push(member(20 + u16::from(a), 2, 0, a, [62, 62], [1, 127]));
    }
    p.zones.push(member(30, 2, 1, 0, [62, 62], [100, 127]));
    let v = p.note_voices(62);
    assert_eq!((v[5], v[50], v[100]), (9, 8, 9), "eight plain + layer 0; seven + layer 0; seven + both layers");
    // A bound pad counts one on its note at every velocity; its zone is not scanned.
    let mut q = eight();
    q.set_pad(PadEntry { index: 0, zone: Some(0), note: Some(70), ..PadEntry::default() });
    assert_eq!(q.note_voices(62)[5], 7, "zone 1 plays through its pad only");
    assert_eq!(q.note_voices(70)[1], 1);
    assert_eq!(q.note_voices(70)[127], 1);
    // A pad without a zone starts nothing.
    q.set_pad(PadEntry { index: 1, note: Some(62), ..PadEntry::default() });
    assert_eq!(q.note_voices(62)[5], 7);
}

#[test]
fn an_edit_over_eight_names_the_voice_it_added_and_an_old_overload_does_not_block_another_edit() {
    let before = eight();
    // A ninth plain zone over note 62 at velocity 5.
    let mut after = before.clone();
    let mut z = ZoneEntry::whole(40, 0, 1000);
    z.keys = [62, 62];
    after.zones.push(z);
    let o = after.layer_overload(&before).expect("a ninth voice");
    assert_eq!((o.note, o.velocity, o.voices, o.by), (62, 1, 9, Voice::Zone(40)));
    assert_eq!(o.text(MiddleC::C3), "Zone 41 would be a ninth layer on D3.");
    assert_eq!(o.text(MiddleC::C4), "Zone 41 would be a ninth layer on D4.");
    // A new group layer of two alternatives is one voice, named by its first zone the edit added.
    let mut more = before.clone();
    more.zones.push(member(50, 1, 0, 0, [62, 62], [1, 127]));
    more.zones.push(member(51, 1, 0, 1, [62, 62], [1, 127]));
    assert_eq!(more.layer_overload(&before).map(|o| (o.voices, o.by)), Some((9, Voice::Zone(50))));
    // A pad moved onto the note.
    let mut padded = before.clone();
    padded.zones.push(ZoneEntry::whole(60, 0, 1000));
    padded.set_pad(PadEntry { index: 9, zone: Some(60), note: Some(80), ..PadEntry::default() });
    let mut moved = padded.clone();
    moved.set_pad(PadEntry { index: 9, zone: Some(60), note: Some(63), ..PadEntry::default() });
    let o = moved.layer_overload(&padded).expect("the pad is a ninth voice");
    assert_eq!((o.by, o.text(MiddleC::C3)), (Voice::Pad(9), "Pad A10 would be a ninth layer on D#3.".to_string()));
    // Already over the bound (a loaded map): an edit that adds nothing there is not refused, one
    // that adds there is, and one that only lowers the count is not.
    let mut loaded = after.clone();
    assert_eq!(loaded.layer_overload(&after), None, "no edit");
    loaded.zones[0].transpose = 3.0;
    assert_eq!(loaded.layer_overload(&after), None, "another edit");
    let mut worse = after.clone();
    let mut z = ZoneEntry::whole(41, 0, 1000);
    z.keys = [62, 62];
    worse.zones.push(z);
    assert_eq!(worse.layer_overload(&after).map(|o| (o.voices, o.by)), Some((10, Voice::Zone(41))));
    let mut better = after.clone();
    better.zones.retain(|z| z.id != 0);
    assert_eq!(better.layer_overload(&after), None);
    // Eight is the bound, not over it.
    assert_eq!(before.layer_overload(&with(Vec::new())), None);
}
