use super::*;
use crate::{ChannelFormat, Insert, Session, TrackKind};
use serde_json::json;

/// A patch with every field away from its default.
fn full() -> PatchV1 {
    let mut extra = Extra::new();
    extra.insert("future".into(), json!({"x": [1, 2]}));
    let zone = ZoneEntry {
        id: 7,
        source: 1,
        start: 10,
        end: 4000,
        loop_mode: LoopMode::PingPong,
        loop_start: 100,
        loop_end: 3000,
        keys: [36, 72],
        velocities: [20, 110],
        root: 48,
        transpose: -12.0,
        cents: 25.0,
        reverse: true,
        mode: PlayMode::OneShot,
        key_tracking: false,
        attack_ms: 5.0,
        decay_ms: 200.0,
        sustain: 0.5,
        release_ms: 80.0,
        declick_ms: 2.0,
        velocity_sensitivity: 0.25,
        membership: None,
        slice: Some(SliceBinding { set: SliceSet::Manual, id: 9, extra: extra.clone() }),
        label: Some("snare".into()),
        extra: extra.clone(),
    };
    let layered = ZoneEntry {
        id: 200,
        membership: Some(Membership { group: 3, layer: 2, alternative: 1, extra: extra.clone() }),
        ..ZoneEntry::whole(200, 0, 512)
    };
    let marker = |id, frame, label: Option<&str>| Marker { id, frame, label: label.map(str::to_string), extra: extra.clone() };
    PatchV1 {
        schema: SCHEMA,
        seed: 0xDEAD_BEEF,
        sources: vec![
            SourceRef { source: SourceId(11), ownership: Ownership::Managed, extra: Extra::new() },
            SourceRef { source: SourceId(12), ownership: Ownership::Referenced, extra: extra.clone() },
        ],
        zones: vec![zone, layered],
        pads: vec![PadEntry {
            index: 5,
            zone: Some(7),
            note: Some(40),
            mute: true,
            solo: true,
            choke: 16,
            name: Some("Snare".into()),
            extra: extra.clone(),
        }],
        groups: vec![GroupEntry { id: 3, round_robin: RoundRobin::Random, extra: extra.clone() }],
        slicing: Some(SliceLoop {
            source: 1,
            start: 0,
            end: 4000,
            mode: SliceMode::Manual,
            snap: false,
            detector: Detector { source: AutoSource::Transients, sensitivity: 0.8, extra: extra.clone() },
            division: Division::ThirtySecond,
            bars: Some(2),
            grid_bpm: Some(120.5),
            root: 36,
            beat_length: Some(8.0),
            auto: Some(Markers { markers: vec![marker(1, 0, Some("one")), marker(2, 2000, None)], next_id: 0, extra: extra.clone() }),
            manual: Some(Markers { markers: vec![marker(8, 0, None), marker(9, 1000, Some("ghost"))], next_id: 0, extra: extra.clone() }),
            markers: None,
            extra: extra.clone(),
        }),
        controls: [("zone008_level".to_string(), -6.0), ("zone008_mod1_amount".to_string(), 25.0)].into_iter().collect(),
        extra,
    }
}

#[test]
fn every_field_populated_round_trips_and_every_key_is_written() {
    let p = full();
    assert_eq!(p.validate(), Ok(()));
    let v = serde_json::to_value(SamplerPatch { state: PatchState::Ready(Box::new(p.clone())) }).unwrap();
    for key in ["schema", "seed", "sources", "zones", "pads", "groups", "loop", "controls", "future"] {
        assert!(v.get(key).is_some(), "{key} missing: {v}");
    }
    let l = &v["loop"];
    for key in ["mode", "snap", "detector", "division", "bars", "root", "beat_length", "auto", "manual", "future"] {
        assert!(l.get(key).is_some(), "loop.{key} missing: {l}");
    }
    assert_eq!(l["division"], "1/32");
    assert_eq!(v["sources"][1]["ownership"], "referenced");
    assert!(v["sources"][0].get("ownership").is_none(), "managed is the default and not written");
    let back = SamplerPatch::from_value(v);
    assert_eq!(back.ready(), Some(&p));
}

