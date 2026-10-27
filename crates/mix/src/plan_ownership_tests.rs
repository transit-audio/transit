#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan::CellKind;
use super::plan_tests::*;
use super::*;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use transit_model::Insert;

#[test]
fn a_rejected_plans_owners_drop_on_the_control_thread() {
    let (mut rig, _, mut env) = rig_with_pending();
    env.plan.moves[0].expect = SlotGen(9_999);
    let control = std::thread::current().id();
    let drops = rig.drops();
    let audio = std::thread::scope(|sc| {
        sc.spawn(|| {
            assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
            std::thread::current().id()
        })
        .join()
        .unwrap()
    });
    assert_eq!(rig.drops(), drops, "nothing dropped on the audio thread");
    rig.comp.reclaim(env);
    assert_eq!(rig.comp.pooled(), 1, "the fresh instance waits in the pool");
    // The next compile does not need it: it drops at the end of that compile, here.
    let (s, _, _) = audio_session();
    let env = rig.compile(&s);
    assert_eq!(rig.comp.pooled(), 0);
    let log = rig.log.drops.lock().unwrap().clone();
    assert!(log.len() > drops);
    assert!(log.iter().all(|t| *t == control && *t != audio));
    drop(env);
}

#[test]
fn a_refused_plans_fresh_instance_is_reused_by_the_next_compile() {
    let (mut rig, s2, mut env) = rig_with_pending();
    env.plan.moves[0].expect = SlotGen(9_999);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    let made = rig.log.made.load(Ordering::Relaxed);
    assert_eq!(rig.publish(&s2), Adoption::Adopted);
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made, "no new instance was made");
    assert_eq!(rig.comp.pooled(), 0);
}

#[test]
fn identity_and_generation_rules() {
    assert_eq!(PlanGeneration::from_raw(u64::MAX).checked_next(), None);
    assert_eq!(PlanGeneration::EMPTY.checked_next(), Some(PlanGeneration::from_raw(1)));
    let (s, _, _) = audio_session();
    let mut comp = PlanCompiler::new(target(), PlanLimits::default());
    let log = SharedLog::default();
    let mut make = factory(&log);
    let g = PlanGeneration::from_raw;
    let s = Arc::new(s);
    // Stale base.
    let e = comp.compile(Arc::clone(&s), g(3), g(4), &mut make).unwrap_err();
    assert_eq!(e, CompileError::BaseNotActive { base: g(3), active: PlanGeneration::EMPTY });
    // Not increasing.
    let e = comp.compile(Arc::clone(&s), PlanGeneration::EMPTY, PlanGeneration::EMPTY, &mut make).unwrap_err();
    assert!(matches!(e, CompileError::GenerationNotIncreasing { .. }));
    // Target rate.
    let mut other = (*s).clone();
    other.sample_rate = transit_time::SampleRate::new(44_100).unwrap();
    let e = comp.compile(Arc::new(other), PlanGeneration::EMPTY, g(1), &mut make).unwrap_err();
    assert!(matches!(e, CompileError::TargetMismatch { .. }));
    // In flight (N = 1), then exhausted after the last generation.
    let env = comp.compile(Arc::clone(&s), PlanGeneration::EMPTY, g(u64::MAX), &mut make).unwrap();
    assert_eq!(comp.compile(Arc::clone(&s), PlanGeneration::EMPTY, g(5), &mut make).unwrap_err(), CompileError::InFlight { pending: g(u64::MAX) });
    let mut eng = MixEngine::realtime(target());
    let mut env = env;
    assert_eq!(eng.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    assert_eq!(comp.active(), g(u64::MAX));
    assert_eq!(comp.compile(Arc::clone(&s), g(u64::MAX), g(u64::MAX), &mut make).unwrap_err(), CompileError::GenerationExhausted);
}

#[test]
fn a_refused_replacement_dropped_from_the_pool_is_released_at_an_occupied_key() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.a"));
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let _ = rig.comp.take_released();
    let mut s2 = s.clone();
    s2.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.b"));
    let mut env = rig.compile(&s2);
    let i = env.plan.moves.iter().position(|m| m.kind == CellKind::Strip).unwrap();
    env.plan.moves[i].expect = SlotGen(9_999);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    assert_eq!(rig.comp.pooled(), 1, "B waits in the pool");
    let drops = rig.drops();
    let env = rig.compile(&s);
    assert_eq!(rig.drops(), drops + 1, "B was destroyed by drop_pool");
    let released = rig.comp.take_released();
    drop(env);
    assert!(!released.is_empty(), "B's destruction is reported: released = {released:?}");
}

