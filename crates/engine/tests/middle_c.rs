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
        out.push(e.execute("midi.chord_add", &json!({"clip": clip, "root": root, "quality": "maj", "start": "1|1", "length": "1|2"})).map_err(|x| x.to_string()));
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
