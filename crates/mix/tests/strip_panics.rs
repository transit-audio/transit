//! A panic out of a strip's processing (a plug-in's) never has its payload dropped on the thread
//! that rendered the strip (every last-owner drop happens on the permitted thread). The
//! render keeps each strip's payload in the strip, re-raises the first one to its caller, and the
//! rest go back to the control side with the strips. Before this, Rayon re-raised one panic of a
//! parallel render and dropped the others' payloads on its workers.
//!
//! Both paths: the plan on the engine's render workers and serially. The render-worker tests set
//! a far strip deadline, so the block waits for every strip's completion instead of racing the
//! default share of a 5.3 ms period (E49 recurrence 2026-10-29); a strip still away at the
//! deadline is the late test's subject, and its payload comes out of the block that brings it home. The serial run also counts the audio thread's heap operations: the
//! render adds none to what the panics themselves cost (each panic's payload box and the unwind's
//! own exception box, plus one more exception box for the re-raise).
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and a panic hook silent for the
//! instruments' panics.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};
use transit_dsp::Plugin;
use transit_mix::{
    DeviceGeneration, INSTRUMENT_SLOT, InstanceSpec, MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget,
    mark_audio_thread,
};
use transit_model::{ChannelFormat, Insert, Session, TrackId, TrackKind};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;
/// More than `PARALLEL_STRIPS` (4), so the strips render in parallel where a pool exists.
const TRACKS: usize = 6;
/// A strip deadline no worker misses: the block waits for every published strip's completion.
const FAR: Duration = Duration::from_secs(60);

/// `MixEngine::force_serial` is process-wide: one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

struct Shared {
    /// Where each panic payload was dropped.
    payload_drops: Mutex<Vec<ThreadId>>,
    /// Where each panic was raised (reserved: recording one allocates nothing).
    raised: Mutex<Vec<ThreadId>>,
    armed: AtomicBool,
    /// While set, an armed instrument rendering off the `callback` thread waits in `process`
    /// before it panics: its strip is still on its worker at the deadline (`Late`).
    hold: AtomicBool,
    /// The callback thread (the hold never applies there, so the callback cannot wait on itself).
    callback: Mutex<Option<ThreadId>>,
    /// Armed instruments that entered `process` (a held one counts before it waits).
    entered: std::sync::atomic::AtomicUsize,
}

impl Shared {
    fn new() -> Arc<Shared> {
        Arc::new(Shared {
            payload_drops: Mutex::default(),
            raised: Mutex::new(Vec::with_capacity(64)),
            armed: AtomicBool::new(false),
            hold: AtomicBool::new(false),
            callback: Mutex::new(None),
            entered: std::sync::atomic::AtomicUsize::new(0),
        })
    }
}

/// A panic payload that owns something and records the thread it is dropped on.
struct Payload(Arc<Shared>);

impl Drop for Payload {
    fn drop(&mut self) {
        self.0.payload_drops.lock().unwrap().push(std::thread::current().id());
    }
}

/// An instrument that panics in `process` with a [`Payload`] while armed.
struct Panics {
    inner: Box<dyn Plugin>,
    shared: Arc<Shared>,
}

impl Plugin for Panics {
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
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.shared.armed.load(Ordering::Relaxed) {
            self.shared.entered.fetch_add(1, Ordering::Relaxed);
            let here = std::thread::current().id();
            if *self.shared.callback.lock().unwrap() != Some(here) {
                // timing: sleep-ok waits for the test's release, measures nothing
                while self.shared.hold.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_micros(100));
                }
            }
            self.shared.raised.lock().unwrap().push(std::thread::current().id());
            std::panic::panic_any(Payload(Arc::clone(&self.shared)));
        }
        self.inner.process(io, frames);
    }
}

fn panics(shared: &Arc<Shared>, ch: usize) -> Box<dyn Plugin> {
    let mut inner = transit_dsp::create("gain").unwrap();
    inner.prepare(SR as f32, BLOCK, ch);
    Box::new(Panics { inner, shared: Arc::clone(shared) })
}

fn session(tracks: usize) -> (Session, Vec<TrackId>) {
    let mut s = Session::default();
    s.edit.click = false;
    let ids = (0..tracks)
        .map(|_| {
            let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
            s.track_mut(t).unwrap().instrument = Some(Insert::new("clap:test.panics"));
            t
        })
        .collect();
    (s, ids)
}

fn target() -> RenderTarget {
    RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 }
}

/// The instruments' panics report nothing (the default hook would format and print them, which
/// allocates on the measured thread); any other panic, a failed assertion, reports as usual.
fn silence_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if !info.payload().is::<Payload>() {
                prev(info);
            }
        }));
    });
}

