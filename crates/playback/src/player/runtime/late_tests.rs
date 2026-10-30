//! Issue #82 (escape E48): a hosted instance that is made after the request stopped waiting for
//! it is adopted, never discarded; the slot reads "Not responding" only at the plug-in host's
//! creation budget; Retry makes a not-responding slot's plug-in again with its stored state.
//!
//! The instances are the built-in synth standing in for a hosted plug-in, made by a factory that
//! can hold its creation until the test lets go (a heavy instrument on a loaded machine) on a
//! real plug-in host thread. The display budget runs on a manual clock where a test needs exact
//! timing; the host's budgets are short real ones.
#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::player::{Clock, Factory, PluginTestHooks, SlotLoad};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};
use transit_dsp::{Plugin, PluginInfo};
use transit_model::{ChannelFormat, Insert, TrackId, TrackKind};
use transit_plugin_host::{Config, Host};

const SLOT: usize = transit_mix::INSTRUMENT_SLOT;
const SYNTH: &str = "macro_oscillator";
/// The hosted id the session names (a third-party id, so the session's stored state reaches the
/// factory); the factory makes the built-in synth for it.
const HOSTED: &str = "clap:transit.test.late82";

fn target() -> RenderTarget {
    RenderTarget { device: transit_mix::DeviceGeneration(1), sample_rate: 48_000, max_block: 512, out_channels: 0, in_channels: 0 }
}

/// A clock the test moves by hand.
#[derive(Clone)]
struct ManualClock {
    base: Instant,
    offset_ms: Arc<AtomicU64>,
}

impl ManualClock {
    fn new() -> ManualClock {
        ManualClock { base: Instant::now(), offset_ms: Arc::new(AtomicU64::new(0)) }
    }
    fn clock(&self) -> Clock {
        let c = self.clone();
        Arc::new(move || c.base + Duration::from_millis(c.offset_ms.load(Ordering::SeqCst)))
    }
    fn advance(&self, by: Duration) {
        self.offset_ms.fetch_add(u64::try_from(by.as_millis()).unwrap(), Ordering::SeqCst);
    }
}

/// The factory's record and its gate: the first `hold` creations wait until the gate opens.
#[derive(Default)]
struct Gate {
    calls: AtomicUsize,
    /// Instances the factory made that were destroyed.
    drops: Arc<AtomicUsize>,
    states: Mutex<Vec<Option<Vec<u8>>>>,
    /// A second gate the factory waits at once it reported the restoring phase (issue #67).
    mid: Mutex<Option<Arc<Gate>>>,
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn open(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.opened.wait(open).unwrap();
        }
    }
}

/// Opens the gate when the test ends, so held creations return and their threads finish.
struct OpenOnDrop(Arc<Gate>);
impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// The synth, reporting the state it was made with (so a capture shows what was restored).
struct Echo {
    inner: Box<dyn Plugin>,
    state: Option<Vec<u8>>,
    drops: Arc<AtomicUsize>,
}
impl Drop for Echo {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
impl Plugin for Echo {
    fn info(&self) -> &'static PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, rate: f32, frames: usize, channels: usize) {
        self.inner.prepare(rate, frames, channels);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, data: &mut [Vec<f32>], frames: usize) {
        self.inner.process(data, frames);
    }
    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.inner.note_on(offset, note, velocity);
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.inner.note_off(offset, note);
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.inner.midi(offset, msg);
    }
    fn event(&mut self, offset: u32, ev: &transit_dsp::event::PluginEvent) -> bool {
        self.inner.event(offset, ev)
    }
    fn state_reader(&mut self) -> Option<Box<dyn transit_dsp::PluginStateReader>> {
        let state = self.state.clone();
        Some(Box::new(move || state.clone()))
    }
}

/// A factory whose first `hold` calls wait for the gate.
fn gated(gate: &Arc<Gate>, hold: usize) -> Factory {
    let gate = Arc::clone(gate);
    Arc::new(move |spec: &transit_mix::InstanceSpec| {
        let n = gate.calls.fetch_add(1, Ordering::SeqCst);
        gate.states.lock().unwrap().push(spec.state.clone());
        if n < hold {
            gate.wait();
        }
        transit_plugin_host::report_phase(transit_plugin_host::LoadPhase::Restoring);
        let mid = gate.mid.lock().unwrap().clone();
        if let Some(mid) = mid {
            mid.calls.fetch_add(1, Ordering::SeqCst);
            mid.wait();
        }
        let made = transit_mix::create_instance(SYNTH, None, 48_000.0, 512, spec.ch)?;
        Some(NewInstance {
            plugin: Box::new(Echo { inner: made.plugin, state: spec.state.clone(), drops: Arc::clone(&gate.drops) }),
            refused_state: false,
        })
    })
}

