use super::plan::PlanTables;
use super::*;

pub(super) struct TailCtx<'a, 'b> {
    pub(super) s: &'a Session,
    pub(super) p1: &'a mut [Strip],
    pub(super) auxes: &'a mut [Strip],
    pub(super) phase1: usize,
    pub(super) keys: &'a mut KeyBank,
    pub(super) graph: &'a mut graph::StripGraph,
    pub(super) busses: &'a mut BusTable,
    pub(super) main: &'a mut [Vec<f32>],
    pub(super) main_fmt: ChannelFormat,
    pub(super) sinks: &'a mut [Sink],
    pub(super) pos: Samples,
    pub(super) frames: usize,
    pub(super) host_frame: u64,
    pub(super) any_solo: bool,
    pub(super) live_in: &'a LiveInput<'b>,
    pub(super) law: PanLaw,
    pub(super) main_arrival: usize,
    pub(super) status: &'a mut PlanStatus,
    pub(super) pool: Option<&'a strip_pool::StripPool>,
    #[cfg(feature = "testing")]
    pub(super) mutes: &'a sched_seam::Mutes,
}

impl TailCtx<'_, '_> {
    pub(super) fn render(self) {
        let Self {
            s,
            p1,
            auxes,
            phase1,
            keys,
            graph,
            busses,
            main,
            main_fmt,
            sinks,
            pos,
            frames,
            host_frame,
            any_solo,
            live_in,
            law,
            main_arrival,
            status,
            pool,
            ..
        } = self;
        let track = |st: &Strip| s.tracks.get(usize::from(st.track));
        for (i, st) in p1.iter_mut().enumerate() {
            let Some(t) = track(st).filter(|t| live(t) && !is_aux(t)) else { continue };
            // An overrun, a faulted or a suspect strip is silence for the block (ADR 0029).
            if pool.is_some_and(|p| p.is_silent(i) || p.is_away(i)) {
                continue;
            }
            #[cfg(feature = "testing")]
            if self.mutes.silent(i) {
                continue;
            }
            st.meter.measure(&st.buf, frames, st.gr, host_frame);
            st.meter.plugin_failures = st.failures;
            let need = route_into(t, st, pos, frames, MixOut { busses, main, main_fmt, sinks, law, main_arrival });
            status.note_capacity(need, st);
        }
        // Phase 2: auxes in dependency order (they read busses).
        for k in 0..auxes.len() {
            let (before, rest) = auxes.split_at_mut(k);
            let Some(st) = rest.first_mut() else { continue };
            let Some(t) = track(st).filter(|t| live(t)) else { continue };
            let at = phase1.saturating_add(k);
            // The graph's rule on the serial tail (#174): an aux keyed from a phase-1 strip that
            // missed this block, or from an earlier aux skipped by this rule, is skipped (silent, not
            // metered, its lines neither run nor idled). A skipped aux's buffer still holds the last
            // block it ran, so a later aux keyed from it must not run either.
            let skipped = pool.is_some_and(|p| {
                let tail = &graph.tail_skipped;
                keys.sources_of(at).any(|j| match j.checked_sub(phase1) {
                    None => p.is_silent(j) || p.is_away(j),
                    Some(a) => a < k && tail.get(a).copied().unwrap_or(false),
                })
            });
            // Test seam (issue #210): a muted serial block applies the same rule to its mutes.
            #[cfg(feature = "testing")]
            let skipped = skipped
                || (self.mutes.any()
                    && keys.sources_of(at).any(|j| match j.checked_sub(phase1) {
                        None => self.mutes.silent(j),
                        Some(a) => a < k && graph.tail_skipped.get(a).copied().unwrap_or(false),
                    }));
            if let Some(f) = graph.tail_skipped.get_mut(k) {
                *f = skipped;
            }
            if skipped {
                if let Some(p) = pool {
                    p.note_tail_skip();
                }
                continue;
            }
            let strip = |i: usize| match i.checked_sub(phase1) {
                None => p1.get(i),
                Some(j) => before.get(j).filter(|b| track(b).is_some_and(live)),
            };
            keys.fill(at..at + 1, strip, busses, frames);
            process_strip(t, st, &StripRender { s, busses, live: live_in, pos, frames, any_solo, keys });
            st.meter.measure(&st.buf, frames, st.gr, host_frame);
            st.meter.plugin_failures = st.failures;
            let need = route_into(t, st, pos, frames, MixOut { busses, main, main_fmt, sinks, law, main_arrival });
            status.note_capacity(need, st);
        }
    }
}

