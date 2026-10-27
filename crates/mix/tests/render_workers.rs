//! The multicore strip render through a real `transit-rt` worker set (issue #124 slice 4, ADR
//! 0029): continuity with injected worker stalls and live MIDI through the deferred-MIDI ring,
//! the reentry guard, zero allocations on the callback, adoption while a worker holds a strip,
//! and retirement with quarantine.
//!
//! The binary gets `common`'s `transit_rt::testing::CountingAlloc` (`measure` counts this thread). `MixEngine::force_serial` is
//! process-wide, so the tests take turns.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use common::workers::{Player, Rig, Rng, become_callback};
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use transit_mix::{Adoption, EventOutcome, InstanceSpec, MixEngine, NewInstance, RejectReason, SlotGen};
use transit_rt::testing::measure;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const FRAMES: usize = 128;
const INSTRUMENTS: usize = 6;

fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// A rig with render workers on, its strips' deadline at `deadline`.
fn rig(deadline: Duration) -> Option<Rig> {
    let mut rig = Rig::new(INSTRUMENTS, 0, FRAMES);
    if rig.mix.start_render_workers(64) == 0 {
        assert!(cores() < 2, "workers are expected on a machine with {} cores", cores());
        return None;
    }
    rig.mix.set_strip_deadline(Some(deadline));
    Some(rig)
}

/// Renders until every strip is home and has processed a few more blocks, paced so a stalled
/// worker can finish.
fn settle(rig: &mut Rig) {
    let start = Instant::now();
    while !rig.mix.render_workers_settled() {
        assert!(start.elapsed() < Duration::from_secs(10), "a worker never returned");
        std::thread::sleep(Duration::from_millis(1));
        rig.block();
    }
    for _ in 0..8 {
        rig.block();
    }
}

/// #124 gate 4 and gate 6 (continuity): many blocks of live notes and sustain pedal on six
/// instruments rendered by the worker set, with worker stalls injected (each makes its strip
/// `Late`, then `Busy` until the worker is back, so its live MIDI goes through the ring). No note
/// or pedal is left stuck, every hole in an instrument's time is a counted `Busy` or `Unstarted`
/// overrun, no instance is entered twice at once, and no event arrives out of order.
#[test]
fn continuity_through_a_real_worker_set_with_stalls_and_live_midi() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(rig) = rig(Duration::from_micros(1500)) else {
        MixEngine::force_serial(true);
        return;
    };
    let seeds: u64 = std::env::var("TRANSIT_WORKER_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(124);
    let rig = std::thread::spawn(move || {
        let mut rig = rig;
        become_callback();
        let mut player = Player::new(INSTRUMENTS, seeds);
        let mut rng = Rng::new(seeds ^ 0x5eed);
        let period = Duration::from_nanos(FRAMES as u64 * 1_000_000_000 / u64::from(common::workers::SR));
        let first = Instant::now();
        for b in 0..3000u64 {
            // Paced like a device: one block per period.
            if let Some(wait) = (first + period * u32::try_from(b).unwrap()).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            player.play(&mut rig, b);
            if rng.below(30) == 0 {
                let k = usize::try_from(rng.below(INSTRUMENTS as u64)).unwrap();
                rig.shared[k].stall_ns.store(2_000_000 + rng.below(8_000_000), Ordering::Relaxed);
            }
            rig.block();
        }
        for s in &rig.shared {
            s.stall_ns.store(0, Ordering::Relaxed);
        }
        player.release_all(&mut rig);
        settle(&mut rig);
        transit_mix::unmark_audio_thread();
        rig
    })
    .join()
    .unwrap();
    MixEngine::force_serial(true);
    let f = rig.mix.strip_faults().unwrap().snapshot();
    println!("{f:?}");
    let reports: Vec<_> = rig.shared.iter().map(|s| s.report()).collect();
    let hole_samples: u64 = reports.iter().map(|r| r.hole_samples).sum();
    let holes: u64 = reports.iter().map(|r| r.holes).sum();
    let on_worker: u64 = rig.shared.iter().map(|s| s.on_worker.load(Ordering::Relaxed)).sum();
    let zero_length: u64 = reports.iter().map(|r| r.faults.zero_length).sum();
    println!("holes {holes} ({hole_samples} samples); renders on a worker {on_worker}; zero-length notes {zero_length}");
    for (k, (r, s)) in reports.iter().zip(&rig.shared).enumerate() {
        assert_eq!((r.stuck_notes, r.stuck_pedals), (0, 0), "instrument {k}: {r:?}");
        let x = r.faults;
        assert_eq!((x.orphan_offs, x.overlaps, x.unsorted, x.out_of_block, x.restrikes), (0, 0, 0, 0, 0), "instrument {k}: {r:?}");
        assert_eq!(x.zero_length, 0, "instrument {k}: a carried note-off landed on its note-on's sample");
        assert_eq!(x.unrecorded, 0, "instrument {k}: the trace held everything");
        assert!(r.events > 0, "instrument {k} was played");
        assert_eq!(s.reentries.load(Ordering::Relaxed), 0, "instrument {k} was entered by two threads at once");
    }
    assert_eq!(f.conflicts, 0, "no refused cell borrow, no result of another block, no refused publisher call");
    assert!(f.overruns_late > 0 && f.overruns_busy > 0, "the stalls overran: {f:?}");
    assert!(f.midi_deferred > 0, "live MIDI went through the ring while a worker held a strip: {f:?}");
    assert!(on_worker > 0, "the workers rendered");
    assert_eq!(rig.marked, f.overrun_blocks, "every block with an overrun is reported for the take journal, and only those");
    assert_eq!(
        hole_samples,
        (f.overruns_busy + f.overruns_unstarted) * FRAMES as u64,
        "every hole in an instrument's time is a counted busy or unstarted overrun, and every such overrun a hole: {f:?}"
    );
    let retired = rig.mix.into_retired();
    assert_eq!(retired.instances().len(), INSTRUMENTS, "every strip came home");
}

