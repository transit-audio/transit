#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan::Attach;
use super::plan_tests::*;
use super::*;
use std::sync::atomic::Ordering;

/// Burst into a reverb and a compressor on track `a`; renders 40 blocks, applying `edit` (as a
/// published plan) after 6.
fn tail_with_edit(edit: Option<&dyn Fn(&mut Session)>) -> Vec<f32> {
    let (mut s, a, _) = audio_session();
    let ins = &mut s.track_mut(a).unwrap().mixer.inserts;
    ins[0] = Some(builtin("compressor"));
    ins[1] = Some(builtin("room_reverb"));
    let mut rig = Rig::new();
    rig.publish(&s);
    let mut out = rig.render(6);
    if let Some(edit) = edit {
        edit(&mut s);
        assert_eq!(rig.publish(&s), Adoption::Adopted);
    }
    out.extend(rig.render(34));
    out
}

#[test]
fn a_reused_slot_keeps_its_state_across_plans() {
    let reference = tail_with_edit(None);
    assert!(reference[20 * BLOCK..].iter().any(|x| x.abs() > 1e-4), "the reverb tail rings past the burst");
    // A volume-only plan on the other track, and a structural plan that adds an insert there.
    let volume = tail_with_edit(Some(&|s: &mut Session| s.tracks[1].mixer.volume_db = -3.0));
    let insert = tail_with_edit(Some(&|s: &mut Session| s.tracks[1].mixer.inserts[0] = Some(builtin("plate_reverb"))));
    assert_eq!(volume, reference, "volume-only plan");
    assert_eq!(insert, reference, "unrelated insert-add plan");
}

#[test]
fn applied_storage_grows_through_an_attachment() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(1);
    let ins = s.track_mut(a).unwrap().mixer.inserts[0].as_mut().unwrap();
    for i in 0..70 {
        ins.params.insert(format!("extra{i:02}"), 0.5);
    }
    let n = ins.params.len();
    let env = rig.compile(&s);
    assert!(env.plan.attach.iter().any(|a| matches!(a, Attach::Applied { storage, .. } if storage.len() == n)));
    let mut env = env;
    assert_eq!(adopt_measured(&mut rig.eng, &mut env), Adoption::Adopted);
    rig.comp.reclaim(env);
    rig.render(2);
    assert_eq!(rig.eng.status().applied_mismatch, 0);
}

#[test]
fn reset_zeroes_the_plan_meters() {
    let (s, _, _) = audio_session();
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(2);
    assert!(rig.eng.meters().any(|(_, m)| m.peaks.iter().any(|x| *x > 0.0)));
    rig.eng.reset();
    assert!(rig.eng.meters().all(|(_, m)| m.peaks.iter().all(|x| *x == 0.0) && m.peak == [0.0; 2]));
    assert!(rig.eng.active_main_meter().peaks.iter().all(|x| *x == 0.0));
}

/// Track `a` with a gain (+12 dB), a saturator and a compressor in slots 0..3.
fn chain_session() -> (Session, TrackId) {
    let (mut s, a, _) = audio_session();
    let mut gain = builtin("gain");
    gain.params.insert("gain".into(), 12.0);
    let ins = &mut s.track_mut(a).unwrap().mixer.inserts;
    ins[0] = Some(gain);
    ins[1] = Some(builtin("saturator"));
    ins[2] = Some(builtin("compressor"));
    (s, a)
}

