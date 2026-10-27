#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan_tests::*;
use super::*;
use std::sync::atomic::Ordering;
use transit_model::{Insert, SourceId};

/// Delay compensation holds track `a`'s audio in its strip's delay line (a lookahead insert on
/// track `b`); a structural plan arrives mid-burst. Renders 30 blocks.
fn delayed_with_edit(edit: bool) -> Vec<f32> {
    let (mut s, _, b) = audio_session();
    s.edit.delay_compensation = true;
    let mut lookahead = builtin("compressor");
    lookahead.params.insert("lookahead".into(), 10.0);
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(lookahead);
    let mut rig = Rig::new();
    rig.publish(&s);
    let mut out = rig.render(3);
    assert!(rig.eng.latency > 400, "the lookahead delays the mix: {}", rig.eng.latency);
    if edit {
        s.track_mut(b).unwrap().mixer.inserts[1] = Some(builtin("plate_reverb"));
        assert_eq!(rig.publish(&s), Adoption::Adopted);
    }
    out.extend(rig.render(27));
    out
}

#[test]
fn strip_state_moves_across_a_structural_plan() {
    let reference = delayed_with_edit(false);
    assert_eq!(delayed_with_edit(true), reference, "the delay lines (and the rest of the strip state) carry over");
}

#[test]
fn delay_beyond_capacity_is_clamped_and_reported() {
    let (mut s, a, b) = audio_session();
    s.edit.delay_compensation = true;
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.latent"));
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(2);
    assert_eq!(rig.eng.status(), PlanStatus::default());
    rig.log.latency.store(20_000, Ordering::Relaxed);
    rig.render(2);
    let st = rig.eng.status();
    assert!(st.capacity_exceeded > 0);
    assert_eq!(st.required_latency, 20_000);
    assert_eq!(st.affected.map(|(t, _)| t), Some(b), "the strip that needed the longer delay");
    // The next plan prepares that much.
    rig.comp.observe(&st);
    s.track_mut(a).unwrap().mixer.volume_db = -1.0;
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.render(2);
    assert_eq!(rig.eng.status().capacity_exceeded, 0);
}

