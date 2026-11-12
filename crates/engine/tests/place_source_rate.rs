//! `clip.place_source` and sources at another sample rate (#451). Sampler sources keep their native
//! rate (ADR 0041) while timeline clips play at the session rate, so the command refuses them.
use serde_json::json;
use transit_engine::Engine;
use transit_model::{Source, SourceId};

fn source(id: u64, rate: u32) -> Source {
    Source {
        id: SourceId(id),
        name: "tone".into(),
        path: "tone.wav".into(),
        channels: 1,
        frames: 4_410,
        sample_rate: rate,
        format: transit_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
        disk: false,
    }
}

fn history_depth(e: &mut Engine) -> usize {
    e.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len()
}

#[test]
fn a_source_at_the_session_rate_is_placed() {
    let mut e = Engine::default();
    let rate = e.session().sample_rate.hz();
    e.session_mut().sources.push(source(7, rate));
    let before = history_depth(&mut e);
    let out = e.execute("clip.place_source", &json!({"source": 7, "at": 0})).unwrap();
    assert!(out["clip"].is_u64(), "{out}");
    assert_eq!(history_depth(&mut e), before + 1);
}

#[test]
fn a_source_at_another_rate_is_refused_and_leaves_no_trace() {
    let mut e = Engine::default();
    let rate = e.session().sample_rate.hz();
    let native = if rate == 44_100 { 48_000 } else { 44_100 };
    e.session_mut().sources.push(source(7, native));
    let track = e.execute("track.new", &json!({"count": 1})).unwrap()["tracks"][0].as_u64().unwrap();
    let (json_before, tracks_before, depth) = (e.session().to_json().unwrap(), e.session().tracks.len(), history_depth(&mut e));
    for params in [json!({"source": 7, "at": 0}), json!({"source": 7, "at": 0, "track": track})] {
        let err = e.execute("clip.place_source", &params).unwrap_err().to_string();
        assert!(err.contains(&format!("{native} Hz")) && err.contains(&format!("{rate} Hz")), "both rates named: {err}");
        assert!(err.contains("import the file to the timeline"), "{err}");
    }
    assert_eq!(e.session().tracks.len(), tracks_before, "no track was added");
    assert_eq!(e.session().to_json().unwrap(), json_before);
    assert_eq!(history_depth(&mut e), depth, "no undo step");
}