#[test]
fn a_moved_insert_keeps_its_instance_and_the_order_changes_the_sound() {
    let (s, a) = chain_session();
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let before = rig.comp.active_bindings();
    assert_eq!(before.len(), 3);
    let gen_at = |b: &[SlotBinding], slot| b.iter().find(|x| x.0 == a && x.1 == slot).map(|x| x.2);
    let made = rig.log.made.load(Ordering::Relaxed);
    // Gain to the end: [saturator, compressor, gain].
    let mut s2 = s.clone();
    assert!(s2.track_mut(a).unwrap().move_insert(0, 2));
    let mut env = rig.compile(&s2);
    assert_eq!(slot_moves(&env).len(), 3, "all three instances move");
    assert_eq!(adopt_measured(&mut rig.eng, &mut env), Adoption::Adopted);
    assert_eq!(holdings(&env).1, 0, "nothing was replaced");
    rig.comp.reclaim(env);
    let after = rig.comp.active_bindings();
    assert_eq!(gen_at(&after, 2), gen_at(&before, 0), "the gain's instance is now in slot 2");
    assert_eq!(gen_at(&after, 0), gen_at(&before, 1));
    assert_eq!(gen_at(&after, 1), gen_at(&before, 2));
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made, "no instance was made");
    assert_eq!(rig.drops(), 0, "none was dropped");
    // Undo (the original session again) puts every instance back.
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(rig.comp.active_bindings(), before);
    assert_eq!(rig.drops(), 0);
    // A move before any audio rendered sounds like a fresh plan of the new order; the order matters.
    let mut moved = Rig::new();
    assert_eq!(moved.publish(&s), Adoption::Adopted);
    assert_eq!(moved.publish(&s2), Adoption::Adopted);
    let moved_out = moved.render(4);
    let mut fresh = Rig::new();
    assert_eq!(fresh.publish(&s2), Adoption::Adopted);
    assert_eq!(moved_out, fresh.render(4), "the moved chain renders as a fresh chain in the new order");
    let mut orig = Rig::new();
    assert_eq!(orig.publish(&s), Adoption::Adopted);
    assert_ne!(moved_out, orig.render(4), "gain then saturator is not saturator then gain");
}

/// How many plug-ins a compile of `s` asks the factory for.
fn instances_made(mut comp: PlanCompiler, s: &Session) -> usize {
    let made = std::cell::Cell::new(0usize);
    let mut make = |spec: &InstanceSpec, _: SlotGen| {
        made.set(made.get() + 1);
        create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch)
    };
    let next = comp.active().checked_next().unwrap();
    let env = comp.compile(Arc::new(s.clone()), comp.active(), next, &mut make).unwrap();
    comp.reclaim(env);
    made.get()
}

#[test]
fn an_inactive_tracks_plugins_are_made_by_a_live_compile_but_not_an_offline_one() {
    let (mut s, a, b) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(builtin("maximizer"));
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(builtin("maximizer"));
    s.track_mut(b).unwrap().mixer.inserts[1] = Some(builtin("maximizer"));
    let live = || PlanCompiler::new(target(), PlanLimits::default());
    assert_eq!(instances_made(live(), &s), 3);
    assert_eq!(instances_made(live().skipping_inactive_instances(), &s), 3, "active tracks keep every instance");
    s.track_mut(b).unwrap().inactive = true;
    assert_eq!(instances_made(live(), &s), 3, "the live engine keeps an inactive track warm");
    assert_eq!(instances_made(live().skipping_inactive_instances(), &s), 1, "an offline compile skips it");
    // A muted track can be unmuted by automation: it stays.
    s.track_mut(b).unwrap().inactive = false;
    s.track_mut(b).unwrap().mixer.mute = true;
    assert_eq!(instances_made(live().skipping_inactive_instances(), &s), 3);
}

#[test]
fn clip_fx_with_short_prepared_buffers_renders_every_sample_without_allocating() {
    mark_audio_thread();
    for modules in [false, true] {
        for short_channel in [0, 1] {
            let (mut s, a, _) = audio_session();
            let clip = s.track(a).unwrap().clips()[0].id.0;
            s.edit.values.insert(format!("clip_fx.{clip}.gain"), -6.0);
            if modules {
                s.edit.values.insert(format!("clip_fx.{clip}.eq.mid_gain"), 3.0);
                s.edit.values.insert(format!("clip_fx.{clip}.comp.threshold"), -12.0);
            }
            let mut reference = Rig::new();
            let mut short = Rig::new();
            assert_eq!(reference.publish(&s), Adoption::Adopted);
            assert_eq!(short.publish(&s), Adoption::Adopted);
            let tables = &mut short.eng.plan.as_deref_mut().unwrap().tables;
            let at = tables.strip_of(a).unwrap();
            tables.strips[at].clipbuf[short_channel] = vec![0.0; 31];
            let mut expected = vec![vec![0.0; BLOCK]; 2];
            for block in 0..4 {
                let pos = (block * BLOCK) as i64;
                // The EQ updates smoothed coefficients per process chunk. Match the short
                // buffer's chunk boundaries in the independently rendered reference.
                for done in (0..BLOCK).step_by(31) {
                    let n = (BLOCK - done).min(31);
                    reference.eng.render_active(pos + done as i64, n, &mut reference.out);
                    for (dst, src) in expected.iter_mut().zip(&reference.out) {
                        dst[done..done + n].copy_from_slice(&src[..n]);
                    }
                }
                let (_, stats) = transit_rt::testing::measure(|| short.eng.render_active(pos, BLOCK, &mut short.out));
                assert!(stats.is_zero(), "short clip-FX buffer allocated or freed: {stats:?}");
                assert!(expected.iter().flatten().any(|v| v.abs() > 0.01));
                for (actual, expected) in short.out.iter().flatten().zip(expected.iter().flatten()) {
                    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
                }
            }
        }
    }
}