#[test]
fn every_field_defaulted_loads_as_the_defaults_and_writes_only_the_schema() {
    let empty = SamplerPatch::from_value(json!({}));
    assert_eq!(empty.ready(), Some(&PatchV1::default()));
    let p = PatchV1::default();
    assert_eq!((p.seed, p.schema), (1, 1));
    assert_eq!(serde_json::to_value(SamplerPatch::new()).unwrap(), json!({"schema": 1}));
    // A zone with only its id, source and region loads with the Instrument view's defaults.
    let v = json!({"sources": [{"source": 4}], "zones": [{"id": 3, "end": 100}]});
    let z = SamplerPatch::from_value(v).ready().unwrap().zones[0].clone();
    assert_eq!(z, ZoneEntry { id: 3, end: 100, ..ZoneEntry::default() });
    assert_eq!((z.root, z.mode, z.key_tracking, z.keys, z.velocities), (60, PlayMode::Gate, true, [0, 127], [1, 127]));
    // A loop with no fields set: Auto, snap on, 1/16, From tempo, root 60, no sets.
    let v = json!({"sources": [{"source": 4}], "loop": {"end": 10}});
    let l = SamplerPatch::from_value(v).ready().unwrap().slicing.clone().unwrap();
    assert_eq!(l, SliceLoop { end: 10, ..SliceLoop::default() });
    assert!(l.snap && l.mode == SliceMode::Auto && l.division == Division::Sixteenth && l.bars.is_none() && l.root == 60);
}

#[test]
fn unknown_fields_are_kept_at_every_level() {
    let v = json!({
        "schema": 1,
        "next": 1,
        "sources": [{"source": 2, "hash": "abc"}],
        "zones": [{"id": 0, "end": 10, "lfo3": true, "membership": {"group": 1, "weight": 2}}],
        "pads": [{"index": 1, "color": "red"}],
        "groups": [{"id": 1, "xfade": 3}],
        "loop": {"end": 10, "zoom": 4, "detector": {"band": 2}, "manual": {"markers": [{"id": 1, "frame": 0, "tag": 1}], "cursor": 3}},
    });
    let p = SamplerPatch::from_value(v.clone());
    assert!(p.ready().is_some(), "{p:?}");
    let out = serde_json::to_value(&p).unwrap();
    let kept = [
        &out["next"],
        &out["sources"][0]["hash"],
        &out["zones"][0]["lfo3"],
        &out["zones"][0]["membership"]["weight"],
        &out["pads"][0]["color"],
        &out["groups"][0]["xfade"],
        &out["loop"]["zoom"],
        &out["loop"]["detector"]["band"],
        &out["loop"]["manual"]["cursor"],
        &out["loop"]["manual"]["markers"][0]["tag"],
    ];
    let want = [json!(1), json!("abc"), json!(true), json!(2), json!("red"), json!(3), json!(4), json!(2), json!(3), json!(1)];
    for (k, w) in kept.iter().zip(&want) {
        assert_eq!(*k, w, "{out}");
    }
    assert_eq!(SamplerPatch::from_value(out), p, "and they read back the same");
}

#[test]
fn an_unknown_schema_is_kept_whole_plays_nothing_and_says_so() {
    let v = json!({"schema": 2, "sources": [{"source": 41}, {"source": 42}], "voices": "many", "loop": {}});
    let p = SamplerPatch::from_value(v.clone());
    assert!(matches!(p.state, PatchState::Unknown { schema: 2, .. }));
    assert!(p.ready().is_none());
    assert_eq!(p.seed(), 1);
    assert_eq!(p.media(), vec![SourceId(41), SourceId(42)], "its media references are kept");
    assert!(p.uses_slicing());
    assert!(p.note("Drums").unwrap().contains("schema 2"));
    assert_eq!(serde_json::to_value(&p).unwrap(), v, "never rewritten into an empty patch");
}

#[test]
fn an_invalid_patch_is_kept_opaque_with_a_note() {
    let cases = [
        json!({"sources": [{"source": 1}], "zones": [{"id": 0, "start": 10, "end": 5}]}),
        json!({"zones": [{"id": 0, "end": 5}]}),
        json!({"sources": [{"source": 1}], "zones": [{"id": 300, "end": 5}]}),
        json!({"pads": [{"index": 0, "note": 61}]}),
        json!({"pads": [{"index": 0, "zone": 3}]}),
        json!({"controls": {"zone000_level": 1.0}}),
        json!({"sources": [{"source": 1}], "zones": [{"id": 0, "end": 5, "attack_ms": 1e9}]}),
        json!({"sources": [{"source": 1}], "loop": {"end": 10, "manual": {"markers": [{"id": 1, "frame": 5}, {"id": 2, "frame": 5}]}}}),
        json!({"seed": "x"}),
        json!([1, 2]),
        json!({"schema": "one"}),
    ];
    for v in cases {
        let p = SamplerPatch::from_value(v.clone());
        assert!(matches!(p.state, PatchState::Invalid { .. }), "{v} should be invalid: {p:?}");
        assert!(p.note("T").unwrap().contains("not valid"), "{v}");
        assert_eq!(serde_json::to_value(&p).unwrap(), v, "kept as saved");
    }
}

