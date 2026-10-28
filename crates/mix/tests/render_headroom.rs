//! The #124 measurement (ADR 0012 gate G1, ADR 0029 gate 6): serial versus the render workers,
//! through the live render path, at 64 and 128 frames, idle and under load. Ignored by default;
//! a measurement, not a gate:
//!
//! ```text
//! cargo test --release -p transit-mix --test render_headroom -- --ignored --nocapture
//! TRANSIT_RENDER_NO_PRIORITY=1 cargo test --release -p transit-mix --test render_headroom -- --ignored --nocapture
//! TRANSIT_BENCH_LOAD=external cargo test --release ...   # while something else loads the machine
//! TRANSIT_STRIP_DEADLINE_PERCENT=150 cargo test --release ...   # the strips' deadline at 150% (#172)
//! ```
//!
//! The fixture stands in for the reference project until one is declared (ADR 0012 G13): 16
//! audio strips (a compressor or a reverb each) and 4 continuity instruments (instrument strips
//! with a cost per sample, `TRANSIT_BENCH_COST`, played live through `MixEngine::queue_event`
//! with notes and the sustain pedal). The callback thread is promoted as a driver's would be,
//! and so are the workers (unless `TRANSIT_RENDER_NO_PRIORITY=1`). Each configuration renders
//! 2000 blocks, one per period, as a device calls, after 64 blocks of warm-up.
//!
//! Load: `idle`; `spin`, one busy normal-priority thread per logical core (generated here); with
//! `TRANSIT_BENCH_LOAD=external` the rows are taken under whatever load the caller runs
//! alongside (for example a `cargo check --workspace`).
//!
//! The strips' deadline is the engine's: by default the share of the period for the row's buffer
//! size (#172: 150% at 64 frames and below, 100% above; the bench hands the engine its block
//! length as a device callback would), or `TRANSIT_STRIP_DEADLINE_PERCENT` of the period (25 to
//! 300) at every size, read once per process, so one run measures one override; each worker row
//! prints the share its engine used (`-` for serial, which has no deadline). Unset the variable
//! before an ordinary `cargo test`: every pool test in the process would run at that share.
//!
//! Per configuration: late blocks per 2000 (render time over the period), the render time's p50,
//! p95, p99 and max, strip overruns by cause, inline renders, and the continuity instruments'
//! holes (blocks of instrument time never processed) and stuck notes and pedals. A last row
//! forces an overrun (a worker stalls 10 ms inside a strip that holds a sustained note) and checks
//! the note resumes and is released. A second table, for the worker rows (#169): the lateness of
//! late results and the workers' wake latency (publish to claim) by bucket, and the three strips
//! with the most overruns (track index, then busy/late/unstarted).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use common::workers::{Player, Rig, become_callback};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use transit_mix::{MixEngine, StripFaults};

const BLOCKS: usize = 2000;
const WARMUP: u64 = 64;
const INSTRUMENTS: usize = 4;
const AUDIO: usize = 16;

/// One normal-priority thread per logical core, spinning until dropped.
struct Load(Arc<AtomicBool>, Vec<std::thread::JoinHandle<()>>);

impl Load {
    fn new() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let handles = (0..n)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut x = 1u64;
                    while !stop.load(Ordering::Relaxed) {
                        x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
                    }
                })
            })
            .collect();
        Load(stop, handles)
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
        for h in self.1.drain(..) {
            let _ = h.join();
        }
    }
}

struct Row {
    frames: usize,
    mode: &'static str,
    load: &'static str,
    late: usize,
    p50: Duration,
    p95: Duration,
    p99: Duration,
    max: Duration,
    faults: StripFaults,
    holes: u64,
    stuck: u64,
}

/// The strips' deadline share a row's engine used (the effective one: the rule for the row's
/// buffer size, or the override), percent of the period; `-` without workers.
fn share(f: &StripFaults) -> String {
    if f.workers == 0 { "-".into() } else { f.deadline_percent.to_string() }
}

fn cost() -> u64 {
    std::env::var("TRANSIT_BENCH_COST").ok().and_then(|v| v.parse().ok()).unwrap_or(6)
}