struct Rig {
    runtime: RuntimeControl<u8, u8>,
    ports: AudioPorts<u8, u8>,
    tracks: Vec<TrackId>,
    gate: Arc<Gate>,
    open: OpenOnDrop,
}

/// `n` instrument tracks whose synths (stored state `state`) are requested after the first plan,
/// made by a factory that holds its first `hold` creations, on a host with `config`; the display
/// budget is `slow_after` on `clock`.
fn rig(n: usize, state: Option<&[u8]>, config: Config, clock: Clock, slow_after: Duration, hold: usize) -> Rig {
    let mut session = Session::default();
    let tracks: Vec<_> = (0..n).map(|i| session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(&format!("keys {i}")))).collect();
    let (mut runtime, mut ports) = RuntimeControl::<u8, u8>::new(Arc::new(session), None, target(), PlanLimits::default()).unwrap();
    runtime.service();
    ports.plans.service(&mut plan_bridge::callback_scope(), true);
    runtime.service();
    let gate = Arc::new(Gate::default());
    runtime.set_plugin_test_hooks(PluginTestHooks {
        host: Host::new(Config { inline: false, ..config }),
        factory: gated(&gate, hold),
        deferred: Arc::new(|id: &str| id == HOSTED),
        clock,
        slow_after,
    });
    let mut edited = (**runtime.requested_session()).clone();
    for t in &tracks {
        let mut insert = Insert::new(HOSTED);
        insert.state = state.map(transit_model::b64::encode);
        edited.track_mut(*t).unwrap().instrument = Some(insert);
    }
    runtime.update_session(Arc::new(edited)).unwrap();
    let open = OpenOnDrop(Arc::clone(&gate));
    Rig { runtime, ports, tracks, gate, open }
}

impl Rig {
    /// One frame: the control thread's service, then the audio side adopts what was published.
    fn frame(&mut self) {
        self.runtime.service();
        let _ = self.ports.plans.service(&mut plan_bridge::callback_scope(), true);
        std::thread::sleep(Duration::from_millis(5));
    }

    /// Frames until `done`, at most `limit`; whether it came true.
    fn frames_until(&mut self, limit: Duration, done: impl Fn(&RuntimeControl<u8, u8>) -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if done(&self.runtime) {
                return true;
            }
            self.frame();
        }
        done(&self.runtime)
    }

    /// Whether the plug-in in `track`'s instrument slot is live in the active plan.
    fn bound(&self, track: TrackId) -> bool {
        self.runtime.binding(track, SLOT).is_some()
    }

    /// Plays a note on `track` and returns the output energy.
    fn play(&mut self, track: TrackId) -> f32 {
        let plans = &mut self.ports.plans;
        plans.live_note(track, 60, 100);
        let mut total = 0.0;
        for _ in 0..8 {
            plans.render(0, 64);
            total += plans.output().iter().flatten().map(|s| s * s).sum::<f32>();
        }
        total
    }

    fn retire(self) {
        let Rig { runtime, ports, open, .. } = self;
        drop(open);
        runtime.retire(ports);
    }
}

/// The app's host shape with short budgets: the request timeout `timeout`, creations wedge after
/// `creation`.
fn config(timeout: Duration, creation: Duration) -> Config {
    Config { timeout, creation_timeout: creation, ..Config::default() }
}

