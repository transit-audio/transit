//! Section 11 host decisions, ownership and panic checkpoints, with injected windows and costs.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::graph_host::Host;
use super::tail::TailWork;
use super::*;
use crate::tail::DeviceLatency;
use crate::tests::PlanRig;
use std::sync::atomic::AtomicUsize;
use transit_model::{Bus, Insert, KeyFrom, KeySource, Tap};
use transit_rt::task_graph::{GraphHost, Rendered, Start, StartAt, Window};
use transit_rt::worker_set::seams::{CostClock, SlotOverrides};

const N: usize = 64;
const FAR: Duration = Duration::from_secs(60);
/// The fit's wake margin in nanoseconds, added to an injected budget that is meant to fit.
const M: u64 = super::graph_host::TAIL_WAKE_MARGIN.as_nanos() as u64;
static NO_KEYS: KeyBank = KeyBank::EMPTY;

mod forgo;
#[path = "../../tests/common/stall.rs"]
mod stall;

/// A Claim owns its snapshot even before the reader enters the job cell.
/// timing: expired-deadline the next drive cannot wait for the deliberately paused claim.
#[test]
fn claimed_aux_before_cell_entry_keeps_snapshot_across_render_boundary() {
    let (mut rig, ..) = fixture(false);
    let slot = host(&mut rig, |h, slot, task| {
        override_slot(h, slot, 1, 1, true, true);
        assert_eq!(h.start(task, StartAt::from(Window::Open)), Start::Publish);
        assert_eq!(h.pool.set.publish(slot), Ok(transit_rt::worker_set::Published::Published));
        let claim = h.pool.set.jobs().slot(slot).unwrap().try_claim().unwrap();
        assert_eq!(claim.generation(), h.g);
        // Leaving this scope does not complete the ClaimToken: model a reader paused indefinitely
        // before cell entry, without a worker thread or a held mutex hiding early reuse.
        slot
    });
    let pool = rig.eng.pool.as_mut().unwrap();
    assert!(pool.cells.with(slot, |job| assert_eq!(job.busses.0.len(), 1)), "cell is free while Claimed");
    let plan = rig.eng.plan.as_deref_mut().unwrap();
    let empty = BusTable::default();
    let live =
        LiveInput { input: &[], input_offset: 0, monitor_only: false, recording: false, no_suspend: true, frame: N as u64, io: None, marker: None };
    let b = PoolBlock {
        session: &plan.session,
        busses: &empty,
        keys: &NO_KEYS,
        live: &live,
        pos: N as i64,
        frames: N,
        any_solo: false,
        sample_rate: 48_000,
        start: Instant::now(),
        budget: None,
        deadline: Some(Instant::now()),
    };
    assert!(pool.render_tail(&b, &mut plan.tables, &mut rig.eng.events, &mut plan.status, PanLaw::default(), N as u64).is_some());
    assert_eq!(pool.set.outcome(slot), Some(Outcome::Overrun(OverrunCause::Busy)));
    assert!(pool.is_away(slot));
    let mut input = BusTable(Vec::with_capacity(1));
    assert!(!plan.tables.inputs.lend(slot, &mut input), "snapshot became reusable while its reader still owns Claimed");
}

#[derive(Default)]
struct Probe {
    process: AtomicUsize,
    reset: AtomicUsize,
    notes_off: AtomicUsize,
    latency: AtomicUsize,
    params: AtomicUsize,
    panic: AtomicUsize,
    key: AtomicU32,
    input: AtomicU32,
    hold: AtomicBool,
    entered: AtomicBool,
    latency_value: AtomicUsize,
}
struct Echo {
    inner: Box<dyn Plugin>,
    probe: Arc<Probe>,
}
impl Plugin for Echo {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, n: usize, ch: usize) {
        self.inner.prepare(sr, n, ch);
    }
    fn reset(&mut self) {
        self.probe.reset.fetch_add(1, Ordering::Relaxed);
    }
    fn all_notes_off(&mut self) {
        self.probe.notes_off.fetch_add(1, Ordering::Relaxed);
    }
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        self.probe.params.fetch_add(1, Ordering::Relaxed);
        assert_ne!(self.probe.panic.load(Ordering::Relaxed), 3, "params payload");
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn latency(&self) -> usize {
        self.probe.latency.fetch_add(1, Ordering::Relaxed);
        assert_ne!(self.probe.panic.load(Ordering::Relaxed), 2, "latency payload");
        let value = self.probe.latency_value.load(Ordering::Relaxed);
        if value == 0 { 7 } else { value }
    }
    fn process(&mut self, _: &mut [Vec<f32>], _: usize) {
        self.probe.process.fetch_add(1, Ordering::Relaxed);
        if self.probe.hold.load(Ordering::Acquire) {
            self.probe.entered.store(true, Ordering::Release);
            stall::wait_for_release(&self.probe.hold, &self.probe.process, std::thread::yield_now);
        }
        assert_ne!(self.probe.panic.load(Ordering::Relaxed), 1, "render payload");
    }
    fn process_with_key(&mut self, io: &mut [Vec<f32>], key: &[Vec<f32>], n: usize) {
        self.probe.input.store(io[0][0].to_bits(), Ordering::Relaxed);
        self.process(io, n);
        let x = key[0][0];
        self.probe.key.store(x.to_bits(), Ordering::Relaxed);
        for c in io {
            c[..n].fill(x);
        }
    }
}
fn echo(probe: &Arc<Probe>) -> Echo {
    Echo { inner: crate::create_instance("gain", None, 48_000.0, N, 1).unwrap().plugin, probe: Arc::clone(probe) }
}
fn fixture(bus_key: bool) -> (PlanRig, Session, TrackId, Arc<Probe>) {
    let mut s = Session::default();
    s.edit.click = false;
    s.edit.delay_compensation = true;
    let bus = BusId(s.alloc());
    s.busses.push(Bus { id: bus, name: "input".into(), format: ChannelFormat::Mono });
    let source = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("key"));
    for _ in 0..3 {
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    }
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("aux"));
    let t = s.track_mut(aux).unwrap();
    t.mixer.input = Route::Bus(bus);
    t.mixer.output = Route::Main;
    let mut insert = Insert::new("gain");
    insert.params.insert("gain".into(), 1.0);
    insert.key = Some(KeySource { from: if bus_key { KeyFrom::Bus(bus) } else { KeyFrom::Track(source) }, tap: Tap::Post });
    t.mixer.inserts[0] = Some(insert);
    let probe = Arc::new(Probe::default());
    let mut rig = PlanRig::new(N).fake(aux, 0, echo(&probe));
    rig.publish(&s);
    rig.eng.pool = Some(StripPool::new(0, 16, false, None, N as u32, 48_000).unwrap());
    (rig, s, aux, probe)
}

