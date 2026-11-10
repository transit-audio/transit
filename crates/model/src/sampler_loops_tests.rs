use super::*;
use crate::SourceId;
use crate::sampler_patch::{Marker, Markers, PadEntry, PatchState, SamplerPatch, SliceBinding, SliceLoop, SliceMode, SourceRef};
use serde_json::{Value, json};

/// A patch over source 0 (48,000 frames) with a loop 0..48,000 whose Manual set has slices
/// id 1 at 0, id 2 at 12,000 (one frame long), id 3 at 12,001 and id 4 at 30,000 (to the end).
fn sliced() -> PatchV1 {
    let marker = |id, frame| Marker { id, frame, ..Marker::default() };
    let manual = Markers { markers: vec![marker(1, 0), marker(2, 12_000), marker(3, 12_001), marker(4, 30_000)], next_id: 5, ..Markers::default() };
    PatchV1 {
        sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }],
        slicing: Some(SliceLoop { source: 0, start: 0, end: 48_000, mode: SliceMode::Manual, manual: Some(manual), ..SliceLoop::default() }),
        ..PatchV1::default()
    }
}

/// Zone `id` bound to Manual slice `slice` on pad `id`, Gate, looping `mode`, its stored fields
/// the whole source.
fn bound(p: &mut PatchV1, id: u16, slice: u32, mode: LoopMode) {
    let mut z = ZoneEntry::whole(id, 0, 48_000);
    z.slice = Some(SliceBinding { set: SliceSet::Manual, id: slice, ..SliceBinding::default() });
    z.loop_mode = mode;
    p.zones.push(z);
    p.set_pad(PadEntry { zone: Some(id), ..p.pad(id as u8) });
}

/// Loads `raw` and asserts it loads audible (ready, no load note) and writes back unchanged.
fn loads_audible(raw: &Value) -> PatchV1 {
    let loaded = SamplerPatch::from_value(raw.clone());
    let PatchState::Ready(p) = &loaded.state else { panic!("the patch loaded opaque: {:?}", loaded.state) };
    assert_eq!(loaded.note("t"), None);
    assert_eq!(p.validate(), Ok(()), "the store check accepts it too");
    assert_eq!(&serde_json::to_value(&loaded).unwrap(), raw, "opening writes nothing: the stored fields are written back as they were");
    (**p).clone()
}

#[test]
fn crossfade_ms_to_frames_rounding_at_44100_and_48000() {
    // (tenths of a millisecond, rate, frames): floor(m * r / 1000 + 1/2), ties upward.
    for (tenths, rate, frames) in [
        (47, 48_000, 226),
        (1, 48_000, 5),
        (10, 48_000, 48),
        (94, 48_000, 451),
        (0, 48_000, 0),
        (47, 44_100, 207),
        (1, 44_100, 4),
        (10, 44_100, 44),
        (50, 44_100, 221),  // 220.5: a tie, upward
        (150, 44_100, 662), // 661.5
        (100, 44_100, 441),
    ] {
        assert_eq!(crossfade_frames(tenths, rate), frames, "{tenths} tenths at {rate} Hz");
    }
    // The display: X * 1000 / r to one decimal, ties upward.
    assert_eq!(crossfade_ms_text(226, 48_000), "4.7");
    assert_eq!(crossfade_ms_text(12, 48_000), "0.3", "0.25 ms is a tie, shown upward");
    assert_eq!(crossfade_ms_text(221, 44_100), "5.0");
    assert_eq!(crossfade_ms_text(220, 44_100), "5.0");
    assert_eq!(crossfade_ms_text(0, 44_100), "0.0");
    assert_eq!(crossfade_tenths(5, 0), 0);
    // The inverse returns every one-decimal entry (a frame lasts less than half a tenth).
    for rate in [44_100, 48_000] {
        for t in 0..=2000 {
            assert_eq!(crossfade_tenths(crossfade_frames(t, rate), rate), t, "{t} tenths at {rate} Hz");
        }
    }
    assert_eq!(tenths_of_ms(4.7), Some(47));
    assert_eq!(tenths_of_ms(0.25), Some(3), "ties upward");
    assert_eq!(tenths_of_ms(-0.1), None);
    assert_eq!(tenths_of_ms(f64::NAN), None);
}

#[test]
fn crossfade_used_is_zero_unless_forward() {
    let mut p = PatchV1 { sources: vec![SourceRef::default()], ..PatchV1::default() };
    let mut z = ZoneEntry::whole(0, 0, 4000);
    (z.loop_start, z.loop_end, z.crossfade) = (100, 1100, 300);
    p.zones.push(z);
    let mut used = |mode, play| {
        p.zones[0].loop_mode = mode;
        p.zones[0].mode = play;
        (p.loop_used(0).unwrap(), p.crossfade_used(0).unwrap())
    };
    assert_eq!(used(LoopMode::Forward, PlayMode::Gate), (LoopMode::Forward, 300));
    assert_eq!(used(LoopMode::PingPong, PlayMode::Gate), (LoopMode::PingPong, 0));
    assert_eq!(used(LoopMode::Off, PlayMode::Gate), (LoopMode::Off, 0));
    assert_eq!(used(LoopMode::Forward, PlayMode::OneShot), (LoopMode::Off, 0));
}