/// Issue #82: Battery made 300 ms after the 10 s wait was discarded. Here the creation outlives
/// the display budget (manual clock): the slot reads "Loading…", then "Loading… (slow)" while the
/// session is compiled without it, and when the creation returns the instance is adopted (made
/// once, never again) and plays.
#[test]
fn a_creation_slower_than_the_display_budget_is_adopted_not_discarded() {
    let clock = ManualClock::new();
    let slow_after = Duration::from_secs(10);
    let mut rig = rig(1, None, config(Duration::from_millis(200), Duration::from_secs(60)), clock.clock(), slow_after, 1);
    let t = rig.tracks[0];
    rig.frame();
    assert!(matches!(rig.runtime.slot_load(t, SLOT), Some(SlotLoad::Loading { .. })), "announced: Loading…");
    let gate = Arc::clone(&rig.gate);
    assert!(rig.frames_until(Duration::from_secs(5), |_| gate.calls() == 1), "the creation started on the host thread");
    assert!(matches!(rig.runtime.slot_load(t, SLOT), Some(SlotLoad::Loading { .. })));
    assert_eq!(rig.runtime.requested_state().1, RequestState::Preparing, "the request waits for it within the display budget");

    // Past the display budget: slow, not stuck; the request goes on without it.
    clock.advance(slow_after + Duration::from_secs(1));
    rig.frame();
    let Some(SlotLoad::Slow { elapsed }) = rig.runtime.slot_load(t, SLOT) else {
        panic!("Loading… (slow), got {:?}", rig.runtime.slot_load(t, SLOT))
    };
    assert!(elapsed >= slow_after, "{elapsed:?}");
    assert!(!rig.runtime.stuck(t, SLOT) && !rig.runtime.any_stuck());
    assert!(rig.runtime.loading(t, SLOT) && rig.runtime.any_loading());
    assert!(
        rig.frames_until(Duration::from_secs(5), |r| !matches!(r.requested_state().1, RequestState::Preparing)),
        "the session compiled without the slow instance"
    );
    assert_eq!(rig.play(t), 0.0, "the vacant slot is silent meanwhile");
    assert_eq!(rig.runtime.plugin_host_wedged(), 0);

    // It returns late: adopted into the next plan.
    rig.gate.open();
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.binding(t, SLOT).is_some() && !r.any_loading()), "the late instance was adopted");
    assert_eq!(rig.runtime.slot_load(t, SLOT), None, "the slot shows the plug-in");
    assert!(rig.play(t) > 0.0, "the late instance plays");
    assert_eq!(rig.gate.calls(), 1, "the late instance itself was taken; nothing was made again");
    rig.retire();
}

/// Issue #82: a creation that never returns is "Not responding" only at the host's creation
/// budget (the ADR 0018 wedge), not at the request timeout or the display budget; if it returns
/// after all, it is still adopted.
#[test]
fn a_creation_that_never_returns_reads_not_responding_only_at_the_creation_budget() {
    let creation = Duration::from_millis(1500);
    let mut rig = rig(1, None, config(Duration::from_millis(200), creation), Arc::new(Instant::now), Duration::from_millis(300), 1);
    let t = rig.tracks[0];
    let gate = Arc::clone(&rig.gate);
    assert!(rig.frames_until(Duration::from_secs(5), |_| gate.calls() == 1));
    let started = Instant::now();
    // Past the request timeout and the display budget: slow, not stuck, no thread wedged.
    assert!(rig.frames_until(Duration::from_secs(5), |r| matches!(r.slot_load(t, SLOT), Some(SlotLoad::Slow { .. }))));
    while started.elapsed() < Duration::from_millis(700) {
        rig.frame();
    }
    assert!(matches!(rig.runtime.slot_load(t, SLOT), Some(SlotLoad::Slow { .. })), "got {:?}", rig.runtime.slot_load(t, SLOT));
    assert!(!rig.runtime.stuck(t, SLOT));
    assert_eq!(rig.runtime.plugin_host_wedged(), 0, "a slow creation does not wedge its thread");
    // At the creation budget: Not responding.
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.stuck(t, SLOT)), "it reads Not responding at the creation budget");
    assert!(started.elapsed() >= creation - Duration::from_millis(100), "not before the creation budget ({:?})", started.elapsed());
    assert!(matches!(rig.runtime.slot_load(t, SLOT), Some(SlotLoad::NotResponding { .. })));
    assert_eq!(rig.runtime.plugin_host_wedged(), 1);
    assert_eq!(rig.runtime.stuck_slots(), vec![(t, SLOT, HOSTED.to_string())]);
    assert!(!rig.runtime.loading(t, SLOT) && !rig.runtime.any_loading());
    // It returns after all: adopted, not discarded.
    rig.gate.open();
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.binding(t, SLOT).is_some()), "the late instance was adopted after Not responding");
    assert!(!rig.runtime.stuck(t, SLOT) && rig.runtime.slot_load(t, SLOT).is_none());
    assert!(rig.play(t) > 0.0);
    assert_eq!(rig.gate.calls(), 1);
    rig.retire();
}