/// Direct production host, with no scheduler or elapsed-time assertion. Each fixture is fresh.
fn host<R>(rig: &mut PlanRig, f: impl FnOnce(&mut Host<'_, '_>, usize, usize) -> R) -> R {
    let plan = rig.eng.plan.as_deref_mut().unwrap();
    let pool = rig.eng.pool.as_mut().unwrap();
    let g = pool.generation + 1;
    assert!(pool.set.begin_block(g));
    pool.generation = g;
    let tables = &mut plan.tables;
    let at = tables.phase1;
    for st in tables.strips.iter_mut().take(at) {
        for c in &mut st.buf {
            c.fill(0.75);
        }
    }
    for (_, bus) in &mut tables.busses.0 {
        for c in &mut bus.buf {
            c.fill(0.75);
        }
    }
    let live = LiveInput { input: &[], input_offset: 0, monitor_only: false, recording: false, no_suspend: true, frame: 0, io: None, marker: None };
    let empty = BusTable::default();
    let deadline = Instant::now() + FAR;
    let b = PoolBlock {
        session: &plan.session,
        busses: &empty,
        keys: &NO_KEYS,
        live: &live,
        pos: 0,
        frames: N,
        any_solo: false,
        sample_rate: 48_000,
        start: Instant::now(),
        budget: None,
        deadline: Some(deadline),
    };
    pool.set.set_block_timing(deadline, block_period(N, 48_000));
    let (strips, others) = tables.strips.split_at_mut(tables.aux_end);
    let mut h = Host {
        pool,
        p1: strips,
        events: &mut rig.eng.events,
        keys: &mut tables.keys,
        graph: &tables.graph.tail.graph,
        layer_start: &tables.graph.layer_start,
        b: &b,
        g,
        callback: 0,
        tail: Some(TailWork {
            phase1: at,
            others,
            busses: &mut tables.busses,
            main: &mut tables.main,
            main_fmt: tables.main_fmt,
            sinks: &mut tables.sinks,
            status: &mut plan.status,
            inputs: &mut tables.inputs,
            law: PanLaw::default(),
            host_frame: 0,
            latency: DeviceLatency { main: 0, device: 0, aborted: false },
        }),
    };
    f(&mut h, at, at + 1)
}
fn override_slot(h: &mut Host<'_, '_>, at: usize, worst: u64, left: u64, force: bool, no_reclaim: bool) {
    h.pool.set.set_cost_estimate(at, Duration::from_nanos(worst));
    h.pool.set.arm_slot_overrides(at, SlotOverrides { budget_left: Some(Duration::from_nanos(left)), force_publish: force, no_reclaim });
    h.pool.set.install_test_seams();
}
fn execute(h: &mut Host<'_, '_>, task: usize, slot: usize, start: Start) -> bool {
    match start {
        Start::Callback => h.render_here(task) == Rendered::Usable,
        Start::Publish => {
            assert!(h.pool.set.publish(slot).is_ok());
            let _ = h.poll(Instant::now());
            assert_eq!(h.pool.set.outcome(slot), Some(Outcome::Inline));
            h.take(task, Outcome::Inline)
        }
        other => panic!("unexpected {other:?}"),
    }
}

/// #293 item 2: the fit uses the time left the driver read with the window, not a clock read
/// of its own: with no injected budget, the worst case plus the margin equal to that time publishes
/// and one nanosecond more does not. Mutation: reading the clock again in the fit sees a deadline
/// 60 s away and publishes both.
#[test]
fn the_fit_uses_the_drivers_time_left() {
    for (left, want) in [(10 + M, Start::Publish), (10 + M - 1, Start::Callback)] {
        let (mut rig, ..) = fixture(false);
        host(&mut rig, |h, slot, task| {
            h.pool.set.set_cost_estimate(slot, Duration::from_nanos(10));
            let at = StartAt { window: Window::Open, alone: false, left: Duration::from_nanos(left) };
            let start = h.start(task, at);
            assert_eq!(start, want, "left {left}");
            assert!(execute(h, task, slot, start));
        });
    }
}

/// T4a: always-publish must fail here; the worst case plus the wake margin equal to the time
/// left is accepted (G5: one nanosecond less is not), Closed is never published.
#[test]
fn t4a_production_fit() {
    for (worst, left, window, want) in [
        (0, 10 + M, Window::Open, Start::Callback),
        (5, 10 + M, Window::Open, Start::Publish),
        (10, 10 + M, Window::Open, Start::Publish),
        (10, 10 + M - 1, Window::Open, Start::Callback),
        (10, 10, Window::Open, Start::Callback),
        (11, 10 + M, Window::Open, Start::Callback),
        (5, 10 + M, Window::Closed, Start::Callback),
    ] {
        let (mut rig, ..) = fixture(false);
        host(&mut rig, |h, slot, task| {
            override_slot(h, slot, worst, left, false, false);
            let start = h.start(task, StartAt::from(window));
            assert_eq!(start, want);
            assert!(execute(h, task, slot, start));
        });
    }
}
/// T4b: force bypasses fit only. Always-publish leaves the positive force case passing.
#[test]
fn t4b_force_publish_only() {
    for refusal in 0..6 {
        let (mut rig, ..) = fixture(false);
        host(&mut rig, |h, slot, task| {
            override_slot(h, slot, 0, 0, true, false);
            match refusal {
                1 => {
                    h.p1[slot].panic = Some(Box::new("retained"));
                }
                2 => {
                    h.pool.set.mark_suspect(slot);
                }
                3 => {
                    h.pool.away[slot] = true;
                }
                4 => {
                    h.tail.as_mut().unwrap().latency.aborted = true;
                }
                _ => {}
            }
            let window = if refusal == 5 { Window::Closed } else { Window::Open };
            let start = h.start(task, StartAt::from(window));
            if refusal == 0 {
                assert_eq!(start, Start::Publish);
                assert!(execute(h, task, slot, start));
            } else if refusal == 5 {
                assert_eq!(start, Start::Callback);
            } else {
                assert_eq!(start, Start::Unavailable);
            }
            if refusal == 3 {
                h.pool.away[slot] = false;
            }
        });
    }
    let (mut rig, mut s, aux, _) = fixture(false);
    s.track_mut(aux).unwrap().kind = TrackKind::Audio;
    s.track_mut(aux).unwrap().mixer.input_monitor = true;
    rig.publish(&s);
    host(&mut rig, |h, _, _| {
        let slot = h.p1.iter().position(|st| st.id == aux).unwrap();
        override_slot(h, slot, 0, 0, true, false);
        assert_eq!(h.start(slot, StartAt::from(Window::Open)), Start::Callback, "pinned is never published");
    });
    let (mut rig, ..) = fixture(false);
    rig.eng.pool = Some(StripPool::new(0, 4, false, None, N as u32, 48_000).unwrap());
    host(&mut rig, |h, slot, task| {
        override_slot(h, slot, 0, 0, true, false);
        assert_eq!(h.start(task, StartAt::from(Window::Open)), Start::Callback, "out of capacity is never published");
    });
}

