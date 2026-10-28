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
pub(crate) const PARALLEL_STRIPS: usize = 4;

/// An engine's render workers never number more than this, whatever the core count.
pub const MAX_RENDER_WORKERS: usize = 8;

/// Environment variable: `1` opts in to the multicore strip render for the audio thread, as
/// [`MixEngine::force_serial`]`(false)` does: the live engine starts its render workers
/// ([`MixEngine::start_render_workers`], issue #124, ADR 0029). Read once, when the first engine
/// is made.
///
/// Serial is the realtime default until the measurement of ADR 0012 gate G1 on the reference
/// hardware says otherwise (#124 gate 6). `SOUNDCRAFT_RENDER_POOL` remains an alias when this
/// variable is unset.
pub const RENDER_POOL_ENV: &str = "TRANSIT_RENDER_POOL";

/// Environment variable: `1` leaves the render workers at the OS default priority instead of
/// promoting them with [`transit_rt::promote_audio_thread`]. Read when the workers start. It does
/// not affect the audio callback thread: this crate never promotes that.
/// `SOUNDCRAFT_RENDER_NO_PRIORITY` remains an alias when this variable is unset.
pub const RENDER_NO_PRIORITY_ENV: &str = "TRANSIT_RENDER_NO_PRIORITY";

/// Audio-thread strips render serially when set ([`MixEngine::force_serial`]). On by default;
/// [`RENDER_POOL_ENV`] clears it at startup.
pub(crate) static FORCE_SERIAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// True when `name` is set to `1` (or `true`/`yes`, any case).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn env_flag(name: &str, alias: &str) -> bool {
    flag_value(std::env::var(name).ok().as_deref(), std::env::var(alias).ok().as_deref())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn flag_value(value: Option<&str>, alias: Option<&str>) -> bool {
    value.or(alias).is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

/// Reads [`RENDER_POOL_ENV`] once per process (never on the audio thread): set, it clears
/// [`FORCE_SERIAL`]. [`MixEngine::new`] calls this.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn init_render_switch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !on_audio_thread() {
        ONCE.call_once(|| {
            if env_flag(RENDER_POOL_ENV, "SOUNDCRAFT_RENDER_POOL") {
                log::info!("mix: {RENDER_POOL_ENV} is set; the live engine renders strips on its render workers");
                FORCE_SERIAL.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn init_render_switch() {}

/// Runs `f` (one phase-1 strip's processing) unless the strip holds a panic, and keeps a panic
/// out of it in [`Strip::panic`] instead of letting it unwind. A panic that reached a worker's
/// frame would end there and drop its payload, which a plug-in may own, on that thread (every
/// last-owner drop belongs on the permitted thread). Caught the same way on every path (serial,
/// render workers, offline), so the render behaves the same whichever way its strips ran: it
/// re-raises the first strip's.
pub(crate) fn guarded(strip: &mut Strip, f: impl FnOnce(&mut Strip)) {
    if strip.panic.is_some() {
        return;
    }
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *strip))) {
        strip.panic = Some(panic);
    }
}

/// Process strips: serially on the audio thread (its render workers are driven by
/// `StripPool::render`, not here), in parallel on Rayon's global pool for an offline render when
/// there are enough of them. The strips are independent and are summed afterwards in a fixed
/// order, so the result is the same whichever way they ran.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_strips<T: Send>(work: &mut [T], parallel: bool, f: impl Fn(&mut T) + Sync + Send) {
    use rayon::prelude::*;
    if !parallel || work.len() < PARALLEL_STRIPS || on_audio_thread() {
        work.iter_mut().for_each(f);
    } else {
        // A global worker renders for an offline caller here: it is not the audio thread.
        work.par_iter_mut().for_each(|w| as_audio_thread(false, || f(w)));
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn run_strips<T: Send>(work: &mut [T], _parallel: bool, f: impl Fn(&mut T) + Sync + Send) {
    work.iter_mut().for_each(f);
}

/// Stop or panic for the parts of a strip a render owns: plug-ins reset, the instrument's
/// sources released and the instrument reset ([`MixEngine::reset`] does the rest).
pub(crate) fn reset_render_state(st: &mut Strip) {
    for p in st.plugins.iter_mut().flatten() {
        p.plugin.reset();
        p.failed = false;
    }
    if let Some(i) = &mut st.instrument {
        if let Some(e) = i.events.as_deref_mut() {
            e.release_all(&mut *i.plugin);
        }
        i.plugin.all_notes_off();
        i.plugin.reset();
        i.failed = false;
    }
    st.chase = true;
}

/// Releases every source the strip's instrument holds, and tells the plug-in to release too.
pub(crate) fn release_instrument(st: &mut Strip) {
    if let Some(i) = &mut st.instrument {
        if let Some(e) = i.events.as_deref_mut() {
            e.release_all(&mut *i.plugin);
        }
        i.plugin.all_notes_off();
    }
}

/// The multicore strip render's stream faults (ADR 0029), cumulative for the engine's life. Read
/// with [`MixEngine::strip_faults`]; `docs/troubleshooting.md` defines each figure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StripFaults {
    /// Render worker threads (0: no pool, the strips render serially).
    pub workers: u64,
    /// Blocks the pool rendered.
    pub blocks: u64,
    /// Strip overruns because a worker still held the strip from an earlier block: not rendered
    /// this block (silence, and a hole in the strip's own time).
    pub overruns_busy: u64,
    /// Strip overruns because the worker that took the strip had not finished at the deadline
    /// (silence this block; the strip's time stays continuous).
    pub overruns_late: u64,
    /// Strip overruns because no thread had started the strip at the deadline and the time left
    /// could not fit it (silence, and a hole).
    pub overruns_unstarted: u64,
    /// Late results: renders finished after their block's deadline, discarded.
    pub late_results: u64,
    /// Strips the callback rendered itself inside the deadline (unstarted by any worker).
    pub inline_renders: u64,
    /// Unstarted strips the callback did not render itself because their processing chain had
    /// never been rendered (no cost history; once per strip per block).
    pub inline_refused_no_history: u64,
    /// Unstarted strips the callback did not render itself because their worst observed render
    /// time did not fit in the time left.
    pub inline_refused_worst_case: u64,
    /// Unstarted strips the callback did not render itself because they had overrun in the last
    /// few blocks.
    pub inline_refused_recent_overrun: u64,
    /// Strips the callback rendered itself because they read the live input or hardware, or lie
    /// past the pool's cells.
    pub callback_renders: u64,
    /// Strip renders that faulted on a worker (silence; the strip is suspect).
    pub faulted: u64,
    /// Blocks a suspect strip stayed silent until its instance was replaced.
    pub suspect: u64,
    /// Refused cell borrows and results of the wrong block (both protocol faults: zero).
    pub conflicts: u64,
    /// Blocks with at least one strip overrun.
    pub overrun_blocks: u64,
    /// Blocks whose wait for the workers ended more than 100 µs past the strips' deadline (the
    /// callback started late, or an inline render ran longer than its worst observed time).
    pub deadline_overshoots: u64,
    /// Live MIDI events deferred for a strip a worker held.
    pub midi_deferred: u64,
    /// Deferred MIDI events the overflow rule dropped (silences included).
    pub midi_dropped: u64,
    /// Deferred-MIDI rings that overflowed past their release class: the strip got All Sound
    /// Off, All Notes Off and sustain off.
    pub midi_silences: u64,
    /// The strips with the most overruns, most first (#169): at most [`TOP_STRIPS`], only strips
    /// with an overrun, the rest `None`.
    pub top_strips: [Option<StripFigure>; TOP_STRIPS],
    /// Late results by how far past their block's deadline the render finished, by bucket
    /// ([`POOL_HISTOGRAM_LIMITS_NS`], then the open last bucket).
    pub lateness: [u64; POOL_HISTOGRAM_BUCKETS],
    /// Worker wake latency, from a block's publish of a strip to a worker's claim of it, by
    /// bucket ([`POOL_HISTOGRAM_LIMITS_NS`], then the open last bucket).
    pub wake: [u64; POOL_HISTOGRAM_BUCKETS],
}

/// The most strips [`StripFaults::top_strips`] lists.
pub const TOP_STRIPS: usize = 8;

/// Upper limits of the buckets of [`StripFaults::lateness`] and [`StripFaults::wake`],
/// nanoseconds: 50 µs, 100 µs, 200 µs, 500 µs, 1 ms, 2 ms.
pub const POOL_HISTOGRAM_LIMITS_NS: [u64; 6] = transit_rt::worker_set::figures::LIMITS_NS;

/// Buckets of [`StripFaults::lateness`] and [`StripFaults::wake`]: one per limit, then the open
/// last one.
pub const POOL_HISTOGRAM_BUCKETS: usize = transit_rt::worker_set::figures::BUCKETS;

/// One strip's render worker figures (#169), cumulative for the strip index since the device
/// opened, named by the track the index holds now. The counts restart from zero when another
/// track's strip takes the index (the cost figures follow the processing chain, as admission
/// does).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StripFigure {
    /// The track index, as the callback's phase marker names a strip.
    pub track: u32,
    /// Overruns because a worker still held the strip.
    pub overruns_busy: u64,
    /// Overruns because the strip's worker had not finished by the deadline.
    pub overruns_late: u64,
    /// Overruns because no thread had started the strip by the deadline.
    pub overruns_unstarted: u64,
    /// Inline renders by the audio thread.
    pub inline_renders: u64,
    /// The render cost estimate (an average), nanoseconds; 0 without a history.
    pub cost_ns: u64,
    /// The worst recent render cost (a decaying maximum), nanoseconds; 0 without a history.
    pub worst_ns: u64,
}