/// The callback allocates and frees nothing with the worker set on: begin, deliver, publish,
/// wake, finish, inline renders, overruns, live MIDI parked for an away strip and delivered
/// later (the allocation-armed pool test on the new path).
#[test]
fn the_callback_allocates_nothing_with_render_workers() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(rig) = rig(Duration::from_micros(800)) else {
        MixEngine::force_serial(true);
        return;
    };
    let (stats, f) = std::thread::spawn(move || {
        let mut rig = rig;
        become_callback();
        let mut player = Player::new(INSTRUMENTS, 7);
        let mut rng = Rng::new(77);
        // Warm up: the first blocks chase controllers and fill the cost estimates.
        for b in 0..200 {
            player.play(&mut rig, b);
            rig.block();
        }
        let ((), stats) = measure(|| {
            for b in 200..1400u64 {
                player.play(&mut rig, b);
                if rng.below(20) == 0 {
                    let k = usize::try_from(rng.below(INSTRUMENTS as u64)).unwrap();
                    rig.shared[k].stall_ns.store(1_000_000 + rng.below(3_000_000), Ordering::Relaxed);
                }
                rig.block();
                if b % 3 == 0 {
                    std::thread::sleep(Duration::from_micros(500));
                }
            }
        });
        for s in &rig.shared {
            s.stall_ns.store(0, Ordering::Relaxed);
        }
        player.release_all(&mut rig);
        settle(&mut rig);
        transit_mix::unmark_audio_thread();
        let f = rig.mix.strip_faults().unwrap().snapshot();
        drop(rig.mix.into_retired());
        (stats, f)
    })
    .join()
    .unwrap();
    MixEngine::force_serial(true);
    println!("{f:?}");
    assert!(f.overruns() > 0 && f.midi_deferred > 0, "the overrun and deferred-MIDI paths ran: {f:?}");
    assert!(stats.is_zero(), "the callback allocated or freed: {stats:?}");
}