impl PlanTables {
    /// `away`: the strip at that table index is held by a render worker (its keys and its
    /// chain are in its cell): its delays stay as they were until it is home.
    pub(super) fn align_keys(&mut self, s: &Session, pos: Samples, duplex: bool, status: &mut PlanStatus, away: &dyn Fn(usize) -> bool) {
        let PlanTables { strips, phase1, aux_end, keys, busses, sinks, .. } = self;
        let (phase1, aux_end) = (*phase1, *aux_end);
        let track = |st: &Strip| s.tracks.get(usize::from(st.track));
        // Sidechain keys (ADR 0015 section 4): a key and the signal it meets must come from the
        // same source sample time, but a strip's output latency is known only after phase 1. So
        // with keys in the plan, the latencies of this block (each strip's inserts, as the
        // forward pass below counts them) are walked once before it, in render order, to give
        // every key edge its delay (`KeyBank::align`). The busses' marks are cleared after.
        if !keys.is_empty() {
            let pdc = s.edit.delay_compensation;
            let mut walked = 0usize;
            for at in 0..aux_end.min(strips.len()) {
                let (before, rest) = strips.split_at_mut(at);
                let Some(st) = rest.first_mut() else { continue };
                let Some(t) = track(st) else { continue };
                let aux = is_aux(t);
                let alive = live(t);
                if at < phase1 && (!alive || aux) {
                    continue;
                }
                let mut arrive = 0usize;
                if aux && let Route::Bus(b) = &t.mixer.input {
                    arrive = busses.bus(*b).map_or(0, |x| x.arrival);
                }
                if alive && !away(at) {
                    let from = |sp: &KeySpec| match sp.from {
                        KeyEnd::Strip(j) => before.get(usize::from(j)).filter(|b| track(b).is_some_and(live)).map_or(0, |b| b.pdc_out),
                        KeyEnd::Bus(b) => {
                            if aux {
                                busses.bus(b).map_or(0, |x| x.arrival)
                            } else {
                                0
                            }
                        }
                    };
                    let short = keys.align(s, t, st, key::KeyAlignment { at, arrive, pos, pdc, duplex }, &from);
                    status.note_capacity(short, st);
                }
                pdc_visit(t, st, at, alive, PdcWalk { busses, sinks, main_arrival: &mut walked, pdc });
            }
            for (_, b) in busses.0.iter_mut() {
                b.arrival = 0;
                b.reader = usize::MAX;
            }
            for k in sinks.iter_mut() {
                k.arrival = 0;
            }
        }
    }
}

pub(super) struct RenderPhases<'a> {
    pub(super) p1: &'a mut [Strip],
    pub(super) auxes: &'a mut [Strip],
    pub(super) others: &'a mut [Strip],
    pub(super) phase1: usize,
}

pub(super) struct DeviceLatency {
    pub(super) main: usize,
    pub(super) device: usize,
}

/// Walk the rendered strips and pending aux/master inserts, then align every sink to the device.
pub(super) fn fold_latency(
    s: &Session,
    phases: RenderPhases<'_>,
    busses: &mut BusTable,
    sinks: &mut [Sink],
    pos: Samples,
    duplex: bool,
) -> DeviceLatency {
    let RenderPhases { p1, auxes, others, phase1 } = phases;
    let track = |st: &Strip| s.tracks.get(usize::from(st.track));
    let pdc = s.edit.delay_compensation;
    if pdc {
        for st in auxes.iter_mut() {
            if let Some(t) = track(st).filter(|t| !t.inactive) {
                note_insert_latency(t, st, pos, duplex);
            }
        }
    }
    let mut lmax_m = 0usize;
    if pdc {
        for st in others.iter_mut() {
            if let Some(t) = track(st).filter(|t| t.kind == TrackKind::Master && !t.inactive) {
                note_insert_latency(t, st, pos, duplex);
                lmax_m = lmax_m.saturating_add(st.latency);
            }
        }
    }
    // Forward pass in render order (the table order): phase-1 strips, then the auxes.
    let mut main_arrival = 0usize;
    for (at, st) in p1.iter_mut().enumerate() {
        if let Some(t) = track(st).filter(|t| live(t) && !is_aux(t)) {
            pdc_visit(t, st, at, true, PdcWalk { busses, sinks, main_arrival: &mut main_arrival, pdc });
        }
    }
    // Inactive auxes still mark the bus they read (see `pdc_visit`).
    for (k, st) in auxes.iter_mut().enumerate() {
        if let Some(t) = track(st) {
            pdc_visit(t, st, phase1.saturating_add(k), live(t), PdcWalk { busses, sinks, main_arrival: &mut main_arrival, pdc });
        }
    }
    // One device latency for every sink (ADR 0015 section 6): Main's (its sources' arrival plus
    // the master inserts') or the slowest path's, whichever is longer. Each sink is padded to
    // it, Main to it less its inserts, so stems and cue mixes leave the device with Main. With
    // no sinks this is Main's own arrival, as before.
    let device = sinks.iter().fold(main_arrival.saturating_add(lmax_m), |l, k| l.max(k.arrival));
    main_arrival = device.saturating_sub(lmax_m);
    for k in sinks.iter_mut() {
        k.arrival = device;
    }
    DeviceLatency { main: main_arrival, device }
}
