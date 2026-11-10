//! Clamp-on-use state and audible load for the complete sampler fixture.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../mix/tests/support/sampler_full_load.rs"]
mod fixture;
use fixture::*;
use serde_json::json;
use transit_engine::Engine;
use transit_model::Session;
use transit_model::sampler_patch::{Fades, LoopMode, PlayMode};
use transit_time::Range;

#[test]
fn fixture_with_clamped_values_loads_audible() {
    let mut p = patch(true);
    p.zones[144].mode = PlayMode::OneShot;
    p.zones[145].crossfade = 1000;
    p.zones[146].fades = Fades::of([8, 8, 100, 100]);
    p.zones[128].start = 3000;
    p.zones[128].end = 3001;
    p.zones[128].loop_start = 3000;
    p.zones[128].loop_end = 3001;
    let mut s = Session::default();
    media(&mut s);
    let t = install(&mut s, p);
    let mut e = Engine::new(s);
    let state = e.execute("mix.sampler_state", &json!({"track": t})).unwrap();
    let zs = state["zones"].as_array().unwrap();
    let z = |id| zs.iter().find(|z| z["zone"] == id).unwrap();
    assert_eq!(z(145)["loop_used"], "off");
    assert_eq!((z(146)["crossfade"].as_u64(), z(146)["crossfade_used"].as_u64()), (Some(1000), Some(192)));
    assert_eq!(z(147)["fades_used"], json!({"key_lo":0,"key_hi":0,"vel_lo":99,"vel_hi":0}));
    assert_eq!(z(129)["region"]["start"], 0);
    assert_eq!(z(129)["region"]["end"], 256);
    // Each clamped case must sound by itself, rather than being hidden by another zone.
    for (zone, key) in [(144, 64), (145, 64), (146, 65), (128, 36)] {
        let mut single = e.session().clone();
        let ins = single.track_mut(t).unwrap().instrument.as_mut().unwrap();
        let mut q = ins.sampler_patch.as_ref().unwrap().ready().unwrap().clone();
        q.zones.retain(|z| z.id == zone);
        q.pads.retain(|p| p.zone == Some(zone));
        q.groups.clear();
        ins.sampler_patch = Some(transit_model::SamplerPatch::from_value(serde_json::to_value(&q).unwrap()));
        assert!(ins.sampler_patch.as_ref().unwrap().ready().is_some());
        let mut seq = transit_midi::Sequence::default();
        seq.notes.push(transit_midi::Note { pitch: key, velocity: 80, release_velocity: 64, channel: 0, start: 0, length: 100 });
        let cid = single.new_clip_id();
        single.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "Clamp", 0, 2048, seq));
        let out = transit_mix::render_range(&single, Range::new(0, 2048), 97);
        assert!(out.iter().flatten().any(|x| *x != 0.0), "zone {zone} became silent");
    }
    assert_eq!(
        e.session().track(t).unwrap().instrument.as_ref().unwrap().sampler_patch.as_ref().unwrap().ready().unwrap().zones[144].loop_mode,
        LoopMode::PingPong
    );
}
