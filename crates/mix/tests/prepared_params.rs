//! Stored values are initial state; automation after preparation remains a move (#280).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_mix::{Adoption, DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanLimits, RenderTarget};
use transit_model::{AutoParam, ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackKind};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};
use transit_time::Range;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const BLOCK: usize = 64;

fn scene() -> Session {
    scene_frames(BLOCK)
}

fn scene_frames(frames: usize) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; frames]; 2] })));
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "dc", SourceId(1), 0, 0, frames as i64));
    let mut filter = Insert::new("filter");
    filter.params.insert("cutoff".into(), 300.0);
    tr.mixer.inserts[0] = Some(filter);
    s
}

fn live(s: &Session, initialized_reference: bool) -> Vec<Vec<f32>> {
    live_frames(s, initialized_reference, BLOCK)
}

fn live_frames(s: &Session, initialized_reference: bool, frames: usize) -> Vec<Vec<f32>> {
    MixEngine::force_serial(true);
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    let mut engine = MixEngine::realtime(target);
    let mut make = |spec: &InstanceSpec, _: transit_mix::SlotGen| {
        if initialized_reference {
            let mut plugin = transit_dsp::create(&spec.id).unwrap();
            plugin.set_param("cutoff", 300.0);
            plugin.prepare(48_000.0, BLOCK, spec.ch);
            Some(transit_mix::NewInstance { plugin, refused_state: false })
        } else {
            transit_mix::create_instance(&spec.id, spec.state.as_deref(), 48_000.0, BLOCK, spec.ch)
        }
    };
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), compiler.active().checked_next().unwrap(), &mut make).unwrap();
    // Creation, prepare and initial parameter snapping run on the compiler/control thread.
    // Measure adoption and the very first render, with no warm-up block.
    let mut out = vec![vec![0.0; BLOCK]; 2];
    let (adoption, stats) = measure(|| {
        let adoption = engine.adopt_plan(&mut env);
        engine.render_active(0, BLOCK, &mut out);
        adoption
    });
    assert_eq!(adoption, Adoption::Adopted);
    assert_eq!(stats, AllocStats::default(), "adoption and first block allocate and free nothing");
    compiler.reclaim(env);
    let mut result = out.clone();
    for pos in (BLOCK..frames).step_by(BLOCK) {
        engine.render_active(pos as i64, BLOCK, &mut out);
        for (dst, src) in result.iter_mut().zip(&out) {
            dst.extend_from_slice(src);
        }
    }
    result
}

fn assert_initial_state(got: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(got.len(), expected.len());
    for (actual, reference) in got.iter().zip(expected) {
        assert_eq!(actual.len(), reference.len());
    }
    let first = got.iter().flatten().zip(expected.iter().flatten()).position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None, "first block must start at the stored 300 Hz, including sample 0");
    assert!(expected[0][0] > 0.0, "the first sample exercises the filter");
}

#[test]
fn offline_filter_starts_at_its_stored_cutoff() {
    let s = scene();
    assert_initial_state(&transit_mix::render_range(&s, Range::new(0, BLOCK as i64), BLOCK), &live(&s, true));
}

#[test]
fn fresh_live_filter_starts_at_its_stored_cutoff() {
    let s = scene();
    assert_initial_state(&live(&s, false), &live(&s, true));
}

/// Independent DSP reference: the stored value is prepared, then the block-start automation
/// values are ordinary moves. Comparing every sample catches both a lost lane and a snapped ramp.
fn automated_reference(values: &[f32]) -> Vec<Vec<f32>> {
    let mut filter = transit_dsp::create("filter").unwrap();
    filter.set_param("cutoff", 300.0);
    filter.prepare(48_000.0, BLOCK, 2);
    let mut result = vec![Vec::new(); 2];
    for &value in values {
        filter.set_param("cutoff", value);
        let mut io = vec![vec![0.25; BLOCK]; 2];
        filter.process(&mut io, BLOCK);
        for (dst, src) in result.iter_mut().zip(io) {
            dst.extend(src);
        }
    }
    result
}

#[test]
fn time_zero_points_and_ramps_are_applied_after_preparation() {
    let frames = 8 * BLOCK;
    for (points, values) in [
        (vec![(0, 300.0)], vec![300.0; 8]),
        (vec![(0, 1500.0)], vec![1500.0; 8]),
        (vec![(0, 300.0), (256, 1500.0)], vec![300.0, 600.0, 900.0, 1200.0, 1500.0, 1500.0, 1500.0, 1500.0]),
    ] {
        let mut s = scene_frames(frames);
        let track = s.tracks.iter_mut().find(|t| t.kind == TrackKind::Audio).unwrap();
        for (at, value) in points {
            track.lane_mut(&AutoParam::Plugin { slot: 0, param: "cutoff".into() }).set_point(at, value);
        }
        let expected = automated_reference(&values);
        assert_eq!(transit_mix::render_range(&s, Range::new(0, frames as i64), BLOCK), expected);
        assert_eq!(live_frames(&s, false, frames), expected);
        if values[0] == 300.0 && values[1] != 300.0 {
            let steady = automated_reference(&[300.0; 8]);
            assert_eq!(&expected[0][..BLOCK], &steady[0][..BLOCK]);
            assert_ne!(&expected[0][BLOCK..], &steady[0][BLOCK..], "the ramp must move after time zero");
        }
    }
}
