use super::*;
use crate::SourceId;
use crate::sampler_patch::{PadEntry, PatchState, SamplerPatch, SourceRef};
use serde_json::json;

fn zone(keys: [u8; 2], velocities: [u8; 2], w: [u8; 4]) -> ZoneEntry {
    ZoneEntry { keys, velocities, fades: Fades::of(w), ..ZoneEntry::whole(0, 0, 100) }
}

#[test]
fn fades_used_clamps_low_then_high() {
    // Keys 60..70 (span 10), velocities 40..44 (span 4).
    let cases: [([u8; 4], [u8; 4]); 7] = [
        ([3, 4, 1, 2], [3, 4, 1, 2]),      // fits: unchanged
        ([6, 4, 2, 2], [6, 4, 2, 2]),      // exactly the span on both axes
        ([6, 7, 3, 3], [6, 4, 3, 1]),      // too wide: the high width shrinks first
        ([12, 5, 9, 9], [10, 0, 4, 0]),    // the low width alone past the span: clamped to it
        ([0, 127, 0, 127], [0, 10, 0, 4]), // a high width alone clamps to the span
        ([127, 127, 127, 127], [10, 0, 4, 0]),
        ([10, 0, 0, 0], [10, 0, 0, 0]),
    ];
    for (stored, used) in cases {
        assert_eq!(fades_used(&zone([60, 70], [40, 44], stored), false), used, "stored {stored:?}");
        // Never past the span, and always the low width first.
        let [kl, kh, vl, vh] = used;
        assert!(kl + kh <= 10 && vl + vh <= 4);
    }
    // A one-key, one-velocity zone has nothing to fade.
    assert_eq!(fades_used(&zone([64, 64], [100, 100], [5, 5, 5, 5]), false), [0; 4]);
    // A later range edit leaves the stored widths and narrows what is used.
    let mut z = zone([48, 72], [1, 127], [8, 8, 20, 20]);
    assert_eq!(fades_used(&z, false), [8, 8, 20, 20]);
    z.keys = [60, 70];
    assert_eq!(fades_used(&z, false), [8, 2, 20, 20]);
    assert_eq!(z.fades.widths(), [8, 8, 20, 20], "the stored widths are kept");
}

#[test]
fn pad_bound_zone_fades_used_are_zero() {
    let z = zone([0, 127], [1, 127], [3, 4, 5, 6]);
    assert_eq!(fades_used(&z, true), [0; 4]);
    assert_eq!(fades_used(&z, false), [3, 4, 5, 6]);
    // Through the patch: zone 2 bound to pad 9, zone 3 unbound, the same widths.
    let mut p = PatchV1 { sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }], ..PatchV1::default() };
    p.zones = vec![ZoneEntry { id: 2, ..z.clone() }, ZoneEntry { id: 3, ..z }];
    p.set_pad(PadEntry { index: 9, zone: Some(2), ..PadEntry::default() });
    assert_eq!(p.fades_used(2), Some([0; 4]));
    assert_eq!(p.fades_used(3), Some([3, 4, 5, 6]));
    assert_eq!(p.fades_used(4), None);
}

/// A patch with zone 1 (unbound, keys 60..64 and velocities 100..102: too narrow for its widths)
/// and zone 2 (bound to pad 1, with nonzero widths).
fn clamped_patch() -> serde_json::Value {
    json!({
        "schema": 1,
        "sources": [{"source": 4}],
        "zones": [
            {"id": 0, "end": 100, "keys": [60, 64], "velocities": [100, 102], "fades": {"key_lo": 4, "key_hi": 4, "vel_lo": 9, "vel_hi": 1}},
            {"id": 1, "end": 100, "fades": {"key_lo": 1, "key_hi": 0, "vel_lo": 0, "vel_hi": 2}}
        ],
        "pads": [{"index": 0, "zone": 1}]
    })
}

#[test]
fn validator_accepts_over_wide_and_pad_bound_fades_on_load_and_store() {
    // Load: ready (audible), not opaque.
    let loaded = SamplerPatch::from_value(clamped_patch());
    let p = match &loaded.state {
        PatchState::Ready(p) => p.as_ref().clone(),
        other => panic!("the patch loaded opaque: {other:?}"),
    };
    assert_eq!(loaded.note("t"), None);
    assert_eq!(p.validate(), Ok(()));
    // The stored widths are kept as stored; playback uses the clamped ones.
    assert_eq!(p.zone(0).map(|z| z.fades.widths()), Some([4, 4, 9, 1]));
    assert_eq!(p.zone(1).map(|z| z.fades.widths()), Some([1, 0, 0, 2]));
    assert_eq!(p.fades_used(0), Some([4, 0, 2, 0]));
    assert_eq!(p.fades_used(1), Some([0; 4]));
    // Store: each case on its own passes the validator every store runs, and writes back as it was.
    let mut wide = PatchV1 { sources: p.sources.clone(), ..PatchV1::default() };
    wide.zones = vec![zone([60, 61], [1, 2], [127, 127, 127, 127])];
    assert_eq!(wide.validate(), Ok(()), "over-wide widths");
    let mut bound = wide.clone();
    bound.zones = vec![zone([0, 127], [1, 127], [0, 0, 0, 1])];
    bound.set_pad(PadEntry { index: 3, zone: Some(0), ..PadEntry::default() });
    assert_eq!(bound.validate(), Ok(()), "a pad-bound zone with a width");
    let back = serde_json::to_value(&loaded).unwrap();
    for i in 0..2 {
        assert_eq!(back["zones"][i]["fades"], clamped_patch()["zones"][i]["fades"], "zone {i}: stored widths written back unchanged");
    }
    assert_eq!(SamplerPatch::from_value(back), loaded);
}