#[test]
fn crossfade_absent_at_zero_and_round_trips_otherwise() {
    let z = ZoneEntry::whole(0, 0, 4000);
    assert!(serde_json::to_value(&z).unwrap().get("crossfade").is_none());
    let z = ZoneEntry { crossfade: 450, ..z };
    let v = serde_json::to_value(&z).unwrap();
    assert_eq!(v["crossfade"], json!(450));
    assert_eq!(serde_json::from_value::<ZoneEntry>(v).unwrap(), z);
    let old = json!({"id": 0, "source": 0, "start": 0, "end": 4000, "loop_mode": "forward", "loop_start": 0, "loop_end": 4000});
    assert_eq!(serde_json::from_value::<ZoneEntry>(old).unwrap().crossfade, 0, "a zone from before the field has none");
}

#[test]
fn slice_loop_follows_marker_move_in_the_derived_region() {
    let mut p = sliced();
    bound(&mut p, 0, 3, LoopMode::Forward);
    bound(&mut p, 1, 4, LoopMode::Off);
    let region = |p: &PatchV1, id: u16| derived_region(p.zone(id).unwrap(), p).unwrap();
    assert_eq!(region(&p, 0), DerivedRegion { source: 0, start: 12_001, end: 30_000, loop_start: 12_001, loop_end: 30_000 });
    assert_eq!(
        region(&p, 1),
        DerivedRegion { source: 0, start: 30_000, end: 48_000, loop_start: 30_000, loop_end: 48_000 },
        "the last slice runs to E"
    );
    // Moving the next boundary moves the slice's end and its loop end with it.
    let set = p.slicing.as_mut().unwrap().manual.as_mut().unwrap();
    set.move_to(0, 48_000, 4, 20_000).unwrap();
    assert_eq!(region(&p, 0), DerivedRegion { source: 0, start: 12_001, end: 20_000, loop_start: 12_001, loop_end: 20_000 });
    assert_eq!(region(&p, 1).start, 20_000);
    // Moving its own marker moves its start and loop start.
    let set = p.slicing.as_mut().unwrap().manual.as_mut().unwrap();
    set.move_to(0, 48_000, 3, 15_000).unwrap();
    assert_eq!(region(&p, 0), DerivedRegion { source: 0, start: 15_000, end: 20_000, loop_start: 15_000, loop_end: 20_000 });
    // Removing the next marker merges: the slice now runs to E.
    let set = p.slicing.as_mut().unwrap().manual.as_mut().unwrap();
    set.remove(0, 48_000, 4).unwrap();
    assert_eq!(region(&p, 0), DerivedRegion { source: 0, start: 15_000, end: 48_000, loop_start: 15_000, loop_end: 48_000 });
    // The stored fields were never touched (the write-through is the commands'), and an unbound
    // zone derives nothing.
    assert_eq!((p.zone(0).unwrap().start, p.zone(0).unwrap().end), (0, 48_000));
    let mut q = sliced();
    q.zones.push(ZoneEntry::whole(5, 0, 100));
    assert_eq!(derived_region(q.zone(5).unwrap(), &q), None);
}

#[test]
fn slice_edit_leaving_crossfade_above_half_plays_the_used_value() {
    let mut p = sliced();
    bound(&mut p, 0, 3, LoopMode::Forward);
    p.zone_mut(0).unwrap().crossfade = 4000;
    assert_eq!(crossfade_max(p.zone(0).unwrap(), &p), 8999, "L = 17,999 when it was set");
    assert_eq!(p.crossfade_used(0), Some(4000));
    // A marker move shortens the slice to L = 2,999: the stored value stands, the used one is
    // floor(L / 2).
    p.slicing.as_mut().unwrap().manual.as_mut().unwrap().move_to(0, 48_000, 4, 15_000).unwrap();
    assert_eq!(p.zone(0).unwrap().crossfade, 4000);
    assert_eq!(p.crossfade_used(0), Some(1499));
    assert_eq!(p.validate(), Ok(()));
    // Lengthened again, the stored value plays in full.
    p.slicing.as_mut().unwrap().manual.as_mut().unwrap().move_to(0, 48_000, 4, 40_000).unwrap();
    assert_eq!(p.crossfade_used(0), Some(4000));
}

#[test]
fn one_shot_with_loop_loads_as_one_shot() {
    let mut p = sliced();
    let mut z = ZoneEntry::whole(0, 0, 48_000);
    (z.mode, z.loop_mode, z.loop_start, z.loop_end, z.crossfade) = (PlayMode::OneShot, LoopMode::Forward, 100, 1100, 40);
    p.zones.push(z);
    bound(&mut p, 1, 3, LoopMode::PingPong);
    p.zone_mut(1).unwrap().mode = PlayMode::OneShot;
    let p = loads_audible(&serde_json::to_value(&p).unwrap());
    assert_eq!(p.zone(0).unwrap().loop_mode, LoopMode::Forward, "the stored mode is kept");
    for id in [0, 1] {
        assert_eq!((p.loop_used(id), p.crossfade_used(id)), (Some(LoopMode::Off), Some(0)), "zone {id}");
    }
}

