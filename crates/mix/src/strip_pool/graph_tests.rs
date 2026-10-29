//! The graph path through the real strips (issue #174, ADR 0029 amendment section 8),
//! deterministic: a key source held inside its plug-in on the one render worker (released by the
//! test, never by a clock), a pinned strip on the callback that waits on observable state (the
//! hold entered, the block's deadline passed) so the deadline arrives with the source still held,
//! and a pool with no worker for the unstarted case.
//!
//! - a key source late, busy and unstarted: its dependents are `Overrun(UpstreamMissed)` with the
//!   source as the origin, once each per block, the source's own overrun counted once, the
//!   unrelated pinned strip renders;
//! - delay-line history across the skip: the keyed strip's own line (a late key) and its key
//!   entry's line (an early key) are neither written nor idled while it is skipped, and continue
//!   from their history after;
//! - a forced completion-identity mismatch is retired unread and counted, and skips the branch;
//! - a graph change while the worker holds an old task waits until it is home; the old result is
//!   retired unread and the new graph renders.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::tests::PlanRig;
use std::sync::atomic::AtomicUsize;
use std::sync::{Mutex as StdMutex, PoisonError};
use transit_model::{Insert, KeyFrom, KeySource, Tap};

const FRAMES: usize = 128;
/// A stall bound, not a latency claim: only turns a hang into a failure.
const STALL: Duration = Duration::from_secs(60);

std::thread_local! {
    /// Set on the test's callback thread: a hold applies only on a render worker.
    static CALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A gain stage with a latency that, armed, holds a render worker inside `process` until
/// released.
struct Hold {
    inner: Box<dyn Plugin>,
    latency: usize,
    hold: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
}

/// A pinned strip's plug-in that, armed, waits on the callback until the source's hold is entered
/// (when asked) and the block's deadline has passed.
struct Wait {
    inner: Box<dyn Plugin>,
    armed: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    need_entered: Arc<AtomicBool>,
    deadline: Arc<StdMutex<Option<Instant>>>,
    renders: Arc<AtomicUsize>,
}

fn gain() -> Box<dyn Plugin> {
    crate::create_instance("gain", None, 48_000.0, FRAMES, 1).unwrap().plugin
}

macro_rules! delegate {
    () => {
        fn info(&self) -> &'static transit_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
    };
}

impl Plugin for Hold {
    delegate!();
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.hold.load(Ordering::Acquire) && !CALLBACK.get() {
            self.entered.store(true, Ordering::Release);
            let t0 = Instant::now();
            while self.hold.load(Ordering::Acquire) {
                assert!(t0.elapsed() < STALL, "never released");
                // timing: sleep-ok paces a wait for a release flag, measures nothing
                std::thread::sleep(Duration::from_millis(1));
            }
            self.entered.store(false, Ordering::Release);
        }
        self.inner.process(io, frames);
    }
    fn latency(&self) -> usize {
        self.latency
    }
}

impl Plugin for Wait {
    delegate!();
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.renders.fetch_add(1, Ordering::Relaxed);
        if self.armed.load(Ordering::Acquire) {
            let t0 = Instant::now();
            let deadline = self.deadline.lock().unwrap_or_else(PoisonError::into_inner).unwrap_or(t0);
            while (self.need_entered.load(Ordering::Acquire) && !self.entered.load(Ordering::Acquire)) || Instant::now() < deadline {
                assert!(t0.elapsed() < STALL, "the hold was never entered");
                std::hint::spin_loop();
            }
        }
        self.inner.process(io, frames);
    }
}

/// The shared handles of one rig's plug-ins.
struct Knobs {
    hold: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    armed: Arc<AtomicBool>,
    need_entered: Arc<AtomicBool>,
    deadline: Arc<StdMutex<Option<Instant>>>,
    pad_renders: Arc<AtomicUsize>,
}