impl StripFigure {
    /// Overruns of every cause.
    #[must_use]
    pub const fn overruns(&self) -> u64 {
        self.overruns_busy + self.overruns_late + self.overruns_unstarted
    }
}

impl StripFaults {
    /// Overruns of every cause.
    #[must_use]
    pub const fn overruns(&self) -> u64 {
        self.overruns_busy + self.overruns_late + self.overruns_unstarted
    }
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
    }

    fn jobs(audio: bool) -> Vec<Job> {
        std::thread::spawn(move || {
            if audio {
                mark_audio_thread();
            }
            let mut work = vec![Job::default(); 64];
            run_strips(&mut work, true, |w| {
                // Long enough that the global pool's workers take part.
                std::thread::sleep(std::time::Duration::from_millis(1));
                *w = Job { audio: on_audio_thread(), global: rayon::current_thread_index().is_some() };
            });
            work
        })
        .join()
        .unwrap()
    }

    #[test]
    fn strips_rendered_for_the_audio_thread_count_as_the_audio_thread_and_only_then() {
        assert!(seen_by_strips(true).iter().all(|&rt| rt), "a strip rendered for the audio thread is on it");
        assert!(seen_by_strips(false).iter().all(|&rt| !rt), "an offline render's strips are not");
    }

    /// ADR 0012 gate G1: a render on the audio thread dispatches no job to the global pool (its
    /// render workers are an engine's own `transit-rt` worker set, see `strip_pool`).
    #[test]
    fn audio_thread_strips_never_reach_the_global_rayon_pool() {
        assert!(jobs(true).iter().all(|j| !j.global), "an audio-thread strip ran on the global pool");
        assert!(jobs(false).iter().any(|j| j.global), "an offline render still fans out on the global pool");
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

    /// Render `blocks` blocks of 512 on a fresh thread, marked as the audio thread or not; with
    /// `workers`, through the engine's render workers (the pool path).
    fn render_blocks(s: &Session, serial: bool, audio: bool, blocks: i64) -> Vec<Vec<f32>> {
        render_blocks_with(s, serial, audio, false, blocks)
    }

    fn render_blocks_with(s: &Session, serial: bool, audio: bool, workers: bool, blocks: i64) -> Vec<Vec<f32>> {
        let s = s.clone();
        let mut eng = RangeRender::new(&s, None, Range::new(0, blocks * 512), 512).eng;
        eng.serial = serial;
        if workers {
            assert!(eng.start_render_workers(64) > 0 || std::thread::available_parallelism().map_or(1, |n| n.get()) < 2);
            // A worker descheduled past the default deadline would make its strip silence: wait.
            eng.set_strip_deadline(Some(std::time::Duration::from_secs(30)));
        }
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
            if let Some(f) = eng.strip_faults().map(|r| r.snapshot()) {
                assert_eq!((f.blocks, f.overruns()), (u64::try_from(blocks).unwrap(), 0), "every block went through the workers: {f:?}");
            }
            // Off the audio thread: the workers shut down here.
            unmark_audio_thread();
            drop(eng.into_retired());
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
        assert!(serial == render_blocks(&s, false, false, 40), "global pool differs from serial");
        assert!(serial == render_blocks(&s, true, true, 40), "serial on the audio thread differs");
        let _switch = crate::strip_pool::SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        MixEngine::force_serial(false);
        let pool = render_blocks_with(&s, false, true, true, 40);
        MixEngine::force_serial(true);
        assert!(serial == pool, "the render workers differ from serial");
    }
}
