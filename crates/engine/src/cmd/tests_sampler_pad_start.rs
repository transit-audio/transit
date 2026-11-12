//! The pad start note through the commands (ADR 0041 amendment 2026-11-09; ADR 0051 for the note
//! names in message text): `mix.sampler_pad_start`, `note: null` on `mix.sampler_pad_set`, the
//! loop root that follows the start note, and `mix.sampler_state`. Files are generated tones in a
//! scratch folder per test; no sleeps.

use crate::{Engine, EngineError};
use serde_json::{Value, json};
use std::path::Path;
use transit_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use transit_midi::{MiddleC, Note, Sequence};
use transit_model::sampler_patch::PatchV1;
use transit_model::{Clip, TrackId};

struct Dir(transit_test_support::TestDir);

impl Dir {
    fn new(tag: &str) -> Dir {
        let p = transit_test_support::TestDir::new(&format!("pad-start-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

/// A generated tone (no recorded audio) as a 16-bit mono WAV.
fn tone(path: &Path) {
    let one: Vec<f32> = (0..4800).map(|i| 0.5 * (std::f32::consts::TAU * 220.0 * i as f32 / 48_000.0).sin()).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![one] };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None, bitrate_kbps: None, title: None };
    std::fs::write(path, transit_audio_io::encode(&buf, &opts).unwrap()).unwrap();
}

fn patch(e: &Engine, t: TrackId) -> PatchV1 {
    e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().clone()
}

/// A new kit (start note 36): pad A1 on zone 1, pad A3 on zone 2, zone 3 unbound over keys 70 to
/// 80, and a MIDI clip on the old pad notes.
fn drums(e: &mut Engine, d: &Dir) -> TrackId {
    let wav = d.0.join("tone.wav");
    tone(&wav);
    e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
    let t = e.session().tracks.last().unwrap().id;
    e.execute("mix.sampler_new_patch", &json!({"track": t})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 1, "pad": 1, "path": wav.to_string_lossy()})).unwrap();
    let src = patch(e, t).sources[0].source.0;
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 2, "pad": 3, "source": src})).unwrap();
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 3, "source": src, "playback": {"keys": [70, 80]}})).unwrap();
    let mut seq = Sequence::default();
    for (i, pitch) in [36u8, 38, 40].into_iter().enumerate() {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 64, channel: 0, start: i as i64 * 480, length: 240 });
    }
    let s = e.session_mut();
    let cid = s.new_clip_id();
    crate::edit::place_clip(s, t, Clip::midi(cid, "beat", 0, 48_000, seq));
    t
}

/// The refusal text of a command that must be refused.
fn refusal(r: Result<Value, EngineError>) -> String {
    match r {
        Err(EngineError::BadParams(_, m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The session without any Sampler patch: what a start change must leave alone.
fn without_patches(e: &Engine) -> Value {
    let mut v = serde_json::to_value(e.session()).unwrap();
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("sampler_patch");
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut v);
    v
}

fn doc(e: &Engine) -> Value {
    serde_json::to_value(e.session()).unwrap()
}

fn history(e: &mut Engine) -> Value {
    e.execute("engine.history", &json!({})).unwrap()
}

#[test]
fn a_new_patch_stores_36_and_the_state_reports_effective_and_stored_notes() {
    let d = Dir::new("state");
    let mut e = Engine::default();
    let t = drums(&mut e, &d);
    assert_eq!(patch(&e, t).pad_start_note, 36);
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 2, "note": 37})).unwrap();
    let st = e.execute("mix.sampler_state", &json!({"track": t})).unwrap();
    assert_eq!(st["pad_start_note"], 36);
    assert_eq!(st["patch"]["pad_start_note"], 36, "the stored patch is reported too");
    assert_eq!(st["seed"], 1);
    let pad =
        |i: usize| (st["pads"][i]["pad"].clone(), st["pads"][i]["note"].clone(), st["pads"][i]["note_stored"].clone(), st["pads"][i]["zone"].clone());
    assert_eq!(pad(0), (json!(1), json!(36), json!(false), json!(1)));
    assert_eq!(pad(1), (json!(2), json!(37), json!(true), json!(null)), "a note equal to its default is stored");
    assert_eq!(pad(2), (json!(3), json!(38), json!(false), json!(2)));
    assert_eq!(pad(63), (json!(64), json!(99), json!(false), json!(null)));
    assert_eq!(st["pads"].as_array().unwrap().len(), 64);
    assert_eq!(st["loop"], Value::Null);
    // The zone bound from pad A3 took its effective note as its root.
    assert_eq!(patch(&e, t).zone(1).unwrap().root, 38);
}