/// Renders the fixture for `BLOCKS` paced blocks; with `stall`, one worker stall of 10 ms in the
/// middle of a sustained note on instrument 0.
fn run(frames: usize, pool: bool, promote: bool, stall: bool) -> (Vec<Duration>, StripFaults, u64, u64) {
    MixEngine::force_serial(!pool);
    let mut rig = Rig::new(INSTRUMENTS, AUDIO, frames);
    if pool {
        assert!(rig.mix.start_render_workers(64) > 0, "no render workers on this machine");
        // The device's callback length, as playback hands it (#172): the share follows it.
        rig.mix.set_device_frames(u32::try_from(frames).unwrap());
    }
    for s in &rig.shared {
        s.cost.store(cost(), Ordering::Relaxed);
    }
    let (times, rig) = std::thread::spawn(move || {
        let mut rig = rig;
        let p = if promote { transit_rt::promote_audio_thread() } else { transit_rt::Promotion::Unsupported("disabled") };
        become_callback();
        let mut player = Player::new(INSTRUMENTS, 2026);
        let period = Duration::from_nanos(frames as u64 * 1_000_000_000 / u64::from(common::workers::SR));
        for b in 0..WARMUP {
            player.play(&mut rig, b);
            rig.block();
        }
        let mut times = Vec::with_capacity(BLOCKS);
        // Paced like a device callback: one block per period, idle in between (a back-to-back
        // loop would be a 100% duty cycle, which MMCSS throttles and no driver produces).
        let first = Instant::now();
        for k in 0..BLOCKS {
            if let Some(wait) = (first + period * u32::try_from(k).unwrap()).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let b = WARMUP + k as u64;
            if stall && k == BLOCKS / 2 {
                // A sustained note on instrument 0 through a forced overrun.
                assert!(rig.note_on(0, 0, 100).accepted());
                rig.shared[0].stall_ns.store(10_000_000, Ordering::Relaxed);
            }
            if stall && k == BLOCKS / 2 + 200 {
                assert!(rig.note_off(0, 0, 100).accepted());
            }
            player.play(&mut rig, b);
            let t = Instant::now();
            rig.block();
            times.push(t.elapsed());
        }
        player.release_all(&mut rig);
        let start = Instant::now();
        while !rig.mix.render_workers_settled() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(period);
            rig.block();
        }
        for _ in 0..8 {
            rig.block();
        }
        transit_rt::demote(p);
        transit_mix::unmark_audio_thread();
        (times, rig)
    })
    .join()
    .unwrap();
    MixEngine::force_serial(true);
    let faults = rig.mix.strip_faults().map(|r| r.snapshot()).unwrap_or_default();
    let reports: Vec<_> = rig.shared.iter().map(|s| s.report()).collect();
    let holes = reports.iter().map(|r| r.hole_samples).sum::<u64>() / frames as u64;
    let stuck = reports.iter().map(|r| r.stuck_notes + r.stuck_pedals).sum();
    drop(rig.mix.into_retired());
    (times, faults, holes, stuck)
}

fn row(frames: usize, mode: &'static str, load: &'static str, promote: bool) -> Row {
    let (pool, stall) = match mode {
        "serial" => (false, false),
        "workers" => (true, false),
        _ => (true, true),
    };
    let (mut times, faults, holes, stuck) = run(frames, pool, promote, stall);
    let period = Duration::from_nanos(frames as u64 * 1_000_000_000 / u64::from(common::workers::SR));
    let late = times.iter().filter(|t| **t > period).count();
    times.sort();
    let at = |q: usize| times[(times.len() * q / 1000).min(times.len() - 1)];
    Row { frames, mode, load, late, p50: at(500), p95: at(950), p99: at(990), max: times[times.len() - 1], faults, holes, stuck }
}

#[test]
#[ignore = "measurement: run in release with --ignored --nocapture"]
fn serial_versus_render_workers() {
    let promote = !std::env::var(transit_mix::RENDER_NO_PRIORITY_ENV).is_ok_and(|v| v.trim() == "1");
    let external = std::env::var("TRANSIT_BENCH_LOAD").is_ok_and(|v| v.trim() == "external");
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    // Warm the plug-ins' and the allocator's first-use paths outside the measurement.
    let _ = run(128, false, promote, false);
    println!(
        "cores {cores}; workers {}; priority {}; instrument cost {} sin/sample; {AUDIO} audio strips + {INSTRUMENTS} instruments; {BLOCKS} blocks per row",
        cores.saturating_sub(1).min(transit_mix::MAX_RENDER_WORKERS),
        if promote { "promoted" } else { "default" },
        cost()
    );
    println!(
        "| frames | path | load | deadline % | late /2000 | p50 µs | p95 µs | p99 µs | max µs | overruns busy/late/unstarted | inline | holes (blocks) | stuck |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    let loads: &[&'static str] = if external { &["external"] } else { &["idle", "spin"] };
    let mut rows = Vec::new();
    for &frames in &[64usize, 128] {
        for &load in loads {
            for mode in ["serial", "workers"] {
                let spin = (load == "spin").then(Load::new);
                rows.push(row(frames, mode, load, promote));
                drop(spin);
            }
        }
    }
    rows.push(row(128, "workers+stall", if external { "external" } else { "idle" }, promote));
    for r in &rows {
        let f = r.faults;
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {}/{}/{} | {} | {} | {} |",
            r.frames,
            r.mode,
            r.load,
            share(&f),
            r.late,
            r.p50.as_micros(),
            r.p95.as_micros(),
            r.p99.as_micros(),
            r.max.as_micros(),
            f.overruns_busy,
            f.overruns_late,
            f.overruns_unstarted,
            f.inline_renders,
            r.holes,
            r.stuck
        );
    }
    let buckets = |h: &[u64]| h.iter().map(u64::to_string).collect::<Vec<_>>().join("/");
    println!();
    println!(
        "| frames | path | load | deadline % | lateness \u{2264}50 \u{b5}s/100/200/500/1 ms/2 ms/more | wake \u{2264}50 \u{b5}s/100/200/500/1 ms/2 ms/more | top strips: track busy/late/unstarted |"
    );
    println!("|---|---|---|---|---|---|---|");
    for r in rows.iter().filter(|r| r.mode != "serial") {
        let f = r.faults;
        let top: Vec<String> = f
            .top_strips
            .iter()
            .flatten()
            .take(3)
            .map(|s| format!("t{} {}/{}/{}", s.track, s.overruns_busy, s.overruns_late, s.overruns_unstarted))
            .collect();
        println!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.frames,
            r.mode,
            r.load,
            share(&f),
            buckets(&f.lateness),
            buckets(&f.wake),
            if top.is_empty() { "none".into() } else { top.join(", ") }
        );
    }
    for r in &rows {
        assert_eq!(r.stuck, 0, "{} {} {}: a note or pedal stuck", r.frames, r.mode, r.load);
        assert_eq!(r.holes, r.faults.overruns_busy + r.faults.overruns_unstarted, "{} {} {}: a hole without an overrun", r.frames, r.mode, r.load);
    }
}
