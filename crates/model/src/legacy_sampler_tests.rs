#![allow(clippy::unwrap_used)]
use super::*;
use crate::sampler_patch::PatchV1;
use crate::{ChannelFormat, MacroMapping, Mapping, MidiSource, Rack, Source, SourceKind, TrackKind, UserPage};
use serde_json::{Value, json};

fn plugin_lane(slot: u8, param: &str) -> AutoParam {
    AutoParam::Plugin { slot, param: param.into() }
}

fn map(t: TrackId, slot: u8, param: &str, cc: u8) -> Mapping {
    let source = MidiSource { port: "kb".into(), channel: Some(0), kind: SourceKind::Cc, number: cc };
    Mapping::new(source, crate::ControlTarget { strip: Strip::Track(t), control: Control::Plugin { slot, param: param.into() } })
}

fn source(s: &mut Session, path: &str) -> SourceId {
    let id = SourceId(s.alloc());
    s.sources.push(Source {
        id,
        name: "Hit".into(),
        path: path.into(),
        channels: 1,
        frames: 4_410,
        sample_rate: 44_100,
        format: transit_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
        disk: false,
    });
    id
}

/// The one-sample Sampler as an old session stored it: a sample, every one of the 19 ids at a value
/// other than its default, a preset name and a user page.
fn one_sample(sample: Option<SourceId>) -> Insert {
    let mut i = Insert::new(SAMPLER_ID);
    for (n, id) in RETIRED_PARAMS.iter().enumerate() {
        i.params.insert((*id).into(), 1.0 + n as f32);
    }
    i.sample = sample;
    i.preset = "Plucky".into();
    let mut page = UserPage::named("Mine");
    page.slots[0] = Some("root_note".into());
    i.pages.push(page);
    i
}

fn instrument(s: &Session, t: TrackId) -> &Insert {
    s.track(t).unwrap().instrument.as_ref().unwrap()
}

/// A session with an old one-sample Sampler on "Keys" (lanes, a mapping, a macro target and a page
/// on its old ids) and a Macro Oscillator on "Lead" with the same kinds of targets.
fn old_session() -> (Session, TrackId, TrackId, TrackId, SourceId) {
    let mut s = Session::default();
    let sid = source(&mut s, "Audio Files/Hit.wav");
    let keys = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let lead = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
    let mut worn = one_sample(Some(sid));
    (worn.bypass, worn.active) = (true, false);
    s.track_mut(keys).unwrap().instrument = Some(worn);
    s.track_mut(lead).unwrap().instrument = Some(Insert::new("macro_oscillator"));
    for t in [keys, lead] {
        let tr = s.track_mut(t).unwrap();
        tr.lane_mut(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "attack")).set_point(0, 0.5);
        tr.lane_mut(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "level")).set_point(0, 0.5);
        tr.lane_mut(&plugin_lane(0, "attack")).set_point(0, 0.5);
    }
    s.midi_mappings.set(map(keys, AutoParam::INSTRUMENT_SLOT, "root_note", 1)).unwrap();
    s.midi_mappings.set(map(keys, 0, "root_note", 2)).unwrap();
    s.midi_mappings.set(map(lead, AutoParam::INSTRUMENT_SLOT, "level", 3)).unwrap();
    let mut rack = Rack::default();
    for (t, slot) in [(keys, AutoParam::INSTRUMENT_SLOT), (keys, 0), (lead, AutoParam::INSTRUMENT_SLOT)] {
        rack.macros[0].mappings.push(MacroMapping { track: t, slot, param: "level".into(), ..MacroMapping::default() });
    }
    let host = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Rack"));
    s.track_mut(host).unwrap().rack = Some(rack);
    (s, keys, lead, host, sid)
}

