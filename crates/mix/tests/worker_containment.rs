//! ADR 0059 R1, Decision 2, the worker-held case: an instrument whose render a worker holds
//! reports a processing error inside that render. The callback gives up on the strip (it is
//! late, away with its worker); events queued while it is away are answered `Queued` and parked;
//! when the worker returns, the strip comes home contained: its owned notes and pedal were released
//! at the containment (the mix's `release_all` reached the adapter before Stop), the parked note
//! never reaches it, and after Stop the owed releases leave an empty ledger, no held or sustained
//! voice and no pedal. A note played after that sounds exactly once.
//!
//! Deterministic: the worker is held by an atomic handshake (the instrument spins with
//! `yield_now` until the test releases it), and the scheduler's existing test seam ends the block
//! once only the held strip is unresolved (`MixEngine::hold_until_alone`, feature `testing`); no
//! clock decides an outcome and nothing sleeps.
// timing: held-worker the instrument waits on an atomic released by the test (a handshake with a
// hang watchdog, `common::stall`), never on elapsed time
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use common::workers::{Rig, become_callback};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_dsp::Plugin;
use transit_dsp::plugins::instrument_queue::delivery::Delivery;
use transit_mix::{
    Arrival, Due, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, Message, NewInstance, NoteId, PluginEvent, RenderFrame, Source,
};
use transit_model::{ChannelFormat, Insert, TrackKind};
use transit_rt::worker_set::{Outcome, OverrunCause};

const HELD: &str = "clap:test.held";
const FRAMES: usize = 256;

/// What the held instrument shares with the test.
#[derive(Default)]
struct Shared {
    hold: AtomicBool,
    holding: AtomicBool,
    progress: AtomicUsize,
    err: AtomicBool,
    errors: AtomicUsize,
    /// Every event the mix gave the adapter (admitted or not), in order.
    given: Mutex<Vec<PluginEvent>>,
    /// Every event a call delivered, in order.
    delivered: Mutex<Vec<PluginEvent>>,
    outstanding: AtomicUsize,
    /// The callback's thread: only a worker is ever held (the callback must never wait).
    callback: Mutex<Option<std::thread::ThreadId>>,
}

/// A contract-conforming instrument (the shared ledger) whose render the test can hold.
struct Held {
    inner: Box<dyn Plugin>,
    queue: Delivery,
    sh: Arc<Shared>,
}

impl Plugin for Held {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.queue.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let on_worker = *self.sh.callback.lock().unwrap() != Some(std::thread::current().id());
        if on_worker && self.sh.hold.load(Ordering::Acquire) {
            self.sh.holding.store(true, Ordering::Release);
            common::stall::wait_for_release(&self.sh.hold, &self.sh.progress, std::thread::yield_now);
            self.sh.holding.store(false, Ordering::Release);
        }
        let erring = self.sh.err.load(Ordering::Acquire);
        let n = u32::try_from(frames).unwrap_or(u32::MAX);
        self.queue.prepare(n);
        if !erring {
            self.sh.delivered.lock().unwrap().extend(self.queue.batch(n).iter().map(|it| it.event));
        } else {
            let _ = self.queue.batch(n);
            self.sh.errors.fetch_add(1, Ordering::Release);
        }
        self.queue.finish(!erring);
        self.queue.carry();
        self.sh.outstanding.store(self.queue.ledger().outstanding(), Ordering::Release);
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }
    fn process_errors(&self) -> u64 {
        self.sh.errors.load(Ordering::Acquire) as u64
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.sh.given.lock().unwrap().push(*ev);
        self.queue.admit(offset, ev)
    }
    fn all_notes_off(&mut self) {
        self.queue.all_notes_off();
    }
}

