//! Tail description invalidation, isolated from every pre-existing description field.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use transit_model::{Bus, SendSlot};

#[test]
fn t20_send_retarget_rebuilds_but_level_and_mute_reuse_tables() {
    let _switch = crate::strip_pool::SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: 64, out_channels: 0, in_channels: 0 };
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    let mut engine = MixEngine::realtime(target);
    let mut s = Session::default();
    let a = BusId(s.alloc());
    let b = BusId(s.alloc());
    let input = BusId(s.alloc());
    s.edit.click = false;
    for id in [a, b, input] {
        s.busses.push(Bus { id, name: format!("bus {}", id.0), format: ChannelFormat::Stereo });
    }
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    let other = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    let reader = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, None);
    s.track_mut(aux).unwrap().mixer.input = Route::Bus(input);
    s.track_mut(aux).unwrap().mixer.output = Route::Bus(b);
    s.track_mut(reader).unwrap().mixer.input = Route::Bus(b);
    for k in 0..4 {
        let feed = crate::tests::add_dc_track(&mut s, 0.25, 48_000);
        s.track_mut(feed).unwrap().mixer.output = if k == 0 { Route::Bus(input) } else { Route::None };
    }
    s.track_mut(aux).unwrap().mixer.sends[0] = Some(SendSlot::new(Route::Bus(a)));
    s.track_mut(other).unwrap().mixer.output = Route::Bus(a);
    let mut make = |_: &InstanceSpec, _: SlotGen| None;
    let mut first = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut make).unwrap();
    assert_eq!(engine.adopt_plan(&mut first), Adoption::Adopted);
    compiler.reclaim(first);
    let generation = engine.plan.as_deref().unwrap().tables.graph.tail.graph.generation();
    let old = compiler.describe(&s, 0, CompileMode::Update).desc;
    s.track_mut(aux).unwrap().mixer.sends[0].as_mut().unwrap().target = Route::Bus(b);
    let mut changed = compiler.describe(&s, 0, CompileMode::Update).desc;
    assert_ne!(old.tail, changed.tail);
    changed.tail = old.tail.clone();
    assert!(old == changed, "all pre-existing description fields equal");
    let mut next = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(2), &mut make).unwrap();
    assert!(next.plan().is_structural());
    assert_eq!(engine.adopt_plan(&mut next), Adoption::Adopted);
    compiler.reclaim(next);
    assert_ne!(generation, engine.plan.as_deref().unwrap().tables.graph.tail.graph.generation());
    let mut reference = crate::tests::PlanRig::new(64);
    reference.publish(&s);
    engine.start_render_workers_with(64, 0);
    engine.seed_strip_costs(std::time::Duration::from_nanos(1));
    engine.set_strip_deadline(Some(std::time::Duration::from_secs(60)));
    MixEngine::force_graph(true);
    MixEngine::force_tail(true);
    MixEngine::force_serial(false);
    mark_audio_thread();
    let mut out = vec![vec![0.0; 64]; 2];
    engine.render_active(0, 64, &mut out);
    let serial = reference.render(0);
    unmark_audio_thread();
    MixEngine::force_tail(false);
    MixEngine::force_serial(true);
    assert_eq!(out, serial, "retargeted commits and snapshots follow serial routing");
    assert!(out.iter().flatten().any(|x| x.abs() > 0.0));
    assert_eq!(engine.strip_counts().len(), engine.strip_phases().1);
    for (generation, mute) in [(3, false), (4, true)] {
        let send = s.track_mut(aux).unwrap().mixer.sends[0].as_mut().unwrap();
        send.level_db = -9.0;
        send.mute = mute;
        let mut next = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(generation), &mut make).unwrap();
        assert!(!next.plan().is_structural());
        assert_eq!(engine.adopt_plan(&mut next), Adoption::Adopted);
        compiler.reclaim(next);
    }
}
