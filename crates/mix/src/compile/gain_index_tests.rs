//! The plan's group indexes (ADR 0058, slice G1a; record test 11a's index part): admitted against the
//! prepared-plan limit before anything is allocated, a refusal leaving the active plan rendering, and
//! an index held for every mixer gain lane and clip envelope the plan reads.

#![allow(clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::plan_tests::{Rig, audio_session, factory};
use transit_model::gain_lane::build_group_index;
use transit_model::{AutoParam, AutomationPoint};

/// The record's example: a clip envelope of 100,000 points.
fn with_envelope(mut s: Session, track: TrackId, points: i64) -> Session {
    let clip = &mut s.track_mut(track).unwrap().playlist_mut().unwrap().clips[0];
    clip.gain_env = (0..points).map(|i| (i, -6.0)).collect();
    s
}

/// The prepared bytes `s` needs, as the first (smallest-delay) measure reports them.
fn need(s: &Session) -> usize {
    let limits = PlanLimits { prepared_bytes: 0, ..PlanLimits::default() };
    match crate::compile::measure(s, &limits, 256, &|_, _| 0, &|_, _| 0) {
        Err(CompileError::ResourceLimit { what: "prepared_bytes", need, .. }) => need,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_100_000_point_envelope_admits_400_000_bytes_of_index() {
    let (base, a, _) = audio_session();
    let big = with_envelope(base.clone(), a, 100_000);
    assert_eq!(bytes(&base).unwrap(), 0, "no lane and no envelope: no index");
    assert_eq!(bytes(&big).unwrap(), 400_000 + size_of::<IndexEntry>(), "4 bytes a point, plus the index's entry");
    assert_eq!(need(&big) - need(&base), 400_000 + size_of::<IndexEntry>(), "counted in measure");
}

#[test]
fn a_plan_refused_for_its_indexes_allocates_nothing_and_leaves_the_active_plan_rendering() {
    let (base, a, _) = audio_session();
    let big = with_envelope(base.clone(), a, 100_000);

    // The reference: the active plan rendering on, undisturbed.
    let mut reference = Rig::new();
    assert_eq!(reference.publish(&base), Adoption::Adopted);
    let want: Vec<u32> = reference.render(8).iter().map(|x| x.to_bits()).collect();

    let mut rig = Rig::new();
    assert_eq!(rig.publish(&base), Adoption::Adopted);
    let mut got = rig.render(4);
    // A compile against the active plan, measured: its result and what it allocated. (Its need is
    // smaller than a fresh compiler's: strips that keep their live delay lines count none.)
    let attempt = |rig: &mut Rig, s: &Session| {
        rig.issued += 1;
        let (active, next) = (rig.comp.active(), PlanGeneration::from_raw(rig.issued));
        let mut make = factory(&rig.log);
        let session = Arc::new(s.clone());
        transit_rt::testing::measure(|| rig.comp.compile(Arc::clone(&session), active, next, &mut make))
    };
    let need_of = |rig: &mut Rig, s: &Session| {
        rig.comp.limits.prepared_bytes = 1;
        match attempt(rig, s).0 {
            Err(CompileError::ResourceLimit { what: "prepared_bytes", need, .. }) => need,
            other => panic!("{other:?}"),
        }
    };
    let need_big = need_of(&mut rig, &big);
    assert_eq!(need_big - need_of(&mut rig, &base), 400_000 + size_of::<IndexEntry>());

    // One byte short of the plan with the envelope: refused before anything is allocated.
    rig.comp.limits.prepared_bytes = need_big - 1;
    let (r, stats) = attempt(&mut rig, &big);
    match r {
        Err(CompileError::ResourceLimit { what: "prepared_bytes", need, limit }) => assert_eq!((need, limit), (need_big, need_big - 1)),
        other => panic!("{other:?}"),
    }
    assert_eq!(stats.allocs, 0, "refused before anything was allocated");
    got.extend(rig.render(4));
    let got: Vec<u32> = got.iter().map(|x| x.to_bits()).collect();
    assert_eq!(got, want, "the active plan keeps rendering, bit for bit");

    // Exactly at the limit: admitted, with the envelope's index in the plan.
    rig.comp.limits.prepared_bytes = need_big;
    let mut env = rig.compile(&big);
    let clip = big.track(a).unwrap().clips()[0].clone();
    assert_eq!(env.plan().gain_indexes.envelope(a, clip.id), Some(&*build_group_index(&clip.gain_env).unwrap()));
    assert_eq!(rig.eng.adopt_plan(&mut env), Adoption::Adopted);
    rig.comp.reclaim(env);
}

fn points(pts: &[(i64, f32)]) -> Vec<AutomationPoint> {
    pts.iter().map(|&(at, value)| AutomationPoint { at, value }).collect()
}

#[test]
fn every_mixer_gain_lane_and_clip_envelope_gets_its_index_and_device_lanes_get_none() {
    let (mut s, a, b) = audio_session();
    let groups = points(&[(0, 0.0), (64, -6.0), (64, -12.0), (64, -3.0), (128, 0.0)]);
    let trim = AutoParam::Plugin { slot: u8::MAX, param: "trim".into() };
    let device = AutoParam::Plugin { slot: 0, param: "gain".into() };
    {
        let t = s.track_mut(a).unwrap();
        for p in [
            AutoParam::Volume,
            trim,
            AutoParam::Pan(0),
            AutoParam::Mute,
            AutoParam::SendLevel(0),
            AutoParam::SendPan(1),
            AutoParam::SendMute(2),
            device,
        ] {
            t.lane_mut(&p).points = groups.clone();
        }
        t.lane_mut(&AutoParam::Pan(1)); // a mixer lane with no points: nothing to index
        t.playlist_mut().unwrap().clips[0].gain_env = vec![(0, 0.0), (10, -6.0), (10, 0.0), (20, -3.0)];
    }
    s.track_mut(b).unwrap().lane_mut(&AutoParam::Volume).points = points(&[(5, -1.0)]);
    let mut rig = Rig::new();
    let env = rig.compile(&s);
    let check = |env: &PlanEnvelope, s: &Session| {
        let ix = &env.plan().gain_indexes;
        let mut counted = 0;
        for t in &s.tracks {
            for (i, l) in t.automation.iter().enumerate() {
                let want = (l.param.is_mixer_gain() && !l.points.is_empty()).then(|| build_group_index(&l.points).unwrap());
                assert_eq!(ix.lane(t.id, i), want.as_deref(), "{:?} on {:?}", l.param, t.id);
                counted += usize::from(want.is_some());
            }
            for c in t.clips() {
                let want = (!c.gain_env.is_empty()).then(|| build_group_index(&c.gain_env).unwrap());
                assert_eq!(ix.envelope(t.id, c.id), want.as_deref());
                counted += usize::from(want.is_some());
            }
        }
        assert_eq!(ix.entries.len(), counted);
        // What was allocated is what measure counted.
        assert_eq!(bytes(s).unwrap(), ix.data.len() * size_of::<u32>() + ix.entries.len() * size_of::<IndexEntry>());
    };
    check(&env, &s);
    assert_eq!(env.plan().gain_indexes.lane(a, 0).unwrap(), &[0, 3, 3, 3, 4], "the group at 64 ends at its third point");
    let mut env = env;
    assert_eq!(rig.eng.adopt_plan(&mut env), Adoption::Adopted);
    rig.comp.reclaim(env);

    // A lane edit alone publishes no tables, and its plan still carries fresh indexes.
    let mut s2 = s.clone();
    s2.track_mut(a).unwrap().lane_mut(&AutoParam::Volume).points = points(&[(0, 0.0), (7, -1.0), (7, -2.0)]);
    let env = rig.compile(&s2);
    assert!(!env.plan().is_structural());
    check(&env, &s2);
    rig.comp.reclaim(env);
}
