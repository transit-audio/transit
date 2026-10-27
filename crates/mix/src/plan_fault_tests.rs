#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::active::FAULT_AT;
use super::plan::{Attach, CellKind};
use super::plan_ownership_tests::sorted;
use super::plan_tests::*;
use super::*;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, Weak};
use transit_audio_io::AudioBuffer;
use transit_model::{Insert, SourceId};

#[test]
fn a_mid_commit_unwind_loses_no_owner_and_stops_the_engine() {
    for step in 1..=4 {
        let (mut rig, _, mut env) = rig_with_pending();
        let session: Weak<Session> = Arc::downgrade(&env.plan.session);
        let total = rig.log.made.load(Ordering::Relaxed);
        FAULT_AT.set(step);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.adopt_plan(&mut env)));
        FAULT_AT.set(0);
        assert!(r.is_err(), "step {step} unwound");
        assert_eq!(rig.drops(), 0, "step {step}: no owner dropped by the unwind");
        assert!(rig.eng.faulted());
        assert_eq!(env.state(), EnvelopeState::Pending, "the port keeps the envelope");
        assert_eq!(rig.render(1), vec![0.0; BLOCK], "the fault path renders silence");
        let mut again = rig.compile_after_fault_is_refused();
        assert_eq!(rig.eng.adopt_plan(&mut again), Adoption::Rejected(RejectReason::Faulted));
        drop(again);
        // Teardown after quiescence: both halves, on this (control) thread.
        let retired = std::mem::replace(&mut rig.eng, MixEngine::realtime(target())).into_retired();
        drop(retired);
        drop(env);
        assert_eq!(rig.drops(), total, "step {step}: every instance dropped exactly once, at teardown");
        assert!(session.upgrade().is_none(), "step {step}: the Session too");
    }
}

impl Rig {
    /// A plan forged against the active generation after a fault (the compiler still has one in
    /// flight, so it is built by hand from a fresh compiler).
    fn compile_after_fault_is_refused(&mut self) -> PlanEnvelope {
        let (s, _, _) = audio_session();
        let mut comp = PlanCompiler::new(target(), PlanLimits::default());
        let mut make = factory(&self.log);
        let mut env = comp.compile(Arc::new(s), PlanGeneration::EMPTY, PlanGeneration::from_raw(99), &mut make).unwrap();
        env.plan.id.base = self.eng.active().generation;
        env
    }
}

#[test]
fn a_panicking_drop_in_the_pool_does_not_wedge_the_compiler() {
    let (mut rig, _, mut env) = rig_with_pending();
    env.plan.moves[0].expect = SlotGen(9_999);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    assert_eq!(rig.comp.pooled(), 1);
    rig.log.panic_on_drop.store(true, Ordering::Relaxed);
    let (s, _, _) = audio_session();
    let mut make = factory(&rig.log);
    let active = rig.comp.active();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.comp.compile(Arc::new(s.clone()), active, PlanGeneration::from_raw(40), &mut make).map(|_| ())
    }));
    assert!(r.is_err(), "the pooled instance's drop panicked inside compile");
    let again = rig.comp.compile(Arc::new(s), active, PlanGeneration::from_raw(41), &mut make);
    assert!(again.is_ok(), "compiler wedged after a panicking drop: {:?}", again.err());
}

#[test]
fn reclaiming_a_torn_envelope_pools_nothing_under_the_wrong_spec() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("room_reverb"));
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(2);
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("chorus"));
    let mut env = rig.compile(&s);
    FAULT_AT.set(4);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.adopt_plan(&mut env)));
    FAULT_AT.set(0);
    assert!(r.is_err());
    assert_eq!(env.state(), EnvelopeState::Pending);
    rig.comp.reclaim(env);
    // Nothing from the torn envelope waits in the pool to be installed under the wrong spec, and
    // the faulted compiler makes no further plan at all.
    assert_eq!(rig.comp.pooled(), 0);
    let _ = a;
    let mut make = factory(&rig.log);
    let r = rig.comp.compile(Arc::new(s.clone()), rig.comp.active(), PlanGeneration::from_raw(30), &mut make);
    assert_eq!(r.unwrap_err(), CompileError::Faulted);
}