/// Host entry installs and expires pending seams even when the complete block is serial.
#[test]
fn t4c_serial_host_installs_then_expires_overrides_and_captures_before_publication() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut rig, _, aux, _) = fixture(false);
    let slot = rig.eng.strip_index(aux).unwrap();
    let mut reader = rig.eng.render_measurement_reader().unwrap();
    assert!(reader.request(174));
    let value = SlotOverrides { force_publish: true, no_reclaim: true, budget_left: Some(Duration::ZERO) };
    rig.eng.arm_tail_slot(slot, value);
    MixEngine::force_serial(true);
    mark_audio_thread();
    let _ = rig.render(0);
    assert_eq!(rig.eng.pool.as_ref().unwrap().set.slot_overrides(slot), value);
    let snapshot = reader.read().unwrap();
    assert!(snapshot.complete);
    assert_eq!(snapshot.counters.published, 0, "mailbox captured before any new publication");
    MixEngine::force_graph(true);
    MixEngine::force_tail(true);
    MixEngine::force_serial(false);
    rig.eng.set_strip_deadline(Some(FAR));
    rig.eng.seed_strip_costs(Duration::from_nanos(1));
    let _ = rig.render(N as i64);
    assert_eq!(rig.eng.pool.as_ref().unwrap().set.slot_overrides(slot), SlotOverrides::default());
    unmark_audio_thread();
    MixEngine::force_tail(false);
    MixEngine::force_serial(true);
}
/// T4c: no-reclaim alone does not force fit, and cannot suppress the deadline reclaim.
#[test]
fn t4c_no_reclaim_only_and_lifetime() {
    let (mut rig, ..) = fixture(false);
    host(&mut rig, |h, slot, task| {
        override_slot(h, slot, 0, 0, false, true);
        assert_eq!(h.start(task, StartAt::from(Window::Open)), Start::Callback);
        override_slot(h, slot, 10, 10 + M, false, true);
        assert_eq!(h.start(task, StartAt::from(Window::Open)), Start::Publish);
        let _ = h.pool.set.publish(slot).unwrap();
        h.pool.set.set_cost_estimate(0, Duration::from_nanos(1));
        let _ = h.pool.set.publish(0).unwrap();
        assert!(matches!(h.poll(Instant::now() + FAR), transit_rt::worker_set::Poll::Resolved { strip: 0, .. }));
        assert_eq!(h.pool.set.outcome(slot), Some(Outcome::Pending));
        let _ = h.poll(Instant::now());
        assert!(h.take(task, Outcome::Inline));
        assert_eq!(h.pool.set.queue_snapshot().deadline_reclaimed_tail, 1);
        assert!(h.pool.set.begin_block(2));
        assert!(h.pool.set.slot_overrides(slot).no_reclaim);
        h.pool.host_entry(N, 48_000);
        assert_eq!(h.pool.set.slot_overrides(slot), SlotOverrides::default());
    });
}
/// T7: distinct previous/current values, both source kinds, every executing branch. Omit
/// fill_deferred in no-fit or Closed and the captured key and output cease to be 0.75.
#[test]
fn t7_current_keys_in_every_window_and_fit_branch() {
    for bus in [false, true] {
        for (worst, left, window) in [(0, 20, Window::Open), (30, 20 + M, Window::Open), (10, 20 + M, Window::Open), (10, 20, Window::Closed)] {
            let (mut rig, _, _, probe) = fixture(bus);
            host(&mut rig, |h, slot, task| {
                // Fill old keys before changing their actual sources.
                for st in h.p1.iter_mut().take(slot) {
                    for c in &mut st.buf {
                        c.fill(0.125);
                    }
                }
                for (_, b) in &mut h.tail.as_mut().unwrap().busses.0 {
                    for c in &mut b.buf {
                        c.fill(0.125);
                    }
                }
                let buses = &*h.tail.as_ref().unwrap().busses;
                h.keys.fill(slot..slot + 1, |i| h.p1.get(i), buses, N);
                for st in h.p1.iter_mut().take(slot) {
                    for c in &mut st.buf {
                        c.fill(0.75);
                    }
                }
                for (_, b) in &mut h.tail.as_mut().unwrap().busses.0 {
                    for c in &mut b.buf {
                        c.fill(0.75);
                    }
                }
                override_slot(h, slot, worst, left, false, false);
                let start = h.start(task, StartAt::from(window));
                assert!(execute(h, task, slot, start));
                assert_eq!(f32::from_bits(probe.key.load(Ordering::Relaxed)), 0.75);
                assert_eq!(h.p1[slot].buf[0][0], 0.75);
                assert!(!h.keys.any_pending(slot));
            });
        }
    }
}
/// B3: a monitored keyed phase-1 strip renders in Closed with the current key.
#[test]
fn b3_closed_pinned_phase1_prepares_fresh_keys() {
    let (mut rig, mut s, aux, probe) = fixture(false);
    s.track_mut(aux).unwrap().kind = TrackKind::Audio;
    s.track_mut(aux).unwrap().mixer.input_monitor = true;
    rig.publish(&s);
    host(&mut rig, |h, _, _| {
        let slot = h.p1.iter().position(|st| st.id == aux).unwrap();
        assert_eq!(h.start(slot, StartAt::from(Window::Closed)), Start::Callback);
        assert_eq!(h.render_here(slot), Rendered::Usable);
        assert_eq!(f32::from_bits(probe.key.load(Ordering::Relaxed)), 0.75);
        h.p1[slot].panic = Some(Box::new("retained"));
        assert_eq!(h.start(slot, StartAt::from(Window::Closed)), Start::Unavailable);
        h.p1[slot].panic = None;
        h.pool.set.mark_suspect(slot);
        assert_eq!(h.start(slot, StartAt::from(Window::Closed)), Start::Unavailable);
        h.pool.away[slot] = true;
        h.pool.set.mark_busy(slot);
        assert_eq!(h.start(slot, StartAt::from(Window::Closed)), Start::Busy);
        h.pool.away[slot] = false;
        assert_eq!(probe.process.load(Ordering::Relaxed), 1);
    });
}
/// T5b/c/h: captured payloads cannot become results or pass reset/adoption checkpoints.
#[test]
fn t5_render_panic_is_unusable_inline_and_callback() {
    for publish in [false, true] {
        let (mut rig, _, _, probe) = fixture(false);
        probe.panic.store(1, Ordering::Relaxed);
        host(&mut rig, |h, slot, task| {
            override_slot(h, slot, 1, 1, publish, false);
            let start = h.start(task, StartAt::from(if publish { Window::Open } else { Window::Closed }));
            assert!(!execute(h, task, slot, start));
            assert!(h.p1[slot].panic.is_some());
            assert!(h.pool.silent[slot]);
            h.start(task + 1, StartAt::from(Window::Closed));
            assert!(h.tail.as_ref().unwrap().main.iter().flatten().all(|x| *x == 0.0));
        });
    }
}
/// T5f: MIX catches latency/parameter panics and aborts before any routing.
fn finish_aborted_drive(h: &mut Host<'_, '_>) {
    use transit_rt::task_graph::{GraphRun, TaskState, drive};
    let graph = h.graph;
    let mut run = GraphRun::new(graph.tasks());
    drive(h, graph, &mut run, Instant::now());
    assert!((0..graph.tasks()).all(|t| !matches!(run.state(t), None | Some(TaskState::Waiting))));
    assert!(h.pool.set.outcomes().iter().all(|o| *o != Outcome::Pending));
    assert!(h.tail.as_ref().unwrap().main.iter().flatten().all(|x| *x == 0.0));
}

