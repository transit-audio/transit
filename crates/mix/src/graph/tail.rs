//! Tail topology, described before table reuse is decided. Routes are structural: gains,
//! mutes and automation never remove a writer from this description.
use super::*;
use transit_rt::task_graph::EdgeKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Destination {
    Bus(BusId),
    Main,
    Sink(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub(crate) track: TrackId,
    pub(crate) live: bool,
    pub(crate) input: Option<(BusId, ChannelFormat)>,
    pub(crate) keys: Vec<KeySpec>,
    pub(crate) destinations: Vec<Destination>,
}

pub(crate) fn describe(
    s: &Session,
    order: &[usize],
    phase1: usize,
    aux_end: usize,
    keys: &[KeySpec],
    routes: &[Vec<Option<usize>>],
    hardware: &[(TrackId, crate::hardware::HwSpec)],
) -> Vec<Row> {
    let reader = |bus: BusId| {
        order.iter().enumerate().take(aux_end).skip(phase1)
            .find(|(_, ti)| s.tracks.get(**ti).is_some_and(|t| t.mixer.input == Route::Bus(bus)))
            .map_or(usize::MAX, |(at, _)| at)
    };
    order.iter().enumerate().take(aux_end).skip(phase1).filter_map(|(at, &ti)| {
        let t = s.tracks.get(ti)?;
        let input = match t.mixer.input {
            Route::Bus(b) => s.busses.iter().find(|x| x.id == b).map(|x| (b, x.format)),
            _ => None,
        };
        let mut destinations = Vec::new();
        if live(t) {
            let mut add = |edge: usize, route: &Route| match route {
                Route::Bus(b) if at < reader(*b) && s.busses.iter().any(|x| x.id == *b) => destinations.push(Destination::Bus(*b)),
                Route::Main => destinations.push(Destination::Main),
                Route::Hardware(_) => {
                    if let Some(k) = routes.get(at).and_then(|r| r.get(edge)).copied().flatten() {
                        destinations.push(Destination::Sink(k));
                    }
                }
                _ => {},
            };
            add(0, &t.mixer.output);
            for (i, send) in t.mixer.sends.iter().enumerate() {
                if let Some(send) = send && matches!(send.target, Route::Bus(_) | Route::Hardware(_)) {
                    add(i + 1, &send.target);
                }
            }
            for (_, hw) in hardware.iter().filter(|(id, _)| *id == t.id) {
                if let Some(k) = hw.sink { destinations.push(Destination::Sink(usize::from(k))); }
            }
        }
        destinations.sort_unstable();
        destinations.dedup();
        Some(Row { track: t.id, live: live(t), input,
            keys: keys.iter().filter(|k| usize::from(k.keyed) == at).copied().collect(), destinations })
    }).collect()
}

#[derive(Default)]
pub(crate) struct TailGraph {
    pub(crate) graph: TaskGraph,
    pub(crate) run: GraphRun,
}

pub(crate) fn render_task(phase1: usize, row: usize) -> usize { phase1 + 1 + 2 * row }
pub(crate) fn commit_task(phase1: usize, row: usize) -> usize { render_task(phase1, row) + 1 }

pub(crate) fn build(base: &StripGraph, rows: &[Row]) -> TailGraph {
    let phase1 = base.graph.tasks();
    let n = phase1 + 1 + 2 * rows.len();
    let mut slots = vec![None; n];
    let mut edges = Vec::new();
    for (i, slot) in slots.iter_mut().enumerate().take(phase1) {
        *slot = Some(i);
        edges.push((i, phase1, EdgeKind::Soft));
        for &to in base.graph.dependents(i) { edges.push((i, to, EdgeKind::Hard)); }
    }
    for (k, row) in rows.iter().enumerate() {
        let r = render_task(phase1, k);
        let c = r + 1;
        edges.push((phase1, r, EdgeKind::Soft));
        edges.push((r, c, EdgeKind::Soft));
        if !row.live { continue; }
        slots[r] = Some(phase1 + k);
        for key in &row.keys {
            if let KeyEnd::Strip(j) = key.from {
                let j = usize::from(j);
                if j < phase1 + k {
                    let source = if j < phase1 { j } else { render_task(phase1, j - phase1) };
                    edges.push((source, r, EdgeKind::Hard));
                }
            }
        }
        for (w, writer) in rows.iter().enumerate() {
            let input = row.input.is_some_and(|(b, _)| writer.destinations.contains(&Destination::Bus(b)));
            let key = row.keys.iter().any(|key| match key.from {
                KeyEnd::Bus(b) => writer.destinations.contains(&Destination::Bus(b)),
                _ => false,
            });
            if w < k && (input || key) { edges.push((commit_task(phase1, w), r, EdgeKind::Soft)); }
            if w > k && key { edges.push((r, commit_task(phase1, w), EdgeKind::Soft)); }
        }
        for dst in &row.destinations {
            if let Some(w) = (0..k).rev().find(|&w| rows[w].destinations.contains(dst)) {
                edges.push((commit_task(phase1, w), c, EdgeKind::Soft));
            }
        }
    }
    TailGraph { graph: TaskGraph::with_slots(NEXT_GRAPH.fetch_add(1, Ordering::Relaxed), &slots, &edges), run: GraphRun::new(n) }
}
