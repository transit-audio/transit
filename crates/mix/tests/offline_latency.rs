//! #251: timeline placement with real Texture and Compressor automation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/offline_latency.rs"]
mod fixture;

use fixture::{IMPULSES, LEN, positions, session};
use std::sync::Arc;
use transit_audio_io::AudioBuffer;
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, PlanCompiler, PlanLimits, RUN_IN_SECONDS, RangeRender, RenderTarget, create_instance,
};
use transit_model::{ChannelFormat, IoMap, Route, SendSlot, Session, SourceAudio, SourceId};
use transit_time::Range;

#[test]
fn changing_latency_keeps_both_impulse_tracks_on_the_timeline() {
    let mut measured = Vec::new();
    let mut expected = Vec::new();
    for (device, values) in
        [("texture", [3.0, 0.0]), ("texture", [0.0, 3.0]), ("texture", [3.0, 2.0]), ("compressor", [0.0, 5.0]), ("compressor", [5.0, 0.0])]
    {
        for block in [16, 64, 512, 1024] {
            for change in [2048, 2085] {
                let (s, _) = session(device, &values, &[change]);
                let audio = transit_mix::render_range(&s, Range::new(0, LEN as i64), block);
                assert!(audio.iter().all(|c| c.len() == LEN));
                let got = positions(&audio);
                println!("{device} {values:?}, block {block}, change {change}: {got:?}");
                measured.extend(got);
                // Dirty Tape's wow/flutter follows even the dry/wet mixer (texture/processor.rs).
                // These are its deterministic effect offsets, measured before the offline fix
                // after subtracting the old renderer's erroneous 138-frame shift. Do not "fix"
                // that intentional modulation. Vintage Digital and the compressor are exact.
                // Starting in Dirty Tape, the print starts from a settled chain (ADR 0038, #332
                // amendment): the wow has run through the run-in's four seconds before frame
                // zero, as a live chain's has before play, so the first impulse lands at 1078
                // (a fresh chain's first second put it at 1128).
                let left = if device == "texture" && values[0] == 0.0 {
                    [1078, 4096, 8192]
                } else if device == "texture" && values[1] == 0.0 {
                    match (change, block) {
                        (2048, _) => [1024, 4203, 8314],
                        (_, 16 | 64) => [1024, 4202, 8314],
                        (_, 512) => [1024, 4200, 8312],
                        _ => [1024, 4198, 8311],
                    }
                } else {
                    IMPULSES
                };
                expected.extend(left);
                expected.extend(IMPULSES);
            }
        }
    }
    assert_eq!(measured, expected, "all measured impulse positions, in the case order printed above");
}

/// Independent oracle: timestamp every live sample with that block's latency, keep the first
/// sample for each output position, and leave missing positions zero. No streaming skip state.
/// Live play starts from a stop: the chain first runs [`RUN_IN_SECONDS`] with the transport
/// stopped at 0, in the offline run-in's block partition (ADR 0038, #332 amendment).
fn live_reference(s: &Session, block: usize) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    MixEngine::force_serial(true);
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: block as u32, out_channels: 0, in_channels: 0 };
    let mut mix = MixEngine::realtime(target);
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _| create_instance(&spec.id, spec.state.as_deref(), 48_000.0, block, spec.ch);
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), compiler.active().checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let mut aligned = vec![vec![0.0; LEN]; 2];
    let mut raw = vec![Vec::new(); 2];
    let mut next = 0;
    let mut initial = 0;
    let mut out = vec![vec![0.0; block]; 2];
    mix.monitor_only = true;
    mix.no_suspend = true;
    let run_in = 48_000 * RUN_IN_SECONDS as usize;
    let mut done = 0;
    while done < run_in {
        let n = block.min(run_in - done);
        mix.render_active(0, n, &mut out);
        done += n;
    }
    mix.monitor_only = false;
    mix.no_suspend = false;
    for pos in (0..LEN + 512).step_by(block) {
        mix.render_active(pos as i64, block, &mut out);
        if pos == 0 {
            initial = mix.latency;
        }
        for (r, c) in raw.iter_mut().zip(&out) {
            r.extend_from_slice(c);
        }
        for i in 0..block {
            let Some(at) = (pos + i).checked_sub(mix.latency) else { continue };
            if at >= next && at < LEN {
                for (a, c) in aligned.iter_mut().zip(&out) {
                    a[at] = c[i];
                }
                next = at + 1;
            }
        }
    }
    let legacy = raw.into_iter().map(|c| c[initial..initial + LEN].to_vec()).collect();
    (aligned, legacy)
}