/// T5f: MIX catches latency/parameter panics and aborts before any routing.
#[test]
fn t5f_mix_guards_preparation() {
    for panic in [2, 3] {
        let (mut rig, _, _, probe) = fixture(false);
        probe.panic.store(panic, Ordering::Relaxed);
        host(&mut rig, |h, slot, task| {
            assert_eq!(h.start(slot, StartAt::from(Window::Closed)), Start::Ready);
            assert!(h.tail.as_ref().unwrap().latency.aborted);
            assert!(h.p1[slot].panic.is_some());
            assert_eq!(h.start(task, StartAt::from(Window::Closed)), Start::Unavailable);
            finish_aborted_drive(h);
        });
    }
}
/// T17: MIX must not query latency or apply parameters to a suspect or away spare.
#[test]
fn t17_mix_keeps_away_latency_and_does_not_call_suspect() {
    for away in [false, true] {
        let (mut rig, _, _, probe) = fixture(false);
        host(&mut rig, |h, slot, task| {
            h.p1[slot].latency = 123;
            if away {
                h.pool.away[slot] = true;
                h.pool.set.mark_busy(slot);
            } else {
                h.pool.set.mark_suspect(slot);
            }
            let before = (probe.params.load(Ordering::Relaxed), probe.latency.load(Ordering::Relaxed));
            h.start(slot, StartAt::from(Window::Closed));
            assert_eq!(h.p1[slot].latency, 123);
            assert_eq!((probe.params.load(Ordering::Relaxed), probe.latency.load(Ordering::Relaxed)), before);
            assert_eq!(h.start(task, StartAt::from(Window::Closed)), if away { Start::Busy } else { Start::Unavailable });
            h.pool.away[slot] = false;
        });
    }
}
/// Removing reset's checkpoint would reset a processor that still owns this payload.
#[test]
fn t5h_direct_reset_checkpoint() {
    let (mut rig, _, aux, probe) = fixture(false);
    rig.eng.pool = None;
    let at = rig.eng.strip_index(aux).unwrap();
    probe.panic.store(1, Ordering::Relaxed);
    guarded(&mut rig.eng.plan.as_deref_mut().unwrap().tables.strips[at], |st| {
        st.plugins[0].as_mut().unwrap().plugin.process(&mut st.buf, N);
    });
    let before = probe.reset.load(Ordering::Relaxed);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.reset())).is_err());
    assert_eq!(probe.reset.load(Ordering::Relaxed), before);
    assert_eq!(probe.notes_off.load(Ordering::Relaxed), 0);
}
/// Removing adoption's checkpoint would separate the payload from its moved processor.
#[test]
fn t5h_direct_adoption_checkpoint_and_pool_order() {
    for pool in [false, true] {
        let (mut rig, mut s, aux, probe) = fixture(false);
        if !pool {
            rig.eng.pool = None;
        }
        let at = rig.eng.strip_index(aux).unwrap();
        if !pool {
            probe.panic.store(1, Ordering::Relaxed);
            guarded(&mut rig.eng.plan.as_deref_mut().unwrap().tables.strips[at], |st| {
                st.plugins[0].as_mut().unwrap().plugin.process(&mut st.buf, N);
            });
        }
        s.track_mut(aux).unwrap().mixer.output = Route::None;
        let mut make = |spec: &InstanceSpec, _: SlotGen| crate::create_instance(&spec.id, None, 48_000.0, N, spec.ch);
        let mut env = rig.comp.compile(Arc::new(s), rig.comp.active(), PlanGeneration::from_raw(2), &mut make).unwrap();
        assert!(!env.plan.moves.is_empty(), "the routing offer really moves the processor");
        let generation = rig.eng.plan.as_deref().unwrap().tables.strips[at].plugins[0].as_ref().unwrap().generation;
        let before = (
            probe.process.load(Ordering::Relaxed),
            probe.reset.load(Ordering::Relaxed),
            probe.params.load(Ordering::Relaxed),
            probe.latency.load(Ordering::Relaxed),
        );
        crate::sched_seam::checkpoint_observer(true);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.adopt_plan(&mut env)));
        let marks = crate::sched_seam::checkpoint_observer(false);
        if pool {
            assert_eq!(result.unwrap(), Adoption::Adopted);
            assert_eq!(&marks[..3], [1, 2, 3]);
        } else {
            assert!(result.is_err());
            assert_eq!(marks, [2]);
            assert_eq!(rig.eng.active().generation, PlanGeneration::from_raw(1));
            assert_eq!(env.state(), EnvelopeState::Pending);
            assert!(!env.committing);
            assert_eq!(rig.eng.plan.as_deref().unwrap().tables.strips[at].plugins[0].as_ref().unwrap().generation, generation);
            assert_eq!(
                (
                    probe.process.load(Ordering::Relaxed),
                    probe.reset.load(Ordering::Relaxed),
                    probe.params.load(Ordering::Relaxed),
                    probe.latency.load(Ordering::Relaxed)
                ),
                before
            );
        }
        rig.comp.reclaim(env);
    }
}
/// T18: callback renders seed both cost figures without external seeding; the next fit publishes.
#[test]
fn t18_callback_cost_warms_a_cold_aux() {
    let (mut rig, ..) = fixture(false);
    host(&mut rig, |h, slot, task| {
        h.pool.set.arm_cost_clock(CostClock::Fixed(Duration::from_nanos(6400)));
        h.pool.set.install_test_seams();
        let start = h.start(task, StartAt::from(Window::Open));
        assert_eq!(start, Start::Callback);
        assert!(execute(h, task, slot, start));
        assert_eq!(h.pool.set.cost_estimate(slot), Some(Duration::from_nanos(6400)));
        assert_eq!(h.pool.set.worst_case(slot), Some(Duration::from_nanos(6400)));
        assert_eq!(h.pool.set.queue_snapshot().tail_callback, 1);
        h.pool.set.arm_slot_overrides(slot, SlotOverrides { budget_left: Some(Duration::from_nanos(6400 + M)), ..SlotOverrides::default() });
        h.pool.set.install_test_seams();
        let start = h.start(task, StartAt::from(Window::Open));
        assert_eq!(start, Start::Publish);
        assert!(execute(h, task, slot, start));
    });
}

