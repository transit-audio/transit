use super::*;
use transit_rt::midi_defer::DeferStats;
use transit_rt::worker_set::figures::Figures;

/// The counters the callback keeps (relaxed; read by [`StripFaultsReader`]).
#[derive(Default)]
pub(super) struct Counters {
    pub(super) queue: [AtomicU64; 11],
    pub(super) measurement_status: AtomicU32,
    pub(super) workers: AtomicU64,
    pub(super) blocks: AtomicU64,
    pub(super) overruns_busy: AtomicU64,
    pub(super) overruns_late: AtomicU64,
    pub(super) overruns_unstarted: AtomicU64,
    pub(super) refused_no_history: AtomicU64,
    pub(super) refused_worst_case: AtomicU64,
    pub(super) refused_recent_overrun: AtomicU64,
    pub(super) late_results: AtomicU64,
    pub(super) inline_renders: AtomicU64,
    pub(super) callback_renders: AtomicU64,
    pub(super) faulted: AtomicU64,
    pub(super) suspect: AtomicU64,
    pub(super) conflicts: AtomicU64,
    pub(super) stale: AtomicU64,
    pub(super) overrun_blocks: AtomicU64,
    pub(super) misuse: AtomicU64,
    pub(super) overshoot: AtomicU64,
    pub(super) deadline_percent: AtomicU64,
    pub(super) deadline_frames: AtomicU64,
    pub(super) deadline_rate: AtomicU64,
    pub(super) deadline_overridden: AtomicBool,
    /// Strips skipped because a key source missed (#174): the worker set's count, mirrored.
    pub(super) upstream_missed: AtomicU64,
    /// Auxes skipped because a phase-1 key source missed (#174).
    pub(super) upstream_tail: AtomicU64,
}

/// Reads an engine's [`StripFaults`] from any thread (relaxed loads; each figure exact on its own).
#[derive(Clone)]
pub struct StripFaultsReader {
    pub(super) counters: Arc<Counters>,
    pub(super) defer: Arc<DeferStats>,
    /// The worker set's per-strip figures and histograms (#169; only the figures, never a strip).
    pub(super) figures: Arc<Figures>,
    /// Per strip index: the track it last published (the phase marker's strip index).
    pub(super) tracks: Arc<[AtomicU32]>,
    /// Per strip index: blocks in a row its strip has been away (#213).
    pub(super) held: Arc<[AtomicU32]>,
    /// Per strip index: the id of the track it runs, stored while it is away (#213).
    pub(super) held_ids: Arc<[AtomicU64]>,
}

impl StripFaultsReader {
    /// The ids of the tracks whose strips a render worker has
    /// held for [`HELD_SECS`] or more: the plug-in on it does not return from `process`. Relaxed
    /// loads off the audio thread (#213).
    #[must_use]
    pub fn held_tracks(&self) -> Vec<u64> {
        let (frames, rate) = (self.counters.deadline_frames.load(Ordering::Relaxed), self.counters.deadline_rate.load(Ordering::Relaxed));
        if frames == 0 {
            return Vec::new();
        }
        let limit = (u64::from(HELD_SECS) * rate).div_ceil(frames);
        self.held
            .iter()
            .enumerate()
            .filter(|(_, h)| u64::from(h.load(Ordering::Relaxed)) >= limit.max(1))
            .filter_map(|(i, _)| self.held_ids.get(i).map(|t| t.load(Ordering::Relaxed)))
            .collect()
    }

    /// The figures now.
    #[must_use]
    pub fn snapshot(&self) -> StripFaults {
        let c = &self.counters;
        let r = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let d = self.defer.total();
        StripFaults {
            queue: transit_rt::worker_set::queue::QueueSnapshot {
                published: r(&c.queue[0]), claimed: r(&c.queue[1]), inline_pre: r(&c.queue[2]),
                unclaimed_at_deadline: r(&c.queue[3]), deadline_reclaimed_tail: r(&c.queue[4]),
                classified: r(&c.queue[5]), queue_over: r(&c.queue[6]), late_w: r(&c.queue[7]),
                late_after_queue: r(&c.queue[8]), forgone: r(&c.queue[9]), tail_callback: r(&c.queue[10]),
            },
            measurement_status: c.measurement_status.load(Ordering::Relaxed),
            workers: r(&c.workers),
            deadline_percent: r(&c.deadline_percent),
            deadline_frames: r(&c.deadline_frames),
            deadline_rate: r(&c.deadline_rate),
            deadline_overridden: c.deadline_overridden.load(Ordering::Relaxed),
            blocks: r(&c.blocks),
            overruns_busy: r(&c.overruns_busy),
            overruns_late: r(&c.overruns_late),
            overruns_unstarted: r(&c.overruns_unstarted),
            late_results: r(&c.late_results),
            inline_renders: r(&c.inline_renders),
            inline_refused_no_history: r(&c.refused_no_history),
            inline_refused_worst_case: r(&c.refused_worst_case),
            inline_refused_recent_overrun: r(&c.refused_recent_overrun),
            callback_renders: r(&c.callback_renders),
            faulted: r(&c.faulted),
            suspect: r(&c.suspect),
            conflicts: r(&c.conflicts).saturating_add(r(&c.stale)).saturating_add(r(&c.misuse)),
            overrun_blocks: r(&c.overrun_blocks),
            deadline_overshoots: r(&c.overshoot),
            midi_deferred: d.deferred,
            midi_dropped: d.dropped,
            midi_silences: d.silences,
            upstream_missed: r(&c.upstream_missed).saturating_add(r(&c.upstream_tail)),
            top_strips: self.figures.top_overruns::<TOP_STRIPS>().map(|t| {
                t.map(|(i, f)| StripFigure {
                    track: self.tracks.get(i).map_or(u32::MAX, |t| t.load(Ordering::Relaxed)),
                    upstream_missed: f.upstream_missed,
                    upstream_origin: f.upstream_origin.and_then(|o| self.tracks.get(o)).map_or(u32::MAX, |t| t.load(Ordering::Relaxed)),
                    overruns_busy: f.overruns_busy,
                    overruns_late: f.overruns_late,
                    overruns_unstarted: f.overruns_unstarted,
                    inline_renders: f.inline_renders,
                    cost_ns: f.cost_ns,
                    worst_ns: f.worst_ns,
                })
            }),
            lateness: self.figures.lateness(),
            wake: self.figures.wake(),
        }
    }
}