#[test]
fn a_plugin_panic_in_render_drops_nothing_and_the_engine_continues() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.fx"));
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(1);
    rig.log.panic_on_process.store(true, Ordering::Relaxed);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.render(1)));
    assert!(r.is_err());
    rig.log.panic_on_process.store(false, Ordering::Relaxed);
    assert_eq!(rig.drops(), 0);
    rig.render(2);
    s.track_mut(a).unwrap().mixer.inserts[1] = Some(builtin("gain"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(rig.drops(), 0);
}

// Second review repros.
#[test]
fn a_factory_panic_mid_compile_reports_the_instances_already_made() {
    let (mut s, a, _) = audio_session();
    let mut rig = Rig::new();
    rig.publish(&s);
    let _ = rig.comp.take_released();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.a"));
    s.track_mut(a).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.boom"));
    let handed = Arc::new(Mutex::new(Vec::new()));
    let mut inner = factory(&rig.log);
    let h2 = Arc::clone(&handed);
    let mut make = move |spec: &InstanceSpec, g: SlotGen| {
        if spec.id == "clap:test.boom" {
            panic!("factory unwinds");
        }
        let n = inner(spec, g);
        if n.is_some() {
            h2.lock().unwrap().push(g);
        }
        n
    };
    let active = rig.comp.active();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.comp.compile(Arc::new(s.clone()), active, PlanGeneration::from_raw(50), &mut make).map(|_| ())
    }));
    assert!(r.is_err());
    let made = handed.lock().unwrap().clone();
    assert_eq!(made.len(), 1);
    assert_eq!(rig.drops(), 1, "the made instance was destroyed by the unwind");
    let rel: Vec<SlotGen> = rig.comp.take_released().iter().map(|b| b.2).collect();
    assert!(rel.contains(&made[0]), "destroyed generation {:?} not reported; released {rel:?}", made[0]);
}

/// A sampler track with a gain insert; the next plan removes the gain and changes the sample.
fn sampler_with_gain() -> (Session, Session) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let audio = |v: f32| Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![vec![v; 4_800]] }));
    s.pool.insert(SourceId(1), audio(0.5));
    s.pool.insert(SourceId(2), audio(0.25));
    let mut ins = builtin("sampler");
    ins.sample = Some(SourceId(1));
    s.track_mut(t).unwrap().instrument = Some(ins);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    let mut s2 = s.clone();
    s2.track_mut(t).unwrap().mixer.inserts[0] = None;
    s2.track_mut(t).unwrap().instrument.as_mut().unwrap().sample = Some(SourceId(2));
    (s, s2)
}

#[test]
fn a_torn_reclaim_never_leaves_a_released_instance_active() {
    let (s, s2) = sampler_with_gain();
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let before = rig.comp.active_bindings();
    assert_eq!(before.len(), 2, "the sampler and the gain");
    let mut env = rig.compile(&s2);
    assert!(env.plan.attach.iter().any(|a| matches!(a, Attach::Sample { .. })));
    // The sampler refuses its new sample during the commit, after the tables swapped.
    rig.log.panic_on_sample.store(true, Ordering::Relaxed);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.adopt_plan(&mut env)));
    rig.log.panic_on_sample.store(false, Ordering::Relaxed);
    assert!(r.is_err());
    assert!(env.torn(), "the envelope says torn");
    assert!(!rig.comp.faulted(), "nothing known yet: the envelope has not come back");
    rig.comp.reclaim(env);
    assert!(rig.comp.faulted(), "the compiler knows the active plan is torn, not merely pending");
    let released = rig.comp.take_released();
    let active = rig.comp.active_bindings();
    assert!(!released.is_empty(), "the gain, displaced into the envelope, is destroyed");
    assert!(active.iter().all(|b| !released.contains(b)), "released {released:?} must not be active {active:?}");
    // What is reported active is what the faulted engine still owns.
    let retired = std::mem::replace(&mut rig.eng, MixEngine::realtime(target())).into_retired();
    assert_eq!(sorted(retired.instances()), active);
    drop(retired);
    assert_eq!(rig.drops(), before.len(), "every instance dropped once, nothing leaked or doubled");
}

#[test]
fn a_plain_pending_return_is_not_torn() {
    let (s, s2) = sampler_with_gain();
    let mut rig = Rig::new();
    rig.publish(&s);
    let env = rig.compile(&s2);
    assert!(!env.torn());
    rig.comp.reclaim(env);
    assert!(!rig.comp.faulted());
    assert_eq!(rig.comp.active_bindings().len(), 2);
}

#[test]
fn after_a_torn_reclaim_at_any_commit_step_the_active_bindings_are_what_the_engine_owns() {
    for step in 1..=4 {
        let (mut rig, _, mut env) = rig_with_pending();
        let _ = rig.comp.take_released();
        FAULT_AT.set(step);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.adopt_plan(&mut env)));
        FAULT_AT.set(0);
        assert!(r.is_err() && env.torn(), "step {step}");
        rig.comp.reclaim(env);
        assert!(rig.comp.faulted());
        let released = rig.comp.take_released();
        let active = rig.comp.active_bindings();
        assert!(active.iter().all(|b| !released.contains(b)), "step {step}: released {released:?} active {active:?}");
        let retired = std::mem::replace(&mut rig.eng, MixEngine::realtime(target())).into_retired();
        assert_eq!(sorted(retired.instances()), active, "step {step}");
        drop(retired);
        assert_eq!(rig.drops(), rig.log.made.load(Ordering::Relaxed), "step {step}: every instance dropped once");
    }
}