/// A plan waits while a worker holds a strip (the strip cannot move): `adopt_plan` refuses it
/// with `StripsInFlight`, `render_workers_settled` says why, and once the worker is back the same
/// plan is adopted. Events for the away strip are checked (a stale binding is refused) and
/// parked, then delivered when it is back.
#[test]
fn a_plan_waits_while_a_worker_holds_a_strip() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(mut rig) = rig(Duration::from_micros(500)) else {
        MixEngine::force_serial(true);
        return;
    };
    become_callback();
    for _ in 0..20 {
        rig.block();
    }
    rig.shared[2].stall_ns.store(300_000_000, Ordering::Relaxed);
    let start = Instant::now();
    while rig.mix.render_workers_settled() {
        assert!(start.elapsed() < Duration::from_secs(10), "the stalled strip never went away");
        rig.block();
    }
    // Away: an event with a stale binding is refused; the right one is parked as `Queued`.
    let mut stale = rig.bindings[2];
    stale.2 = SlotGen::VACANT;
    let good = rig.bindings[2];
    rig.bindings[2] = stale;
    assert_eq!(rig.note_on(2, 3, 60), EventOutcome::StaleBinding);
    rig.bindings[2] = good;
    assert_eq!(rig.note_on(2, 3, 60), EventOutcome::Queued);
    // A new plan (a fader move) is refused while the strip is away.
    let mut s = rig.session.clone();
    let t = rig.bindings[0].0;
    s.track_mut(t).unwrap().mixer.volume_db = -3.0;
    let mut none = |_: &InstanceSpec, _: SlotGen| -> Option<NewInstance> { None };
    let mut env = rig.comp.compile(std::sync::Arc::new(s), rig.comp.active(), transit_mix::PlanGeneration::from_raw(2), &mut none).unwrap();
    assert_eq!(rig.mix.adopt_plan(&mut env), Adoption::Rejected(RejectReason::StripsInFlight));
    rig.comp.reclaim(env);
    rig.shared[2].stall_ns.store(0, Ordering::Relaxed);
    settle(&mut rig);
    let mut s = rig.session.clone();
    s.track_mut(t).unwrap().mixer.volume_db = -3.0;
    let mut env = rig.comp.compile(std::sync::Arc::new(s), rig.comp.active(), transit_mix::PlanGeneration::from_raw(3), &mut none).unwrap();
    assert_eq!(rig.mix.adopt_plan(&mut env), Adoption::Adopted, "adopted once every strip is home");
    rig.comp.reclaim(env);
    assert!(rig.note_off(2, 0, 60).accepted());
    for _ in 0..4 {
        rig.block();
    }
    let r = rig.shared[2].report();
    let trace: Vec<_> = rig.shared[2].inst.lock().unwrap().trace().iter().map(|l| (l.event.status & 0xF0, l.event.data1)).collect();
    assert!(trace.contains(&(0x90, 60)) && trace.contains(&(0x80, 60)), "the parked note-on reached the instrument when it was back: {trace:?}");
    assert_eq!(r.stuck_notes, 0, "{r:?}");
    transit_mix::unmark_audio_thread();
    MixEngine::force_serial(true);
    drop(rig.mix.into_retired());
}

/// Gate 5 through the engine: retirement waits for each worker within a bound; a worker still
/// inside a plug-in is quarantined (its strip stays in its leaked cell, never freed or reused;
/// the log names the track and its plug-ins) and every other strip comes home.
#[test]
fn retirement_quarantines_a_worker_that_does_not_return() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(rig) = rig(Duration::from_micros(500)) else {
        MixEngine::force_serial(true);
        return;
    };
    let rig = std::thread::spawn(move || {
        let mut rig = rig;
        become_callback();
        for _ in 0..20 {
            rig.block();
        }
        rig.shared[4].stall_ns.store(4_000_000_000, Ordering::Relaxed);
        let start = Instant::now();
        while rig.mix.render_workers_settled() {
            assert!(start.elapsed() < Duration::from_secs(10), "the stalled strip never went away");
            rig.block();
        }
        transit_mix::unmark_audio_thread();
        rig
    })
    .join()
    .unwrap();
    MixEngine::force_serial(true);
    let start = Instant::now();
    let retired = rig.mix.into_retired();
    let took = start.elapsed();
    assert!(took < Duration::from_secs(2), "retirement waited {took:?} for a worker inside a plug-in");
    let instances = retired.instances();
    assert_eq!(instances.len(), INSTRUMENTS - 1, "the quarantined strip stays in its cell: {instances:?}");
    assert!(!instances.iter().any(|b| b.0 == rig.bindings[4].0), "the quarantined instance is never handed back");
}