#[test]
fn a_width_past_127_is_not_a_width() {
    let mut v = clamped_patch();
    v["zones"][0]["fades"]["key_hi"] = json!(128);
    assert!(matches!(SamplerPatch::from_value(v).state, PatchState::Invalid { .. }));
}

#[test]
fn fades_absent_at_default_and_round_trip_otherwise() {
    let mut p = PatchV1 { sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }], ..PatchV1::default() };
    p.zones = vec![ZoneEntry::whole(0, 0, 10)];
    let v = serde_json::to_value(&p).unwrap();
    assert!(v["zones"][0].get("fades").is_none(), "{v}");
    p.zones[0].fades = Fades::of([0, 0, 0, 7]);
    let v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["zones"][0]["fades"], json!({"key_lo": 0, "key_hi": 0, "vel_lo": 0, "vel_hi": 7}));
    let back: PatchV1 = serde_json::from_value(v).unwrap();
    assert_eq!(back, p);
    // Absent fields inside default to 0; unknown ones are kept and written back.
    let z: ZoneEntry = serde_json::from_value(json!({"end": 5, "fades": {"vel_lo": 2, "future": true}})).unwrap();
    assert_eq!(z.fades.widths(), [0, 0, 2, 0]);
    assert_eq!(serde_json::to_value(&z).unwrap()["fades"]["future"], json!(true));
}

#[test]
fn the_edit_check_refuses_too_wide_and_pad_bound_widths() {
    let ok = |w: [u8; 4]| check_fades([60, 70], [40, 44], &Fades::of(w), false);
    assert_eq!(ok([5, 5, 2, 2]), Ok(()));
    assert_eq!(ok([10, 0, 0, 4]), Ok(()));
    assert_eq!(ok([6, 5, 0, 0]), Err(FadeRefusal::TooWide { axis: Axis::Keys, max: 10 }));
    assert_eq!(ok([0, 0, 3, 2]), Err(FadeRefusal::TooWide { axis: Axis::Velocities, max: 4 }));
    assert_eq!(ok([0, 128, 0, 0]), Err(FadeRefusal::Range));
    assert_eq!(check_fades([60, 60], [100, 100], &Fades::of([0, 0, 0, 1]), true), Err(FadeRefusal::PadBound));
    assert_eq!(check_fades([60, 60], [100, 100], &Fades::default(), true), Ok(()));
    assert_eq!(ok([6, 5, 0, 0]).unwrap_err().to_string(), "Key fades are at most 10 keys together for this zone.");
}

#[test]
fn conversion_to_adr_0034_edges_gives_the_same_factors() {
    // A rack zone with fade_low = low + lo_w and fade_high = high - hi_w gives, at every value,
    // the factor these widths give: (k + 1)/(n + 1) on both sides, 1 between, 0 outside.
    let mut checked = 0;
    for (lo, hi) in [(0u8, 127u8), (36, 48), (60, 63), (1, 127), (100, 101), (64, 64)] {
        let span = hi - lo;
        for lo_w in 0..=span.min(12) {
            for hi_w in 0..=(span - lo_w).min(12) {
                let rack = crate::Zone { low: lo, high: hi, fade_low: lo + lo_w, fade_high: hi - hi_w };
                let used = used_pair([lo, hi], lo_w, hi_w);
                assert_eq!(used, [lo_w, hi_w], "fitting widths are used as stored");
                for x in 0..=127u8 {
                    let (num, den) = factor(x, [lo, hi], used);
                    assert_eq!(rack.gain(x), f32::from(num) / f32::from(den), "zone {lo}..{hi}, widths {lo_w}/{hi_w}, value {x}");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 10_000);
    // ADR 0034's own example: a fade from 60 to 63 gives 60..63 the factors 1/4, 1/2, 3/4 and 1.
    let got: Vec<(u8, u8)> = (60..=63).map(|x| factor(x, [60, 80], [3, 0])).collect();
    assert_eq!(got, [(1, 4), (2, 4), (3, 4), (1, 1)]);
}
