//! User pages on the slot record (ADR 0047 decision 4, slice 1 tests of decision 8.1): the
//! serialized shape and its defaults, the read rules, the `device_pages` need and the load note
//! for dropped pages.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::device_pages::{DeviceRef, MAX_USER_PAGES, PAGE_SLOTS, PagesError, UserPage, read_pages};
use crate::{ChannelFormat, Insert, MidiEffectSlot, Session, TrackKind};
use serde_json::{Value, json};

fn page(name: &str, ids: &[&str]) -> UserPage {
    let mut p = UserPage::named(name);
    for (slot, id) in p.slots.iter_mut().zip(ids) {
        *slot = Some((*id).to_owned());
    }
    p
}

/// An instrument track with a Macro Oscillator, an EQ on insert b and a transpose MIDI effect.
fn session() -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.name = "Keys".into();
    tr.instrument = Some(Insert::new("macro_oscillator"));
    tr.mixer.inserts[1] = Some(Insert::new("eq_7band"));
    tr.add_midi_effect(MidiEffectSlot::new("midi_transpose"), None);
    s
}

/// The `KNOWN_NEEDS` of the last release before user pages.
const PREVIOUS_RELEASE: [&str; 4] = ["clip_loop", "clip_scale_mode", "sampler_patch", "sampler_slicing_modes"];

#[test]
fn pages_write_eight_slots_with_null_and_absent_or_empty_pages_are_not_written() {
    let mut ins = Insert::new("clap:vendor.synth");
    let v = serde_json::to_value(&ins).unwrap();
    assert!(v.get("pages").is_none(), "an empty list is not written");
    let back: Insert = serde_json::from_value(v).unwrap();
    assert!(back.pages.is_empty(), "an absent field reads empty");
    ins.pages.push(page("User 1", &["1032", "1033"]));
    let v = serde_json::to_value(&ins).unwrap();
    assert_eq!(v["pages"], json!([{ "name": "User 1", "slots": ["1032", "1033", null, null, null, null, null, null] }]));
    let back: Insert = serde_json::from_value(v).unwrap();
    assert_eq!(back, ins);
    let mut fx = MidiEffectSlot::new("midi_transpose");
    assert!(serde_json::to_value(&fx).unwrap().get("pages").is_none());
    fx.pages.push(page("", &["semitones"]));
    let back: MidiEffectSlot = serde_json::from_value(serde_json::to_value(&fx).unwrap()).unwrap();
    assert_eq!(back, fx);
}

#[test]
fn sessions_without_pages_save_as_before() {
    // Every released corpus session: no `pages` key and no `device_pages` need appear on a save,
    // and a save of the save is the same bytes.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "transit") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let first = Session::from_json(&text).unwrap().to_json().unwrap();
        assert!(!first.contains("\"pages\""), "{}", path.display());
        assert!(!first.contains("device_pages"), "{}", path.display());
        let second = Session::from_json(&first).unwrap().to_json().unwrap();
        assert_eq!(first, second, "{}", path.display());
        n += 1;
    }
    assert!(n >= 2);
    let plain = session();
    assert!(plain.needs().is_empty());
    assert!(!plain.to_json().unwrap().contains("\"pages\""));
}

#[test]
fn the_need_is_listed_exactly_when_an_instrument_insert_or_midi_effect_has_pages() {
    let base = session();
    assert!(!base.needs().contains(&"device_pages"));
    for device in [DeviceRef::Instrument, DeviceRef::Insert(1), DeviceRef::MidiEffect(0)] {
        let mut s = base.clone();
        assert_eq!(s.tracks[0].set_device_pages(device, &[page("User 1", &["level"])]), Ok(true));
        assert_eq!(s.needs(), vec!["device_pages"], "{device}");
        let v: Value = serde_json::from_str(&s.to_json().unwrap()).unwrap();
        assert_eq!(v["needs"], json!(["device_pages"]), "{device}");
        // This build knows it: no note, the pages come back.
        let back = Session::from_json(&s.to_json().unwrap()).unwrap();
        assert!(back.load_notes.is_empty(), "{:?}", back.load_notes);
        assert_eq!(back.tracks[0].device_pages(device).unwrap(), s.tracks[0].device_pages(device).unwrap());
        // The previous release warns about it.
        let old = crate::unknown_needs(&v, &PREVIOUS_RELEASE);
        assert_eq!(old, vec!["device_pages"]);
        // A file with the marker removed still loads the pages.
        let mut unmarked = v.clone();
        unmarked.as_object_mut().unwrap().remove("needs");
        let back = Session::from_json(&unmarked.to_string()).unwrap();
        assert_eq!(back.tracks[0].device_pages(device).unwrap().len(), 1, "{device}");
        // Clearing the pages clears the need.
        assert_eq!(s.tracks[0].set_device_pages(device, &[]), Ok(true));
        assert!(s.needs().is_empty(), "{device}");
    }
}