/// Kick (audio, a held 64-sample latency) keys a compressor on Bass (a late key: Bass's own line
/// runs) and on Lead (128 samples of latency ahead of it: an early key, the entry's line runs);
/// Pad monitors its input (pinned to the callback) and waits.
fn session() -> (Session, [TrackId; 4]) {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.edit.click = false;
    let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Kick"));
    let bass = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Bass"));
    let lead = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead"));
    let pad = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Pad"));
    let comp = || {
        let mut c = Insert::new("compressor");
        c.key = Some(KeySource { from: KeyFrom::Track(kick), tap: Tap::Post });
        Some(c)
    };
    s.track_mut(kick).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    s.track_mut(bass).unwrap().mixer.inserts[0] = comp();
    s.track_mut(lead).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    s.track_mut(lead).unwrap().mixer.inserts[1] = comp();
    let p = s.track_mut(pad).unwrap();
    p.mixer.input_monitor = true;
    p.mixer.inserts[0] = Some(Insert::new("gain"));
    (s, [kick, bass, lead, pad])
}

fn rig(workers: usize) -> (PlanRig, Session, [TrackId; 4], Knobs) {
    let (s, ids) = session();
    let [kick, _, lead, pad] = ids;
    let knobs = Knobs {
        hold: Arc::default(),
        entered: Arc::default(),
        armed: Arc::default(),
        need_entered: Arc::default(),
        deadline: Arc::default(),
        pad_renders: Arc::default(),
    };
    let hold = Hold { inner: gain(), latency: 64, hold: Arc::clone(&knobs.hold), entered: Arc::clone(&knobs.entered) };
    let latent = Hold { inner: gain(), latency: 128, hold: Arc::default(), entered: Arc::default() };
    let wait = Wait {
        inner: gain(),
        armed: Arc::clone(&knobs.armed),
        entered: Arc::clone(&knobs.entered),
        need_entered: Arc::clone(&knobs.need_entered),
        deadline: Arc::clone(&knobs.deadline),
        renders: Arc::clone(&knobs.pad_renders),
    };
    let mut rig = PlanRig::new(FRAMES).fake(kick, 0, hold).fake(lead, 0, latent).fake(pad, 0, wait);
    rig.publish(&s);
    rig.eng.pool = Some(StripPool::new(workers, 16, false, None, u32::try_from(FRAMES).unwrap(), 48_000).unwrap());
    (rig, s, ids, knobs)
}

/// One block on the calling thread (the test's callback).
fn block(rig: &mut PlanRig, b: &mut i64) {
    let mut out = vec![vec![0.0f32; FRAMES]; 2];
    rig.eng.render_active(*b * FRAMES as i64, FRAMES, &mut out);
    *b += 1;
}

/// Arms the pinned strip to hold the next block until its deadline (and, when asked, until the
/// source's hold is entered), and gives the engine that deadline.
fn deadline_block(rig: &mut PlanRig, knobs: &Knobs, b: &mut i64, need_entered: bool) {
    let d = Instant::now() + Duration::from_millis(5);
    *knobs.deadline.lock().unwrap() = Some(d);
    knobs.need_entered.store(need_entered, Ordering::Release);
    knobs.armed.store(true, Ordering::Release);
    rig.eng.set_strip_deadline(None);
    rig.eng.set_block_deadline(Some(d));
    block(rig, b);
    knobs.armed.store(false, Ordering::Release);
}

fn pool(rig: &PlanRig) -> &StripPool {
    rig.eng.pool.as_ref().unwrap()
}

fn index(rig: &PlanRig, t: TrackId) -> usize {
    rig.eng.plan.as_deref().unwrap().tables.strip_of(t).unwrap()
}

fn outcome(rig: &PlanRig, t: TrackId) -> Outcome {
    pool(rig).set.outcome(index(rig, t)).unwrap()
}

/// The delay lines a skip must leave alone: Bass's own line before its keyed insert, and the
/// line of Lead's key entry.
fn lines(rig: &PlanRig, ids: &[TrackId; 4]) -> ((bool, usize, usize), Vec<(bool, usize, usize)>) {
    let tables = &rig.eng.plan.as_deref().unwrap().tables;
    let bass = &tables.strips[index(rig, ids[1])];
    (bass.key_in[0].line.history(), tables.keys.lines_of(index(rig, ids[2])))
}

