//! ADR 0051 decision 3: every display site names notes through the namer in the Middle C
//! setting. Each test flips the setting and reads the very next frame: the site's names change and
//! every other text in the frame stays (the control). The Tuner's test is in
//! `builtin_editor_tests.rs` and the Macro Oscillator's in `macro_osc_panel/hero_tests.rs`, beside
//! their rigs.

use crate::middle_c_tests::{Rig, minus};
use crate::midi_monitor::{Filters, Learn, copy_text, passes, report_text};
use serde_json::json;
use transit_midi::MiddleC::{self, C3, C4};
use transit_midi::note_name;
use transit_playback::player::monitor::{Dir, MonitorEvent};

/// The pitch a text names in `convention`, when the whole text is a note name.
fn named(text: &str, convention: MiddleC) -> Option<u8> {
    (0..=127u8).find(|p| note_name(*p, convention) == text)
}

#[test]
fn the_piano_roll_key_labels_follow_the_setting() {
    let mut r = Rig::new();
    let (_, clip, _) = crate::tests::track_with_clip(&mut r.app);
    assert!(r.app.open_keys(clip));
    r.frames(3);
    let (before, after) = r.flip_frames(C4);
    let (gone, new) = (minus(&before, &after), minus(&after, &before));
    assert!(!gone.is_empty(), "the key labels changed");
    assert!(gone.iter().all(|t| named(t, C3).is_some()), "only C3 names left the frame: {gone:?}");
    assert!(new.iter().all(|t| named(t, C4).is_some()), "only C4 names came in: {new:?}");
    // The same keys are labeled: the names differ, the pitches do not.
    let mut keys_before: Vec<u8> = before.iter().filter_map(|t| named(t, C3)).collect();
    let mut keys_after: Vec<u8> = after.iter().filter_map(|t| named(t, C4)).collect();
    keys_before.sort_unstable();
    keys_after.sort_unstable();
    assert_eq!(keys_before, keys_after);
    assert!(keys_before.contains(&60), "middle C is labeled: {keys_before:?}");
    // Control: flipping to the setting in force changes nothing.
    assert_eq!(r.flip(C4), (vec![], vec![]));
}

fn ev(status: u8, d1: u8, d2: u8) -> MonitorEvent {
    let len = if (0xC0..0xE0).contains(&status) { 2 } else { 3 };
    MonitorEvent::message(Dir::In, 0, 5_000_000_000, &[status, d1, d2][..len])
}

#[test]
fn the_midi_monitor_re_derives_note_rows_on_a_flip_and_leaves_the_rest() {
    let mut r = Rig::new();
    r.app.ui.midi_monitor.open = true;
    let s = &mut r.app.midi_monitor;
    s.push(&ev(0x90, 60, 100));
    s.push(&ev(0x80, 61, 64));
    s.push(&ev(0x90, 62, 0));
    s.push(&ev(0xA0, 69, 40));
    s.push(&ev(0xB0, 74, 12));
    s.push(&ev(0xC0, 5, 0));
    s.push(&ev(0xE0, 0, 64));
    s.learn = Learn::Armed;
    s.push(&ev(0x90, 48, 90));
    r.frames(3);
    let old: Vec<String> = r.app.midi_monitor.rows.iter().map(|row| row.text.clone()).collect();
    assert_eq!(old[0], "Note On C3 (60) vel 100", "captured under C3");
    let (gone, new) = r.flip(C4);
    let rows: Vec<String> = r.app.midi_monitor.rows.iter().map(|row| row.text.clone()).collect();
    assert_eq!(
        rows,
        [
            "Note On C4 (60) vel 100",
            "Note Off C#4 (61) vel 64",
            "Note Off D4 (62) vel 0",
            "Poly Aftertouch A4 (69) 40",
            "CC 74 Brightness 12",
            "Program 5",
            "Pitch Bend +0",
            "Note On C3 (48) vel 90",
        ]
    );
    assert_eq!(old[4..7], rows[4..7], "the rows that name no note are untouched");
    assert!(matches!(&r.app.midi_monitor.learn, Learn::Caught(row) if row.text == "Note On C3 (48) vel 90"), "the caught row follows");
    // The next frame shows the new names and nothing else changed on screen.
    let was = ["C3 (60)", "C#3 (61)", "D3 (62)", "A3 (69)", "C2 (48)"];
    let now = ["C4 (60)", "C#4 (61)", "D4 (62)", "A4 (69)", "C3 (48)"];
    assert!(!gone.is_empty() && gone.iter().all(|t| was.iter().any(|w| t.contains(w))), "{gone:?}");
    assert!(!new.is_empty() && new.iter().all(|t| now.iter().any(|w| t.contains(w))), "{new:?}");
    // Copy and the report come out in one convention, and the filter matches the new names.
    let copied = copy_text(&r.app.midi_monitor.rows, &Filters::default(), None, 100);
    assert!(copied.contains("Note On C4 (60)") && !copied.contains("C3 (60)") && copied.contains("Note On C3 (48)"), "{copied}");
    let report = report_text(&[], (0, 0), &r.app.midi_monitor.rows);
    assert!(report.contains("Note On C4 (60) vel 100  [90 3C 64]") && !report.contains("C3 (60)"), "{report}");
    let by = |text: &str| Filters { text: text.into(), ..Filters::default() };
    let first = &r.app.midi_monitor.rows[0];
    assert!(passes(&by("C4 (60)"), first) && !passes(&by("C3 (60)"), first));
    // A row captured after the flip is in the new convention from the start.
    r.app.midi_monitor.push(&ev(0x90, 60, 1));
    assert_eq!(r.app.midi_monitor.rows.back().unwrap().text, "Note On C4 (60) vel 1");
    // Control: back to C3 restores the captured text exactly.
    r.flip(C3);
    let back: Vec<String> = r.app.midi_monitor.rows.iter().take(8).map(|row| row.text.clone()).collect();
    assert_eq!(back, old);
}

