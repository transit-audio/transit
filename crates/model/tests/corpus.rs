//! The session compatibility corpus (#161): sessions saved by released builds, kept as files in
//! `tests/corpus/` and loaded by every later build. See `tests/corpus/README.md`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use transit_model::Session;

fn corpus() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "transit")).collect();
    files.sort();
    assert!(files.len() >= 2, "the corpus needs at least the demo and features sessions of one release, found {}", files.len());
    files.into_iter().map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap())).collect()
}

/// Fixtures that carry a retired feature on purpose (ADR 0031). They are loaded by their own test, not
/// by the lossless round trip: loading them drops parts and says so.
const SANITIZED: &[&str] = &["v0.3.2-video.transit"];

/// Keys a later build stopped writing, so the key-drop test does not count them (`videos`, ADR 0031).
const RETIRED_KEYS: &[&str] = &["/videos"];

/// Does the file name the Subtractive Synth, retired in v0.6.0 (#234)? Builds before that wrote it into
/// every instrument slot. Loading such a file substitutes a Macro Oscillator and drops what pointed at
/// the old synth's parameters (its own test, below), so the lossless round trip and the key-drop test
/// read these files with the old id swapped for the new one in the text: every other key still has to
/// survive a save.
fn carries_retired_synth(text: &str) -> bool {
    text.contains("\"subtractive_synth\"")
}

/// The file with every inherited effect retired in stage 1 (#264) swapped for a Gain in the text.
/// Loading the original removes those slots and says so (its own test, below); the lossless round
/// trip and the key-drop test read the swapped text, so every other key still has to survive a save.
fn without_retired_effects(text: &str) -> String {
    transit_model::legacy_effects::RETIRED_EFFECTS
        .iter()
        .fold(text.to_string(), |t, (id, _)| t.replace(&format!("\"plugin\": \"{id}\""), "\"plugin\": \"gain\""))
}

fn released() -> Vec<(String, String)> {
    corpus()
        .into_iter()
        .filter(|(n, _)| !SANITIZED.contains(&n.as_str()))
        .map(|(n, t)| (n, without_retired_effects(&t.replace("\"subtractive_synth\"", "\"macro_oscillator\""))))
        .collect()
}