/// Runs `f` as the audio callback with the graph on (and the switches restored after).
fn as_callback<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    MixEngine::force_serial(false);
    MixEngine::force_graph(true);
    let r = std::thread::spawn(move || {
        mark_audio_thread();
        CALLBACK.set(true);
        let r = f();
        unmark_audio_thread();
        r
    })
    .join();
    MixEngine::force_graph(false);
    MixEngine::force_serial(true);
    r.unwrap()
}

#[test]
fn a_late_then_busy_key_source_skips_its_dependents_and_their_lines_keep_their_history() {
    let _switch = SWITCH.lock().unwrap_or_else(PoisonError::into_inner);
    let (rig, _, ids, knobs) = rig(1);
    let [kick, bass, lead, pad] = ids;
    let (rig, knobs) = as_callback(move || {
        let (mut rig, mut b) = (rig, 0i64);
        // Warm-up, a far deadline: everything renders, the lines run.
        rig.eng.set_strip_deadline(Some(STALL));
        for _ in 0..12 {
            block(&mut rig, &mut b);
        }
        for t in [kick, bass, lead] {
            assert!(matches!(outcome(&rig, t), Outcome::Mixed | Outcome::Inline), "{t:?}: {:?}", outcome(&rig, t));
        }
        let before = lines(&rig, &ids);
        assert!(before.0.0 && before.1.iter().all(|l| l.0), "both lines run before the skip: {before:?}");
        let pad_renders = knobs.pad_renders.load(Ordering::Relaxed);

        // Late: the worker is inside the kick's hold when the deadline comes.
        knobs.hold.store(true, Ordering::Release);
        deadline_block(&mut rig, &knobs, &mut b, true);
        assert_eq!(outcome(&rig, kick), Outcome::Overrun(OverrunCause::Late));
        assert_eq!(outcome(&rig, bass), Outcome::Overrun(OverrunCause::UpstreamMissed));
        assert_eq!(outcome(&rig, lead), Outcome::Overrun(OverrunCause::UpstreamMissed));
        assert_eq!(knobs.pad_renders.load(Ordering::Relaxed), pad_renders + 1, "the unrelated pinned strip rendered");
        assert!(!pool(&rig).is_silent(index(&rig, pad)));
        assert_eq!(lines(&rig, &ids), before, "a skipped strip's lines are neither written nor idled");
        assert_eq!(rig.eng.block_overruns(), 1, "the take journal counts the origin once");

        // Busy: the worker still holds it.
        deadline_block(&mut rig, &knobs, &mut b, false);
        assert_eq!(outcome(&rig, kick), Outcome::Overrun(OverrunCause::Busy));
        assert_eq!(
            (outcome(&rig, bass), outcome(&rig, lead)),
            (Outcome::Overrun(OverrunCause::UpstreamMissed), Outcome::Overrun(OverrunCause::UpstreamMissed))
        );
        assert_eq!(lines(&rig, &ids), before);

        // Released: the late result is retired unread, the branch renders and the lines continue.
        knobs.hold.store(false, Ordering::Release);
        let k = index(&rig, kick);
        let t0 = Instant::now();
        while pool(&rig).set.jobs().slot(k).unwrap().state() == transit_rt::strip_jobs::State::Claimed {
            assert!(t0.elapsed() < STALL, "the worker never returned");
            std::thread::yield_now();
        }
        rig.eng.set_block_deadline(None);
        rig.eng.set_strip_deadline(Some(STALL));
        block(&mut rig, &mut b);
        for t in [kick, bass, lead] {
            assert!(matches!(outcome(&rig, t), Outcome::Mixed | Outcome::Inline), "{t:?} renders again");
        }
        // A line that continues holds one block more (or stays full); a reset one holds one block.
        let continued = |a: (bool, usize, usize), z: (bool, usize, usize)| a.0 && a.1 != z.1 && (a.2 == z.2 + FRAMES || (a.2 == z.2 && z.2 > FRAMES));
        let after = lines(&rig, &ids);
        assert!(continued(after.0, before.0), "Bass's line continues from its history, not reset: {before:?} -> {after:?}");
        assert!(after.1.iter().zip(&before.1).all(|(a, z)| continued(*a, *z)), "Lead's key line too: {before:?} -> {after:?}");
        (rig, knobs)
    });
    drop(knobs);
    let f = pool(&rig).reader().snapshot();
    assert_eq!((f.overruns_late, f.overruns_busy, f.upstream_missed, f.conflicts), (1, 1, 4, 0), "{f:?}");
    assert_eq!(f.overrun_blocks, 2);
    assert_eq!(pool(&rig).set.stats().late_results, 1);
    let origin = u32::from(rig.eng.plan.as_deref().unwrap().tables.strips[index(&rig, kick)].track);
    let bass_row = f.top_strips.iter().flatten().find(|s| s.upstream_missed > 0).unwrap();
    assert_eq!((bass_row.upstream_missed, bass_row.upstream_origin, bass_row.overruns()), (2, origin, 0), "named with its origin: {f:?}");
    let PlanRig { eng, .. } = rig;
    drop(eng.into_retired());
}

