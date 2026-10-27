//! Backlog 12: creating a heavy plug-in on the control thread must not touch the audio side. The
//! fixture's synth takes 500 ms to create; meanwhile another instance of the same binary runs
//! blocks on a thread standing in for the audio callback, which must keep its budget. This is
//! the host half of the claim: no lock the creation path holds (the registry, the loaded-module
//! list, the module thread) is taken by `process`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

mod common;
use common::slow_fixture_dir;

const FRAMES: usize = 512;

fn id_of(name: &str) -> String {
    transit_vst3_host::scan_paths(&[slow_fixture_dir().to_path_buf()]).into_iter().find(|d| d.name == name).unwrap().id
}

fn run_while_another_is_created() -> (u32, Duration, Duration, Duration) {
    let (gain_id, synth_id) = (id_of("Fixture Gain"), id_of("Fixture Synth"));
    let mut gain = transit_vst3_host::create(&gain_id).unwrap();
    gain.prepare(48_000.0, FRAMES, 2);

    let stop = Arc::new(AtomicBool::new(false));
    let audio = {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut io = vec![vec![0.5f32; FRAMES]; 2];
            let (mut blocks, mut worst, mut max_gap) = (0u32, Duration::ZERO, Duration::ZERO);
            let mut last = Instant::now();
            while !stop.load(Ordering::Acquire) {
                let t = Instant::now();
                max_gap = max_gap.max(t.duration_since(last));
                last = t;
                gain.process(&mut io, FRAMES);
                worst = worst.max(t.elapsed());
                blocks += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
            (blocks, worst, max_gap)
        })
    };
    std::thread::sleep(Duration::from_millis(20));
    let started = Instant::now();
    let synth = transit_vst3_host::create(&synth_id).unwrap();
    let took = started.elapsed();
    stop.store(true, Ordering::Release);
    let (blocks, worst, max_gap) = audio.join().unwrap();
    drop(synth);
    (blocks, worst, max_gap, took)
}

/// Progress, not timing: the audio side kept completing blocks during the creation, so no lock
/// the creation holds is taken by `process`. Not a throughput floor (#87, E49): a loaded machine
/// can starve the loop to a few blocks, so the bounds are at least 2 blocks (a blocked loop
/// completes one) and a 250 ms stall bound on the longest gap, against the 500 ms creation.
#[test]
fn a_running_instance_keeps_rendering_while_another_is_created() {
    let (blocks, _, max_gap, took) = run_while_another_is_created();
    assert!(took >= Duration::from_millis(500), "the fixture's synth is slow to create: {took:?}");
    assert!(blocks >= 2, "the audio side ran only {blocks} blocks during the creation");
    assert!(max_gap < Duration::from_millis(250), "the audio side stalled for {max_gap:?} during the creation");
}

/// Wall-clock companion for quiet hardware (hardware-evidence convention): run with
/// `cargo test -p transit-vst3-host -- --ignored hardware_timing`.
#[test]
#[ignore = "wall-clock timing; run on an idle machine"]
fn hardware_timing_a_running_instance_keeps_its_budget_while_another_is_created() {
    let (blocks, worst, _, took) = run_while_another_is_created();
    assert!(took >= Duration::from_millis(500), "the fixture's synth is slow to create: {took:?}");
    assert!(blocks >= 20, "the audio side ran {blocks} blocks during the creation");
    // 512 frames at 48 kHz: a 10.7 ms budget.
    assert!(worst < Duration::from_millis(10), "worst block {worst:?} while the other instance was created");
}