/// Every key path of `v` (`/tracks/0/mixer/volume_db`); array elements by index, null values included.
fn key_paths(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                let p = format!("{path}/{}", k.replace('~', "~0").replace('/', "~1"));
                out.push(p.clone());
                key_paths(x, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                key_paths(x, &format!("{path}/{i}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn every_corpus_session_loads_and_round_trips_losslessly() {
    for (name, text) in released() {
        let first = Session::from_json(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
        assert!(first.load_notes.is_empty(), "{name} loads with notes: {:?}", first.load_notes);
        let saved = first.to_json().unwrap_or_else(|e| panic!("{name} no longer saves: {e}"));
        let second = Session::from_json(&saved).unwrap_or_else(|e| panic!("{name} saved by this build does not load: {e}"));
        assert!(second.load_notes.is_empty(), "{name} reloads with notes: {:?}", second.load_notes);
        assert_eq!(first, second, "{name}: save then load changed the session");
    }
}

#[test]
fn no_key_of_a_corpus_file_is_dropped_by_a_round_trip() {
    for (name, text) in released() {
        let original: Value = serde_json::from_str(&text).unwrap();
        let resaved: Value = serde_json::from_str(&Session::from_json(&text).unwrap().to_json().unwrap()).unwrap();
        let (mut before, mut after) = (Vec::new(), Vec::new());
        key_paths(&original, "", &mut before);
        key_paths(&resaved, "", &mut after);
        let after: std::collections::HashSet<_> = after.into_iter().collect();
        before.retain(|p| !RETIRED_KEYS.contains(&p.as_str()));
        let missing: Vec<_> = before.iter().filter(|p| !after.contains(*p)).take(20).collect();
        assert!(
            missing.is_empty(),
            "{name}: a save dropped {} key path(s), first: {missing:?}",
            before.iter().filter(|p| !after.contains(*p)).count()
        );
    }
}

#[test]
fn no_corpus_file_is_from_a_newer_format_than_this_build() {
    for (name, text) in corpus() {
        let v: Value = serde_json::from_str(&text).unwrap();
        let ver = v["format_version"].as_u64().unwrap_or_else(|| panic!("{name} has no format_version"));
        assert!(ver <= u64::from(Session::FORMAT_VERSION), "{name} has format_version {ver}, newer than {}", Session::FORMAT_VERSION);
    }
}

#[test]
fn corpus_files_are_named_by_release_and_stay_small() {
    for (name, text) in corpus() {
        let ok = name.strip_prefix('v').and_then(|r| r.strip_suffix(".transit")).is_some_and(|r| {
            r.split_once('-').is_some_and(|(version, kind)| {
                version.split('.').count() == 3 && version.split('.').all(|n| n.parse::<u32>().is_ok()) && !kind.is_empty()
            })
        });
        assert!(ok, "{name}: corpus files are named v<major>.<minor>.<patch>-<name>.transit");
        assert!(text.len() < 400_000, "{name} is {} bytes: a corpus file carries no audio and stays small", text.len());
    }
}

#[test]
fn a_session_with_video_opens_with_the_video_dropped_and_a_note() {
    let (name, text) = corpus().into_iter().find(|(n, _)| n == "v0.3.2-video.transit").expect("the video fixture is in the corpus");
    let s = Session::from_json(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
    assert_eq!(s.load_notes, vec![transit_model::legacy::VIDEO_REMOVED_NOTE.to_string()], "{name}");
    // The Video track also holds an audio clip on its inactive playlist: it stays, as an Audio track.
    assert_eq!(s.tracks.len(), 2, "{name}: the Video track that holds audio stays");
    assert!(s.tracks.iter().all(|t| t.kind == transit_model::TrackKind::Audio), "{name}");
    let clips: Vec<_> = s.tracks.iter().flat_map(|t| t.playlists.iter().flat_map(|p| p.clips.iter())).collect();
    assert_eq!(clips.len(), 2, "{name}: both audio clips stay, the video clip goes");
    assert!(clips.iter().all(|c| c.is_audio()), "{name}");
    let saved = s.to_json().unwrap();
    assert!(!saved.contains("Video") && !saved.contains("videos"), "{name}: a save carries no video");
    let again = Session::from_json(&saved).unwrap();
    assert!(again.load_notes.is_empty(), "{name}: the note is not saved back");
    let mut loaded = s.clone();
    loaded.load_notes.clear();
    assert_eq!(loaded, again);
}

#[test]
fn the_rack_fixture_loads_its_rack() {
    let (name, text) = corpus().into_iter().find(|(n, _)| n == "v0.6.0-rack.transit").expect("the rack fixture is in the corpus");
    let s = Session::from_json(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
    let racks: Vec<_> = s.tracks.iter().filter(|t| t.is_rack()).collect();
    assert_eq!(racks.len(), 1, "{name}: one rack");
    let rack = racks[0];
    assert_eq!(s.rack_members(rack).count(), 2, "{name}: both members play");
    let r = rack.rack.as_ref().unwrap();
    assert_eq!(r.macros[0].mappings.len(), 1, "{name}: macro 1 is mapped");
    assert!(r.macros[1].midi.is_some(), "{name}: macro 2 has a controller");
    assert!(rack.clips().iter().any(|c| !c.is_audio()), "{name}: the rack holds a MIDI clip");
}

#[test]
fn the_takes_fixture_loads_its_take_stacks_and_what_plays() {
    let (name, text) = corpus().into_iter().find(|(n, _)| n == "v0.6.0-takes.transit").expect("the takes fixture is in the corpus");
    let s = Session::from_json(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
    let vox = s.track_by_name("Vox").unwrap_or_else(|| panic!("{name}: the Vox track"));
    let st = vox.take_stacks();
    assert_eq!(st.len(), 1, "{name}: one audio stack");
    assert_eq!(st[0].takes.iter().map(|t| (t.number, t.is_active())).collect::<Vec<_>>(), vec![(1, false), (2, true), (3, true)], "{name}");
    assert_eq!(st[0].active(), Some(3), "{name}: take 3 plays most of it");
    assert_eq!(vox.clips().len(), 3, "{name}: two takes and the clip after the stack play");
    let keys = s.track_by_name("Keys").unwrap_or_else(|| panic!("{name}: the Keys track"));
    let st = keys.take_stacks();
    assert_eq!((st.len(), st[0].takes.len(), st[0].active()), (1, 3, Some(3)), "{name}: the MIDI stack");
}

#[test]
fn the_midi_effect_fixture_loads_its_chain() {
    let (name, text) = corpus().into_iter().find(|(n, _)| n == "v0.6.0-midi-effects.transit").expect("the MIDI effect fixture is in the corpus");
    let s = Session::from_json(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
    let t = s.tracks.iter().find(|t| !t.midi_effects.is_empty()).unwrap_or_else(|| panic!("{name}: a track with MIDI effects"));
    let chain: Vec<_> = t.midi_effects.iter().map(|f| (f.plugin.as_str(), f.bypass)).collect();
    assert_eq!(chain, [("midi_transpose", false), ("midi_velocity", true)], "{name}");
    assert_eq!(t.midi_effects[0].params.get("semitones"), Some(&7.0), "{name}");
    let lane = transit_model::AutoParam::Plugin { slot: transit_model::MIDI_EFFECT_SLOT, param: "semitones".into() };
    assert_eq!(t.lane(&lane).map(|l| l.points.len()), Some(2), "{name}: the first effect's automated parameter");
}

#[test]
fn a_release_session_with_the_old_synth_opens_with_a_macro_oscillator_and_one_report() {
    let old: Vec<_> = corpus().into_iter().filter(|(_, t)| carries_retired_synth(t)).collect();
    assert!(old.len() >= 2, "the corpus keeps sessions saved by builds that had the synth: {:?}", old.iter().map(|(n, _)| n).collect::<Vec<_>>());
    for (name, text) in old {
        let text = without_retired_effects(&text);
        let (s, stored) = Session::from_json_as_saved(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
        let stored = stored.unwrap_or_else(|| panic!("{name}: loading did not substitute"));
        let swapped = stored.tracks.iter().filter(|t| t.instrument.as_ref().is_some_and(|i| i.plugin == "subtractive_synth")).count();
        assert!(swapped > 0, "{name}");
        assert!(s.tracks.iter().all(|t| t.instrument.as_ref().is_none_or(|i| i.plugin != "subtractive_synth")), "{name}");
        let status = format!("{swapped} track(s) used the retired Subtractive Synth and now carry the Macro Oscillator");
        assert_eq!(s.load_notes.last(), Some(&status), "{name}: {:?}", s.load_notes);
        // Saved, it is an ordinary session: no old id, no note, and it loads clean and lossless.
        let saved = s.to_json().unwrap();
        assert!(!saved.contains("subtractive_synth"), "{name}");
        let again = Session::from_json(&saved).unwrap();
        assert!(again.load_notes.is_empty(), "{name}: {:?}", again.load_notes);
        let mut loaded = s.clone();
        loaded.load_notes.clear();
        assert_eq!(loaded, again, "{name}");
    }
}

#[test]
fn the_substitution_drops_only_what_addressed_the_old_instrument() {
    use transit_model::{AutoParam, Control, Strip};
    let old: Vec<_> = corpus().into_iter().filter(|(_, t)| carries_retired_synth(t)).collect();
    assert!(!old.is_empty());
    let (mut lanes_seen, mut maps_seen) = (0, 0);
    for (name, text) in old {
        let text = without_retired_effects(&text);
        let real = Session::from_json(&text).unwrap();
        // The same file with the id swapped in the text: nothing is substituted or dropped.
        let swapped = Session::from_json(&text.replace("\"subtractive_synth\"", "\"macro_oscillator\"")).unwrap();
        assert!(swapped.load_notes.is_empty(), "{name}");
        let retired: Vec<_> = text_tracks_with_old_synth(&text);
        let mut expected = swapped.clone();
        for id in &retired {
            let t = expected.track_mut(*id).unwrap_or_else(|| panic!("{name}: track {id:?}"));
            t.instrument = real.track(*id).unwrap().instrument.clone();
            let before = t.automation.len();
            t.automation.retain(|l| !matches!(&l.param, AutoParam::Plugin { slot, .. } if *slot == AutoParam::INSTRUMENT_SLOT));
            lanes_seen += before - t.automation.len();
            maps_seen += expected.midi_mappings.remove_where(|m| {
                matches!((&m.strip, &m.control), (Strip::Track(x), Control::Plugin { slot, .. }) if x == id && *slot == AutoParam::INSTRUMENT_SLOT)
            });
        }
        let mut real = real;
        real.load_notes.clear();
        expected.load_notes.clear();
        assert_eq!(real.to_json().unwrap(), expected.to_json().unwrap(), "{name}: the substitution changed more than the old instrument's slot");
        for id in &retired {
            let ins = real.track(*id).unwrap().instrument.as_ref().unwrap();
            assert_eq!(ins.plugin, "macro_oscillator", "{name}");
        }
    }
    println!("dropped across the corpus: {lanes_seen} lane(s), {maps_seen} mapping(s)");
}

/// The tracks whose instrument slot names the old synth, read from the file's JSON.
fn text_tracks_with_old_synth(text: &str) -> Vec<transit_model::TrackId> {
    let v: Value = serde_json::from_str(text).unwrap();
    v["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["instrument"]["plugin"] == "subtractive_synth")
        .map(|t| transit_model::TrackId(t["id"].as_u64().unwrap()))
        .collect()
}

#[test]
fn a_release_session_with_the_inherited_effects_opens_with_their_slots_emptied_and_one_report() {
    use transit_model::legacy_effects::{retired_name, status_line};
    let old: Vec<_> =
        corpus().into_iter().filter(|(_, t)| RETIRED_PLUGIN_IDS.iter().any(|id| t.contains(&format!("\"plugin\": \"{id}\"")))).collect();
    assert!(old.len() >= 2, "the corpus keeps sessions saved by builds that had the effects: {:?}", old.iter().map(|(n, _)| n).collect::<Vec<_>>());
    for (name, text) in old {
        let (s, stored) = Session::from_json_as_saved(&text).unwrap_or_else(|e| panic!("{name} no longer loads: {e}"));
        let stored = stored.unwrap_or_else(|| panic!("{name}: loading did not remove"));
        // Every slot of the file that names a retired device is empty now, and every one is reported by track, slot and name.
        let mut removed = 0;
        for t in &stored.tracks {
            for (slot, ins) in t.mixer.inserts.iter().enumerate() {
                let Some(device) = ins.as_ref().and_then(|i| retired_name(&i.plugin)) else { continue };
                removed += 1;
                assert!(s.track(t.id).unwrap().mixer.inserts[slot].is_none(), "{name}: {} insert {slot}", t.name);
                let line = format!("track \"{}\", insert {}: the retired {device} was removed", t.name, (b'A' + slot as u8) as char);
                assert!(s.load_notes.iter().any(|n| n.starts_with(&line)), "{name}: no line `{line}` in {:?}", s.load_notes);
            }
        }
        assert!(removed > 0, "{name}");
        assert_eq!(s.load_notes.last(), Some(&status_line(removed)), "{name}: {:?}", s.load_notes);
        // The rest is the file as it was: the same session with the slots emptied by hand.
        let mut by_hand = Session::from_json(&without_retired_effects(&text)).unwrap();
        for t in &mut by_hand.tracks {
            for (slot, ins) in stored.track(t.id).unwrap().mixer.inserts.iter().enumerate() {
                if ins.as_ref().is_some_and(|i| retired_name(&i.plugin).is_some()) {
                    t.mixer.inserts[slot] = None;
                }
            }
        }
        let (mut loaded, mut by_hand) = (s.clone(), by_hand);
        loaded.load_notes.clear();
        by_hand.load_notes.clear();
        assert_eq!(loaded.to_json().unwrap(), by_hand.to_json().unwrap(), "{name}: the removal changed more than the retired slots");
        // Saved, it is an ordinary session: no retired id, no note, loads clean.
        let saved = s.to_json().unwrap();
        assert!(!RETIRED_PLUGIN_IDS.iter().any(|id| saved.contains(&format!("\"plugin\": \"{id}\""))), "{name}");
        assert!(Session::from_json(&saved).unwrap().load_notes.is_empty(), "{name}");
    }
}

/// The retired devices' plug-in ids.
const RETIRED_PLUGIN_IDS: [&str; 12] = [
    "pitch_shifter",
    "drum_synth",
    "channel_strip",
    "maximizer",
    "de_esser",
    "rectifier",
    "lofi",
    "time_shift",
    "mod_delay",
    "chorus",
    "flanger",
    "phaser",
];