#[test]
fn an_unstarted_key_source_skips_its_dependents_and_the_pinned_strip_renders() {
    let _switch = SWITCH.lock().unwrap_or_else(PoisonError::into_inner);
    let (rig, _, ids, knobs) = rig(0);
    let [kick, bass, lead, pad] = ids;
    let rig = as_callback(move || {
        let (mut rig, mut b) = (rig, 0i64);
        // No worker and no cost history: nothing is rendered inline, the kick is taken back
        // unrendered at the deadline.
        deadline_block(&mut rig, &knobs, &mut b, false);
        assert_eq!(outcome(&rig, kick), Outcome::Overrun(OverrunCause::Unstarted));
        assert_eq!(
            (outcome(&rig, bass), outcome(&rig, lead)),
            (Outcome::Overrun(OverrunCause::UpstreamMissed), Outcome::Overrun(OverrunCause::UpstreamMissed))
        );
        assert_eq!(knobs.pad_renders.load(Ordering::Relaxed), 1);
        assert!(!pool(&rig).is_silent(index(&rig, pad)) && pool(&rig).is_silent(index(&rig, bass)));
        assert!(!pool(&rig).any_away(), "every strip home");
        rig
    });
    let f = pool(&rig).reader().snapshot();
    assert_eq!((f.overruns_unstarted, f.upstream_missed, f.conflicts), (1, 2, 0), "{f:?}");
    let PlanRig { eng, .. } = rig;
    drop(eng.into_retired());
}

#[test]
fn a_result_with_another_identity_is_retired_unread_and_skips_the_branch() {
    let _switch = SWITCH.lock().unwrap_or_else(PoisonError::into_inner);
    let (rig, _, ids, _knobs) = rig(1);
    let [kick, bass, lead, _] = ids;
    let k = rig.eng.plan.as_deref().unwrap().tables.strip_of(kick).unwrap();
    let rig = as_callback(move || {
        let (mut rig, mut b) = (rig, 0i64);
        rig.eng.set_strip_deadline(Some(STALL));
        block(&mut rig, &mut b);
        tests::CORRUPT_STAMP.store(k, Ordering::Relaxed);
        block(&mut rig, &mut b);
        tests::CORRUPT_STAMP.store(usize::MAX, Ordering::Relaxed);
        assert!(pool(&rig).is_silent(k), "never mixed");
        assert_eq!(
            (outcome(&rig, bass), outcome(&rig, lead)),
            (Outcome::Overrun(OverrunCause::UpstreamMissed), Outcome::Overrun(OverrunCause::UpstreamMissed))
        );
        block(&mut rig, &mut b);
        assert!(matches!(outcome(&rig, bass), Outcome::Mixed | Outcome::Inline), "the next block renders");
        rig
    });
    let f = pool(&rig).reader().snapshot();
    assert_eq!((f.conflicts, f.upstream_missed, f.overruns()), (1, 2, 0), "counted as a conflict, not an overrun: {f:?}");
    let PlanRig { eng, .. } = rig;
    drop(eng.into_retired());
}

