//! Control-side Low Latency selection. No session flags are changed.
use crate::*;
use std::collections::{BTreeMap, BTreeSet};
use transit_model::{EdgeKind, RouteNode};

pub(crate) fn observe_raw<'a>(strips: impl Iterator<Item = &'a mut Strip>, status: &mut PlanStatus) {
    use std::sync::atomic::Ordering;
    for st in strips {
        guarded(st, |st| {
            for slot in st.instrument.iter().chain(st.plugins.iter().flatten()) {
                let raw = if slot.failed { 0 } else { slot.plugin.latency() };
                if slot.raw_latency.swap(raw, Ordering::AcqRel) != raw {
                    status.raw_latency_revision = status.raw_latency_revision.saturating_add(1);
                }
            }
        });
    }
}

/// Raw, available instrument/insert latencies, keyed by track and slot.
pub type SlotLatencies = BTreeMap<(TrackId, usize), usize>;

/// The machine limit in frames; invalid preferences use the zero-latency default.
#[must_use]
pub fn low_latency_frames(limit_ms: f64, rate: u32) -> usize {
    if !limit_ms.is_finite() || limit_ms < 0.0 {
        return 0;
    }
    (limit_ms * f64::from(rate) / 1000.0).floor().min(MAX_LATENCY as f64) as usize
}

/// Select only available software audio inserts on over-budget live audio paths.
/// Longest prefixes/suffixes identify the union of over-budget paths without enumerating
/// exponentially many paths through a branching bus graph. Each iteration removes one slot.
#[must_use]
pub fn low_latency_bypass(s: &Session, io: &transit_model::IoMap, target: RenderTarget, raw: &SlotLatencies, limit_ms: f64) -> BTreeSet<u64> {
    let graph = Paths::new(s, io, target);
    let limit = low_latency_frames(limit_ms, target.sample_rate);
    let mut selected = BTreeSet::new();
    loop {
        let own = graph.costs(s, io, target, raw, &selected, false);
        let (prefix, suffix) = graph.walk(&own, &graph.seeds);
        let mut best: Option<(usize, u64, u64)> = None;
        for (ti, t) in s.tracks.iter().enumerate().filter(|(_, t)| !t.inactive && t.freeze.is_none()) {
            let node = if t.kind == TrackKind::Master { graph.main } else { ti };
            let (Some(pre), Some(post)) = (prefix[node], suffix[node]) else { continue };
            if pre.saturating_sub(own[node]).saturating_add(post) <= limit {
                continue;
            }
            for (i, ins) in t.mixer.inserts.iter().enumerate().filter_map(|(i, x)| x.as_ref().map(|x| (i, x))) {
                if !ins.active || ins.bypass || ins.hardware.is_some() || selected.contains(&ins.uid.get()) {
                    continue;
                }
                let latency = raw.get(&(t.id, i)).copied().unwrap_or(0);
                if latency == 0 {
                    continue;
                }
                let candidate = (latency, t.id.0, ins.uid.get());
                if best.is_none_or(|b| latency > b.0 || (latency == b.0 && (candidate.1, candidate.2) < (b.1, b.2))) {
                    best = Some(candidate);
                }
            }
        }
        let Some((_, _, uid)) = best else { break };
        selected.insert(uid);
    }
    selected
}

fn hardware_cost(t: &Track, io: &transit_model::IoMap, target: RenderTarget, sinks: &[SinkSpec]) -> usize {
    hardware::specs_of(t, io, sinks, target.in_channels, target.sample_rate)
        .iter()
        .filter(|h| h.runs() && t.mixer.inserts.get(usize::from(h.slot)).and_then(Option::as_ref).is_some_and(|i| i.active && !i.bypass))
        .fold(0usize, |sum, h| sum.saturating_add(h.latency as usize))
}