#[test]
fn a_format_change_of_the_same_width_makes_a_fresh_instance() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Quad, None);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let made = rig.log.made.load(Ordering::Relaxed);
    s.track_mut(t).unwrap().format = ChannelFormat::Ambisonics(1);
    assert_eq!(rig.publish(&s), Adoption::Adopted, "a new layout is a new cell, not a move");
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made + 1);
}

// Instance identity (F5): the factory gets each instance's generation, and every instance the
// compiler destroys comes back once from `take_released` with that generation.

fn compile_recording(rig: &mut Rig, s: &Session, made: &Arc<Mutex<Vec<SlotBinding>>>) -> PlanEnvelope {
    rig.issued += 1;
    let mut inner = factory(&rig.log);
    let made = Arc::clone(made);
    let mut make = move |spec: &InstanceSpec, generation: SlotGen| {
        made.lock().unwrap().push((spec.track, spec.slot, generation));
        inner(spec, generation)
    };
    rig.comp.compile(Arc::new(s.clone()), rig.comp.active(), PlanGeneration::from_raw(rig.issued), &mut make).unwrap()
}

fn publish_recording(rig: &mut Rig, s: &Session, made: &Arc<Mutex<Vec<SlotBinding>>>) -> Adoption {
    let mut env = compile_recording(rig, s, made);
    let a = rig.eng.adopt_plan(&mut env);
    rig.comp.reclaim(env);
    a
}

pub(super) fn sorted(mut v: Vec<SlotBinding>) -> Vec<SlotBinding> {
    v.sort_unstable();
    v
}

#[test]
fn the_factory_gets_the_generation_each_instance_carries() {
    let (mut s, a, b) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("room_reverb"));
    s.track_mut(b).unwrap().mixer.inserts[3] = Some(Insert::new("clap:test.fx"));
    let (mut rig, made) = (Rig::new(), Arc::default());
    let mut env = compile_recording(&mut rig, &s, &made);
    assert_eq!(rig.comp.pending_bindings(), Some(sorted(made.lock().unwrap().clone())), "bound before publication");
    assert_eq!(rig.eng.adopt_plan(&mut env), Adoption::Adopted);
    rig.comp.reclaim(env);
    assert_eq!(rig.comp.active_bindings(), sorted(made.lock().unwrap().clone()));
    assert!(rig.comp.take_released().is_empty());
}

#[test]
fn a_displaced_instance_is_released_with_its_generation() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.a"));
    let (mut rig, made) = (Rig::new(), Arc::default());
    publish_recording(&mut rig, &s, &made);
    let old = made.lock().unwrap()[0];
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.b"));
    assert_eq!(publish_recording(&mut rig, &s, &made), Adoption::Adopted);
    let new = made.lock().unwrap()[1];
    assert_eq!((new.0, new.1), (old.0, old.1), "the same slot");
    assert_eq!(rig.comp.take_released(), vec![old], "the displaced instance, by its generation");
    assert_eq!(rig.comp.active_bindings(), vec![new], "the new one lives at that slot");
}

#[test]
fn a_rejected_instance_is_released_when_the_pool_drops_it_and_not_when_it_is_reused() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.a"));
    let (mut rig, made) = (Rig::new(), Arc::default());
    publish_recording(&mut rig, &s, &made);
    let mut s2 = s.clone();
    s2.track_mut(a).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.b"));
    let mut env = compile_recording(&mut rig, &s2, &made);
    let b = *made.lock().unwrap().last().unwrap();
    env.plan.moves[0].expect = SlotGen(9_999);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    assert!(rig.comp.take_released().is_empty(), "rejected: B waits in the pool, alive");
    // Reused: same binding, nothing released.
    let mut env = compile_recording(&mut rig, &s2, &made);
    assert!(rig.comp.pending_bindings().unwrap().contains(&b), "the pooled instance keeps its generation");
    assert!(rig.comp.take_released().is_empty());
    env.plan.moves[0].expect = SlotGen(9_999);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    // Not wanted by the next compile: dropped from the pool, released.
    let env = compile_recording(&mut rig, &s, &made);
    assert_eq!(rig.comp.take_released(), vec![b]);
    drop(env);
}

#[test]
fn a_pool_drop_at_an_occupied_slot_is_released_by_generation() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.a"));
    let (mut rig, made) = (Rig::new(), Arc::default());
    publish_recording(&mut rig, &s, &made);
    let a_bound = made.lock().unwrap()[0];
    let mut s2 = s.clone();
    s2.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.b"));
    let mut env = compile_recording(&mut rig, &s2, &made);
    let b_bound = made.lock().unwrap()[1];
    env.plan.moves[0].expect = SlotGen(9_999);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    let env = compile_recording(&mut rig, &s, &made);
    assert_eq!(rig.comp.take_released(), vec![b_bound], "B is gone although A lives at the same slot");
    assert_eq!(rig.comp.pending_bindings(), Some(vec![a_bound]));
    drop(env);
}