/// T5a/d/e/g/h: a real worker captures a payload after a Late block. Settlement must
/// clean up every input before resuming it, before pending reset/adoption or output.
/// timing: held-worker the aux hold and deterministic deadline force Late, never a sleep.
#[test]
fn t5_held_worker_panic_settlement_adoption_and_reset() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for serial in [false, true] {
        for action in 0..4 {
            let (mut rig, mut s, aux, probe) = fixture(false);
            rig.eng.pool = Some(StripPool::new(1, 16, false, None, N as u32, 48_000).unwrap());
            let slot = rig.eng.strip_index(aux).unwrap();
            s.track_mut(aux).unwrap().mixer.output = Route::None;
            let mut make = |spec: &InstanceSpec, _: SlotGen| crate::create_instance(&spec.id, None, 48_000.0, N, spec.ch);
            let mut env = rig.comp.compile(Arc::new(s), rig.comp.active(), PlanGeneration::from_raw(2), &mut make).unwrap();
            MixEngine::force_graph(true);
            MixEngine::force_tail(true);
            MixEngine::force_serial(false);
            rig.eng.set_strip_deadline(Some(FAR));
            probe.hold.store(true, Ordering::Release);
            probe.panic.store(1, Ordering::Relaxed);
            rig.eng.arm_tail_slot(slot, SlotOverrides { force_publish: true, no_reclaim: true, ..Default::default() });
            rig.eng.hold_until_alone(&[slot]);
            mark_audio_thread();
            let mut output = vec![vec![0.0; N]; 2];
            rig.eng.render_active(0, N, &mut output);
            let entered = Instant::now();
            // timing: stall-bound waits for an explicitly held worker; this is only a deadlock watchdog
            while !probe.entered.load(Ordering::Acquire) {
                assert!(entered.elapsed() < stall::HANG, "claimed worker never entered its hold");
                std::thread::yield_now();
            }
            assert!(probe.entered.load(Ordering::Acquire));
            assert!(rig.eng.strip_claimed(slot));
            rig.eng.reset();
            rig.eng.pool.as_mut().unwrap().mark_away(PENDING_SEEK);
            assert!(rig.eng.pool.as_ref().unwrap().pending[slot] & PENDING_RESET != 0);
            probe.hold.store(false, Ordering::Release);
            let wait = Instant::now();
            // timing: stall-bound waits for an explicitly held worker; this is only a deadlock watchdog
            while rig.eng.strip_claimed(slot) {
                assert!(wait.elapsed() < stall::HANG);
                std::thread::yield_now();
            }
            let before = (probe.reset.load(Ordering::Relaxed), probe.latency.load(Ordering::Relaxed), probe.params.load(Ordering::Relaxed));
            MixEngine::force_serial(serial);
            let mut returned_normally = false;
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match action {
                    0 => {
                        rig.eng.render_workers_settled();
                    }
                    1 => {
                        rig.eng.adopt_plan(&mut env);
                    }
                    2 => {
                        rig.eng.render_workers_settled();
                        rig.eng.reset();
                    }
                    _ => {
                        rig.eng.render_active(N as i64, N, &mut output);
                    }
                }
                returned_normally = true;
            }));
            assert!(caught.is_err());
            assert!(!returned_normally);
            assert!(!rig.eng.pool.as_ref().unwrap().is_away(slot));
            assert_eq!((probe.reset.load(Ordering::Relaxed), probe.latency.load(Ordering::Relaxed), probe.params.load(Ordering::Relaxed)), before);
            let plan = rig.eng.plan.as_deref_mut().unwrap();
            let mut input = BusTable(Vec::with_capacity(1));
            assert!(plan.tables.inputs.lend(slot, &mut input), "snapshot was taken home before unwind");
            assert_eq!(input.0.len(), 1);
            plan.tables.inputs.take_back(slot, &mut input);
            assert!(output.iter().flatten().all(|x| *x == 0.0));
            assert_eq!(rig.eng.active().generation, PlanGeneration::from_raw(1));
            unmark_audio_thread();
            drop(rig.eng.into_retired());
        }
    }
    MixEngine::force_tail(false);
    MixEngine::force_serial(true);
}

/// Removing settlement's checkpoint, or applying another strip's reset before it, fails.
#[test]
fn t5g_settlement_cleans_every_owner_before_any_processor_operation() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for entry in [false, true] {
        let (mut rig, mut s, aux, probe) = fixture(false);
        let second = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("second"));
        s.track_mut(second).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
        rig.fakes.push((second, 0, Box::new(echo(&probe))));
        rig.publish(&s);
        let plan = rig.eng.plan.as_deref_mut().unwrap();
        let first = plan.tables.strip_of(aux).unwrap();
        let second = plan.tables.strip_of(second).unwrap();
        assert_ne!(first, second, "the cleanup witness is a distinct owner");
        let pool = rig.eng.pool.as_mut().unwrap();
        let before = probe.reset.load(Ordering::Relaxed);
        for slot in [first, second] {
            let st = &mut plan.tables.strips[slot];
            pool.cells.with(slot, |job| {
                send_away(st, &mut job.strip);
                if slot == first {
                    job.strip.panic = Some(Box::new("settled payload"));
                }
            });
            pool.away[slot] = true;
            pool.pending[slot] = PENDING_RESET;
        }
        let caught = if entry {
            MixEngine::force_serial(false);
            MixEngine::force_graph(true);
            MixEngine::force_tail(true);
            rig.eng.set_strip_deadline(Some(FAR));
            rig.eng.seed_strip_costs(Duration::from_nanos(1));
            mark_audio_thread();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.render(0)));
            unmark_audio_thread();
            MixEngine::force_tail(false);
            MixEngine::force_serial(true);
            result.map(|_| ())
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.settle(&mut plan.tables.strips, &mut plan.tables.keys, &mut plan.tables.inputs, &mut rig.eng.events, &plan.session, N);
            }))
        };
        let pool = rig.eng.pool.as_ref().unwrap();
        assert!(caught.is_err());
        assert!(!pool.away[first] && !pool.away[second]);
        assert_eq!(pool.pending[first], 0);
        assert_eq!(probe.reset.load(Ordering::Relaxed), before);
    }
}
/// T8: table slot 4 changes from aux to phase 1; its old deadline-inline permission expires.
#[test]
fn t8_aux_to_phase1_clears_deadline_reclaim_flag() {
    let (mut rig, mut s, aux, _) = fixture(false);
    let original = rig.eng.strip_index(aux).unwrap();
    host(&mut rig, |h, slot, task| {
        override_slot(h, slot, 1, 1, true, true);
        let start = h.start(task, StartAt::from(Window::Open));
        assert!(execute(h, task, slot, start));
    });
    s.track_mut(aux).unwrap().kind = TrackKind::Audio;
    rig.publish(&s);
    assert_eq!(rig.eng.strip_index(aux), Some(original));
    host(&mut rig, |h, _, _| {
        override_slot(h, original, 3_600_000_000_000, 0, false, true);
        assert_eq!(h.start(original, StartAt::from(Window::Open)), Start::Publish);
        let _ = h.pool.set.publish(original).unwrap();
        let _ = h.poll(Instant::now());
        assert_eq!(h.pool.set.outcome(original), Some(Outcome::Overrun(OverrunCause::Unstarted)));
        h.refused(original);
    });
}