#[test]
fn an_old_one_sample_sampler_opens_as_an_empty_kit_with_its_note() {
    let (s, keys, lead, host, sid) = old_session();
    let text = s.to_json().unwrap();
    let (opened, stored) = Session::from_json_as_saved(&text).unwrap();

    let ins = instrument(&opened, keys);
    assert_eq!(ins.plugin, SAMPLER_ID);
    let kit = ins.sampler_patch.as_ref().and_then(SamplerPatch::ready).expect("a ready patch");
    assert_eq!(kit, &PatchV1::default(), "an empty kit");
    assert_eq!((kit.seed, kit.pad_start_note, kit.zones.len(), kit.sources.len()), (1, 36, 0, 0));
    assert_eq!(ins.sample, None);
    assert!(ins.params.is_empty(), "none of the 19 ids: {:?}", ins.params);
    assert!(ins.pages.is_empty(), "the user pages went");
    assert_eq!(ins.preset, Insert::new("x").preset, "the preset label is reset");
    assert!(ins.bypass && !ins.active, "bypass and active carry over");
    assert!(opened.source(sid).is_some(), "the source stays in the session");

    let tr = opened.track(keys).unwrap();
    assert!(tr.lane(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "attack")).is_none());
    assert!(tr.lane(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "level")).is_none());
    assert!(tr.lane(&plugin_lane(0, "attack")).is_some(), "an insert's lane stays");
    let other = opened.track(lead).unwrap();
    assert!(other.lane(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "attack")).is_some(), "other tracks keep theirs");
    assert!(other.lane(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "level")).is_some());
    assert_eq!(opened.midi_mappings.len(), 2, "the old instrument's mapping went, the insert's and the other track's stay");
    let kept = &opened.track(host).unwrap().rack.as_ref().unwrap().macros[0].mappings;
    assert_eq!(kept.iter().map(|m| (m.track, m.slot)).collect::<Vec<_>>(), vec![(keys, 0), (lead, AutoParam::INSTRUMENT_SLOT)]);

    assert_eq!(
        opened.load_notes,
        vec![
            "track \"Keys\": the old one-sample Sampler was removed and is now an empty kit; its sample \"Hit.wav\" stays in the \
             session's files and can be loaded onto a pad again; dropped 2 automation lane(s), 1 MIDI mapping(s), 1 rack macro \
             target(s), 1 user page(s)"
                .to_string(),
            "1 track(s) had the old one-sample Sampler and now have an empty Sampler kit".to_string(),
        ]
    );

    // The caller gets the session as stored, so the open reads as modified.
    let stored = stored.expect("the drop is a change");
    assert!(is_one_sample(instrument(&stored, keys)));
    assert_eq!(instrument(&stored, keys).sample, Some(sid));

    // The save writes the empty kit and lists it in `needs`; a second open has nothing to drop.
    let saved = opened.to_json().unwrap();
    let v: Value = serde_json::from_str(&saved).unwrap();
    assert!(v["needs"].as_array().unwrap().contains(&json!("sampler_patch")), "{}", v["needs"]);
    let slot = v["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Keys").unwrap()["instrument"].clone();
    assert!(slot.get("sample").is_none() && slot["params"] == json!({}), "{slot}");
    assert_eq!(slot["sampler_patch"]["pad_start_note"], 36);
    let (again, stored) = Session::from_json_as_saved(&saved).unwrap();
    assert!(stored.is_none(), "a saved kit loads clean");
    assert!(again.load_notes.is_empty(), "{:?}", again.load_notes);
}

#[test]
fn a_one_sample_sampler_without_a_sample_or_targets_gets_the_short_note() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Empty"));
    let mut ins = one_sample(None);
    ins.pages.clear();
    s.track_mut(t).unwrap().instrument = Some(ins);
    assert_eq!(retire_one_sample_sampler(&mut s), 1);
    assert_eq!(
        s.load_notes,
        vec![
            "track \"Empty\": the old one-sample Sampler was removed and is now an empty kit".to_string(),
            "1 track(s) had the old one-sample Sampler and now have an empty Sampler kit".to_string(),
        ]
    );
    assert_eq!(retire_one_sample_sampler(&mut s), 0, "a second pass finds nothing");
    assert_eq!(s.load_notes.len(), 2);
}