/// Track `a` plays through PDC delay lines; plan 2 lowers the session's max latency, plan 3 is an
/// unrelated insert add. Returns the left channel.
fn shrink_with_edit(edit: bool) -> Vec<f32> {
    let (mut s, _, b) = audio_session();
    s.edit.delay_compensation = true;
    let mut rig = Rig::new();
    rig.log.latency.store(10_000, Ordering::Relaxed);
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.latent"));
    rig.publish(&s);
    let mut out = rig.render(2);
    rig.log.latency.store(480, Ordering::Relaxed);
    s.track_mut(b).unwrap().mixer.inserts[0] = None;
    s.track_mut(b).unwrap().mixer.inserts[1] = Some(Insert::new("clap:test.latent"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    out.extend(rig.render(4));
    if edit {
        s.track_mut(b).unwrap().mixer.inserts[2] = Some(builtin("gain"));
        assert_eq!(rig.publish(&s), Adoption::Adopted);
    }
    out.extend(rig.render(8));
    out
}

#[test]
fn a_delay_tail_survives_a_capacity_shrink() {
    let r = shrink_with_edit(false);
    let e = shrink_with_edit(true);
    let first = r.iter().zip(&e).position(|(x, y)| x.to_bits() != y.to_bits());
    assert_eq!(first, None, "an unrelated insert add changed the output (cap shrink drops the delay lines)");
}

#[test]
fn one_large_status_does_not_wedge_later_compiles() {
    let (s, _, _) = audio_session();
    let mut rig = Rig::new();
    rig.comp = PlanCompiler::new(target(), PlanLimits { prepared_bytes: 32 << 20, ..PlanLimits::default() });
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.comp.observe(&PlanStatus { required_latency: 1 << 20, capacity_exceeded: 1, ..PlanStatus::default() });
    let mut make = factory(&rig.log);
    let mut s2 = s.clone();
    s2.tracks[0].mixer.inserts[0] = Some(builtin("gain"));
    let r = rig.comp.compile(Arc::new(s2), rig.comp.active(), PlanGeneration::from_raw(50), &mut make);
    assert!(r.is_ok(), "one transient status wedges every structural compile: {:?}", r.err());
}

#[test]
fn a_hint_sizes_every_strip_below_it_once_and_is_dropped_when_it_does_not_fit() {
    let (mut s, a, b) = audio_session();
    s.edit.delay_compensation = true;
    let mut rig = Rig::new();
    rig.publish(&s);
    let cap = |rig: &Rig, t: TrackId| {
        let p = rig.eng.plan.as_deref().unwrap();
        Some(p.tables.strips[p.tables.strip_of(t).unwrap()].tap.len)
    };
    // Status without a clamp changes nothing.
    rig.comp.observe(&PlanStatus { required_latency: 40_000, ..PlanStatus::default() });
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!((cap(&rig, a), cap(&rig, b)), (Some(MIN_DELAY_CAPACITY), Some(MIN_DELAY_CAPACITY)));
    rig.comp.observe(&PlanStatus { required_latency: 40_000, capacity_exceeded: 1, affected: Some((b, SlotGen(1))), ..PlanStatus::default() });
    s.track_mut(a).unwrap().mixer.inserts[2] = Some(builtin("gain"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(cap(&rig, b), Some(65_536), "the strip that asked");
    assert_eq!(cap(&rig, a), Some(65_536), "and every other strip below it, in the same plan");
    // Satisfied hints are gone: the next structural plan keeps those lines and asks for no more.
    s.track_mut(a).unwrap().mixer.inserts[3] = Some(builtin("gain"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!((cap(&rig, a), cap(&rig, b)), (Some(65_536), Some(65_536)));
    // A hint beyond the byte limit is dropped instead of failing every later compile.
    rig.comp = PlanCompiler::new(target(), PlanLimits { prepared_bytes: 8 << 20, ..PlanLimits::default() });
    rig.eng = MixEngine::realtime(target());
    rig.issued = 0;
    rig.publish(&s);
    rig.comp.observe(&PlanStatus { required_latency: 1 << 20, capacity_exceeded: 1, affected: Some((b, SlotGen(1))), ..PlanStatus::default() });
    s.track_mut(a).unwrap().mixer.inserts[1] = Some(builtin("gain"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(cap(&rig, b), Some(MIN_DELAY_CAPACITY));
}

#[test]
fn runtime_latency_growth_is_absorbed_by_one_plan_for_every_strip() {
    let (mut s, a, b) = audio_session();
    s.edit.delay_compensation = true;
    let c = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let d = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let _ = (a, c, d);
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.latent"));
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(2);
    rig.log.latency.store(20_000, Ordering::Relaxed);
    let mut rounds = 0;
    for i in 0..6 {
        rig.render(2);
        let st = rig.eng.status();
        if st.capacity_exceeded == 0 {
            break;
        }
        rounds += 1;
        rig.comp.observe(&st);
        s.tracks[0].mixer.volume_db = -(i as f32) - 1.0;
        assert_eq!(rig.publish(&s), Adoption::Adopted);
    }
    assert!(rounds <= 1, "needed {rounds} plans before no strip clamped");
}

/// Track `a` (a latent insert) and `b` both play the burst; `a` may be frozen on the burst itself,
/// which stands in for the render of `a` (the insert only reports latency, it does not change audio).
fn frozen_pair(freeze_latency: Option<u32>, render: SourceId) -> (Session, TrackId, TrackId) {
    let (mut s, a, b) = audio_session();
    s.edit.delay_compensation = true;
    let id = s.new_clip_id();
    s.track_mut(b).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "y", SourceId(1), 0, 0, 48_000));
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.latent"));
    if let Some(latency) = freeze_latency {
        let t = s.track_mut(a).unwrap();
        // The test insert reports latency without delaying audio, so its render starts that much
        // before the timeline for the frozen track to play where the live one does.
        let start = -i64::from(latency);
        t.freeze = Some(transit_model::Frozen { source: render, start, length: 48_000, hash: t.freeze_hash(), revision: 0, latency });
    }
    (s, a, b)
}

fn frozen_run(s: &Session) -> (Vec<f32>, usize) {
    let mut rig = Rig::new();
    rig.log.latency.store(500, Ordering::Relaxed);
    assert_eq!(rig.publish(s), Adoption::Adopted);
    let out = rig.render(8);
    (out, rig.log.made.load(Ordering::Relaxed))
}

#[test]
fn a_frozen_track_makes_no_instances_plays_its_render_and_keeps_its_latency() {
    let (live, a, _) = frozen_pair(None, SourceId(1));
    let (live_out, live_made) = frozen_run(&live);
    assert_eq!(live_made, 1, "the latent insert is made live");
    // Frozen with the live chain's latency: the same mix, sample for sample, with no instance.
    let (frozen, _, _) = frozen_pair(Some(500), SourceId(1));
    let (frozen_out, made) = frozen_run(&frozen);
    assert_eq!(made, 0, "a frozen track makes no instances");
    // The host stages instances from the same list, so it makes none either.
    assert!(realtime_instance_specs(&frozen).is_empty() && !realtime_instance_specs(&live).is_empty());
    assert_eq!(frozen_out, live_out, "delay compensation is what it was live");
    assert!(frozen_out.iter().any(|x| x.abs() > 0.1));
    // Without the latency the other track would no longer be held back to meet it.
    let (unaligned, _, _) = frozen_pair(Some(0), SourceId(1));
    assert_ne!(frozen_run(&unaligned).0, live_out);
    // The insert's settings stay in the session.
    assert!(frozen.track(a).unwrap().mixer.inserts[0].is_some());
    // A freeze whose render is missing plays live.
    let (missing, _, _) = frozen_pair(Some(500), SourceId(99));
    let (missing_out, made) = frozen_run(&missing);
    assert_eq!((made, missing_out), (1, live_out));
}

#[test]
fn the_session_as_played_replaces_a_frozen_track_by_its_render() {
    let (s, a, b) = frozen_pair(Some(500), SourceId(1));
    let s = Arc::new(s);
    let played = s.with_frozen_audio();
    let t = played.track(a).unwrap();
    assert!(t.mixer.inserts.iter().all(Option::is_none) && t.instrument.is_none());
    assert_eq!(t.clips().len(), 1);
    assert_eq!(played.track(b), s.track(b), "other tracks are untouched");
    let none = Arc::new(audio_session().0);
    assert!(Arc::ptr_eq(&none, &none.with_frozen_audio()), "nothing frozen: the same session");
}
