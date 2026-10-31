//! The Texture insert (#241) through the engine: added and edited with the mix commands (a quality
//! that resamples, the longest buffer, the freeze, manual input gain), its parameters survive a save
//! and load (the session file's JSON) unchanged, and the loaded session renders the same bits. A
//! built-in's state is its parameters; the buffer length set on load rebuilds the chain at a frame
//! the parameters fix, so two renders agree.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use transit_engine::Engine;
use transit_midi::{Note, Sequence};
use transit_model::{ChannelFormat, Clip, Insert, Session, TrackKind};
use transit_time::Range;

#[test]
fn the_textures_parameters_round_trip_through_a_saved_session_and_render_the_same() {
    let mut e = Engine::default();
    let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    {
        let s = e.session_mut();
        let mut seq = Sequence::default();
        for (pitch, start) in [(48u8, 0i64), (55, 500), (60, 1000)] {
            seq.notes.push(Note { pitch, velocity: 110, release_velocity: 0, channel: 0, start, length: 450 });
        }
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.instrument = Some(Insert::new("macro_oscillator"));
        tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 96_000, seq));
    }
    e.execute("mix.insert", &json!({"track": t.0, "slot": 0, "plugin": "texture"})).unwrap();
    let edits = [
        ("quality", 2.0),
        ("buffer", 4.0),
        ("dry_wet", 80.0),
        ("pitch", 12.0),
        ("time_random", -35.0),
        ("input_gain", 1.0),
        ("input_level", 6.0),
        ("reverb", 40.0),
    ];
    for (param, value) in edits {
        e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": param, "value": value})).unwrap();
    }
    let ins = e.session().track(t).unwrap().mixer.inserts[0].clone().unwrap();
    assert_eq!(ins.plugin, "texture");
    for (param, value) in edits {
        assert_eq!(ins.params.get(param).copied(), Some(value), "{param}");
    }
    let text = serde_json::to_string(e.session()).unwrap();
    let restored: Session = serde_json::from_str(&text).unwrap();
    assert_eq!(restored.track(t).unwrap().mixer.inserts[0].as_ref().unwrap().params, ins.params);
    let range = Range::new(0, 48_000);
    let a = transit_mix::render_range(e.session(), range, 256);
    let b = transit_mix::render_range(&restored, range, 256);
    assert!(a.iter().flatten().any(|v| v.abs() > 0.001), "the track sounds");
    assert!(a.iter().flatten().zip(b.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()), "the loaded session renders the same bits");
}
