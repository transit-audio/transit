//! Full-load compatibility through session persistence, the plan renderer and export.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/sampler_full_load.rs"]
mod fixture;
use fixture::*;
use std::sync::Arc;
use transit_mix::{Adoption, DeviceGeneration, MixEngine, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget};
use transit_model::Session;
use transit_time::Range;

fn realtime(s: &Session, block: usize) -> Vec<Vec<f32>> {
    MixEngine::force_serial(true);
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48000, max_block: block as u32, out_channels: 2, in_channels: 0 };
    let mut mix = MixEngine::realtime(target);
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    let mut maker = |spec: &transit_mix::InstanceSpec, _: transit_mix::SlotGen| {
        transit_mix::create_instance(&spec.id, spec.state.as_deref(), 48000.0, block, spec.ch)
    };
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut maker).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let mut output: Vec<_> = (0..2).map(|_| Vec::with_capacity(FRAMES)).collect();
    let mut io = vec![vec![0.0; block]; 2];
    for at in (0..FRAMES).step_by(block) {
        let n = block.min(FRAMES - at);
        mix.render_active(at as i64, n, &mut io);
        for (o, c) in output.iter_mut().zip(&io) {
            o.extend_from_slice(&c[..n]);
        }
    }
    assert_eq!(mix.status().capacity_exceeded, 0);
    output
}

#[test]
fn export_equals_realtime_render_for_the_fixture() {
    for shaped in [false, true] {
        let s = session(shaped);
        let want = bits(&realtime(&s, 64));
        assert!(want.iter().any(|b| *b != 0));
        for block in [64, 128, 512, 97] {
            let got = bits(&transit_mix::render_range(&s, Range::new(0, FRAMES as i64), block));
            assert_eq!(got.iter().zip(&want).position(|(a, b)| a != b), None, "shaped {shaped}, export block {block}");
            assert_eq!(got.len(), want.len());
        }
    }
}

#[test]
fn reopen_restarts_counters_and_rng() {
    for shaped in [false, true] {
        let s = session(shaped);
        let first = bits(&realtime(&s, 97));
        let json = s.to_json().unwrap();
        let mut reopened = Session::from_json(&json).unwrap();
        media(&mut reopened);
        assert_eq!(bits(&realtime(&reopened, 97)), first);
    }
}