/// T9: capacity recovery never processes a spare or fills a dependent from its stale buffer.
#[test]
fn t9_capacity_recovery_with_an_away_phase1_source() {
    let (mut rig, mut s, aux, probe) = fixture(false);
    s.track_mut(aux).unwrap().kind = TrackKind::Audio;
    rig.publish(&s);
    rig.eng.pool = Some(StripPool::new(0, 1, false, None, N as u32, 48_000).unwrap());
    let pool = rig.eng.pool.as_mut().unwrap();
    let st = &mut rig.eng.plan.as_deref_mut().unwrap().tables.strips[0];
    st.latency = 123;
    pool.cells.with(0, |job| send_away(st, &mut job.strip));
    pool.away[0] = true;
    let slot = rig.eng.strip_index(aux).unwrap();
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_graph(true);
    MixEngine::force_tail(true);
    MixEngine::force_serial(false);
    rig.eng.set_strip_deadline(Some(FAR));
    mark_audio_thread();
    let out = rig.render(0);
    unmark_audio_thread();
    assert_eq!(probe.process.load(Ordering::Relaxed), 0);
    assert_eq!(rig.eng.plan.as_deref().unwrap().tables.strips[0].latency, 123, "spare was not processed");
    assert!(rig.eng.plan.as_deref().unwrap().tables.graph.recovery_skipped[slot]);
    assert!(out.iter().flatten().all(|x| *x == 0.0));
    let pool = rig.eng.pool.as_mut().unwrap();
    assert!(pool.bring_home(0, &mut rig.eng.plan.as_deref_mut().unwrap().tables.strips[0], &rig.eng.events));
    MixEngine::force_tail(false);
    MixEngine::force_serial(true);
}

/// T5h pool-present order: parked MIDI really moves into the instrument after both checkpoints.
#[test]
fn t5h_pool_adoption_orders_checkpoints_before_parked_delivery() {
    let mut s = crate::tests::synth_session(-6.0);
    let id = s.tracks[0].id;
    let mut rig = PlanRig::new(N);
    rig.eng.events.epoch = Some(EngineEpoch(1));
    rig.publish(&s);
    rig.eng.pool = Some(StripPool::new(0, 8, false, None, N as u32, 48_000).unwrap());
    let slot = rig.eng.strip_index(id).unwrap();
    let generation = rig.eng.plan.as_deref().unwrap().tables.strips[slot].instrument.as_ref().unwrap().generation;
    let event = InstrumentEvent {
        engine: EngineEpoch(1),
        target: (id, INSTRUMENT_SLOT, generation),
        due: Due::Next,
        transport: None,
        source: Source { port: 0, channel: 0, generation: 1 },
        message: Message::NoteOn { key: 62, velocity: 100, id: transit_dsp::event::NoteId::NONE },
        arrival: Arrival { host_ns: 0, sequence: 1 },
    };
    let pool = rig.eng.pool.as_mut().unwrap();
    assert_eq!(pool.park(slot, &event, &rig.eng.events, N, Some(Some(generation))).0, EventOutcome::Queued);
    assert!(pool.has_parked(slot));
    let queued = |rig: &PlanRig| {
        use std::hash::Hasher;
        let mut hash = std::hash::DefaultHasher::new();
        rig.eng.plan.as_deref().unwrap().tables.strips[slot].instrument.as_ref().unwrap().events.as_deref().unwrap().fingerprint(&mut hash);
        hash.finish()
    };
    let before = queued(&rig);
    s.track_mut(id).unwrap().mixer.volume_db = -3.0;
    crate::sched_seam::checkpoint_observer(true);
    assert_eq!(rig.try_publish(&s), Adoption::Adopted);
    assert_eq!(crate::sched_seam::checkpoint_observer(false).into_iter().filter(|&m| m <= 3).collect::<Vec<_>>(), [1, 2, 3]);
    assert!(!rig.eng.pool.as_ref().unwrap().has_parked(slot));
    assert_ne!(queued(&rig), before, "parked event reached the instrument queue");
}