/// Stop (or panic) while a worker holds a strip: the strip is reset when it is back, and a note
/// parked for it before the stop never starts after it.
#[test]
fn stop_while_a_strip_is_away_drops_what_was_parked_for_it() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(mut rig) = rig(Duration::from_micros(500)) else {
        MixEngine::force_serial(true);
        return;
    };
    become_callback();
    for _ in 0..20 {
        rig.block();
    }
    rig.shared[1].stall_ns.store(100_000_000, Ordering::Relaxed);
    let start = Instant::now();
    while rig.mix.render_workers_settled() {
        assert!(start.elapsed() < Duration::from_secs(10), "the stalled strip never went away");
        rig.block();
    }
    assert_eq!(rig.note_on(1, 0, 70), EventOutcome::Queued, "parked while away");
    rig.mix.reset();
    rig.shared[1].stall_ns.store(0, Ordering::Relaxed);
    settle(&mut rig);
    let trace: Vec<_> = rig.shared[1].inst.lock().unwrap().trace().iter().map(|l| (l.event.status & 0xF0, l.event.data1)).collect();
    assert!(!trace.contains(&(0x90, 70)), "a note parked before the stop started after it: {trace:?}");
    assert_eq!(rig.shared[1].report().stuck_notes, 0);
    transit_mix::unmark_audio_thread();
    MixEngine::force_serial(true);
    drop(rig.mix.into_retired());
}

/// A strip away for many blocks while its player keeps playing: the deferred-MIDI ring overflows
/// (note-ons dropped first, every release kept, a silence past that), and still no note or pedal
/// is stuck once the strip is back.
#[test]
fn an_overflowing_ring_while_a_strip_is_away_leaves_nothing_stuck() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(mut rig) = rig(Duration::from_micros(500)) else {
        MixEngine::force_serial(true);
        return;
    };
    become_callback();
    for _ in 0..20 {
        rig.block();
    }
    rig.shared[3].stall_ns.store(400_000_000, Ordering::Relaxed);
    let start = Instant::now();
    while rig.mix.render_workers_settled() {
        assert!(start.elapsed() < Duration::from_secs(10), "the stalled strip never went away");
        rig.block();
    }
    // Forty blocks of dense playing on the away strip: eight notes struck and released per block
    // and the pedal pressed and lifted.
    for b in 0..40u8 {
        for n in 0..8u8 {
            let key = 30 + (b % 4) * 8 + n;
            assert!(rig.note_on(3, u32::from(n) * 4, key).accepted());
            assert!(rig.note_off(3, u32::from(n) * 4 + 2, key).accepted());
        }
        assert!(rig.pedal(3, 60, b % 2 == 0).accepted());
        rig.block();
    }
    assert!(rig.pedal(3, 0, false).accepted());
    rig.shared[3].stall_ns.store(0, Ordering::Relaxed);
    settle(&mut rig);
    let f = rig.mix.strip_faults().unwrap().snapshot();
    let r = rig.shared[3].report();
    println!("{f:?}\n{r:?}");
    assert!(f.midi_dropped > 0, "the ring overflowed: {f:?}");
    assert_eq!((r.stuck_notes, r.stuck_pedals), (0, 0), "{r:?}");
    assert_eq!((r.faults.orphan_offs, r.faults.zero_length, r.faults.unsorted), (0, 0, 0), "{r:?}");
    transit_mix::unmark_audio_thread();
    MixEngine::force_serial(true);
    drop(rig.mix.into_retired());
}