#[test]
fn a_graph_change_waits_for_the_held_task_and_the_old_result_is_never_read() {
    let _switch = SWITCH.lock().unwrap_or_else(PoisonError::into_inner);
    let (rig, s, ids, knobs) = rig(1);
    let [kick, bass, lead, _] = ids;
    let rig = as_callback(move || {
        let (mut rig, mut b) = (rig, 0i64);
        rig.eng.set_strip_deadline(Some(STALL));
        for _ in 0..4 {
            block(&mut rig, &mut b);
        }
        let old = rig.eng.plan.as_deref().unwrap().tables.graph.graph.generation();
        knobs.hold.store(true, Ordering::Release);
        deadline_block(&mut rig, &knobs, &mut b, true);
        assert_eq!(outcome(&rig, kick), Outcome::Overrun(OverrunCause::Late));
        // The new graph: Bass is no longer keyed.
        let mut s2 = s.clone();
        s2.track_mut(bass).unwrap().mixer.inserts[0].as_mut().unwrap().key = None;
        assert_eq!(rig.try_publish(&s2), Adoption::Rejected(RejectReason::StripsInFlight), "a held task keeps its graph");
        assert!(!rig.eng.render_workers_settled());
        knobs.hold.store(false, Ordering::Release);
        let t0 = Instant::now();
        while !rig.eng.render_workers_settled() {
            assert!(t0.elapsed() < STALL, "the worker never returned");
            std::thread::yield_now();
        }
        rig.publish(&s2);
        let new = rig.eng.plan.as_deref().unwrap().tables.graph.graph.generation();
        assert_ne!(new, old, "a new graph");
        rig.eng.set_block_deadline(None);
        block(&mut rig, &mut b);
        for t in [kick, bass, lead] {
            assert!(matches!(outcome(&rig, t), Outcome::Mixed | Outcome::Inline), "{t:?} renders under the new graph");
        }
        rig
    });
    let p = pool(&rig);
    assert_eq!(p.set.stats().late_results, 1, "the old graph's result retired unread");
    assert_eq!(p.reader().snapshot().conflicts, 0, "the new graph's results carry its identity");
    let PlanRig { eng, .. } = rig;
    drop(eng.into_retired());
}

/// A keyed insert that records the loudest key it was given and how often it ran.
struct Probe {
    inner: Box<dyn Plugin>,
    calls: Arc<AtomicUsize>,
    /// The loudest key sample seen, as `f32` bits.
    loudest: Arc<std::sync::atomic::AtomicU32>,
}

impl Plugin for Probe {
    delegate!();
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.inner.process(io, frames);
    }
    fn process_with_key(&mut self, io: &mut [Vec<f32>], key: &[Vec<f32>], frames: usize) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let peak = key.iter().flat_map(|c| c.iter().take(frames)).fold(0.0f32, |m, v| m.max(v.abs()));
        self.loudest.fetch_max(peak.to_bits(), Ordering::Relaxed);
        self.inner.process(io, frames);
    }
}