/// T5a/b/c/e: the drive releases soft readers but rejects a panicked hard key source.
/// timing: far-deadline worker results use FAR; zero-worker reclaim uses the injected deadline.
#[test]
fn t5_panicked_results_fail_hard_keys_but_release_soft_readers() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for mode in 0..3 {
        let (mut rig, mut s, aux, source) = fixture(false);
        let bus = s.track(aux).unwrap().mixer.input.clone();
        let keyed = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("hard reader"));
        let soft = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("sum reader"));
        for id in [keyed, soft] {
            let t = s.track_mut(id).unwrap();
            t.mixer.input = bus.clone();
            let mut ins = Insert::new("gain");
            if id == keyed {
                ins.key = Some(KeySource { from: KeyFrom::Track(aux), tap: Tap::Post });
            }
            t.mixer.inserts[0] = Some(ins);
        }
        let hard_probe = Arc::new(Probe::default());
        let soft_probe = Arc::new(Probe::default());
        rig.fakes.push((keyed, 0, Box::new(echo(&hard_probe))));
        rig.fakes.push((soft, 0, Box::new(echo(&soft_probe))));
        rig.publish(&s);
        let slot = rig.eng.strip_index(aux).unwrap();
        rig.eng.pool = Some(StripPool::new(usize::from(mode == 0), 16, false, None, N as u32, 48_000).unwrap());
        MixEngine::force_graph(true);
        MixEngine::force_tail(true);
        MixEngine::force_serial(false);
        rig.eng.set_strip_deadline(Some(FAR));
        rig.eng.seed_strip_costs(Duration::from_nanos(1));
        if mode == 2 {
            rig.eng.set_strip_cost(slot, Duration::ZERO);
        } else {
            rig.eng.arm_tail_slot(slot, SlotOverrides { force_publish: true, no_reclaim: true, ..Default::default() });
        }
        if mode == 1 {
            rig.eng.hold_until_alone(&[slot]);
        }
        source.panic.store(1, Ordering::Relaxed);
        mark_audio_thread();
        let mut output = vec![vec![123.0; N]; 2];
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.eng.render_active(0, N, &mut output)));
        unmark_audio_thread();
        assert!(caught.is_err());
        assert_eq!(hard_probe.process.load(Ordering::Relaxed), 0);
        assert_eq!(soft_probe.process.load(Ordering::Relaxed), 1);
        assert!(output.iter().flatten().all(|x| *x == 123.0), "no output published before checkpoint");
        assert!(rig.eng.pool.as_ref().unwrap().set.outcomes().iter().all(|o| *o != Outcome::Pending));
        drop(rig.eng.into_retired());
    }
    // Slice 1 uses the same take result check, with a real worker's panicked phase-1 source.
    let (mut rig, mut s, aux, dependent) = fixture(false);
    let source_id = s.tracks[0].id;
    s.track_mut(source_id).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    s.track_mut(aux).unwrap().kind = TrackKind::Audio;
    let source = Arc::new(Probe::default());
    rig.fakes.push((source_id, 0, Box::new(echo(&source))));
    rig.publish(&s);
    rig.eng.pool = Some(StripPool::new(1, 16, false, None, N as u32, 48_000).unwrap());
    MixEngine::force_tail(false);
    rig.eng.set_strip_deadline(Some(FAR));
    source.panic.store(1, Ordering::Relaxed);
    mark_audio_thread();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.render(0)));
    unmark_audio_thread();
    assert!(caught.is_err());
    assert_eq!(dependent.process.load(Ordering::Relaxed), 0);
    MixEngine::force_serial(true);
    drop(rig.eng.into_retired());
}

/// T5f master latency is guarded by MIX just like aux latency.
#[test]
fn capacity_recovery_propagates_a_new_panic_to_hard_dependents() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut rig, mut s, aux, dependent) = fixture(false);
    let source_id = s.tracks[0].id;
    s.track_mut(source_id).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    s.track_mut(aux).unwrap().kind = TrackKind::Audio;
    let source = Arc::new(Probe::default());
    rig.fakes.push((source_id, 0, Box::new(echo(&source))));
    rig.publish(&s);
    rig.eng.pool = Some(StripPool::new(0, 1, false, None, N as u32, 48_000).unwrap());
    MixEngine::force_graph(true);
    MixEngine::force_serial(false);
    source.panic.store(1, Ordering::Relaxed);
    mark_audio_thread();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rig.render(0)));
    unmark_audio_thread();
    assert!(caught.is_err());
    assert_eq!(source.process.load(Ordering::Relaxed), 1);
    assert_eq!(dependent.process.load(Ordering::Relaxed), 0);
    MixEngine::force_serial(true);
    drop(rig.eng.into_retired());
}

/// Oversized keyed recovery must drain real deferred MIDI for a home instrument.
#[test]
fn capacity_recovery_delivers_parked_home_midi() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = crate::tests::synth_session(-6.0);
    let id = s.tracks[0].id;
    s.tracks[0].playlist_mut().unwrap().clips.clear();
    for _ in 0..4 {
        s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    }
    let dependent = s.tracks.last_mut().unwrap();
    let mut key = Insert::new("gain");
    key.key = Some(KeySource { from: KeyFrom::Track(id), tap: Tap::Post });
    dependent.mixer.inserts[0] = Some(key);
    let mut rig = PlanRig::new(N);
    rig.eng.events.epoch = Some(EngineEpoch(1));
    rig.publish(&s);
    let slot = rig.eng.strip_index(id).unwrap();
    assert!(rig.eng.plan.as_deref().unwrap().tables.phase1 > slot + 1, "fixture exceeds the pool");
    rig.eng.pool = Some(StripPool::new(0, slot + 1, false, None, N as u32, 48_000).unwrap());
    let generation = rig.eng.plan.as_deref().unwrap().tables.strips[slot].instrument.as_ref().unwrap().generation;
    let event = InstrumentEvent {
        engine: EngineEpoch(1),
        target: (id, INSTRUMENT_SLOT, generation),
        due: Due::Next,
        transport: None,
        source: Source { port: 0, channel: 0, generation: 1 },
        message: Message::NoteOn { key: 62, velocity: 100, id: transit_dsp::event::NoteId::NONE },
        arrival: Arrival { host_ns: 0, sequence: 1 },
    };
    let pool = rig.eng.pool.as_mut().unwrap();
    assert_eq!(pool.park(slot, &event, &rig.eng.events, N, Some(Some(generation))).0, EventOutcome::Queued);
    assert!(pool.has_parked(slot));
    MixEngine::force_graph(true);
    MixEngine::force_serial(false);
    mark_audio_thread();
    let out = rig.render(0);
    unmark_audio_thread();
    MixEngine::force_serial(true);
    assert!(!rig.eng.pool.as_ref().unwrap().has_parked(slot));
    assert!(out.iter().flatten().any(|x| x.abs() > 0.0), "parked note reached the synth");
}

/// T5f master latency is guarded by MIX just like aux latency.
#[test]
fn t5f_mix_retains_a_master_latency_panic() {
    let (mut rig, mut s, _, _) = fixture(false);
    let master = s.add_track(TrackKind::Master, ChannelFormat::Mono, Some("master"));
    s.track_mut(master).unwrap().mixer.inserts[0] = Some(Insert::new("gain"));
    let probe = Arc::new(Probe::default());
    rig.fakes.push((master, 0, Box::new(echo(&probe))));
    rig.publish(&s);
    probe.panic.store(2, Ordering::Relaxed);
    host(&mut rig, |h, slot, task| {
        h.start(slot, StartAt::from(Window::Closed));
        assert!(h.tail.as_ref().unwrap().latency.aborted);
        assert!(h.tail.as_ref().unwrap().others.iter().any(|st| st.panic.is_some()));
        assert_eq!(h.start(task, StartAt::from(Window::Closed)), Start::Unavailable);
        finish_aborted_drive(h);
    });
}