/// A realtime engine with the panicking instruments adopted from a compiled plan (control side),
/// warmed up with a few blocks while disarmed.
fn plan_engine(shared: &Arc<Shared>) -> MixEngine {
    let (s, _) = session(TRACKS);
    let mut mix = MixEngine::realtime(target());
    let mut compiler = PlanCompiler::new(target(), PlanLimits::default());
    let sh = Arc::clone(shared);
    let mut make = move |spec: &InstanceSpec, _: transit_mix::SlotGen| {
        assert!(spec.id == "clap:test.panics" && spec.slot == INSTRUMENT_SLOT, "{}", spec.id);
        Some(NewInstance { plugin: panics(&sh, spec.ch), refused_state: false })
    };
    let mut env = compiler.compile(Arc::new(s), compiler.active(), PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert!(matches!(mix.adopt_plan(&mut env), transit_mix::Adoption::Adopted));
    compiler.reclaim(env);
    let mut out = vec![vec![0.0; BLOCK]; 2];
    for b in 0..4 {
        mix.render_active(b * BLOCK as i64, BLOCK, &mut out);
    }
    mix
}

/// After the render that panicked: nothing was dropped yet; the re-raised payload and the
/// engine's teardown (the control side) drop every payload, all on this thread.
fn assert_all_dropped_here(what: &str, shared: &Shared, panic: Box<dyn Any + Send>, engine: MixEngine) {
    let here = std::thread::current().id();
    let early = shared.payload_drops.lock().unwrap().len();
    assert_eq!(early, 0, "{what}: {early} of {TRACKS} panic payloads dropped during the render");
    assert!(panic.is::<Payload>(), "{what}: the strip's own payload comes out of the render");
    drop(panic);
    drop(engine.into_retired());
    let drops = shared.payload_drops.lock().unwrap().clone();
    assert_eq!(drops.len(), TRACKS, "{what}: every strip's payload was kept and handed back");
    assert!(drops.iter().all(|t| *t == here), "{what}: payloads dropped off the control thread: {drops:?}");
}

fn render_active_panic(mix: &mut MixEngine, pos: i64, out: &mut [Vec<f32>]) -> Box<dyn Any + Send> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mix.render_active(pos, BLOCK, out))).expect_err("the render panics")
}

#[test]
fn parallel_panicking_instruments_drop_no_payload_on_a_render_worker() {
    let _one = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    silence_panics();
    let shared = Shared::new();
    // The pool is built when the first engine is made, off the audio thread.
    let mut mix = plan_engine(&shared);
    mix.start_render_workers(16);
    // The block waits for every strip: with the default share of the 5.3 ms period a strip still
    // on a worker at the deadline is `Late`, its payload stays in its cell for a later block and
    // this render returns Ok (CI run 40968743055 on macos-aarch64; E49). That path is
    // `a_strip_late_at_the_deadline_raises_its_panic_from_the_block_that_brings_it_home`.
    mix.set_strip_deadline(Some(FAR));
    let mark = std::thread::spawn(move || {
        MixEngine::force_serial(false);
        mark_audio_thread();
        shared.armed.store(true, Ordering::Relaxed);
        let panic = render_active_panic(&mut mix, 4 * BLOCK as i64, &mut vec![vec![0.0; BLOCK]; 2]);
        (mix, shared, panic)
    });
    let callback = mark.thread().id();
    let (mix, shared, panic) = mark.join().unwrap();
    MixEngine::force_serial(true);
    let workers = mix.render_workers();
    let raised = shared.raised.lock().unwrap().clone();
    let on_workers = raised.iter().filter(|t| **t != callback).count();
    // The callback takes part (it renders what no worker started yet), so how many ran on a
    // worker varies; each strip rendered exactly once either way.
    println!("render workers: {workers}; {on_workers} of {} panics raised on a worker", raised.len());
    assert_eq!(raised.len(), TRACKS);
    assert_all_dropped_here("render workers", &shared, panic, mix);
}