/// Review finding 1 (#174): the tail rule is transitive. Kick (held on the one worker) keys aux
/// A1; aux A2 is keyed from A1. In a block where Kick misses, A1 is skipped and so is A2: A2's
/// keyed insert never sees A1's buffer from the last block it ran (a skipped aux keeps it). Both
/// skips are counted with the upstream-missed ones (`note_tail_skip`).
#[test]
fn an_aux_keyed_from_a_skipped_aux_is_skipped_too() {
    let _switch = SWITCH.lock().unwrap_or_else(PoisonError::into_inner);
    let mut s = Session::default();
    s.edit.click = false;
    let feed = crate::tests::add_dc_track(&mut s, 0.5, 48_000 * 10);
    let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Kick"));
    let pad = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Pad"));
    let filler = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Filler"));
    let (b1, b2) = (BusId(s.alloc()), BusId(s.alloc()));
    s.busses.push(transit_model::Bus { id: b1, name: "B1".into(), format: ChannelFormat::Stereo });
    s.busses.push(transit_model::Bus { id: b2, name: "B2".into(), format: ChannelFormat::Stereo });
    s.track_mut(feed).unwrap().mixer.output = Route::Bus(b1);
    let _ = filler;
    s.track_mut(kick).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    let p = s.track_mut(pad).unwrap();
    p.mixer.input_monitor = true;
    p.mixer.inserts[0] = Some(Insert::new("gain"));
    let keyed = |from: TrackId| {
        let mut c = Insert::new("compressor");
        c.key = Some(KeySource { from: KeyFrom::Track(from), tap: Tap::Post });
        Some(c)
    };
    let a1 = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("A1"));
    let t = s.track_mut(a1).unwrap();
    t.mixer.input = Route::Bus(b1);
    t.mixer.output = Route::Main;
    t.mixer.inserts[0] = keyed(kick);
    let a2 = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("A2"));
    let t = s.track_mut(a2).unwrap();
    t.mixer.input = Route::Bus(b2);
    t.mixer.output = Route::Main;
    t.mixer.inserts[0] = keyed(a1);

    let knobs = Knobs {
        hold: Arc::default(),
        entered: Arc::default(),
        armed: Arc::default(),
        need_entered: Arc::default(),
        deadline: Arc::default(),
        pad_renders: Arc::default(),
    };
    let (calls, loudest) = (Arc::new(AtomicUsize::new(0)), Arc::new(std::sync::atomic::AtomicU32::new(0)));
    let hold = Hold { inner: gain(), latency: 0, hold: Arc::clone(&knobs.hold), entered: Arc::clone(&knobs.entered) };
    let wait = Wait {
        inner: gain(),
        armed: Arc::clone(&knobs.armed),
        entered: Arc::clone(&knobs.entered),
        need_entered: Arc::clone(&knobs.need_entered),
        deadline: Arc::clone(&knobs.deadline),
        renders: Arc::clone(&knobs.pad_renders),
    };
    let probe = Probe { inner: gain(), calls: Arc::clone(&calls), loudest: Arc::clone(&loudest) };
    let mut rig = PlanRig::new(FRAMES).fake(kick, 0, hold).fake(pad, 0, wait).fake(a2, 0, probe);
    rig.publish(&s);
    rig.eng.pool = Some(StripPool::new(1, 16, false, None, u32::try_from(FRAMES).unwrap(), 48_000).unwrap());
    let (c, l) = (Arc::clone(&calls), Arc::clone(&loudest));
    let rig = as_callback(move || {
        let (mut rig, mut b) = (rig, 0i64);
        rig.eng.set_strip_deadline(Some(STALL));
        for _ in 0..6 {
            block(&mut rig, &mut b);
        }
        assert!(c.load(Ordering::Relaxed) > 0 && f32::from_bits(l.load(Ordering::Relaxed)) > 0.1, "A2's key is A1's signal when A1 runs");
        let before = pool(&rig).reader().snapshot().upstream_missed;
        c.store(0, Ordering::Relaxed);
        l.store(0, Ordering::Relaxed);
        knobs.hold.store(true, Ordering::Release);
        deadline_block(&mut rig, &knobs, &mut b, true);
        assert_eq!(outcome(&rig, kick), Outcome::Overrun(OverrunCause::Late));
        assert_eq!(pool(&rig).reader().snapshot().upstream_missed, before + 2, "A1 and A2 skipped, counted as upstream-missed tail skips");
        assert_eq!(rig.eng.plan.as_deref().unwrap().tables.graph.tail_skipped.as_ref(), [true, true]);
        assert_eq!((c.load(Ordering::Relaxed), l.load(Ordering::Relaxed)), (0, 0), "A2's keyed insert never ran on a stale key");
        knobs.hold.store(false, Ordering::Release);
        let t0 = Instant::now();
        while !rig.eng.render_workers_settled() {
            assert!(t0.elapsed() < STALL, "the worker never returned");
            std::thread::yield_now();
        }
        rig.eng.set_block_deadline(None);
        rig.eng.set_strip_deadline(Some(STALL));
        block(&mut rig, &mut b);
        assert_eq!(rig.eng.plan.as_deref().unwrap().tables.graph.tail_skipped.as_ref(), [false, false], "the flags are per block");
        assert!(c.load(Ordering::Relaxed) > 0, "A2 runs again once A1 does");
        rig
    });
    let PlanRig { eng, .. } = rig;
    drop(eng.into_retired());
}