/// Shared latency for the same raw snapshot, including key alignment. Used only on
/// control; the readout's actual L and M always come from the audio status.
pub(crate) fn shared_latency(s: &Session, io: &transit_model::IoMap, target: RenderTarget, raw: &SlotLatencies, selected: &BTreeSet<u64>) -> usize {
    if !s.edit.delay_compensation {
        return 0;
    }
    let mut graph = RouteScratch::new();
    graph.reserve(s);
    graph.layer(s);
    let cut: Vec<_> = graph.dropped().copied().collect();
    let mut tracks = vec![0usize; s.tracks.len()];
    let mut buses = vec![0usize; s.busses.len()];
    let sinks = plan_sinks(s, io, target.out_channels).sinks;
    let mut main = 0usize;
    let mut other = 0usize;
    for &ti in graph.order() {
        let Some(t) = s.tracks.get(ti).filter(|t| live(t)) else { continue };
        let arrive = graph
            .edges()
            .iter()
            .filter(|e| e.to == RouteNode::Track(ti) && e.kind == EdgeKind::Input && !cut.contains(e))
            .filter_map(|e| match e.from {
                RouteNode::Bus(i) => buses.get(i).copied(),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let mut own = t.freeze.as_ref().map_or_else(|| raw.get(&(t.id, INSTRUMENT_SLOT)).copied().unwrap_or(0), |f| f.latency as usize);
        let hw = hardware::specs_of(t, io, &sinks, target.in_channels, target.sample_rate);
        for (k, ins) in t.mixer.inserts.iter().enumerate().filter_map(|(k, i)| i.as_ref().map(|i| (k, i))) {
            if !ins.active || ins.bypass || selected.contains(&ins.uid.get()) {
                continue;
            }
            if ins.hardware.is_some() {
                if target.in_channels > 0 {
                    own = own.saturating_add(hw.iter().find(|h| usize::from(h.slot) == k && h.runs()).map_or(0, |h| h.latency as usize));
                }
                continue;
            }
            if t.freeze.is_some() {
                continue;
            }
            let Some(raw) = raw.get(&(t.id, k)) else { continue };
            for e in graph
                .edges()
                .iter()
                .filter(|e| e.to == RouteNode::Track(ti) && matches!(e.kind, EdgeKind::Key { slot, .. } if slot == k) && !cut.contains(e))
            {
                let key = match e.from {
                    RouteNode::Track(i) => tracks[i],
                    RouteNode::Bus(i) => buses[i],
                    RouteNode::Main => 0,
                }
                .min(MAX_LATENCY);
                own = own.max(key.saturating_sub(arrive));
            }
            own = own.saturating_add(*raw);
        }
        let end = arrive.saturating_add(own.min(MAX_LATENCY));
        tracks[ti] = end;
        for e in graph.edges().iter().filter(|e| e.from == RouteNode::Track(ti) && !matches!(e.kind, EdgeKind::Key { .. }) && !cut.contains(e)) {
            match e.to {
                RouteNode::Bus(i) => buses[i] = buses[i].max(end),
                RouteNode::Main => main = main.max(end),
                _ => {}
            }
        }
        if sink::sink_routes(t, &sinks).iter().any(Option::is_some) {
            other = other.max(end);
        }
    }
    let costs = Paths::new(s, io, target).costs(s, io, target, raw, selected, true);
    let master = costs.last().copied().unwrap_or(0);
    main.saturating_add(master).max(other)
}

struct Paths {
    main: usize,
    edges: Vec<(usize, usize)>,
    order: Vec<usize>,
    seeds: Vec<bool>,
    terminal: Vec<bool>,
    sinks: Vec<SinkSpec>,
}

impl Paths {
    fn new(s: &Session, io: &transit_model::IoMap, target: RenderTarget) -> Self {
        let main = s.tracks.len() + s.busses.len();
        let mut route = RouteScratch::new();
        route.reserve(s);
        route.layer(s);
        let index = |node| match node {
            RouteNode::Track(i) => i,
            RouteNode::Bus(i) => s.tracks.len() + i,
            RouteNode::Main => main,
        };
        let dropped: Vec<_> = route.dropped().copied().collect();
        let edges: Vec<_> = route
            .edges()
            .iter()
            .filter(|e| !matches!(e.kind, EdgeKind::Key { .. }) && !dropped.contains(e))
            .filter(|e| {
                [e.from, e.to].iter().all(|n| match n {
                    RouteNode::Track(i) => s.tracks.get(*i).is_some_and(|t| !t.inactive),
                    _ => true,
                })
            })
            .map(|e| (index(e.from), index(e.to)))
            .collect();
        let mut priority = route.order().to_vec();
        priority.extend(s.tracks.len()..=main);
        let mut order = Vec::with_capacity(main + 1);
        let mut degree = vec![0usize; main + 1];
        for &(_, to) in &edges {
            degree[to] += 1;
        }
        while let Some(&node) = priority.iter().find(|&&i| degree[i] == 0 && !order.contains(&i)) {
            order.push(node);
            for &(_, to) in edges.iter().filter(|(from, _)| *from == node) {
                degree[to] = degree[to].saturating_sub(1);
            }
        }
        let sinks = plan_sinks(s, io, target.out_channels).sinks;
        let mut terminal = vec![false; main + 1];
        terminal[main] = true;
        let mut seeds = vec![false; main + 1];
        for (i, t) in s.tracks.iter().enumerate().filter(|(_, t)| !t.inactive) {
            if t.kind == TrackKind::Master {
                seeds[main] = true;
                continue;
            }
            seeds[i] = t.mixer.record_arm || t.mixer.input_monitor;
            terminal[i] = sink::sink_routes(t, &sinks).iter().any(Option::is_some);
        }
        Self { main, edges, order, seeds, terminal, sinks }
    }

    fn costs(
        &self,
        s: &Session,
        io: &transit_model::IoMap,
        target: RenderTarget,
        raw: &SlotLatencies,
        selected: &BTreeSet<u64>,
        clamp: bool,
    ) -> Vec<usize> {
        let mut cost = vec![0usize; self.main + 1];
        for (i, t) in s.tracks.iter().enumerate().filter(|(_, t)| !t.inactive) {
            let mut n = t.freeze.as_ref().map_or(0, |f| f.latency as usize);
            if t.freeze.is_none() {
                n = n.saturating_add(raw.get(&(t.id, INSTRUMENT_SLOT)).copied().unwrap_or(0));
                for (k, ins) in t.mixer.inserts.iter().enumerate().filter_map(|(k, x)| x.as_ref().map(|x| (k, x))) {
                    if ins.active && !ins.bypass && !selected.contains(&ins.uid.get()) {
                        n = n.saturating_add(raw.get(&(t.id, k)).copied().unwrap_or(0));
                    }
                }
            }
            // Match the renderer: software instances disappear when frozen, but a stored
            // hardware slot can still run through the live duplex input.
            if target.in_channels > 0 {
                n = n.saturating_add(hardware_cost(t, io, target, &self.sinks));
            }
            let at = if t.kind == TrackKind::Master { self.main } else { i };
            cost[at] = cost[at].saturating_add(if clamp { n.min(MAX_LATENCY) } else { n });
        }
        cost
    }

    fn walk(&self, own: &[usize], seeds: &[bool]) -> (Vec<Option<usize>>, Vec<Option<usize>>) {
        let mut prefix: Vec<Option<usize>> = seeds.iter().zip(own).map(|(seed, n)| seed.then_some(*n)).collect();
        for &node in &self.order {
            if let Some(n) = prefix[node] {
                for &(_, to) in self.edges.iter().filter(|(from, _)| *from == node) {
                    let value = n.saturating_add(own[to]);
                    prefix[to] = Some(prefix[to].unwrap_or(0).max(value));
                }
            }
        }
        let mut suffix: Vec<Option<usize>> = self.terminal.iter().zip(own).map(|(end, n)| end.then_some(*n)).collect();
        for &node in self.order.iter().rev() {
            for &(_, to) in self.edges.iter().filter(|(from, _)| *from == node) {
                if let Some(n) = suffix[to] {
                    suffix[node] = Some(suffix[node].unwrap_or(0).max(n.saturating_add(own[node])));
                }
            }
        }
        (prefix, suffix)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn frozen_and_inactive_chains_do_not_inflate_the_shared_latency_savings() {
        let mut s = Session::default();
        s.edit.delay_compensation = true;
        let frozen = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let inactive = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
        let t = s.track_mut(frozen).unwrap();
        t.mixer.input_monitor = true;
        t.mixer.output = Route::Hardware("Out 5-6".into());
        t.mixer.inserts[0] = Some(transit_model::Insert::hardware("Out 3-4", "In 3-4"));
        t.mixer.inserts[1] = Some(transit_model::Insert::new("gain"));
        t.freeze = Some(transit_model::Frozen { source: transit_model::SourceId(1), start: 0, length: 1000, hash: 0, revision: 0, latency: 100 });
        let t = s.track_mut(inactive).unwrap();
        t.inactive = true;
        t.mixer.record_arm = true;
        t.mixer.inserts[0] = Some(transit_model::Insert::new("gain"));
        let ins = transit_model::Insert::new("gain");
        let uid = ins.uid.get();
        s.track_mut(master).unwrap().mixer.inserts[0] = Some(ins);
        let mut io = transit_model::IoMap::for_device("test", "duplex", 4, 6);
        io.set_hardware_latency(transit_model::HardwareLatency {
            send: "Out 3-4".into(),
            ret: "In 3-4".into(),
            round_trip_frames: 700,
            rate: 48_000,
            measured_at: 1,
        });
        let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: 64, out_channels: 6, in_channels: 4 };
        let raw = SlotLatencies::from([((frozen, 1), 192), ((inactive, 0), 999), ((master, 0), 300)]);
        let set = low_latency_bypass(&s, &io, target, &raw, 0.0);
        assert_eq!(set, BTreeSet::from([uid]));
        assert_eq!(shared_latency(&s, &io, target, &raw, &BTreeSet::new()), 800);
        assert_eq!(shared_latency(&s, &io, target, &raw, &set), 800);
        s.track_mut(frozen).unwrap().mixer.inserts[0] = None;
        assert_eq!(shared_latency(&s, &io, target, &raw, &BTreeSet::new()), 300);
        assert_eq!(shared_latency(&s, &io, target, &raw, &set), 100);
    }
}