#[test]
fn one_frame_slice_loop_plays_unlooped() {
    let mut p = sliced();
    bound(&mut p, 0, 2, LoopMode::Forward);
    // The fields as a write-through left them when a marker edit shrank the slice to one frame.
    let z = p.zone_mut(0).unwrap();
    (z.start, z.end, z.loop_start, z.loop_end, z.crossfade) = (12_000, 12_001, 12_000, 12_001, 10);
    let p = loads_audible(&serde_json::to_value(&p).unwrap());
    assert_eq!(derived_region(p.zone(0).unwrap(), &p).unwrap().loop_len(), 1);
    assert_eq!(p.loop_used(0), Some(LoopMode::Off));
    assert_eq!(p.crossfade_used(0), Some(0));
}

#[test]
fn disagreeing_stored_slice_fields_load_with_the_slice_values_and_open_writes_nothing() {
    let raw = json!({
        "schema": 1,
        "sources": [{"source": 1}],
        "zones": [
            // Region and loop both disagree with slice 3 (12,001..30,000).
            {"id": 0, "source": 0, "start": 0, "end": 48000, "loop_mode": "forward", "loop_start": 5, "loop_end": 7,
             "slice": {"set": "manual", "id": 3}},
            // Fields that would not even be a region for an unbound zone.
            {"id": 1, "source": 0, "start": 900, "end": 3, "loop_mode": "ping_pong", "loop_start": 40, "loop_end": 41,
             "slice": {"set": "manual", "id": 4}}
        ],
        "pads": [{"index": 0, "zone": 0}, {"index": 1, "zone": 1}],
        "loop": {"source": 0, "start": 0, "end": 48000, "mode": "manual",
                 "manual": {"markers": [{"id": 1, "frame": 0}, {"id": 2, "frame": 12000}, {"id": 3, "frame": 12001}, {"id": 4, "frame": 30000}], "next_id": 5}}
    });
    // Normalized once (the defaults written out); the stored fields stay the disagreeing ones.
    let raw = serde_json::to_value(serde_json::from_value::<PatchV1>(raw).unwrap()).unwrap();
    assert_eq!((&raw["zones"][0]["start"], &raw["zones"][0]["loop_start"], &raw["zones"][1]["end"]), (&json!(0), &json!(5), &json!(3)));
    let p = loads_audible(&raw);
    let eff = |id: u16| effective_region(p.zone(id).unwrap(), &p);
    assert_eq!(eff(0), DerivedRegion { source: 0, start: 12_001, end: 30_000, loop_start: 12_001, loop_end: 30_000 });
    assert_eq!(eff(1), DerivedRegion { source: 0, start: 30_000, end: 48_000, loop_start: 30_000, loop_end: 48_000 });
    assert_eq!(p.loop_used(0), Some(LoopMode::Forward));
    assert_eq!(p.loop_used(1), Some(LoopMode::PingPong));
    // The same fields on an unbound zone stay refused (the body's checks stand there).
    let mut unbound = raw.clone();
    unbound["zones"][1].as_object_mut().unwrap().remove("slice");
    assert!(matches!(SamplerPatch::from_value(unbound).state, PatchState::Invalid { .. }));
}

#[test]
fn crossfade_above_half_loop_loads_clamped() {
    let mut p = sliced();
    let mut z = ZoneEntry::whole(0, 0, 48_000);
    (z.loop_mode, z.loop_start, z.loop_end, z.crossfade) = (LoopMode::Forward, 100, 111, 9);
    p.zones.push(z);
    bound(&mut p, 1, 3, LoopMode::Forward);
    p.zone_mut(1).unwrap().crossfade = u64::MAX;
    let p = loads_audible(&serde_json::to_value(&p).unwrap());
    assert_eq!((p.zone(0).unwrap().crossfade, p.crossfade_used(0)), (9, Some(5)), "L = 11");
    assert_eq!((p.zone(1).unwrap().crossfade, p.crossfade_used(1)), (u64::MAX, Some(8999)), "the slice's L = 17,999");
}

#[test]
fn pingpong_with_stored_crossfade_loads_with_zero_used() {
    let mut p = sliced();
    let mut z = ZoneEntry::whole(0, 0, 48_000);
    (z.loop_mode, z.loop_start, z.loop_end, z.crossfade) = (LoopMode::PingPong, 100, 1100, 300);
    p.zones.push(z);
    bound(&mut p, 1, 3, LoopMode::PingPong);
    p.zone_mut(1).unwrap().crossfade = 20;
    let p = loads_audible(&serde_json::to_value(&p).unwrap());
    for id in [0, 1] {
        assert_eq!(p.loop_used(id), Some(LoopMode::PingPong), "zone {id} plays audible, ping-pong");
        assert_eq!(p.crossfade_used(id), Some(0), "zone {id}");
    }
    assert_eq!(p.zone(0).unwrap().crossfade, 300);
}