/// Review F1: a strip that comes home between blocks (`render_workers_settled`, as playback's
/// service calls it every callback) delivers what was parked for it first, so the release that
/// arrives next is admitted behind its note-on (or pedal-down), never `Unmatched` ahead of it.
fn parked_then_settled_then_released(press: impl Fn(&mut Rig) -> EventOutcome, release: impl Fn(&mut Rig) -> EventOutcome) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(mut rig) = rig(Duration::from_micros(500)) else {
        MixEngine::force_serial(true);
        return;
    };
    become_callback();
    for _ in 0..20 {
        rig.block();
    }
    rig.shared[2].stall_ns.store(100_000_000, Ordering::Relaxed);
    let start = Instant::now();
    while rig.mix.render_workers_settled() {
        assert!(start.elapsed() < Duration::from_secs(10), "the stalled strip never went away");
        rig.block();
    }
    assert_eq!(press(&mut rig), EventOutcome::Queued, "parked while away");
    // The worker finishes; the strip comes home between blocks, with no block rendered.
    let start = Instant::now();
    while !rig.mix.render_workers_settled() {
        assert!(start.elapsed() < Duration::from_secs(10), "the worker never returned");
        std::thread::sleep(Duration::from_millis(5));
    }
    let off = release(&mut rig);
    assert!(off.accepted() && off != EventOutcome::Unmatched, "the release went ahead of what it releases: {off:?}");
    for _ in 0..16 {
        rig.block();
    }
    let r = rig.shared[2].report();
    assert_eq!((r.stuck_notes, r.stuck_pedals), (0, 0), "{r:?}");
    transit_mix::unmark_audio_thread();
    MixEngine::force_serial(true);
    drop(rig.mix.into_retired());
}

#[test]
fn a_note_parked_while_away_is_released_after_the_strip_comes_home_between_blocks() {
    parked_then_settled_then_released(|rig| rig.note_on(2, 0, 60), |rig| rig.note_off(2, 0, 60));
}

#[test]
fn a_pedal_parked_while_away_is_lifted_after_the_strip_comes_home_between_blocks() {
    parked_then_settled_then_released(|rig| rig.pedal(2, 0, true), |rig| rig.pedal(2, 0, false));
}

/// An `Unstarted` strip keeps its block's events in its instrument's queue; at its next block
/// they are overdue and land at offset 0. A note's on and off overdue together are split by one
/// sample (no zero-length note), as the ring does for a `Busy` strip.
#[test]
fn an_unstarted_strip_plays_its_overdue_notes_without_a_zero_length_one() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(mut rig) = rig(Duration::from_micros(1500)) else {
        MixEngine::force_serial(true);
        return;
    };
    become_callback();
    for _ in 0..20 {
        rig.block();
    }
    for round in 0..50u8 {
        let key = 40 + round % 40;
        for k in 0..INSTRUMENTS {
            assert!(rig.note_on(k, 5, key).accepted());
            assert!(rig.note_off(k, 9, key).accepted());
        }
        // No time at all: every strip no worker took yet is taken back unrendered.
        rig.mix.set_strip_deadline(Some(Duration::ZERO));
        rig.block();
        rig.mix.set_strip_deadline(Some(Duration::from_micros(1500)));
        settle(&mut rig);
    }
    let f = rig.mix.strip_faults().unwrap().snapshot();
    assert!(f.overruns_unstarted > 0, "some strips were unstarted: {f:?}");
    for (k, s) in rig.shared.iter().enumerate() {
        let r = s.report();
        assert_eq!((r.stuck_notes, r.faults.zero_length, r.faults.unsorted), (0, 0, 0), "instrument {k}: {r:?}");
    }
    transit_mix::unmark_audio_thread();
    MixEngine::force_serial(true);
    drop(rig.mix.into_retired());
}

/// The strips' deadline counts from the device callback's entry: a callback that starts the
/// render late (a late wake, its own service work) shows as a counted deadline overshoot, not
/// as a late block with every strip figure at zero.
#[test]
fn a_late_callback_start_is_counted_as_a_deadline_overshoot() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    let Some(mut rig) = rig(Duration::from_micros(1500)) else {
        MixEngine::force_serial(true);
        return;
    };
    rig.mix.set_strip_deadline(None);
    become_callback();
    for _ in 0..20 {
        rig.block();
    }
    let before = rig.mix.strip_faults().unwrap().snapshot().deadline_overshoots;
    // The callback entered 5 ms ago; the block's deadline already passed.
    let entry = Instant::now().checked_sub(Duration::from_millis(5)).unwrap();
    rig.mix.set_block_deadline(Some(entry + Duration::from_millis(2)));
    rig.block();
    let after = rig.mix.strip_faults().unwrap().snapshot().deadline_overshoots;
    assert_eq!(after, before + 1, "the late start is counted");
    // One block only: the next is measured from its own entry again.
    rig.block();
    settle(&mut rig);
    transit_mix::unmark_audio_thread();
    MixEngine::force_serial(true);
    drop(rig.mix.into_retired());
}