#[test]
fn a_start_change_moves_defaulted_pads_and_their_roots_in_one_step_and_touches_no_clip() {
    let d = Dir::new("move");
    let mut e = Engine::default();
    let t = drums(&mut e, &d);
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 2, "note": 37})).unwrap();
    let (before, rest, steps) = (doc(&e), without_patches(&e), history(&mut e)["undo"].as_array().map(Vec::len));
    let r = e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 60})).unwrap();
    assert_eq!(
        r,
        json!({"track": t, "note": 60, "from": 36, "semitones": 24, "notice": "Pad start note C3 (up 24 semitones): transpose pad clips by +24 to keep them on the same pads."})
    );
    let p = patch(&e, t);
    assert_eq!((p.pad_note(0), p.pad_note(1), p.pad_note(2), p.pad_note(63)), (60, 37, 62, 123), "stored A2 stays at 37");
    assert_eq!(
        (p.zone(0).unwrap().root, p.zone(1).unwrap().root, p.zone(2).unwrap().root),
        (60, 62, 60),
        "bound roots move, the unbound zone's does not"
    );
    assert_eq!(without_patches(&e), rest, "no clip, take or automation is touched");
    assert_eq!(history(&mut e)["undo"].as_array().map(Vec::len), steps.map(|n| n + 1), "one undo step");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(doc(&e), before, "undo restores the document bit for bit");
    // An unchanged start note records no step.
    let r = e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 36})).unwrap();
    assert_eq!((r["semitones"].clone(), r["notice"].clone()), (json!(0), json!("Pad start note C1 (unchanged).")));
    assert_eq!(doc(&e), before);
}

#[test]
fn each_refusal_leaves_the_document_identical_and_returns_its_text() {
    let d = Dir::new("refuse");
    let mut e = Engine::default();
    let t = drums(&mut e, &d);
    let check = |e: &mut Engine, p: Value, text: &str| {
        let (before, h) = (doc(e), history(e));
        assert_eq!(refusal(e.execute("mix.sampler_pad_start", &p)), text, "{p}");
        assert_eq!((doc(e), history(e)), (before, h), "{p}: nothing changed");
    };
    // (a)
    for note in [json!(65), json!(-1), json!("C1"), json!(36.5), json!(null)] {
        check(&mut e, json!({"track": t, "note": note}), "note must be an integer from 0 to 64");
    }
    check(&mut e, json!({"track": t}), "note must be an integer from 0 to 64");
    // (b) pad B1 stores 30 (below every default at 36): a start of 30 would put pad A1 there.
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 17, "note": 30})).unwrap();
    check(&mut e, json!({"track": t, "note": 30}), "Pad B1 plays F#0: a start note of F#0 would put pad A1 there.");
    e.execute("edit.undo", &json!({})).unwrap();
    // (d) pad A3's zone root raised to 120: up 8 would leave 127.
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 2, "playback": {"root": 120}})).unwrap();
    check(&mut e, json!({"track": t, "note": 44}), "Pad A3's root would leave 0 to 127.");
    e.execute("edit.undo", &json!({})).unwrap();
    // (c) eight unbound zones over E3: pad A5 (bound) moved onto 64 would start a ninth voice.
    let src = patch(&e, t).sources[0].source.0;
    e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": 3, "playback": {"keys": [60, 70]}})).unwrap();
    for zone in 4..=10 {
        e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": zone, "source": src, "playback": {"keys": [64, 64]}})).unwrap();
    }
    e.execute("mix.sampler_zone_source", &json!({"track": t, "zone": 11, "pad": 5, "source": src})).unwrap();
    check(&mut e, json!({"track": t, "note": 60}), "Pad A5 would be a ninth layer on E3.");
    // The same check on a pad note edit and on `note: null`: A5 stores 30 (below every default),
    // then eight unbound zones move onto 40, its default.
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 5, "note": 30})).unwrap();
    for zone in 3..=10 {
        e.execute("mix.sampler_zone_edit", &json!({"track": t, "zone": zone, "playback": {"keys": [40, 40]}})).unwrap();
    }
    let before = doc(&e);
    for note in [json!(null), json!(40)] {
        let r = e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 5, "note": note}));
        assert_eq!(refusal(r), "Pad A5 would be a ninth layer on E1.", "{note}");
        assert_eq!(doc(&e), before);
    }
}

#[test]
fn note_null_returns_a_pad_to_its_default_and_is_refused_when_the_default_is_taken() {
    let d = Dir::new("null");
    let mut e = Engine::default();
    let t = drums(&mut e, &d);
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 1, "note": 120})).unwrap();
    // Pad B1 (default 52) takes 36, A1's default, as a stored note.
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 17, "note": 36})).unwrap();
    let before = doc(&e);
    assert_eq!(refusal(e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 1, "note": null}))), "note 36 is pad 17's trigger note");
    assert_eq!(doc(&e), before);
    let r = e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 17, "note": null})).unwrap();
    assert_eq!(r["note"], 52, "B1 is back on start + 16");
    let r = e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 1, "note": null})).unwrap();
    assert_eq!(r["note"], 36);
    let p = patch(&e, t);
    assert!(!p.pad_note_stored(0) && !p.pad_note_stored(16));
    // Back on its default, A1 moves with the start note again.
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 48})).unwrap();
    assert_eq!(patch(&e, t).pad_note(0), 48);
}