/// Issue #82: Retry makes a not-responding slot's plug-in again on a responsive host thread, with
/// the state the session stores for it (the #56 store): the new instance is made with it and a
/// save reads it back.
#[test]
fn retry_makes_a_not_responding_plug_in_again_with_its_stored_state() {
    let stored = [0xab, 0xcd];
    let mut rig =
        rig(1, Some(&stored), config(Duration::from_millis(100), Duration::from_millis(400)), Arc::new(Instant::now), Duration::from_millis(100), 1);
    let t = rig.tracks[0];
    assert!(!rig.runtime.retry_plugin(t, SLOT), "nothing to retry while it loads");
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.stuck(t, SLOT)), "the held creation reads Not responding");
    assert!(rig.runtime.retry_plugin(t, SLOT));
    rig.frame();
    assert!(!rig.runtime.stuck(t, SLOT), "retrying: no longer Not responding");
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.binding(t, SLOT).is_some() && !r.any_loading()), "the retried instance is bound");
    assert_eq!(rig.gate.calls(), 2, "one more creation");
    assert_eq!(*rig.gate.states.lock().unwrap(), vec![Some(stored.to_vec()), Some(stored.to_vec())], "both made with the stored state");
    rig.runtime.service();
    let session = Arc::clone(rig.runtime.requested_session());
    assert_eq!(rig.runtime.capture_states(&session).unwrap(), vec![(t, SLOT, stored.to_vec())], "the live instance holds the stored state");
    assert!(rig.play(t) > 0.0);
    assert!(!rig.runtime.retry_plugin(t, SLOT), "nothing left to retry");
    rig.retire();
}

/// Issue #82: "Retry all" retries every slot that is not responding.
#[test]
fn retry_all_makes_every_not_responding_plug_in_again() {
    let mut rig =
        rig(2, Some(&[7]), config(Duration::from_millis(100), Duration::from_millis(400)), Arc::new(Instant::now), Duration::from_millis(100), 2);
    let tracks = rig.tracks.clone();
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.stuck_slots().len() == 2), "both read Not responding");
    assert_eq!(rig.runtime.retry_all_plugins(), 2);
    assert!(
        rig.frames_until(Duration::from_secs(10), |r| tracks.iter().all(|t| r.binding(*t, SLOT).is_some()) && !r.any_loading()),
        "both retried instances are bound"
    );
    assert!(!rig.runtime.any_stuck());
    assert_eq!(rig.gate.calls(), 4);
    for t in &tracks {
        assert!(rig.bound(*t));
    }
    assert_eq!(rig.runtime.retry_all_plugins(), 0);
    rig.retire();
}

/// Review of #82: Retry, then the abandoned creation returns after all. The late instance is
/// destroyed once on its own thread, never bound; the retried instance (with the stored state)
/// keeps its binding and is what a save reads.
#[test]
fn an_abandoned_creation_returning_after_retry_is_destroyed_not_bound() {
    let stored = [0x11, 0x22];
    let mut rig =
        rig(1, Some(&stored), config(Duration::from_millis(100), Duration::from_millis(400)), Arc::new(Instant::now), Duration::from_millis(100), 1);
    let t = rig.tracks[0];
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.stuck(t, SLOT)));
    assert!(rig.runtime.retry_plugin(t, SLOT));
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.binding(t, SLOT).is_some() && !r.any_loading()));
    let drops_before = rig.gate.drops.load(Ordering::SeqCst);
    let binding = rig.runtime.binding(t, SLOT);
    rig.gate.open();
    let drops = Arc::clone(&rig.gate.drops);
    assert!(rig.frames_until(Duration::from_secs(5), |_| drops.load(Ordering::SeqCst) > drops_before), "the late instance was destroyed");
    for _ in 0..20 {
        rig.frame();
    }
    assert_eq!(rig.gate.calls(), 2);
    assert_eq!(rig.gate.drops.load(Ordering::SeqCst), drops_before + 1, "destroyed once");
    assert_eq!(rig.runtime.binding(t, SLOT), binding, "the binding is unchanged by the late return");
    assert!(!rig.runtime.any_loading() && !rig.runtime.any_stuck());
    rig.runtime.service();
    let session = Arc::clone(rig.runtime.requested_session());
    assert_eq!(rig.runtime.capture_states(&session).unwrap(), vec![(t, SLOT, stored.to_vec())]);
    assert!(rig.play(t) > 0.0);
    rig.retire();
}

