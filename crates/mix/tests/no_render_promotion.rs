//! `MixEngine::render_active` makes no OS priority call (MMCSS registration is a service call that can
//! block inside the first timed callback). The audio backend promotes the callback thread before
//! its first data callback. Its own test binary: the promotion counter is process-wide.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};
use transit_audio_io::AudioBuffer;
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, SlotGen, create_instance,
    mark_audio_thread,
};
use transit_model::{ChannelFormat, Clip, Session, SourceAudio, SourceId, TrackKind};
use transit_rt::testing::promotion_calls;

const FRAMES: usize = 256;

/// An engine that has adopted the plan of `s`.
fn planned_engine(s: &Session) -> MixEngine {
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: FRAMES as u32, out_channels: 0, in_channels: 0 };
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    let mut plan = compiler
        .compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut |spec: &InstanceSpec, _: SlotGen| {
            create_instance(&spec.id, spec.state.as_deref(), 48_000.0, FRAMES, spec.ch)
        })
        .unwrap();
    let mut engine = MixEngine::realtime(target);
    assert_eq!(engine.adopt_plan(&mut plan), Adoption::Adopted);
    compiler.reclaim(plan);
    engine
}

fn eight_strips() -> Session {
    let mut s = Session::default();
    let signal: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin() * 0.25).collect();
    s.pool.insert(SourceId(960), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal] })));
    for _ in 0..8 {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "sine", SourceId(960), 0, 0, 48_000));
    }
    s
}

/// Waits until every render worker has made its one promotion (at start, off the callback): each
/// records its result after the call the counter counts, so once every worker's result is there
/// the counter has its final pre-render value. The wait names that state, with a 5 s stall bound;
/// a quiet period on the counter let a worker starved past it promote during the render and fail
/// the test (E49, #226). With promotion turned off no worker calls it and nothing is recorded.
fn settle(engine: &MixEngine) -> usize {
    let disabled = std::env::var(transit_mix::RENDER_NO_PRIORITY_ENV).is_ok_and(|v| v.trim() == "1");
    if !disabled {
        let start = Instant::now();
        loop {
            let seen = engine.render_worker_promotions();
            if seen.iter().all(Option::is_some) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "the render workers never promoted themselves: {seen:?}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    promotion_calls()
}

fn render_on_audio_thread(session: Session, workers: bool) -> (usize, usize) {
    let mut engine = planned_engine(&session);
    // The test's own deadline, not the default share: the assertion is about promotion calls, and a
    // far deadline keeps the blocks the same on any host (E49, #226).
    engine.set_strip_deadline(Some(Duration::from_secs(60)));
    if workers {
        // The render workers promote themselves once, at start, off the callback.
        engine.start_render_workers(16);
    }
    let before = settle(&engine);
    let (after, engine) = std::thread::spawn(move || {
        mark_audio_thread();
        let mut out = vec![vec![0.0f32; FRAMES]; 2];
        for b in 0..50 {
            engine.render_active((b * FRAMES) as i64, FRAMES, &mut out);
        }
        (promotion_calls(), engine)
    })
    .join()
    .unwrap();
    // Off the audio thread: the workers stop here.
    drop(engine.into_retired());
    (before, after)
}

#[test]
fn render_makes_no_os_promotion_call_serial_and_with_render_workers() {
    for serial in [true, false] {
        MixEngine::force_serial(serial);
        let (before, after) = render_on_audio_thread(eight_strips(), !serial);
        assert_eq!(before, after, "render promoted a thread (force_serial = {serial})");
    }
    MixEngine::force_serial(true);
}