/// The voices and sustain pedal after the delivered events (by the mix's voice id).
fn replay(events: &[PluginEvent]) -> (Vec<i32>, Vec<i32>, bool) {
    let (mut held, mut sustained, mut pedal) = (Vec::new(), Vec::new(), false);
    for e in events {
        match e.message {
            Message::NoteOn { velocity, .. } if velocity > 0 => held.push(e.note_id),
            Message::NoteOn { .. } | Message::NoteOff { .. } => {
                if held.contains(&e.note_id) {
                    held.retain(|v| *v != e.note_id);
                    if pedal {
                        sustained.push(e.note_id);
                    }
                }
            }
            Message::Control { cc: 64, value } => {
                pedal = value >= 64;
                if !pedal {
                    sustained.clear();
                }
            }
            _ => {}
        }
    }
    (held, sustained, pedal)
}

/// The scenario runs on its own thread (it becomes the audio callback); the test thread waits for
/// its verdict with a hang ceiling, so a block that never ends fails the test instead of hanging.
#[test]
fn a_worker_held_instrument_is_contained_and_readmitted_with_nothing_owed() {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let verdict = std::panic::catch_unwind(scenario);
        let _ = tx.send(verdict.map_err(|e| e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))));
    });
    // Twice the ceiling: the scenario's own watchdogs (each `HANG`) report their cause first.
    match rx.recv_timeout(common::stall::HANG * 2) {
        Ok(Ok(())) => {}
        Ok(Err(why)) => panic!("{}", why.unwrap_or_default()),
        Err(_) => panic!("the scenario made no verdict within the hang ceiling: a held block never ended"),
    }
}