#[test]
fn a_sample_the_session_no_longer_lists_is_not_named() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Gone"));
    let mut ins = one_sample(Some(SourceId(999)));
    ins.pages.clear();
    s.track_mut(t).unwrap().instrument = Some(ins);
    retire_one_sample_sampler(&mut s);
    assert_eq!(s.load_notes[0], "track \"Gone\": the old one-sample Sampler was removed and is now an empty kit");
}

#[test]
fn patches_in_every_state_and_kits_holding_the_old_ids_are_never_touched() {
    let ready = {
        let mut p = SamplerPatch::new();
        p.ready_mut().unwrap().seed = 7;
        p
    };
    let unknown = SamplerPatch::from_value(json!({"schema": 99, "future": [1, 2, 3]}));
    let invalid = SamplerPatch::from_value(json!({"schema": 1, "zones": "not a list"}));
    assert!(unknown.ready().is_none() && invalid.ready().is_none(), "the two opaque states");
    let mut s = Session::default();
    let mut tracks = Vec::new();
    for (name, patch) in [("Ready", ready), ("Unknown", unknown), ("Invalid", invalid)] {
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(name));
        // A kit saved by a build that wrote the 19 defaults beside the patch, with a lane on one.
        let mut ins = one_sample(None);
        ins.pages.clear();
        ins.sampler_patch = Some(patch);
        s.track_mut(t).unwrap().instrument = Some(ins);
        s.track_mut(t).unwrap().lane_mut(&plugin_lane(AutoParam::INSTRUMENT_SLOT, "level")).set_point(0, 0.5);
        tracks.push(t);
    }
    assert!(!uses_one_sample_sampler(&s));
    let text = s.to_json().unwrap();
    let (opened, stored) = Session::from_json_as_saved(&text).unwrap();
    assert!(stored.is_none(), "the open reads unmodified");
    assert!(!opened.load_notes.iter().any(|n| n.contains("one-sample")), "{:?}", opened.load_notes);
    let before: Value = serde_json::from_str(&text).unwrap();
    let after: Value = serde_json::from_str(&opened.to_json().unwrap()).unwrap();
    for (i, t) in tracks.iter().enumerate() {
        assert_eq!(after["tracks"][i]["instrument"], before["tracks"][i]["instrument"], "track {i}: the slot saves byte for byte");
        assert_eq!(after["tracks"][i]["automation"], before["tracks"][i]["automation"], "track {i}: the lane stays");
        assert_eq!(instrument(&opened, *t).params.len(), RETIRED_PARAMS.len());
    }
}

#[test]
fn the_slot_part_alone_leaves_other_slots_and_the_track_alone() {
    let mut other = Insert::new("macro_oscillator");
    other.params.insert("level".into(), -3.0);
    let kept = other.clone();
    assert_eq!(retire_slot(&mut other), None);
    assert_eq!(other, kept);
    let mut kit = Insert::new(SAMPLER_ID);
    kit.sampler_patch = Some(SamplerPatch::new());
    let kept = kit.clone();
    assert_eq!(retire_slot(&mut kit), None);
    assert_eq!(kit, kept);
    let mut old = one_sample(Some(SourceId(4)));
    old.params.insert("unknown_id".into(), 2.0);
    assert_eq!(retire_slot(&mut old), Some(Some(SourceId(4))));
    assert_eq!(old.params.keys().collect::<Vec<_>>(), vec!["unknown_id"], "only the 19 ids go");
    assert!(old.sampler_patch.as_ref().and_then(SamplerPatch::ready).is_some() && old.sample.is_none() && old.pages.is_empty());
    assert_eq!(old.preset, "<factory default>");
}

#[test]
fn a_track_line_is_found_by_its_track() {
    let line = track_line("Keys", None, &[]);
    assert!(is_track_line(&line, "Keys"));
    assert!(!is_track_line(&line, "Key"));
    assert!(!is_track_line("track \"Keys\": something else", "Keys"));
}
