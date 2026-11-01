//! Tail audio/state equivalence through actual plan compilers and worker hosts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[allow(dead_code)]
mod common;
#[allow(dead_code)]
#[path = "common/sched.rs"]
mod sched;
use sched::Topology;
use std::time::Duration;
use transit_mix::MixEngine;
use transit_model::{Bus, BusId, ChannelFormat, Insert, Route, Session, TrackKind};

/// Warm steady state, including direct callback renders and deadline-inline aux jobs.
/// timing: far-deadline all outcomes are controlled with zero workers and injected costs.
#[test]
fn tail_callback_allocates_and_frees_nothing_for_1200_blocks() {
    let _switch = sched::graph_on();
    MixEngine::force_tail(true);
    let top = Topology::new("allocation", returns_and_groups(), vec![]);
    let mut rig = top.rig();
    rig.mix.start_render_workers_with(64, 0);
    rig.mix.set_strip_deadline(Some(Duration::from_secs(60)));
    rig.mix.seed_strip_costs(Duration::from_nanos(1));
    common::workers::become_callback();
    rig.block();
    let (p1, end) = rig.mix.strip_phases();
    assert!(end > p1);
    let (_, stats) = transit_rt::testing::measure(|| {
        for k in 0..1200 {
            for slot in p1..end {
                rig.mix.arm_tail_slot(
                    slot,
                    transit_rt::worker_set::seams::SlotOverrides {
                        budget_left: Some(Duration::ZERO),
                        force_publish: k % 2 == 0,
                        ..Default::default()
                    },
                );
            }
            rig.block();
        }
    });
    transit_mix::unmark_audio_thread();
    assert_eq!(stats.allocs, 0, "{stats:?}");
    assert_eq!(stats.frees, 0, "{stats:?}");
    MixEngine::force_tail(false);
    drop(rig.mix.into_retired());
}

pub fn returns_and_groups() -> Session {
    let (mut s, _) = common::workers::session(0, 4);
    s.edit.click = false;
    s.edit.delay_compensation = true;
    let mut bus = |name: &str| {
        let id = BusId(s.alloc());
        s.busses.push(Bus { id, name: name.into(), format: ChannelFormat::Stereo });
        id
    };
    let a = bus("Return A input");
    let b = bus("Return B input");
    let music = bus("Music");
    for (k, t) in s.tracks.iter_mut().filter(|t| t.kind == TrackKind::Audio).enumerate() {
        t.mixer.output = Route::Bus(if k % 2 == 0 { a } else { b });
    }
    for (name, input, output) in [
        ("Return A", a, Route::Bus(music)),
        ("Return B", b, Route::Bus(music)),
        ("Music group", music, Route::Main),
        ("Music second reader", music, Route::Main),
    ] {
        let id = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some(name));
        let t = s.track_mut(id).unwrap();
        t.mixer.input = Route::Bus(input);
        t.mixer.output = output;
        t.mixer.inserts[0] = Some(Insert::new("gain"));
    }
    s
}

/// T1: bit-exact samples and DSP fingerprints, with a pool-path vacuity check.
/// timing: far-deadline the helper's explicit 60-second deadline waits for every render.
#[test]
fn t1_tail_topologies_and_corpus_are_bit_exact() {
    let _switch = sched::graph_on();
    MixEngine::force_tail(true);
    let mut sessions = sched::corpus();
    sessions.push(("returns and groups".into(), returns_and_groups()));
    for (name, s) in sessions {
        let top = Topology::new(&name, s, vec![]);
        for workers in [0, 2] {
            let mut graph = top.rig();
            let mut serial = top.rig();
            graph.mix.start_render_workers_with(64, workers);
            graph.mix.set_strip_deadline(Some(Duration::from_secs(60)));
            graph.mix.seed_strip_costs(Duration::from_nanos(1));
            common::workers::become_callback();
            for _ in 0..24 {
                graph.block();
                serial.block();
                assert_eq!(graph.out, serial.out, "{name}, workers {workers}");
                assert_eq!(graph.mix.strip_prints(), serial.mix.strip_prints(), "state: {name}, workers {workers}");
            }
            transit_mix::unmark_audio_thread();
            let faults = graph.mix.strip_faults().unwrap().snapshot();
            assert!(faults.blocks > 0, "{name}: graph reached the pool");
            let (p1, end) = graph.mix.strip_phases();
            if end > p1 {
                assert_eq!(graph.mix.strip_counts().len(), end, "aux tasks reached the tail host");
            }
            assert_eq!(faults.conflicts, 0);
            drop(graph.mix.into_retired());
            drop(serial.mix.into_retired());
        }
    }
    MixEngine::force_tail(false);
}
