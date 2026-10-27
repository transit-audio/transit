use super::*;

std::thread_local! {
    static AUDIO_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Mark the calling thread as the realtime audio thread (`transit_playback`'s cpal callbacks
/// do, every call: one thread-local store). Strips [`MixEngine::render_active`] hands to worker threads
/// count as the audio thread while they render for it. Code that might run there asks
/// [`on_audio_thread`] before doing anything that blocks; the desktop app's logger does, so a
/// `log::` record from the audio thread never takes a lock or touches a file.
pub fn mark_audio_thread() {
    AUDIO_THREAD.set(true);
}

/// Takes the mark of [`mark_audio_thread`] off the calling thread. For a thread that stood in for
/// the audio callback for a while (device-free test stepping) and then goes back to being an
/// ordinary thread, whose panics and log records must then be reported normally.
///
/// Exists for device-free tests only (`Player::step_device_free`); a real audio thread keeps
/// its mark for its whole life.
#[doc(hidden)]
pub fn unmark_audio_thread() {
    AUDIO_THREAD.set(false);
}

/// True on the thread [`mark_audio_thread`] marked, and on a worker while it renders strips for
/// it. Lock-free and allocation-free.
pub fn on_audio_thread() -> bool {
    AUDIO_THREAD.get()
}

/// Run `f` on this thread as (or as not) the audio thread, then restore the thread's own mark
/// (also when `f` unwinds): a global pool worker may render for an offline bounce.
#[cfg(not(target_arch = "wasm32"))]
fn as_audio_thread(audio: bool, f: impl FnOnce()) {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            AUDIO_THREAD.set(self.0);
        }
    }
    let _restore = Restore(AUDIO_THREAD.replace(audio));
    f();
}

/// Strips render in parallel from this many on; below it the fan-out costs more than it saves.
#[cfg(not(target_arch = "wasm32"))]
const PARALLEL_STRIPS: usize = 4;

/// The render pool never has more workers than this, whatever the core count.
pub const MAX_RENDER_WORKERS: usize = 8;

/// Environment variable: `1` opts in to the render pool for audio-thread strips, as
/// [`MixEngine::force_serial`]`(false)` does. Read once, when the render pool is built.
///
/// Serial is the realtime default until the bounded render plan lands; measured
/// 2026-10-18: promoted serial 0 late blocks vs promoted pool 1-5 per 2000 under load.
/// `SOUNDCRAFT_RENDER_POOL` remains an alias when this variable is unset.
pub const RENDER_POOL_ENV: &str = "TRANSIT_RENDER_POOL";

/// Environment variable: `1` leaves the render pool's workers at the OS default priority instead
/// of promoting them with [`transit_rt::promote_audio_thread`]. Read once, when the render
/// pool is built. It does not affect the audio callback thread: this crate never promotes that.
/// `SOUNDCRAFT_RENDER_NO_PRIORITY` remains an alias when this variable is unset.
pub const RENDER_NO_PRIORITY_ENV: &str = "TRANSIT_RENDER_NO_PRIORITY";

/// Audio-thread strips render serially when set ([`MixEngine::force_serial`]). On by default;
/// [`RENDER_POOL_ENV`] clears it at startup.
pub(crate) static FORCE_SERIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// What each render pool worker's [`transit_rt::promote_audio_thread`] achieved, by worker
/// index; written once by the worker's start handler, never on the render path.
#[cfg(not(target_arch = "wasm32"))]
static WORKER_PROMOTION: [std::sync::OnceLock<transit_rt::Promotion>; MAX_RENDER_WORKERS] =
    [const { std::sync::OnceLock::new() }; MAX_RENDER_WORKERS];