/// T19: a held latent aux prevents adoption; once home it uses the new chain, input and graph.
/// timing: held-worker the real worker is released through its hold signal, not elapsed time.
#[test]
fn t19_held_aux_chain_adoption_uses_fresh_input_and_latency() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for published in [false, true] {
        let (mut rig, mut s, aux, old) = fixture(true);
        let feed = crate::tests::add_dc_track(&mut s, 0.25, 48_000);
        s.track_mut(feed).unwrap().mixer.output = s.track(aux).unwrap().mixer.input.clone();
        rig.publish(&s);
        rig.eng.pool = Some(StripPool::new(1, 16, false, None, N as u32, 48_000).unwrap());
        let slot = rig.eng.strip_index(aux).unwrap();
        let old_graph = rig.eng.plan.as_deref().unwrap().tables.graph.tail.graph.generation();
        let new = Arc::new(Probe::default());
        new.latency_value.store(31, Ordering::Relaxed);
        let fresh = Arc::clone(&new);
        s.track_mut(aux).unwrap().mixer.inserts[0].as_mut().unwrap().uid = Default::default();
        // Change the source fader too; mono bus routing retains the centered -3 dB pan law.
        s.track_mut(feed).unwrap().mixer.volume_db = 6.0206;
        let mut make = move |spec: &InstanceSpec, _: SlotGen| {
            if spec.track == aux {
                Some(NewInstance { plugin: Box::new(echo(&fresh)), refused_state: false })
            } else {
                crate::create_instance(&spec.id, None, 48_000.0, N, spec.ch)
            }
        };
        let mut env = rig.comp.compile(Arc::new(s), rig.comp.active(), PlanGeneration::from_raw(3), &mut make).unwrap();
        MixEngine::force_serial(false);
        MixEngine::force_graph(true);
        MixEngine::force_tail(true);
        rig.eng.set_strip_deadline(Some(FAR));
        old.hold.store(true, Ordering::Release);
        rig.eng.arm_tail_slot(slot, SlotOverrides { force_publish: true, no_reclaim: true, ..Default::default() });
        rig.eng.hold_until_alone(&[slot]);
        mark_audio_thread();
        let _ = rig.render(0);
        let wait = Instant::now();
        // timing: stall-bound waits for an explicitly held worker; this is only a deadlock watchdog
        while !old.entered.load(Ordering::Acquire) {
            assert!(wait.elapsed() < stall::HANG);
            std::thread::yield_now();
        }
        assert_eq!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(RejectReason::StripsInFlight));
        old.hold.store(false, Ordering::Release);
        let wait = Instant::now();
        // timing: stall-bound waits for an explicitly held worker; this is only a deadlock watchdog
        while rig.eng.strip_claimed(slot) {
            assert!(wait.elapsed() < stall::HANG);
            std::thread::yield_now();
        }
        assert!(rig.eng.render_workers_settled());
        // Rejected envelopes are immutable; a fresh offer has the same planned new chain.
        rig.comp.reclaim(env);
        rig.issued = 3;
        let mut s = rig.eng.plan.as_deref().unwrap().session.as_ref().clone();
        s.track_mut(aux).unwrap().mixer.inserts[0].as_mut().unwrap().uid = Default::default();
        s.track_mut(feed).unwrap().mixer.volume_db = 6.0206;
        rig.fakes.push((aux, 0, Box::new(echo(&new))));
        rig.publish(&s);
        assert_ne!(old_graph, rig.eng.plan.as_deref().unwrap().tables.graph.tail.graph.generation());
        rig.eng.clock_deadline();
        // Exercise synchronous plan input and a new lent snapshot independently.
        rig.eng.seed_strip_costs(Duration::from_nanos(1));
        rig.eng.arm_tail_slot(slot, SlotOverrides { budget_left: Some(Duration::ZERO), force_publish: published, ..Default::default() });
        let _ = rig.render(N as i64);
        assert_eq!(rig.eng.plan.as_deref().unwrap().tables.strips[slot].latency, 31);
        assert_eq!(new.process.load(Ordering::Relaxed), 1);
        let expected = 0.25 * db_to_gain(6.0206) * db_to_gain(-3.0);
        assert!((f32::from_bits(new.key.load(Ordering::Relaxed)) - expected).abs() < 1e-6, "new input, not the held snapshot");
        assert!((f32::from_bits(new.input.load(Ordering::Relaxed)) - expected).abs() < 1e-6, "new signal snapshot");
        unmark_audio_thread();
        MixEngine::force_tail(false);
        MixEngine::force_serial(true);
        drop(rig.eng.into_retired());
    }
}

/// G11: a lone ready aux (the one return of a small session, ready after MIX with no other render
/// pending or queued) is rendered on the callback, never published: publishing it only woke a
/// worker for the callback to take it back. Four phase-1 strips are published each block, the aux
/// is a tail callback render each block, and the callback makes one wake call per block (the
/// phase-1 batch), not two. Zero workers, a 60 s deadline. Mutation: publishing a lone aux makes
/// `published` five a block and `tail_callback` zero.
#[test]
fn g11_a_lone_ready_aux_renders_on_the_callback_without_a_publish_or_a_wake() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut rig, ..) = fixture(false);
    MixEngine::force_graph(true);
    MixEngine::force_tail(true);
    MixEngine::force_serial(false);
    rig.eng.set_strip_deadline(Some(FAR));
    rig.eng.seed_strip_costs(Duration::from_nanos(1));
    mark_audio_thread();
    let blocks = 6u64;
    for b in 0..blocks {
        let _ = rig.render(b as i64 * N as i64);
    }
    unmark_audio_thread();
    MixEngine::force_tail(false);
    MixEngine::force_serial(true);
    let pool = rig.eng.pool.as_ref().unwrap();
    assert!(pool.last_tail, "the blocks went through the tail graph");
    let q = pool.set.queue_snapshot();
    assert_eq!((q.published, q.tail_callback), (4 * blocks, blocks), "the lone aux is never published");
    assert!(q.conserves());
    assert_eq!(pool.set.stats().wakes, blocks, "one wake call a block, for the phase-1 batch");
}

/// #293 item 5: when `take` cannot read the job's render stamp (the cell's lock refused), the
/// result fails safe and is counted as a protocol conflict, not as a stale result. Mutation:
/// reading the stamp into a default and comparing it counts `stale` instead.
#[test]
fn take_counts_a_refused_stamp_read_as_a_conflict_not_as_stale() {
    let (mut rig, ..) = fixture(false);
    host(&mut rig, |h, slot, task| {
        let cells = Arc::clone(&h.pool.cells);
        let (stale, conflicts) = (h.pool.counters.stale.load(Ordering::Relaxed), h.pool.counters.conflicts.load(Ordering::Relaxed));
        let guard = cells.0[slot].lock().unwrap();
        assert!(!h.take(task, Outcome::Inline), "no usable result without its stamp");
        drop(guard);
        assert_eq!(h.pool.counters.stale.load(Ordering::Relaxed) - stale, 0, "not a stale result");
        // Two refusals of the held lock, both conflicts: the key take-back of `home` and the stamp read.
        assert_eq!(h.pool.counters.conflicts.load(Ordering::Relaxed) - conflicts, 2, "a refused lock is a conflict");
        assert!(h.pool.silent[slot]);
    });
}