#[test]
fn a_first_marker_off_the_loop_start_is_kept_opaque() {
    // S is the first marker, fixed at the loop start: a set that starts elsewhere is invalid. The
    // text is in the writer's (sorted) key order, so a byte-identical save is checkable.
    let set = r#"{"markers":[{"frame":10,"id":1},{"frame":50,"id":2}]}"#;
    let auto = format!(r#"{{"loop":{{"auto":{set},"end":100}},"schema":1,"sources":[{{"source":1}}]}}"#);
    let manual = format!(r#"{{"loop":{{"end":100,"manual":{set}}},"schema":1,"sources":[{{"source":1}}]}}"#);
    for text in [auto, manual] {
        let p: SamplerPatch = serde_json::from_str(&text).unwrap();
        let PatchState::Invalid { reason, .. } = &p.state else { panic!("{text} should load opaque: {p:?}") };
        assert!(reason.contains("first marker is not at the loop start"), "{reason}");
        assert!(p.note("T").unwrap().contains("not valid"));
        assert_eq!(serde_json::to_string(&p).unwrap(), text, "kept byte for byte");
        // The same set with the loop starting at its first marker loads ready.
        let at_start = text.replace(r#""end":100"#, r#""end":100,"start":10"#);
        assert!(serde_json::from_str::<SamplerPatch>(&at_start).unwrap().ready().is_some(), "{at_start}");
    }
}

#[test]
fn only_populated_zones_nondefault_pads_and_nondefault_controls_are_written() {
    let mut p = PatchV1::default();
    p.set_pad(PadEntry { index: 3, note: Some(PadEntry::default_note(3)), ..PadEntry::default() });
    assert!(p.pads.is_empty(), "a pad at its defaults is not stored");
    p.set_pad(PadEntry { index: 3, choke: 2, ..PadEntry::default() });
    p.set_control("zone001_level", -3.0, 0.0);
    p.set_control("zone002_level", 0.0, 0.0);
    let v = serde_json::to_value(SamplerPatch { state: PatchState::Ready(Box::new(p.clone())) }).unwrap();
    assert_eq!(v, json!({"schema": 1, "pads": [{"index": 3, "choke": 2}], "controls": {"zone001_level": -3.0}}));
    p.set_control("zone001_level", 0.0, 0.0);
    p.set_pad(PadEntry { index: 3, ..PadEntry::default() });
    assert_eq!(serde_json::to_value(SamplerPatch { state: PatchState::Ready(Box::new(p)) }).unwrap(), json!({"schema": 1}));
}

#[test]
fn a_single_legacy_marker_set_loads_as_manual_and_two_sets_win() {
    let v = json!({"sources": [{"source": 1}], "loop": {"end": 100, "markers": [{"id": 4, "frame": 0, "label": "a"}, {"id": 5, "frame": 50}]}});
    let l = SamplerPatch::from_value(v).ready().unwrap().slicing.clone().unwrap();
    assert_eq!(l.mode, SliceMode::Manual);
    assert_eq!(l.manual.as_ref().map(|m| m.markers.iter().map(|x| (x.id, x.frame)).collect::<Vec<_>>()), Some(vec![(4, 0), (5, 50)]));
    assert!(l.auto.is_none(), "Auto is S only, not redetected");
    let both =
        json!({"sources": [{"source": 1}], "loop": {"end": 100, "markers": [{"id": 4, "frame": 0}], "auto": {"markers": [{"id": 1, "frame": 0}]}}});
    let l = SamplerPatch::from_value(both).ready().unwrap().slicing.clone().unwrap();
    assert_eq!((l.mode, l.manual.is_none()), (SliceMode::Auto, true), "explicit two-set state wins");
    assert!(!serde_json::to_string(&l).unwrap().contains("\"markers\":[{\"id\":4"), "the legacy key is never written");
}

fn session_with(patch: Option<SamplerPatch>) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut ins = Insert::new("sampler");
    ins.sampler_patch = patch;
    s.track_mut(t).unwrap().instrument = Some(ins);
    s
}

/// The `KNOWN_NEEDS` of the last release before the Sampler patch.
const PREVIOUS_RELEASE: [&str; 2] = ["clip_loop", "clip_scale_mode"];

#[test]
fn a_patch_registers_its_needs_and_older_readers_warn() {
    let plain = session_with(None);
    assert!(plain.needs().is_empty(), "an old one-sample Sampler needs nothing");
    let patched = session_with(Some(SamplerPatch::new()));
    assert_eq!(patched.needs(), vec!["sampler_patch"]);
    let mut p = PatchV1::default();
    p.sources.push(SourceRef::default());
    p.slicing = Some(SliceLoop { end: 10, ..SliceLoop::default() });
    let sliced = session_with(Some(SamplerPatch { state: PatchState::Ready(Box::new(p)) }));
    assert_eq!(sliced.needs(), vec!["sampler_patch", "sampler_slicing_modes"]);
    let v: Value = serde_json::from_str(&sliced.to_json().unwrap()).unwrap();
    assert_eq!(v["needs"], json!(["sampler_patch", "sampler_slicing_modes"]));
    // This build knows both: no note, and the patch comes back.
    let back = Session::from_json(&sliced.to_json().unwrap()).unwrap();
    assert!(back.load_notes.is_empty(), "{:?}", back.load_notes);
    assert!(back.tracks[0].instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().uses_slicing());
    // The previous release warns about both; a reader that knows the patch but not the two-set
    // extension warns about that alone.
    let old = crate::unknown_needs(&v, &PREVIOUS_RELEASE);
    assert_eq!(old, vec!["sampler_patch", "sampler_slicing_modes"]);
    assert!(crate::unknown_needs_note(&old).unwrap().contains("sampler_patch, sampler_slicing_modes"));
    let marker_aware = crate::unknown_needs(&v, &["clip_loop", "clip_scale_mode", "sampler_patch"]);
    assert_eq!(marker_aware, vec!["sampler_slicing_modes"]);
    // What the previous release does with the data: it has no `sampler_patch` field, ignores it
    // (no unknown-field refusal) and drops it on resave: the instrument is a Sampler without a
    // sample or patch, which plays silence (never an arbitrary first sample).
    let mut resaved = v.clone();
    resaved.as_object_mut().unwrap().remove("needs");
    for t in resaved["tracks"].as_array_mut().unwrap() {
        if let Some(i) = t.get_mut("instrument").and_then(Value::as_object_mut) {
            i.remove("sampler_patch");
        }
    }
    let old_view = Session::from_json(&resaved.to_string()).unwrap();
    let ins = old_view.tracks[0].instrument.as_ref().unwrap();
    assert!(ins.sampler_patch.is_none() && ins.sample.is_none() && ins.plugin == "sampler");
}

#[test]
fn an_opaque_patch_loads_with_its_note_and_is_saved_back() {
    let mut s = session_with(None);
    let raw = json!({"schema": 9, "sources": [{"source": 3}]});
    s.tracks[0].instrument.as_mut().unwrap().sampler_patch = Some(SamplerPatch::from_value(raw.clone()));
    let text = s.to_json().unwrap();
    let back = Session::from_json(&text).unwrap();
    assert_eq!(back.load_notes.len(), 1, "{:?}", back.load_notes);
    assert!(back.load_notes[0].contains("schema 9"));
    let v: Value = serde_json::from_str(&back.to_json().unwrap()).unwrap();
    assert_eq!(v["tracks"][0]["instrument"]["sampler_patch"], raw);
}

#[test]
fn control_ids_are_checked_by_form() {
    for ok in ["zone001_level", "zone256_mod16_curve", "zone010_future_field"] {
        assert!(is_control_id(ok), "{ok}");
    }
    for bad in ["zone000_level", "zone257_level", "zone01_level", "zone001_", "level", "zone001level", "zone0a1_level"] {
        assert!(!is_control_id(bad), "{bad}");
    }
}

#[test]
fn unused_sources_are_dropped_and_indices_renumbered() {
    let mut p = full();
    p.zones.retain(|z| z.id == 200);
    p.slicing = None;
    p.zones[0].source = 0;
    p.sources.push(SourceRef { source: SourceId(13), ..SourceRef::default() });
    p.zones.push(ZoneEntry::whole(1, 2, 10));
    p.drop_unused_sources();
    assert_eq!(p.sources.iter().map(|s| s.source.0).collect::<Vec<_>>(), vec![11, 13]);
    assert_eq!(p.zone(1).unwrap().source, 1);
}
