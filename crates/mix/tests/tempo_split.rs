//! Tempo-map host boundaries: audio parity, live inputs, event offsets and bounded work.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "common/tempo_scene.rs"]
mod tempo_scene;
use std::sync::Arc;
use transit_mix::*;
use transit_model::{ChannelFormat, Session, TrackKind};
use transit_rt::testing::{CountingAlloc, measure};
use transit_time::{Range, SampleRate, TempoMap};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn engine(s: &Session, block: usize) -> MixEngine {
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48000, max_block: block as u32, out_channels: 2, in_channels: 2 };
    let mut mix = MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default());
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| create_instance(&spec.id, None, 48000.0, block, spec.ch);
    let mut env = comp.compile(Arc::new(s.clone()), PlanGeneration::EMPTY, PlanGeneration::from_raw(1), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    mix
}

#[test]
fn frame_grid_coalesces_changes_and_handles_both_edges() {
    let mut map = TempoMap::default();
    let sr = SampleRate::HZ_48000;
    map.set_tempo(64, 150.0).unwrap(); // frame 1600
    map.set_tempo(80, 100.0).unwrap(); // frame 1920
    assert_eq!(map.render_span(0, 4096, sr), 1600);
    assert_eq!(map.render_span(1600, 2496, sr), 320);
    assert_eq!(map.render_span(1920, 2176, sr), 2176);
    assert_eq!(map.render_span(1600, 64, sr), 64); // exact first
    assert_eq!(map.render_span(1089, 512, sr), 511); // last included frame
    assert_eq!(map.render_span(1088, 512, sr), 512); // exclusive end
    assert_eq!(map.render_span(0, 0, sr), 0);
    assert_eq!(map.render_span(i64::MAX, 512, sr), 512);
    assert_eq!(map.render_span(i64::MAX - 10, 512, sr), 512);
    assert_eq!(map.render_span(i64::MIN, 512, sr), 512);
    let mut dense = TempoMap::new(1000.0, 4, 4).unwrap();
    for tick in 1..20 {
        dense.set_tempo(tick, 999.0).unwrap();
    }
    let low_rate = SampleRate::new(8000).unwrap();
    let mut at = 0;
    let mut spans = 0;
    while at < 4 {
        let n = dense.render_span(at, (4 - at) as usize, low_rate);
        assert!(n > 0);
        at += n as i64;
        spans += 1;
    }
    assert!(spans <= 4);
}

#[test]
fn tempo_and_meter_changes_agree_through_the_change_and_live_equals_bounce() {
    MixEngine::force_serial(true);
    for meter in [false, true] {
        let mut s = tempo_scene::scene();
        let f = s.tracks[0].mixer.inserts[0].as_mut().unwrap();
        f.params.insert("lfo_amount".into(), 100.0);
        f.params.insert("lfo_shape".into(), 0.0);
        f.params.insert("lfo_division".into(), if meter { 2.0 } else { 6.0 });
        if meter {
            s.tempo.set_meter(0, 1, 4).unwrap();
            s.tempo.set_meter(960, 3, 4).unwrap(); // frame 24000: 64 boundary, inside 512
        } else {
            s.tempo.set_tempo(64, 150.0).unwrap(); // frame 1600
            s.tempo.set_tempo(80, 100.0).unwrap(); // another change inside 4096
        }
        let reference = render_range(&s, Range::new(0, 48000), 64);
        for block in [512, 4096] {
            let bounce = render_range(&s, Range::new(0, 48000), block);
            let worst = reference.iter().flatten().zip(bounce.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            let different = reference.iter().flatten().zip(bounce.iter().flatten()).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
            println!("meter={meter} block={block} differing samples={different} worst={worst}");
            assert!(worst < 1e-6, "meter={meter} block={block}: {worst}");
            assert_eq!(different, 0, "bit equality through the change");
            let mut mix = engine(&s, block);
            let mut output = vec![vec![0.0; block]; 2];
            let mut at = 0;
            while at < 48000 {
                let n = block.min(48000 - at);
                let (_, stats) = measure(|| mix.render_active(at as i64, n, &mut output));
                assert!(stats.is_zero(), "{stats:?}");
                for (got, expected) in output.iter().zip(&bounce) {
                    assert!(got[..n].iter().zip(&expected[at..at + n]).all(|(a, b)| a.to_bits() == b.to_bits()));
                }
                at += n;
            }
        }
    }
}

#[test]
fn monitored_input_keeps_its_host_offsets_across_splits() {
    MixEngine::force_serial(true);
    let mut s = Session::default();
    let id = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    s.track_mut(id).unwrap().mixer.input_monitor = true;
    let mut baseline = engine(&s, 4096);
    s.tempo.set_tempo(64, 150.0).unwrap();
    s.tempo.set_tempo(80, 100.0).unwrap();
    let mut split = engine(&s, 4096);
    let input: Vec<f32> = (0..4096).map(|i| i as f32 / 8192.0).collect();
    baseline.input = vec![input.clone(), input.clone()];
    split.input = baseline.input.clone();
    let mut a = vec![vec![0.0; 4096]; 2];
    let mut b = a.clone();
    baseline.render_active(0, 4096, &mut a);
    let (_, stats) = measure(|| split.render_active(0, 4096, &mut b));
    assert!(stats.is_zero(), "{stats:?}");
    assert_eq!(a, b);
    assert!(b[0][4000] > 0.1);
    assert_eq!(split.input, baseline.input);
    // A quiet final sub-block must not erase the peaks of the earlier host frames.
    for channel in &mut split.input {
        channel[1600..].fill(0.0);
    }
    split.render_active(0, 4096, &mut b);
    assert!(split.active_main_meter().peak[0] > 0.1);
    assert!(split.meters().find(|(track, _)| *track == id).unwrap().1.peak[0] > 0.1);
}

#[test]
fn bus_exports_keep_every_sub_block() {
    MixEngine::force_serial(true);
    let mut s = tempo_scene::scene();
    s.tempo.set_tempo(64, 150.0).unwrap();
    s.tempo.set_tempo(80, 100.0).unwrap();
    let bus = s.add_bus("Print", ChannelFormat::Stereo);
    s.tracks[0].mixer.output = transit_model::Route::Bus(bus);
    let range = Range::new(0, 48000);
    let a = render_bus(&s, bus, range, 64);
    let b = render_bus(&s, bus, range, 4096);
    assert!(a.iter().flatten().any(|v| v.abs() > 0.01));
    assert!(a.iter().flatten().zip(b.iter().flatten()).all(|(a, b)| a.to_bits() == b.to_bits()));
}

#[test]
fn an_unadopted_render_does_not_initialize_a_fallback_session_on_the_audio_thread() {
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48000, max_block: 512, out_channels: 2, in_channels: 0 };
    let mut mix = MixEngine::realtime(target);
    let mut output = vec![vec![1.0; 512]; 2];
    let (_, stats) = measure(|| mix.render_active(i64::MAX, 512, &mut output));
    assert!(stats.is_zero(), "{stats:?}");
    assert!(output.iter().flatten().all(|v| *v == 0.0));
    assert_eq!(mix.next_frame().0, 512);
}
