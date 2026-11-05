//! The Time Stretcher insert (#199) through the engine: added and edited with the mix commands (a
//! quality restart, the pitch, freeze), its parameters survive a save and load (the session file's
//! JSON) unchanged and the loaded session renders the same bits; and the live path (the realtime
//! mix engine adopting the compiled plan, in host blocks of 64 and of 512 frames, played from a stop)
//! puts out what the offline bounce does, delayed by the plan's latency, bit for bit. A built-in's state is its
//! parameters; the quality set on load applies at the first block (no fade), so renders agree.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use std::sync::Arc;
use transit_engine::Engine;
use transit_midi::{Note, Sequence};
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RUN_IN_SECONDS, RenderTarget, create_instance,
};
use transit_model::{ChannelFormat, Clip, Insert, Session, TrackKind};
use transit_time::Range;

const SR: u32 = 48_000;

/// A session with one instrument track playing three notes through the Time Stretcher.
fn engine() -> (Engine, transit_model::TrackId) {
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
    e.execute("mix.insert", &json!({"track": t.0, "slot": 0, "plugin": "time_stretcher"})).unwrap();
    (e, t)
}

const EDITS: [(&str, f32); 7] =
    [("quality", 2.0), ("pitch", 12.0), ("fine", -25.0), ("window", 40.0), ("dry_wet", 80.0), ("reverb", 40.0), ("input_level", -6.0)];

#[test]
fn the_time_stretchers_parameters_round_trip_through_a_saved_session_and_render_the_same() {
    let (mut e, t) = engine();
    for (param, value) in EDITS {
        e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": param, "value": value})).unwrap();
    }
    let ins = e.session().track(t).unwrap().mixer.inserts[0].clone().unwrap();
    assert_eq!(ins.plugin, "time_stretcher");
    for (param, value) in EDITS {
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

/// Renders `frames` of `s` through the realtime mix engine in host blocks of `block`, as the audio
/// callback does, played from a stop: the chain first runs [`RUN_IN_SECONDS`] with the transport
/// stopped at 0, as the player runs it before play and as a print's run-in does (ADR 0038, #332
/// amendment). Returns the output and the plan's latency.
fn live(s: &Session, frames: usize, block: usize) -> (Vec<Vec<f32>>, usize) {
    MixEngine::force_serial(true);
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: block as u32, out_channels: 0, in_channels: 0 };
    let mut mix = MixEngine::realtime(target);
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: transit_mix::SlotGen| create_instance(&spec.id, spec.state.as_deref(), SR as f32, block, spec.ch);
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let ch = transit_mix::main_channels(s);
    let mut out = vec![Vec::with_capacity(frames); ch];
    let mut b = vec![vec![0.0f32; block]; ch];
    mix.monitor_only = true;
    mix.no_suspend = true;
    let run_in = SR as usize * RUN_IN_SECONDS as usize;
    let mut done = 0;
    while done < run_in {
        let n = block.min(run_in - done);
        mix.render_active(0, n, &mut b);
        done += n;
    }
    mix.monitor_only = false;
    mix.no_suspend = false;
    let mut pos = 0usize;
    while pos < frames {
        let n = block.min(frames - pos);
        mix.render_active(pos as i64, n, &mut b);
        for (o, x) in out.iter_mut().zip(&b) {
            o.extend_from_slice(&x[..n]);
        }
        pos += n;
    }
    (out, mix.latency)
}

#[test]
fn the_live_path_at_64_and_512_frames_puts_out_the_bounce() {
    let (mut e, t) = engine();
    for (param, value) in EDITS {
        e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": param, "value": value})).unwrap();
    }
    let s = e.session().clone();
    let frames = 48_000;
    let bounce = transit_mix::render_range(&s, Range::new(0, frames as i64), 512);
    assert!(bounce.iter().flatten().any(|v| v.abs() > 0.001), "the track sounds");
    let mut lives = Vec::new();
    for block in [64, 512] {
        let (out, latency) = live(&s, frames + 1024, block);
        // The live output is the timeline delayed by the plan's latency; the bounce skips it.
        let offset = (0..1024)
            .find(|&d| bounce.iter().zip(&out).all(|(b, o)| b.iter().zip(&o[d..]).all(|(x, y)| x.to_bits() == y.to_bits())))
            .unwrap_or_else(|| panic!("{block}-frame blocks: the live output is not the bounce at any offset under 1024"));
        assert_eq!(offset, latency, "{block}-frame blocks: the live output lines up with the bounce at the plan's latency");
        assert!(latency > 0, "the insert's latency reaches the plan");
        lives.push(out);
    }
    assert!(lives[0].iter().zip(&lives[1]).all(|(a, b)| a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())), "64 and 512 agree");
}