/// True when `name` is set to `1` (or `true`/`yes`, any case).
#[cfg(not(target_arch = "wasm32"))]
fn env_flag(name: &str, alias: &str) -> bool {
    flag_value(std::env::var(name).ok().as_deref(), std::env::var(alias).ok().as_deref())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn flag_value(value: Option<&str>, alias: Option<&str>) -> bool {
    value.or(alias).is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// What each render pool worker's promotion achieved, by worker index (`None` for a worker that
/// does not exist or has not started yet; `Unsupported("disabled")` under
/// [`RENDER_NO_PRIORITY_ENV`]). For diagnostics and tests.
#[must_use]
pub fn render_worker_promotions() -> [Option<transit_rt::Promotion>; MAX_RENDER_WORKERS] {
    #[cfg(not(target_arch = "wasm32"))]
    return std::array::from_fn(|i| WORKER_PROMOTION.get(i).and_then(|slot| slot.get().copied()));
    #[cfg(target_arch = "wasm32")]
    return [None; MAX_RENDER_WORKERS];
}

/// The realtime render pool: strips rendered for the audio thread run here, never on Rayon's
/// global pool, which offline bounces and video decode/encode share and which runs at normal
/// priority behind whatever they queued. `None` when the machine has too few cores for a pool to
/// pay (or the threads could not be spawned): the audio thread then renders its strips itself.
///
/// Serial is the realtime default until the bounded render plan lands; measured
/// 2026-10-18: promoted serial 0 late blocks vs promoted pool 1-5 per 2000 under load.
///
/// The audio callback thread is promoted by the audio backend (`playback`), before its first data
/// callback; `MixEngine::render_active` makes no OS priority call (MMCSS registration is a service call
/// that can block). Only the pool workers are promoted here, by their start handler at pool
/// build, off the callback. The pool
/// is still built (and its workers promoted) so [`MixEngine::force_serial`]`(false)` and
/// [`RENDER_POOL_ENV`] can opt in.
#[cfg(not(target_arch = "wasm32"))]
static RENDER_POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();

/// Spawn the render pool, once per process. [`MixEngine::new`] calls this, so the pool exists
/// before the first callback; it never spawns from the audio thread (a `MixEngine` built there,
/// on a sample-rate change, reuses the pool an earlier engine built). The switches
/// [`RENDER_POOL_ENV`] and [`RENDER_NO_PRIORITY_ENV`] are read here, once.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn init_render_pool() {
    if !on_audio_thread() {
        RENDER_POOL.get_or_init(|| {
            if env_flag(RENDER_POOL_ENV, "SOUNDCRAFT_RENDER_POOL") {
                log::info!("mix: {RENDER_POOL_ENV} is set; audio-thread strips render on the pool");
                FORCE_SERIAL.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            let promote = !env_flag(RENDER_NO_PRIORITY_ENV, "SOUNDCRAFT_RENDER_NO_PRIORITY");
            build_render_pool(promote)
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn init_render_pool() {}

/// One worker per core but the one the audio callback runs on, at most [`MAX_RENDER_WORKERS`].
/// Fewer than two workers is no pool: one worker plus a waiting callback is slower than the
/// callback alone.
///
/// Priority: each worker promotes itself at start with [`transit_rt::promote_audio_thread`]
/// (MMCSS "Pro Audio" on Windows, `SCHED_FIFO` on Linux, the Mach time-constraint policy on
/// macOS; ADR 0013 keeps that `unsafe` out of this crate), unless `promote` is false. The result
/// lands in [`render_worker_promotions`] and is logged once per worker at debug level. A refused
/// promotion leaves the worker at the default priority; the pool is still dedicated, so nothing
/// queued by a bounce or a video job runs ahead of a strip.
#[cfg(not(target_arch = "wasm32"))]
fn build_render_pool(promote: bool) -> Option<rayon::ThreadPool> {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let workers = cores.saturating_sub(1).min(MAX_RENDER_WORKERS);
    if workers < 2 {
        return None;
    }
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|i| format!("mix-render-{i}"))
        .start_handler(move |i| {
            let p = if promote { transit_rt::promote_audio_thread() } else { transit_rt::Promotion::Unsupported("disabled") };
            if let Some(slot) = WORKER_PROMOTION.get(i) {
                let _ = slot.set(p);
            }
            // Before the mark: the logger treats the audio thread as never-block.
            log::debug!("mix: render worker {i} priority: {p:?}");
            // A worker only ever renders for the audio thread, so it is the audio thread for good.
            mark_audio_thread();
        })
        .build()
        .map_err(|e| log::warn!("mix: no render pool ({e}); the audio thread renders strips itself"))
        .ok()
}

/// Runs `f` (one phase-1 strip's processing) unless the strip holds a panic, and keeps a panic
/// out of it in [`Strip::panic`] instead of letting it unwind. A panic that reached a render pool
/// worker's frame would end there: Rayon re-raises one panic of a parallel call and drops the
/// others' payloads, owners plug-ins may have made, on its workers (every last-owner
/// drop on the permitted thread). Caught the same way on every path, serial, pool and offline,
/// so the render behaves the same whichever way its strips ran: it re-raises the first strip's.
pub(crate) fn guarded(strip: &mut Strip, f: impl FnOnce(&mut Strip)) {
    if strip.panic.is_some() {
        return;
    }
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *strip))) {
        strip.panic = Some(panic);
    }
}

/// Process strips, in parallel on native targets when there are enough of them.
///
/// On the audio thread the strips go to the dedicated render pool (or run right here when there
/// is none, or while [`MixEngine::force_serial`] is on, the default); they never wait behind Rayon's global
/// pool. Elsewhere (offline bounces) they use the global pool as before. The strips are
/// independent and are summed afterwards in a fixed order, so the result is the same whichever
/// way they ran.
///
/// Known residual, as before this pool existed: each parallel dispatch pushes one job onto Rayon's
/// injector queue, which allocates a 1.5 KB block on the caller every 63 pushes.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_strips<T: Send>(work: &mut [T], parallel: bool, f: impl Fn(&mut T) + Sync + Send) {
    use rayon::prelude::*;
    if !parallel || work.len() < PARALLEL_STRIPS {
        work.iter_mut().for_each(f);
    } else if on_audio_thread() {
        let pool = if MixEngine::serial_forced() { None } else { RENDER_POOL.get().and_then(Option::as_ref) };
        match pool {
            // The pool's workers are marked as the audio thread (`build_render_pool`).
            Some(pool) => pool.install(|| work.par_iter_mut().for_each(f)),
            None => work.iter_mut().for_each(f),
        }
    } else {
        // A global worker renders for an offline caller here: it is not the audio thread.
        work.par_iter_mut().for_each(|w| as_audio_thread(false, || f(w)));
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn run_strips<T: Send>(work: &mut [T], _parallel: bool, f: impl Fn(&mut T) + Sync + Send) {
    work.iter_mut().for_each(f);
}

#[cfg(test)]
mod audio_thread_tests {
    use super::*;

    #[test]
    fn only_a_marked_thread_is_the_audio_thread() {
        assert!(!on_audio_thread());
        let marked = std::thread::spawn(|| {
            let before = on_audio_thread();
            mark_audio_thread();
            (before, on_audio_thread())
        })
        .join()
        .unwrap();
        assert_eq!(marked, (false, true));
        assert!(!on_audio_thread(), "marking another thread leaves this one alone");
    }

    /// What `on_audio_thread` says inside each strip job, for strips run from a thread that is
    /// (or is not) the audio thread.
    fn seen_by_strips(audio: bool) -> Vec<bool> {
        jobs(audio).into_iter().map(|j| j.audio).collect()
    }

    /// Where one strip job ran.
    #[derive(Clone, Copy, Default)]
    struct Job {
        audio: bool,
        /// On a worker of Rayon's global pool.
        global: bool,
        /// On a worker of the render pool.
        render: bool,
    }

    fn jobs(audio: bool) -> Vec<Job> {
        init_render_pool();
        std::thread::spawn(move || {
            if audio {
                mark_audio_thread();
            }
            let mut work = vec![Job::default(); 64];
            run_strips(&mut work, true, |w| {
                // Long enough that the pool's workers take part.
                std::thread::sleep(std::time::Duration::from_millis(1));
                let render = std::thread::current().name().is_some_and(|n| n.starts_with("mix-render-"));
                let global = rayon::current_thread_index().is_some() && !render;
                *w = Job { audio: on_audio_thread(), global, render };
            });
            work
        })
        .join()
        .unwrap()
    }

    #[test]
    fn strips_rendered_for_the_audio_thread_count_as_the_audio_thread_and_only_then() {
        assert!(seen_by_strips(true).iter().all(|&rt| rt), "a worker rendering for the audio thread is on it");
        assert!(seen_by_strips(false).iter().all(|&rt| !rt), "the workers don't stay marked afterwards");
    }

    /// ADR 0012 gate G1: a render on the audio thread dispatches no job to the global pool.
    #[test]
    fn audio_thread_strips_never_reach_the_global_rayon_pool() {
        let _switch = SERIAL_SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        MixEngine::force_serial(false); // the pool is opt-in; this tests where it runs
        let audio = jobs(true);
        assert!(audio.iter().all(|j| !j.global), "an audio-thread strip ran on the global pool");
        if RENDER_POOL.get().is_some_and(Option::is_some) {
            assert!(audio.iter().any(|j| j.render), "with a render pool, the strips fan out to it");
        }
        let offline = jobs(false);
        MixEngine::force_serial(true); // back to the default
        assert!(offline.iter().all(|j| !j.render), "an offline render never takes the render pool");
    }

    /// Serializes the tests that depend on the process-wide serial switch.
    static SERIAL_SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The F01 safety valve: with the switch on, the audio thread renders every strip itself;
    /// off again, the pool takes them back.
    #[test]
    fn force_serial_keeps_audio_thread_strips_on_the_audio_thread() {
        let _switch = SERIAL_SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        MixEngine::force_serial(true);
        let forced = jobs(true);
        let was_forced = MixEngine::serial_forced();
        MixEngine::force_serial(false);
        assert!(was_forced);
        assert!(forced.iter().all(|j| j.audio && !j.render && !j.global), "a strip left the audio thread while forced serial");
        if RENDER_POOL.get().is_some_and(Option::is_some) {
            assert!(jobs(true).iter().any(|j| j.render), "the pool is used again once the switch is off");
        }
        MixEngine::force_serial(true); // back to the default
    }

    /// Sixteen strips of distinct deterministic material, levels, pans and built-in inserts.
    fn sixteen_strips() -> Session {
        use transit_audio_io::AudioBuffer;
        use transit_model::{Insert, SourceAudio, SourceId};
        let mut s = Session::default();
        let len = 48_000usize;
        for k in 0..16u32 {
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            let f = 0.003 + 0.0007 * k as f32;
            let signal: Vec<f32> = (0..len).map(|i| (i as f32 * f).sin() * 0.2 + ((i * (k as usize + 3)) % 97) as f32 * 1e-3).collect();
            let src = SourceId(1000 + u64::from(k));
            s.pool.insert(src, std::sync::Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal] })));
            let id = s.new_clip_id();
            let tr = s.track_mut(t).unwrap();
            tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", src, 0, 0, len as i64));
            tr.mixer.volume_db = -(k as f32) * 0.5;
            tr.mixer.pan = vec![(k as f32 / 7.5) - 1.0];
            tr.mixer.inserts[0] = Some(Insert::new(if k % 2 == 0 { "compressor" } else { "room_reverb" }));
        }
        s
    }

    /// Render `blocks` blocks of 512 on a fresh thread, marked as the audio thread or not.
    fn render_blocks(s: &Session, serial: bool, audio: bool, blocks: i64) -> Vec<Vec<f32>> {
        let s = s.clone();
        let mut eng = RangeRender::new(&s, None, Range::new(0, blocks * 512), 512).eng;
        eng.serial = serial;
        std::thread::spawn(move || {
            if audio {
                mark_audio_thread();
            }
            let mut all = vec![Vec::new(); 2];
            let mut out = vec![vec![0.0f32; 512]; 2];
            for b in 0..blocks {
                eng.render_active(b * 512, 512, &mut out);
                for (a, o) in all.iter_mut().zip(&out) {
                    a.extend_from_slice(o);
                }
            }
            all
        })
        .join()
        .unwrap()
    }

    #[test]
    fn parallel_and_serial_renders_are_bit_identical() {
        let s = sixteen_strips();
        let serial = render_blocks(&s, true, false, 40);
        assert!(serial[0].iter().any(|x| x.abs() > 1e-3), "the fixture should sound");
        assert!(serial == render_blocks(&s, false, true, 40), "render pool differs from serial");
        assert!(serial == render_blocks(&s, false, false, 40), "global pool differs from serial");
        assert!(serial == render_blocks(&s, true, true, 40), "serial on the audio thread differs");
    }
}