#[test]
fn the_read_rules_never_fail_and_fix_each_case() {
    let long_id = "x".repeat(65);
    let ok_id = "y".repeat(64);
    let raw = json!([
        // Short: three slots pad with empty; the name is trimmed and cut to 32 characters.
        { "name": "  A name that is longer than thirty-two characters  ", "slots": ["a", "b", "c"] },
        // Long: entries past the eighth are dropped.
        { "name": "Long", "slots": ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"] },
        // Empty strings, non-strings and an over-long id read empty; a 64-character id stays.
        { "name": 7, "slots": ["", 12, null, {"id": "a"}, long_id, ok_id, true, "z"] },
        // Duplicates keep the first slot.
        { "slots": ["a", "b", "a", "c", "b", "a", null, "d"] },
        // Not an object, no slots, slots not an array.
        "junk",
        { "name": "No slots" },
        { "name": "Bad slots", "slots": "a" },
    ]);
    let (pages, dropped) = read_pages(&raw);
    assert_eq!(dropped, 0);
    let ids = |p: &UserPage| p.slots.iter().map(|s| s.as_deref().unwrap_or("-")).collect::<Vec<_>>().join(",");
    assert_eq!(pages[0].name, "A name that is longer than thirt");
    assert_eq!(pages[0].name.chars().count(), 32);
    assert_eq!(ids(&pages[0]), "a,b,c,-,-,-,-,-");
    assert_eq!(ids(&pages[1]), "1,2,3,4,5,6,7,8");
    assert_eq!(pages[2].name, "", "a non-string name reads empty");
    assert_eq!(pages[2].display_name(3), "User 3");
    assert_eq!(ids(&pages[2]), format!("-,-,-,-,-,{},-,z", "y".repeat(64)));
    assert_eq!(pages[3].display_name(4), "User 4");
    assert_eq!(ids(&pages[3]), "a,b,-,c,-,-,-,d");
    for p in &pages[4..] {
        assert_eq!(ids(p), "-,-,-,-,-,-,-,-");
    }
    assert_eq!(pages[5].name, "No slots");
    assert_eq!(pages.len(), 7);
    // Not an array: no pages, never an error.
    for junk in [json!("pages"), json!(3), json!({"name": "x"}), Value::Null] {
        assert_eq!(read_pages(&junk), (Vec::new(), 0));
        let mut v = serde_json::to_value(Insert::new("eq_7band")).unwrap();
        v["pages"] = junk;
        let ins: Insert = serde_json::from_value(v).unwrap();
        assert!(ins.pages.is_empty());
    }
    // A name of spaces past the cut reads without trailing space.
    assert_eq!(UserPage::read(&json!({"name": format!("{} b", "a".repeat(31))})).name, "a".repeat(31));
    assert_eq!(PAGE_SLOTS, 8);
}

#[test]
fn a_seventeenth_page_is_dropped_with_one_load_note_naming_the_track_and_device() {
    let s = session();
    let mut v: Value = serde_json::from_str(&s.to_json().unwrap()).unwrap();
    let pages: Vec<Value> = (0..MAX_USER_PAGES + 1).map(|i| json!({ "name": format!("P{i}"), "slots": ["level"] })).collect();
    v["tracks"][0]["mixer"]["inserts"][1]["pages"] = Value::Array(pages.clone());
    v["tracks"][0]["midi_effects"][0]["pages"] = Value::Array(pages[..MAX_USER_PAGES].to_vec());
    let back = Session::from_json(&v.to_string()).unwrap();
    let ins = back.tracks[0].mixer.inserts[1].as_ref().unwrap();
    assert_eq!(ins.pages.len(), MAX_USER_PAGES);
    assert_eq!(ins.pages.last().unwrap().name, "P15", "the first sixteen stay");
    assert_eq!(back.tracks[0].midi_effects[0].pages.len(), MAX_USER_PAGES, "sixteen is not too many");
    assert_eq!(back.load_notes.len(), 1, "{:?}", back.load_notes);
    assert!(back.load_notes[0].contains("track `Keys`, insert b: 1 macro strip page"), "{}", back.load_notes[0]);
    let mut more = v.clone();
    more["tracks"][0]["instrument"]["pages"] = Value::Array(pages.iter().cycle().take(20).cloned().collect());
    let back = Session::from_json(&more.to_string()).unwrap();
    assert_eq!(back.load_notes.len(), 2, "one note per device: {:?}", back.load_notes);
    assert!(back.load_notes.iter().any(|n| n.contains("track `Keys`, instrument: 4 macro strip page")), "{:?}", back.load_notes);
}

#[test]
fn replacing_pages_edits_only_the_pages_and_refuses_what_has_none() {
    let mut s = session();
    let tr = &mut s.tracks[0];
    let before = tr.instrument.clone().unwrap();
    let dup = page("  Mine  ", &["level", "level", "timbre"]);
    assert_eq!(tr.set_device_pages(DeviceRef::Instrument, &[dup]), Ok(true));
    let after = tr.instrument.clone().unwrap();
    assert_eq!(
        after.pages,
        vec![UserPage { name: "Mine".into(), slots: [Some("level".into()), None, Some("timbre".into()), None, None, None, None, None] }]
    );
    assert_eq!(Insert { pages: Vec::new(), ..after.clone() }, before, "nothing else of the slot record changed");
    assert_eq!(after.uid.get(), before.uid.get());
    assert_eq!(tr.set_device_pages(DeviceRef::Instrument, &after.pages), Ok(false), "the same pages change nothing");
    assert_eq!(tr.set_device_pages(DeviceRef::Insert(0), &[]), Err(PagesError::NoDevice));
    assert_eq!(tr.set_device_pages(DeviceRef::MidiEffect(1), &[]), Err(PagesError::NoDevice));
    assert_eq!(tr.set_device_pages(DeviceRef::Rack, &[]), Err(PagesError::NoDevice), "not a rack");
    let many = vec![UserPage::default(); MAX_USER_PAGES + 1];
    assert_eq!(tr.set_device_pages(DeviceRef::Insert(1), &many), Err(PagesError::TooMany(17)));
    assert!(tr.mixer.inserts[1].as_ref().unwrap().pages.is_empty(), "a refusal changes nothing");
    tr.mixer.inserts[2] = Some(Insert::hardware("Out 3", "In 3"));
    assert_eq!(tr.set_device_pages(DeviceRef::Insert(2), &[UserPage::default()]), Err(PagesError::NoParameters));
    tr.instrument = Some(Insert::external("In 1"));
    assert_eq!(tr.set_device_pages(DeviceRef::Instrument, &[UserPage::default()]), Err(PagesError::NoParameters));
}

#[test]
fn device_addresses_parse_and_print() {
    for (text, d) in [
        ("a", DeviceRef::Insert(0)),
        ("J", DeviceRef::Insert(9)),
        ("instrument", DeviceRef::Instrument),
        (" Instrument ", DeviceRef::Instrument),
        ("midi1", DeviceRef::MidiEffect(0)),
        ("midi8", DeviceRef::MidiEffect(7)),
        ("rack", DeviceRef::Rack),
    ] {
        assert_eq!(DeviceRef::parse(text), Some(d), "{text}");
        assert_eq!(DeviceRef::parse(&d.to_string()), Some(d));
    }
    for bad in ["k", "midi0", "midi9", "midi01", "midi", "1", "", "ab", "midi-1", "racks"] {
        assert_eq!(DeviceRef::parse(bad), None, "{bad}");
    }
}