/// A strip still on its worker at the deadline (`Late`) keeps its panic in the strip: that block
/// returns Ok and mixes the strip as silence, and the block that brings the strip home re-raises
/// the payload to its caller. Nothing is dropped on a worker and nothing is lost: every strip's
/// payload comes back to the control side. The strips are held in `process` until every claimed
/// one is past a zero deadline, so the outcome does not depend on the host's timing.
#[test]
fn a_strip_late_at_the_deadline_raises_its_panic_from_the_block_that_brings_it_home() {
    let _one = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    silence_panics();
    let shared = Shared::new();
    let mut mix = plan_engine(&shared);
    if mix.start_render_workers(16) == 0 {
        return;
    }
    let mark = std::thread::spawn(move || {
        MixEngine::force_serial(false);
        mark_audio_thread();
        *shared.callback.lock().unwrap() = Some(std::thread::current().id());
        let mut out = vec![vec![0.0; BLOCK]; 2];
        let mut pos = 4 * BLOCK as i64;
        // No strip is admitted inline at a zero deadline, so only a worker enters an instrument,
        // and it waits there: each such strip is `Late`, the rest come home unrendered.
        mix.set_strip_deadline(Some(Duration::ZERO));
        shared.hold.store(true, Ordering::Release);
        shared.armed.store(true, Ordering::Relaxed);
        let mut blocks = 0;
        while shared.entered.load(Ordering::Relaxed) == 0 {
            assert!(blocks < 10_000, "no worker claimed a strip in {blocks} blocks");
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| mix.render_active(pos, BLOCK, &mut out)));
            assert!(r.is_ok(), "block {blocks}: a strip held on its worker raised nothing yet");
            pos += BLOCK as i64;
            blocks += 1;
        }
        let late = shared.entered.load(Ordering::Relaxed);
        assert!(shared.raised.lock().unwrap().is_empty(), "the held strips have not panicked yet");
        // Released: the held strips panic on their workers (each keeps its payload) and come home.
        shared.hold.store(false, Ordering::Release);
        let start = Instant::now();
        while !mix.render_workers_settled() {
            assert!(start.elapsed() < Duration::from_secs(10), "a worker never returned");
            // timing: sleep-ok paces a wait for the released workers; measures nothing
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(shared.payload_drops.lock().unwrap().len(), 0, "a late strip's payload was dropped");
        mix.set_strip_deadline(Some(FAR));
        let panic = render_active_panic(&mut mix, pos, &mut out);
        (mix, shared, panic, late)
    });
    let (mix, shared, panic, late) = mark.join().unwrap();
    MixEngine::force_serial(true);
    println!("{late} of {TRACKS} strips were late on a worker");
    assert_eq!(shared.raised.lock().unwrap().len(), TRACKS, "each strip rendered (and panicked) exactly once");
    assert_all_dropped_here("late", &shared, panic, mix);
}

#[test]
fn serial_panicking_instruments_keep_their_payloads_and_allocate_nothing_more() {
    let _one = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    silence_panics();
    let shared = Shared::new();
    let mut mix = plan_engine(&shared);
    MixEngine::force_serial(true);
    let here = std::thread::current().id();
    // What the panics themselves cost on this thread, measured the way the render performs them:
    // TRACKS panics caught and kept, then one re-raise caught. The baseline is measured on this
    // platform in the same shape, not derived from per-panic figures: macOS's panic machinery
    // allocates once more (64 bytes) in the sequence than the sum of its parts, so a hand-derived
    // sum is off by that on macOS. The first run warms the panic path; the second is the figure.
    let calibrate = |mut kept: Vec<Box<dyn Any + Send>>| {
        measure(|| {
            for _ in 0..TRACKS {
                kept.push(std::panic::catch_unwind(|| std::panic::panic_any(Payload(Arc::clone(&shared)))).unwrap_err());
            }
            let first = kept.swap_remove(0);
            let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| std::panic::resume_unwind(first))).unwrap_err();
            (kept, again)
        })
    };
    let _ = calibrate(Vec::with_capacity(TRACKS));
    let ((kept, again), expected) = calibrate(Vec::with_capacity(TRACKS));
    drop((kept, again));
    // Warm the render's own unwind path once with a second engine (a panicked engine keeps its
    // strips' payloads, so the measured one must be fresh): panics fire through the full render
    // path, the stats are discarded. Apple's libunwind is thought to allocate once the first time
    // an unwind walks frames it has not seen (known issue K30; inference); the baseline's frames
    // are already warm from its first run, the render's deeper ones are not.
    {
        let warm = plan_engine(&shared);
        shared.armed.store(true, Ordering::Relaxed);
        mark_audio_thread();
        let mut warm_out = vec![vec![0.0; BLOCK]; 2];
        let mut warm = warm;
        let panic = render_active_panic(&mut warm, 4 * BLOCK as i64, &mut warm_out);
        drop(panic);
        drop(warm.into_retired());
        shared.armed.store(false, Ordering::Relaxed);
    }
    shared.payload_drops.lock().unwrap().clear();
    shared.raised.lock().unwrap().clear();
    shared.armed.store(true, Ordering::Relaxed);
    let mut out = vec![vec![0.0; BLOCK]; 2];
    let (panic, stats) = measure(|| render_active_panic(&mut mix, 4 * BLOCK as i64, &mut out));
    println!("the panics alone {expected:?}; the render {stats:?}");
    assert_eq!(stats, expected, "the render adds no heap operation to the panics' own");
    assert!(shared.raised.lock().unwrap().iter().all(|t| *t == here), "serial: every strip rendered on this thread");
    assert_all_dropped_here("serial", &shared, panic, mix);
}