#[test]
fn teardown_reports_every_instance_by_generation() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.a"));
    let (mut rig, made) = (Rig::new(), Arc::default());
    publish_recording(&mut rig, &s, &made);
    s.track_mut(a).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.b"));
    // A plan that never reaches the audio thread, then teardown.
    let env = compile_recording(&mut rig, &s, &made);
    rig.comp.reclaim(env);
    rig.comp.clear_pool();
    let b = made.lock().unwrap()[1];
    assert_eq!(rig.comp.take_released(), vec![b], "the unsent plan's instance");
    let active = rig.comp.active_bindings();
    let retired = std::mem::replace(&mut rig.eng, MixEngine::realtime(target())).into_retired();
    assert_eq!(sorted(retired.instances()), active, "the engine's instances, for playback to release at teardown");
    drop(retired);
    assert_eq!(rig.drops(), 2);
}

#[test]
fn slot_generations_never_wrap() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    let mut rig = Rig::new();
    rig.comp.set_slot_generation_for_tests(u64::MAX - 1);
    let mut make = factory(&rig.log);
    let r = rig.comp.compile(Arc::new(s), PlanGeneration::EMPTY, PlanGeneration::from_raw(1), &mut make);
    assert_eq!(r.unwrap_err(), CompileError::GenerationExhausted);
    assert_eq!(rig.comp.pending(), None, "nothing in flight");
    assert_eq!(rig.log.made.load(Ordering::Relaxed), rig.drops(), "whatever was made was dropped");
    assert_eq!(rig.comp.take_released().len(), rig.drops(), "and reported");
}

#[test]
fn reclaim_after_clear_pool_releases_instead_of_pooling() {
    let (mut rig, s2, env) = rig_with_pending();
    let _ = s2;
    rig.comp.clear_pool();
    rig.comp.reclaim(env); // never serviced (teardown)
    assert_eq!(rig.comp.pooled(), 0, "teardown: reclaim after clear_pool pools instances that die unreported with the compiler");
}

#[test]
fn every_instance_is_released_exactly_once_across_pool_reuse_and_teardown() {
    use std::collections::BTreeSet;
    let (mut s1, a, _) = audio_session();
    s1.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.a"));
    let mut s2 = s1.clone();
    s2.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.b"));
    s2.track_mut(a).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.c"));
    let mut rig = Rig::new();
    let mut released: Vec<SlotGen> = Vec::new();
    let mut seen: BTreeSet<SlotGen> = BTreeSet::new();
    let reject = |env: &mut PlanEnvelope| {
        if let Some(i) = env.plan.moves.iter().position(|m| m.kind == CellKind::Strip) {
            env.plan.moves[i].expect = SlotGen(9_999);
        }
    };
    let steps: [(u8, bool); 7] = [(1, true), (2, false), (2, false), (1, true), (2, false), (2, true), (1, true)];
    for (k, (which, ok)) in steps.into_iter().enumerate() {
        let s = if which == 1 { &s1 } else { &s2 };
        let mut env = rig.compile(s);
        for b in env.plan.tables.as_ref().map(|t| t.bindings()).unwrap_or_default() {
            seen.insert(b.2);
        }
        if !ok {
            reject(&mut env);
        }
        let r = rig.eng.adopt_plan(&mut env);
        assert_eq!(matches!(r, Adoption::Adopted), ok, "step {k}");
        rig.comp.reclaim(env);
        released.extend(rig.comp.take_released().iter().map(|b| b.2));
        rig.render(1);
        let live: Vec<SlotGen> = rig.comp.active_bindings().iter().map(|b| b.2).collect();
        let mut sorted = released.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), released.len(), "step {k}: double release {released:?}");
        assert!(live.iter().all(|g| !released.contains(g)), "step {k}: live {live:?} released {released:?}");
        let mut lv = live.clone();
        lv.sort();
        lv.dedup();
        assert_eq!(lv.len(), live.len(), "step {k}: aliased live generations");
    }
    rig.comp.clear_pool();
    released.extend(rig.comp.take_released().iter().map(|b| b.2));
    let retired = std::mem::replace(&mut rig.eng, MixEngine::realtime(target())).into_retired();
    released.extend(retired.instances().iter().map(|b| b.2));
    drop(retired);
    let all: BTreeSet<SlotGen> = released.iter().copied().collect();
    assert_eq!(all.len(), released.len(), "double release at teardown");
    assert_eq!(all, seen, "every instance ever made is released exactly once");
    assert_eq!(rig.drops(), seen.len());
}