fn rich_torn(step: usize) -> (Vec<SlotBinding>, Vec<SlotBinding>, Vec<SlotBinding>, usize, usize, Rig, Session) {
    let (mut s, a, b) = audio_session();
    let inst = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let audio = |v: f32| Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![vec![v; 480]] }));
    s.pool.insert(SourceId(5), audio(0.5));
    s.pool.insert(SourceId(6), audio(0.25));
    let mut smp = Insert::new("sampler");
    smp.sample = Some(SourceId(5));
    s.track_mut(inst).unwrap().instrument = Some(smp);
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("room_reverb"));
    s.track_mut(a).unwrap().mixer.inserts[1] = Some(builtin("compressor"));
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.render(2);
    // A refused plan leaves a fresh gain at (b, 0) in the pool.
    let mut s2 = s.clone();
    s2.track_mut(b).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.g"));
    let mut env = rig.compile(&s2);
    let i = env.plan.moves.iter().position(|m| m.kind == CellKind::Strip).unwrap();
    env.plan.moves[i].expect = SlotGen(9_999);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    assert_eq!(rig.comp.pooled(), 1);
    // The torn plan: reuse the pooled gain, replace the reverb (displaced), keep the compressor,
    // swap the sampler's sample.
    s2.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("chorus"));
    s2.track_mut(inst).unwrap().instrument.as_mut().unwrap().sample = Some(SourceId(6));
    let mut env = rig.compile(&s2);
    assert!(env.plan.attach.iter().any(|x| matches!(x, Attach::Sample { .. })));
    let mut released = rig.comp.take_released();
    FAULT_AT.set(step);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.adopt_plan(&mut env)));
    FAULT_AT.set(0);
    assert!(r.is_err() && env.torn());
    rig.comp.reclaim(env);
    assert!(rig.comp.faulted());
    released.extend(rig.comp.take_released());
    let active = rig.comp.active_bindings();
    let retired = std::mem::replace(&mut rig.eng, MixEngine::realtime(target())).into_retired();
    let owned = sorted(retired.instances());
    drop(retired);
    rig.comp.clear_pool();
    released.extend(rig.comp.take_released());
    let made = rig.log.made.load(Ordering::Relaxed);
    let drops = rig.drops();
    (active, owned, released, made, drops, rig, s)
}

#[test]
fn a_torn_reclaim_that_displaced_reused_and_moved_instances_matches_the_engine_at_every_step() {
    for step in 1..=4 {
        let (active, owned, released, made, drops, _, _) = rich_torn(step);
        assert_eq!(active, owned, "step {step}");
        assert!(active.iter().all(|b| !released.contains(b)), "step {step}: overlap");
        let mut all: Vec<_> = released.iter().chain(owned.iter()).copied().collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "step {step}: double release");
        assert_eq!(n, made, "step {step}: every instance accounted once");
        assert_eq!(drops, made, "step {step}");
        if step >= 3 {
            assert!(!released.is_empty(), "step {step}: the displaced reverb came back");
        }
    }
}

#[test]
fn compile_after_a_torn_reclaim_is_refused() {
    let (mut rig, _, mut env) = rig_with_pending();
    // replace a module so something is displaced
    FAULT_AT.set(4);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.adopt_plan(&mut env)));
    FAULT_AT.set(0);
    rig.comp.reclaim(env);
    assert!(rig.comp.faulted());
    let released = rig.comp.take_released();
    let (s, _, _) = audio_session();
    let mut make = factory(&rig.log);
    let r = rig.comp.compile(Arc::new(s), rig.comp.active(), PlanGeneration::from_raw(77), &mut make);
    assert_eq!(r.unwrap_err(), CompileError::Faulted, "released: {released:?}");
    assert_eq!(rig.comp.pending_bindings(), None, "nothing in flight");
}

#[test]
fn a_session_only_compile_after_a_torn_reclaim_lists_no_released_binding_as_pending() {
    let (_, _, released, _, _, mut rig, s_old) = rich_torn(4);
    let mut make = factory(&rig.log);
    let mut s = s_old.clone();
    s.tracks[0].mixer.volume_db = -3.0;
    let r = rig.comp.compile(Arc::new(s), rig.comp.active(), PlanGeneration::from_raw(90), &mut make);
    assert_eq!(r.unwrap_err(), CompileError::Faulted);
    let pend = rig.comp.pending_bindings().unwrap_or_default();
    assert!(pend.iter().all(|b| !released.contains(b)), "pending {pend:?} includes released {released:?}");
}