/// One script under message convention `names`: every result and refusal, and the document.
fn flip_script(names: MiddleC) -> (Vec<Result<Value, String>>, Value) {
    let d = Dir::new("flip");
    let mut e = Engine::default();
    e.set_message_middle_c(names);
    let t = drums(&mut e, &d);
    e.execute("mix.sampler_pad_set", &json!({"track": t, "pad": 17, "note": 30})).unwrap();
    let mut out = Vec::new();
    for p in [json!({"track": t, "note": 30}), json!({"track": t, "note": 44}), json!({"track": t, "note": 99})] {
        out.push(e.execute("mix.sampler_pad_start", &p).map_err(|x| x.to_string()));
    }
    out.push(e.execute("mix.sampler_state", &json!({"track": t})).map_err(|x| x.to_string()));
    let mut document = doc(&e);
    // Session ids aside (scratch paths differ per test run), the document is what both runs built.
    document["sources"] = Value::Null;
    (out, document)
}

/// ADR 0051 decision 5 and its review: an engine refusal that names a note follows the message
/// Middle C, while the command's parameters and results are identical under both settings.
#[test]
fn a_refusal_names_its_note_in_the_message_middle_c_and_the_results_do_not_change() {
    let (c3, doc3) = flip_script(MiddleC::C3);
    let (c4, doc4) = flip_script(MiddleC::C4);
    assert_eq!(doc3, doc4, "the same document");
    let text = |r: &Result<Value, String>| r.as_ref().err().cloned().unwrap_or_default();
    assert_eq!(text(&c3[0]), "bad parameters for `mix.sampler_pad_start`: Pad B1 plays F#0: a start note of F#0 would put pad A1 there.");
    assert_eq!(text(&c4[0]), "bad parameters for `mix.sampler_pad_start`: Pad B1 plays F#1: a start note of F#1 would put pad A1 there.");
    assert_eq!(text(&c3[2]), text(&c4[2]), "a text without a note name is the same");
    // The landed change: the same numbers; only the notice's note name differs.
    let (mut a, mut b) = (c3[1].clone().unwrap(), c4[1].clone().unwrap());
    assert_eq!(a["notice"], "Pad start note G#1 (up 8 semitones): transpose pad clips by +8 to keep them on the same pads.");
    assert_eq!(b["notice"], "Pad start note G#2 (up 8 semitones): transpose pad clips by +8 to keep them on the same pads.");
    a["notice"] = Value::Null;
    b["notice"] = Value::Null;
    assert_eq!(a, b, "the result's numbers");
    assert_eq!(c3[3], c4[3], "the state query is all numbers");
}

#[test]
fn a_loop_without_a_stored_root_follows_the_start_note_and_null_clears_a_stored_one() {
    let d = Dir::new("root");
    let mut e = Engine::default();
    let t = drums(&mut e, &d);
    let src = patch(&e, t).sources[0].source.0;
    let r = e.execute("mix.sampler_slice_loop", &json!({"track": t, "source": src})).unwrap();
    assert_eq!((r["root"].clone(), r["root_stored"].clone()), (json!(36), json!(false)), "slice 1 would map to pad A1");
    let root = |e: &mut Engine| {
        let st = e.execute("mix.sampler_state", &json!({"track": t})).unwrap();
        let slices = e.execute("mix.sampler_slice_state", &json!({"track": t})).unwrap();
        assert_eq!(
            (slices["loop"]["root"].clone(), slices["loop"]["root_stored"].clone()),
            (st["loop"]["root"].clone(), st["loop"]["root_stored"].clone())
        );
        (st["loop"]["root"].as_u64().unwrap(), st["loop"]["root_stored"].as_bool().unwrap(), st["pads"][0]["note"].as_u64().unwrap())
    };
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 12})).unwrap();
    assert_eq!(root(&mut e), (12, false, 12), "at start 12 the default root is pad A1's note");
    assert!(patch(&e, t).slicing.unwrap().root.is_none(), "nothing stored");
    let r = e.execute("mix.sampler_slice_set", &json!({"track": t, "root": 60})).unwrap();
    assert_eq!((r["root"].clone(), r["root_stored"].clone()), (json!(60), json!(true)));
    e.execute("mix.sampler_pad_start", &json!({"track": t, "note": 36})).unwrap();
    assert_eq!(root(&mut e), (60, true, 36), "an explicit root stays put");
    let saved = serde_json::to_value(patch(&e, t)).unwrap();
    assert_eq!(saved["loop"]["root"], 60, "a stored 60 is written");
    e.execute("mix.sampler_slice_set", &json!({"track": t, "root": null})).unwrap();
    assert_eq!(root(&mut e), (36, false, 36), "null clears it");
    assert_eq!(refusal(e.execute("mix.sampler_slice_set", &json!({"track": t, "root": 128}))), "`root` must be 0-127, or null");
}