/// Review B2 of #82: the same hosted instrument picked again on a "Loading… (slow)" slot (the
/// engine writes it without a state). The slot keeps its identity, so the compile keeps it vacant
/// and stages nothing; the pending creation must start again with the new spec, and the slot
/// ends bound. The late instance for the old state is destroyed, not bound.
#[test]
fn picking_the_same_plug_in_again_on_a_slow_slot_makes_it_again_and_binds_it() {
    let mut rig =
        rig(1, Some(&[1]), config(Duration::from_millis(100), Duration::from_secs(20)), Arc::new(Instant::now), Duration::from_millis(100), 1);
    let t = rig.tracks[0];
    assert!(rig.frames_until(Duration::from_secs(10), |r| matches!(r.slot_load(t, SLOT), Some(SlotLoad::Slow { .. }))));
    for _ in 0..20 {
        rig.frame();
    }
    assert!(rig.runtime.any_loading() && rig.runtime.binding(t, SLOT).is_none(), "vacant, slow");
    let mut edited = (**rig.runtime.requested_session()).clone();
    // mix.instrument with the same hosted id writes `Insert::new`: no state.
    edited.track_mut(t).unwrap().instrument = Some(Insert::new(HOSTED));
    rig.runtime.update_session(Arc::new(edited)).unwrap();
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.binding(t, SLOT).is_some() && !r.any_loading()), "the slot was made again and bound");
    assert_eq!(rig.gate.calls(), 2);
    assert_eq!(rig.gate.states.lock().unwrap()[1], None, "made again with the new (empty) state");
    rig.gate.open();
    let drops = Arc::clone(&rig.gate.drops);
    assert!(rig.frames_until(Duration::from_secs(5), |_| drops.load(Ordering::SeqCst) == 1), "the late instance for the old state was destroyed");
    for _ in 0..20 {
        rig.frame();
    }
    assert_eq!(rig.gate.drops.load(Ordering::SeqCst), 1);
    assert!(rig.runtime.binding(t, SLOT).is_some());
    rig.runtime.service();
    let session = Arc::clone(rig.runtime.requested_session());
    assert_eq!(rig.runtime.capture_states(&session).unwrap(), vec![], "the live instance is the new one (no state)");
    assert!(rig.play(t) > 0.0);
    rig.retire();
}

/// Issue #67: a load passes through its phases in order and each is published while it runs (the
/// slot's tooltip reads it), and the plan taking the instance reports the total and the time in
/// each phase. The manual clock moves between the phases, so the figures are exact.
#[test]
fn a_load_publishes_its_phases_in_order_and_reports_the_time_in_each() {
    use transit_plugin_host::LoadPhase;
    let clock = ManualClock::new();
    let mut rig = rig(1, None, config(Duration::from_millis(200), Duration::from_secs(60)), clock.clock(), Duration::from_secs(10), 1);
    let t = rig.tracks[0];
    let mid = Arc::new(Gate::default());
    *rig.gate.mid.lock().unwrap() = Some(Arc::clone(&mid));
    assert_eq!(rig.runtime.slot_phase(t, SLOT), None, "nothing pending yet");
    rig.frame();
    assert_eq!(rig.runtime.slot_phase(t, SLOT), Some(LoadPhase::Queued), "announced, not yet on the host thread");
    let gate = Arc::clone(&rig.gate);
    assert!(rig.frames_until(Duration::from_secs(5), |_| gate.calls() == 1));
    assert_eq!(rig.runtime.slot_phase(t, SLOT), Some(LoadPhase::Creating), "the factory is running");

    clock.advance(Duration::from_secs(4));
    rig.gate.open();
    assert!(rig.frames_until(Duration::from_secs(5), |_| mid.calls.load(Ordering::SeqCst) == 1));
    assert_eq!(rig.runtime.slot_phase(t, SLOT), Some(LoadPhase::Restoring), "the stored state is being restored");
    assert!(rig.runtime.take_loaded().is_empty(), "nothing is reported while it loads");

    clock.advance(Duration::from_secs(2));
    mid.open();
    assert!(rig.frames_until(Duration::from_secs(10), |r| r.binding(t, SLOT).is_some() && !r.any_loading()));
    assert_eq!(rig.runtime.slot_phase(t, SLOT), None, "loaded");
    let loaded = rig.runtime.take_loaded();
    assert_eq!(loaded.len(), 1, "once per load");
    assert_eq!(loaded[0].id, HOSTED);
    let order: Vec<_> = loaded[0].phases.iter().map(|(p, _)| *p).collect();
    assert_eq!(order, [LoadPhase::Queued, LoadPhase::Creating, LoadPhase::Restoring, LoadPhase::Binding]);
    let took = |phase| loaded[0].phases.iter().find(|(p, _)| *p == phase).unwrap().1;
    assert_eq!(took(LoadPhase::Creating), Duration::from_secs(4));
    assert_eq!(took(LoadPhase::Restoring), Duration::from_secs(2));
    assert_eq!(loaded[0].total, Duration::from_secs(6));
    assert_eq!(loaded[0].phases.iter().map(|(_, d)| *d).sum::<Duration>(), loaded[0].total, "the phases add up to the total");
    assert!(rig.runtime.take_loaded().is_empty(), "taken once");
    rig.retire();
}