#[test]
fn every_retained_sample_equals_live_with_exactly_the_documented_splice() {
    for (device, values) in [("texture", [3.0, 0.0, 3.0]), ("compressor", [1.0, 10.0, 0.0])] {
        for block in [16, 64, 512, 1024] {
            let (mut s, _) = session(device, &values, &[2085, 6175]);
            // Dense, changing audio makes the overlap, gap and crossfade comparisons meaningful.
            let data = (0..LEN).map(|i| ((i * 17 % 127) as f32 - 63.0) / 1024.0).collect();
            s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![data] })));
            let (reference, legacy) = live_reference(&s, block);
            let got = transit_mix::render_range(&s, Range::new(0, LEN as i64), block);
            for (c, (a, b)) in got.iter().zip(&reference).enumerate() {
                assert_eq!(a.iter().zip(b).position(|(a, b)| a.to_bits() != b.to_bits()), None, "{device} block {block} channel {c}");
            }
            assert_ne!(got, legacy, "a single initial trim cannot align changing latency");
        }
    }
}

fn collect(mut render: RangeRender, block: usize, channels: usize) -> Vec<Vec<f32>> {
    let mut out = vec![Vec::new(); channels];
    // A deterministic bound catches a stalled skip without a scheduling assertion.
    for _ in 0..LEN * 2 {
        let Some((chunk, n)) = render.next_block() else {
            assert!(out.iter().all(|c| c.len() == LEN));
            return out;
        };
        assert!(n <= block);
        for (o, c) in out.iter_mut().zip(chunk) {
            o.extend_from_slice(&c[..n]);
        }
    }
    panic!("render did not finish");
}

#[test]
fn buses_and_all_output_paths_use_the_same_latency_splice() {
    let (mut s, _) = session("compressor", &[0.0, 10.0, 0.0], &[2048, 6144]);
    let r = Range::new(0, LEN as i64);
    let expected = transit_mix::render_range(&s, r, 64);
    let bus = s.add_bus("Stem", ChannelFormat::Stereo);
    let io = IoMap::for_device("test", "test", 0, 4);
    let path = "Out 3-4".to_owned();
    for t in &mut s.tracks {
        let mut send = SendSlot::new(Route::Bus(bus));
        send.level_db = 0.0;
        // Mono sends use their own pan; follow_main_pan selects only the surround position.
        send.pan = t.mixer.pan[0];
        t.mixer.sends[0] = Some(send);
        let mut send = SendSlot::new(Route::Hardware(path.clone()));
        send.level_db = 0.0;
        send.pan = t.mixer.pan[0];
        t.mixer.sends[1] = Some(send);
    }
    assert_audio_eq(&transit_mix::render_bus(&s, bus, r, 64), &expected);
    let render = RangeRender::try_new_paths(&s, r, 64, &io).unwrap();
    let paths = collect(render, 64, 4);
    assert_audio_eq(&paths[..2], &expected);
    assert_audio_eq(&paths[2..], &expected);
}

fn assert_audio_eq(got: &[Vec<f32>], expected: &[Vec<f32>]) {
    assert_eq!(got.len(), expected.len());
    for (c, (a, b)) in got.iter().zip(expected).enumerate() {
        assert_eq!(a.len(), b.len());
        let bad = a.iter().zip(b).position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(bad, None, "channel {c}: {:?}", bad.map(|i| (a[i], b[i])));
    }
}

#[test]
fn a_second_change_during_an_unfinished_skip_does_not_accumulate_drift() {
    let (s, _) = session("compressor", &[0.0, 10.0, 1.0, 10.0, 0.0], &[2048, 2080, 2112, 6144]);
    let got = collect(RangeRender::new(&s, None, Range::new(0, LEN as i64), 16), 16, 2);
    assert_eq!(positions(&got), IMPULSES.into_iter().cycle().take(6).collect::<Vec<_>>());
    assert_eq!(got, live_reference(&s, 16).0);
}

#[test]
fn growth_and_shrink_near_the_end_and_within_the_initial_skip_keep_the_range_length() {
    for changes in [[16, 32], [9984, 10_000]] {
        let (s, _) = session("compressor", &[10.0, 0.0, 10.0], &changes);
        let got = collect(RangeRender::new(&s, None, Range::new(0, LEN as i64), 16), 16, 2);
        assert_eq!(got, live_reference(&s, 16).0);
    }
}