#[test]
fn render_flags_prefer_transit_values_and_accept_legacy_aliases() {
    for (value, alias, expected) in [
        (None, None, false),
        (None, Some("1"), true),
        (None, Some(" YES "), true),
        (Some("true"), Some("0"), true),
        (Some("0"), Some("1"), false),
        (Some(""), Some("yes"), false),
    ] {
        assert_eq!(flag_value(value, alias), expected);
    }
    assert_eq!(RENDER_POOL_ENV, "TRANSIT_RENDER_POOL");
    assert_eq!(RENDER_NO_PRIORITY_ENV, "TRANSIT_RENDER_NO_PRIORITY");
}

#[test]
fn changed_sink_edges_refresh_cached_indices_even_when_sink_order_stays_the_same() {
    use transit_model::{IoDirection, IoPath, SendSlot};
    mark_audio_thread();
    let (mut s, a, b) = audio_session();
    let io = IoMap {
        outputs: 4,
        paths: vec![
            IoPath { name: "A".into(), direction: IoDirection::Output, channels: vec![0, 1], format: ChannelFormat::Stereo },
            IoPath { name: "B".into(), direction: IoDirection::Output, channels: vec![2, 3], format: ChannelFormat::Stereo },
        ],
        ..IoMap::default()
    };
    // The silent first track pins sink order while the audible second track changes routes.
    s.tracks.swap(0, 1);
    let silent = s.track_mut(b).unwrap();
    silent.mixer.output = Route::Hardware("A".into());
    silent.mixer.sends[0] = Some(SendSlot {
        target: Route::Hardware("B".into()),
        level_db: 0.0,
        pan: 0.0,
        mute: false,
        pre_fader: false,
        follow_main_pan: true,
        surround: None,
    });
    s.track_mut(a).unwrap().mixer.output = Route::Hardware("a".into());
    let mut rig = Rig::new();
    rig.comp.set_io(Arc::new(io));
    rig.out = vec![vec![0.0; BLOCK]; 6];
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.eng.render_active(0, BLOCK, &mut rig.out);
    assert!(rig.out[2].iter().any(|x| x.abs() > 0.1));
    assert!(rig.out[4].iter().all(|x| *x == 0.0));
    for (route, expected_sink) in [("b", Some(4)), ("missing", None), ("A", Some(2))] {
        s.track_mut(a).unwrap().mixer.output = Route::Hardware(route.into());
        let mut env = rig.compile(&s);
        assert!(env.plan.tables.is_some(), "a different edge needs a fresh sink index");
        assert_eq!(adopt_measured(&mut rig.eng, &mut env), Adoption::Adopted);
        rig.comp.reclaim(env);
        let (_, stats) = transit_rt::testing::measure(|| rig.eng.render_active(0, BLOCK, &mut rig.out));
        assert!(stats.is_zero(), "cached sink render: {stats:?}");
        for channel in [2, 4] {
            let audible = rig.out[channel].iter().any(|x| x.abs() > 0.1);
            assert_eq!(audible, expected_sink == Some(channel));
        }
    }
}

#[test]
fn the_render_writes_its_phases_to_the_marker_without_allocating() {
    use transit_rt::phase_marker::{Phase, PhaseMarker};
    let (s, _, _) = audio_session();
    let mut rig = Rig::new();
    let marker = std::sync::Arc::new(PhaseMarker::new());
    rig.eng.set_phase_marker(std::sync::Arc::clone(&marker));
    rig.publish(&s);
    rig.render(2);
    marker.begin();
    let (_, stats) = transit_rt::testing::measure(|| rig.eng.render_active(0, BLOCK, &mut rig.out));
    assert!(stats.is_zero(), "{stats:?}");
    let seen = marker.read();
    assert_eq!(seen.phase, Phase::Output, "the block ends in the output copy");
    assert!(seen.at_ns > 0 && seen.gap_phase != Phase::Idle, "{seen:?}");
}
