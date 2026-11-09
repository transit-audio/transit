//! ADR 0051 decision 5: the engine's Middle C field is for message text only. No parser reads it
//! and no result carries a name, so every command means the same under both settings.

use serde_json::{Value, json};
use transit_engine::Engine;
use transit_midi::MiddleC;

/// The same script under one message convention: every result and error, and the document after.
fn run(convention: MiddleC) -> (Vec<Result<Value, String>>, Value) {
    let mut e = Engine::default();
    e.set_message_middle_c(convention);
    assert_eq!(e.message_middle_c(), convention);
    e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let clip = e.execute("midi.new_clip", &json!({"track": "Keys", "start": "1|1", "length": "5|1"})).unwrap()["clip"].clone();
    let mut out = Vec::new();
    for root in [json!("C3"), json!("G"), json!("Bb2"), json!(" F#3 "), json!("C\u{266F}3"), json!(60), json!("C+3"), json!("60"), json!("C4")] {
        out.push(
            e.execute("midi.chord_add", &json!({"clip": clip, "root": root, "quality": "maj", "start": "1|1", "length": "1|2"}))
                .map_err(|x| x.to_string()),
        );
    }
    out.push(e.execute("midi.notes", &json!({"clip": clip})).map_err(|x| x.to_string()));
    (out, serde_json::to_value(e.session()).unwrap())
}

#[test]
fn parsing_and_results_are_identical_under_both_settings() {
    let (c3, doc3) = run(MiddleC::C3);
    let (c4, doc4) = run(MiddleC::C4);
    assert_eq!(c3, c4, "results and errors");
    assert_eq!(doc3, doc4, "the document");
    // `C3` is 60 under either setting: the command's convention is fixed.
    let first = c3[0].as_ref().unwrap();
    assert_eq!(first["notes"][0]["pitch"], 60, "{first}");
    // Negative control: a parser that read the field would put C3 an octave lower under C4.
    let routed = transit_midi::parse_note("C3", MiddleC::C4, Some(3));
    assert_ne!(routed, Some(60));
}

#[test]
fn the_field_is_not_in_the_session_or_undo() {
    let mut e = Engine::default();
    let doc = serde_json::to_value(e.session()).unwrap();
    let history = e.execute("engine.history", &json!({})).unwrap();
    e.set_message_middle_c(MiddleC::C4);
    assert_eq!(serde_json::to_value(e.session()).unwrap(), doc);
    assert_eq!(e.execute("engine.history", &json!({})).unwrap(), history);
    assert!(!e.is_dirty());
    assert!(!serde_json::to_string(&doc).unwrap().contains("middle_c"));
}

/// The parser `midi.chord_add` used before ADR 0051, kept here as the measuring stick.
fn before(name: &str) -> Option<i64> {
    let mut chars = name.chars();
    let pitch = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (accidental, octave) = if let Some(s) = rest.strip_prefix('#') {
        (1, s)
    } else if let Some(s) = rest.strip_prefix('b') {
        (-1, s)
    } else {
        (0, rest)
    };
    let octave = if octave.is_empty() { 3 } else { octave.parse::<i64>().ok()? };
    let number = octave.checked_add(2)?.checked_mul(12)?.checked_add(pitch + accidental)?;
    (0..=127).contains(&number).then_some(number)
}

/// ADR 0051 decision 5's table: through the shared grammar `midi.chord_add` now trims surrounding
/// whitespace, accepts U+266F and U+266D, and refuses an octave written with `+`; every other
/// string reads as before, the bare-letter rule (octave 3) included.
#[test]
fn chord_add_reads_roots_through_the_shared_parser_with_exactly_three_differences() {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    let clip = e.execute("midi.new_clip", &json!({"track": "Keys", "start": "1|1", "length": "5|1"})).unwrap()["clip"].clone();
    let mut after = |root: &str| -> Option<i64> {
        let r = e.execute("midi.chord_add", &json!({"clip": clip, "root": root, "notes": [0], "start": "1|1", "length": "1|2"})).ok()?;
        let pitch = r["notes"][0]["pitch"].as_i64();
        e.execute("edit.undo", &json!({})).unwrap();
        pitch
    };
    let (mut whitespace, mut unicode, mut plus, mut same) = (0, 0, 0, 0);
    for letter in ["C", "c", "G", "a", "B", "b", "H", "6"] {
        for accidental in ["", "#", "b", "B", "\u{266F}", "\u{266D}", "x"] {
            for octave in ["", "3", "-2", "+3", "03", "-0", "8", "10", " 3", "99999999999999999999"] {
                for (pre, post) in [("", ""), (" ", ""), ("", "\t")] {
                    let text = format!("{pre}{letter}{accidental}{octave}{post}");
                    let (old, new) = (before(&text), after(&text));
                    if old == new {
                        same += 1;
                        continue;
                    }
                    // Each difference is one of the three the table states.
                    let core = text.trim().replace('\u{266F}', "#").replace('\u{266D}', "b");
                    if text.contains('+') {
                        assert!(old.is_some() && new.is_none(), "{text:?}: + is refused now ({old:?} before)");
                        plus += 1;
                    } else if text.contains(['\u{266F}', '\u{266D}']) {
                        assert_eq!(new, before(&core), "{text:?}: a Unicode sign reads as its ASCII spelling");
                        unicode += 1;
                    } else if text != text.trim() {
                        assert_eq!((old, new), (None, before(&core)), "{text:?}: surrounding whitespace is trimmed");
                        whitespace += 1;
                    } else {
                        panic!("{text:?} reads {new:?}, {old:?} before: not one of the three differences");
                    }
                }
            }
        }
    }
    println!("chord_add roots: {same} read as before; differences: whitespace {whitespace}, Unicode {unicode}, plus {plus}");
    assert!(whitespace > 0 && unicode > 0 && plus > 0 && same > 0, "{whitespace} {unicode} {plus} {same}");
    // The documented examples and the bare-letter rule, named exactly.
    for (root, pitch) in [("G3", Some(67)), ("Bb2", Some(58)), ("F#3", Some(66)), ("C-2", Some(0)), ("G", Some(67)), ("bb", Some(70))] {
        assert_eq!(after(root), pitch, "{root}");
    }
    assert_eq!((after("C+3"), before("C+3")), (None, Some(60)));
    assert_eq!(after("60"), None, "a numeric string stays an error");
}