fn scenario() {
    // Three audio strips beside the instrument: the pool renders a plan of at least four strips.
    let (mut s, _) = common::workers::session(0, 3);
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(track).unwrap().instrument = Some(Insert::new(HELD));
    let sh = Arc::new(Shared::default());
    let made = Arc::clone(&sh);
    let mut rig = Rig::with_session(s, Vec::new(), FRAMES, move |spec: &InstanceSpec| {
        (spec.id == HELD).then(|| {
            let mut inner = transit_dsp::create("gain").unwrap();
            inner.prepare(48_000.0, FRAMES, spec.ch);
            NewInstance { plugin: Box::new(Held { inner, queue: Delivery::new(false), sh: Arc::clone(&made) }), refused_state: false }
        })
    });
    assert!(rig.mix.start_render_workers(64) > 0, "a render worker to hold");
    // As the scheduling harness does (`common::sched`): every strip has a cost history, so the
    // deterministic deadline's wait for unheld strips always ends.
    transit_mix::MixEngine::force_graph(true);
    rig.mix.seed_strip_costs(Duration::from_nanos(1));
    become_callback(); // after the workers start (the audio thread may not start them)
    *sh.callback.lock().unwrap() = Some(std::thread::current().id());
    let binding = *rig.comp.active_bindings().iter().find(|b| b.0 == track && b.1 == INSTRUMENT_SLOT).unwrap();
    let mut seq = 0u64;
    let mut send = |rig: &mut Rig, offset: u32, message: Message| {
        seq += 1;
        let at = rig.mix.next_frame().0 + u64::from(offset);
        rig.mix.queue_event(&InstrumentEvent {
            engine: rig.epoch,
            target: binding,
            due: Due::Frame(RenderFrame(at)),
            transport: None,
            source: Source { port: 1, channel: 0, generation: 1 },
            message,
            arrival: Arrival { host_ns: 0, sequence: seq },
        })
    };
    let home = |rig: &Rig, i: usize| {
        let t0 = Instant::now();
        while rig.mix.strip_claimed(i) {
            assert!(t0.elapsed() < common::stall::HANG, "the worker never returned");
            std::thread::yield_now();
        }
    };
    let on = |key| Message::NoteOn { key, velocity: 100, id: NoteId::NONE };
    let off = |key| Message::NoteOff { key, velocity: 0, id: NoteId::NONE };
    rig.block();
    // Note A under the sustain pedal, delivered normally.
    assert_eq!(send(&mut rig, 0, Message::Control { cc: 64, value: 127 }), EventOutcome::Queued);
    assert_eq!(send(&mut rig, 0, on(60)), EventOutcome::Queued);
    rig.block();
    assert_eq!(replay(&sh.delivered.lock().unwrap()).0.len(), 1, "A sounds");
    // Hold the strip's worker inside the instrument's render, which then reports an error.
    let i = rig.mix.strip_index(track).unwrap();
    home(&rig, i);
    sh.err.store(true, Ordering::Release);
    sh.hold.store(true, Ordering::Release);
    rig.mix.set_strip_cost(i, Duration::MAX); // the callback never renders it inline
    rig.mix.hold_until_alone(&[i]);
    rig.block();
    assert_eq!(
        rig.mix.strip_outcomes()[i],
        Some(Outcome::Overrun(OverrunCause::Late)),
        "the callback gave up on the held strip: strip {i}, outcomes {:?}, workers {}, phases {:?}",
        rig.mix.strip_outcomes(),
        rig.mix.render_workers(),
        rig.mix.strip_phases()
    );
    // The deadline seam ends the block once the held strip is claimed by a worker, which can be
    // before that worker has entered the instrument's `process` and begun the fixture's hold: wait
    // for the hold itself (`hold` and `err` stay set), with the hang ceiling as the only watchdog.
    let waited = Instant::now();
    while !sh.holding.load(Ordering::Acquire) {
        let expired = waited.elapsed() >= common::stall::HANG;
        assert!(
            !expired || sh.holding.load(Ordering::Acquire),
            "its worker never began the held render within the hang ceiling: the strip was claimed, the fixture's hold never started"
        );
        std::thread::yield_now();
    }
    // While it is away: a note is queued (parked with the strip), then another block passes.
    let given_before = sh.given.lock().unwrap().len();
    assert_eq!(send(&mut rig, 0, on(62)), EventOutcome::Queued, "an away strip parks the event");
    rig.block();
    // The worker returns: the render reports its error and the strip is contained there.
    sh.hold.store(false, Ordering::Release);
    home(&rig, i);
    rig.mix.clock_deadline();
    rig.mix.set_strip_cost(i, Duration::from_nanos(1));
    sh.err.store(false, Ordering::Release);
    rig.block(); // readmission: the strip is home, failed
    let given: Vec<PluginEvent> = sh.given.lock().unwrap()[given_before..].to_vec();
    assert!(
        given.iter().any(|e| matches!(e.message, Message::NoteOff { key: 60, .. }))
            && given.iter().any(|e| e.message == Message::Control { cc: 64, value: 0 }),
        "the containment released A and the pedal at the adapter before Stop: {given:?}"
    );
    assert!(
        !given.iter().any(|e| matches!(e.message, Message::NoteOn { key: 62, .. })),
        "the parked note never reaches the contained instrument: {given:?}"
    );
    assert_eq!(send(&mut rig, 0, on(64)), EventOutcome::NoInstrument, "a contained instrument takes no event");
    // Stop: the owed releases are delivered and the ledger empties.
    rig.mix.reset();
    rig.block();
    rig.block();
    let delivered = sh.delivered.lock().unwrap().clone();
    assert!(!delivered.iter().any(|e| matches!(e.message, Message::NoteOn { key: 62, .. })), "never delivered late");
    assert_eq!(replay(&delivered), (vec![], vec![], false), "no held or sustained voice, no pedal");
    assert_eq!(sh.outstanding.load(Ordering::Acquire), 0, "the ledger is empty");
    // A note after readmission sounds exactly once.
    let before = delivered.len();
    assert_eq!(send(&mut rig, 0, on(65)), EventOutcome::Queued);
    assert_eq!(send(&mut rig, 10, off(65)), EventOutcome::Queued);
    rig.block();
    let after: Vec<Message> = sh.delivered.lock().unwrap()[before..].iter().map(|e| e.message).collect();
    let ons = after.iter().filter(|m| matches!(m, Message::NoteOn { key: 65, .. })).count();
    let offs = after.iter().filter(|m| matches!(m, Message::NoteOff { key: 65, .. })).count();
    assert_eq!((ons, offs), (1, 1), "{after:?}");
}