#[test]
fn the_midi_mappings_table_names_a_note_source_in_the_setting() {
    let mut r = Rig::new();
    let t = crate::tests::new_track(&mut r.app, json!({"kind": "audio", "name": "Gtr"}));
    let note = json!({"port": "Keys", "channel": 1, "kind": "note", "number": 60});
    r.app.run("midi.map", json!({"source": note, "target": format!("track:{}:mute", t.0)})).unwrap();
    let cc = json!({"port": "Keys", "channel": 1, "kind": "cc", "number": 7});
    r.app.run("midi.map", json!({"source": cc, "target": format!("track:{}:volume", t.0)})).unwrap();
    r.app.run("window.midi_mappings", json!({"value": true})).unwrap();
    r.frames(6);
    let sources = |r: &Rig| crate::midi_learn::rows(&r.app).into_iter().map(|row| row.source).collect::<Vec<_>>();
    assert!(sources(&r).contains(&"Note C3 (60)".to_string()), "{:?}", sources(&r));
    let (gone, new) = r.flip(C4);
    assert_eq!((gone, new), (vec!["Note C3 (60)".to_string()], vec!["Note C4 (60)".to_string()]), "only the note source changed");
    assert!(sources(&r).contains(&"CC 7".to_string()), "the CC row is untouched");
}

/// ADR 0051 decision 6: MusicXML keeps its own convention (scientific pitch, C4 = 60) and never
/// reads the setting: the export of one clip is byte-identical under both.
#[test]
fn musicxml_export_is_byte_identical_under_both_settings() {
    let mut app = crate::tests::app();
    let (t, clip, _) = crate::tests::track_with_clip(&mut app);
    for (pitch, start) in [(60, 0), (69, 960), (61, 1920), (48, 2880)] {
        app.run("midi.note_add", json!({"clip": clip.0, "pitch": pitch, "start_ticks": start, "length_ticks": 960})).unwrap();
    }
    let dir = std::env::temp_dir().join(format!("transit-middle-c-xml-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = Vec::new();
    for mc in ["C3", "C4"] {
        app.run("options.middle_c", json!({"value": mc})).unwrap();
        let path = dir.join(format!("score-{mc}.musicxml"));
        app.run("file.export_sibelius", json!({"path": path.to_string_lossy(), "tracks": [t.0]})).unwrap();
        out.push(std::fs::read(&path).unwrap());
    }
    assert_eq!(out[0], out[1], "the setting does not reach the file");
    let text = String::from_utf8(out[0].clone()).unwrap();
    assert!(text.contains("<step>C</step><octave>4</octave>"), "MIDI 60 is C4 in MusicXML: {text}");
    assert!(text.contains("<step>A</step><octave>4</octave>"), "MIDI 69 is A4 in MusicXML");
    let _ = std::fs::remove_dir_all(dir);
}
