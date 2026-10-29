//! Dependency-aware strip scheduling (issue #174, ADR 0029 amendment, slice 1): a plan with a
//! sidechain key renders its phase-1 strips through the render workers as a task graph, one task
//! per strip with an edge from every key source, instead of serially. The default for keyed plans
//! since 2026-10-29 (the ADR's section 8 measurement, #179 and #210); [`RENDER_GRAPH_ENV`]`=0`,
//! read once where the engine starts its render workers ([`init_graph_switch`]), opts out, and a
//! keyed plan renders serially on the audio thread exactly as before the flip.
//!
//! The compiler builds the plan's [`StripGraph`] off the audio path ([`strip_graph`]): the edges
//! are the key edges of `PlanCompiler::describe` (cut cycles and inactive tracks already left
//! out) whose source renders in an earlier layer than the keyed strip, which is exactly what the
//! serial path's per-layer fill honors (a key from a strip of the same or a later layer fills
//! silence there, and here). The graph generation is new with every table built, so a result of
//! another plan's graph never matches a task's completion identity ([`task_identity`]). The
//! per-block run state is allocated with it. The render is `StripPool::render_graph`.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use transit_rt::task_graph::{GraphRun, TaskGraph, fnv};

/// Plans with sidechain keys render through the render workers' task graph by default (issue
/// #174, since 2026-10-29: ADR 0029's amendment, section 8 measurement); `0` opts out and they
/// render serially on the audio thread. Unset or `1`: the task graph. Read once per process where
/// the engine starts its render workers, never on the audio thread; anything else is logged once
/// and leaves the task graph on.
pub(crate) const RENDER_GRAPH_ENV: &str = "TRANSIT_RENDER_GRAPH";

/// Keyed plans render through the task graph ([`MixEngine::force_graph`], [`RENDER_GRAPH_ENV`]).
/// On by default; [`RENDER_GRAPH_ENV`]`=0` clears it at startup.
static GRAPH: AtomicBool = AtomicBool::new(true);

/// What a [`RENDER_GRAPH_ENV`] value asks for ([`render_graph_setting`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RenderGraphSetting {
    /// Unset or `1`: keyed plans render through the task graph (the default).
    Graph,
    /// `0`: the opt-out, keyed plans render serially on the audio thread.
    Serial,
    /// Anything else: logged once, and keyed plans render through the task graph.
    Unrecognized,
}

/// The setting a [`RENDER_GRAPH_ENV`] value (`None`: unset) asks for. Surrounding whitespace is
/// ignored. Pure, so the parse is testable without touching the process environment.
pub(crate) fn render_graph_setting(value: Option<&str>) -> RenderGraphSetting {
    match value.map(str::trim) {
        None | Some("1") => RenderGraphSetting::Graph,
        Some("0") => RenderGraphSetting::Serial,
        Some(_) => RenderGraphSetting::Unrecognized,
    }
}

/// Reads [`RENDER_GRAPH_ENV`] once per process (never on the audio thread): `0` turns the task
/// graph off for keyed plans, the opt-out; anything but unset, `1` or `0` is logged once and
/// leaves it on.
pub(crate) fn init_graph_switch() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !on_audio_thread() {
        ONCE.call_once(|| {
            let value = std::env::var(RENDER_GRAPH_ENV).ok();
            apply_graph_setting(value.as_deref(), &GRAPH);
        });
    }
}

/// What [`init_graph_switch`] does with a [`RENDER_GRAPH_ENV`] value (`None`: unset): `0` clears
/// `switch`, anything else leaves it as it is (an unrecognized value is logged). A seam, so the
/// switch's handling is testable on a switch of the test's own, without the process environment.
fn apply_graph_setting(value: Option<&str>, switch: &AtomicBool) {
    match render_graph_setting(value) {
        RenderGraphSetting::Graph => {}
        RenderGraphSetting::Serial => {
            log::info!("mix: {RENDER_GRAPH_ENV}=0; plans with sidechain keys render serially on the audio thread");
            switch.store(false, Ordering::Relaxed);
        }
        RenderGraphSetting::Unrecognized => log::warn!(
            "mix: {RENDER_GRAPH_ENV}={:?} ignored (0 renders keyed plans serially, 1 or unset through the render workers); the task graph stays on",
            value.unwrap_or_default()
        ),
    }
}

impl MixEngine {
    /// Turns the task graph on or off for every engine in the process: on, a plan with sidechain
    /// keys renders its strips through the render workers (issue #174, the default); off
    /// ([`RENDER_GRAPH_ENV`]`=0`), it renders them serially on the audio thread. Takes effect at
    /// the next block; a strip a worker still holds comes home through the graph path first. For
    /// tests and the bench.
    #[doc(hidden)]
    pub fn force_graph(on: bool) {
        GRAPH.store(on, Ordering::Relaxed);
    }

    /// True while keyed plans render through the task graph: on by default, off after
    /// [`MixEngine::force_graph`]`(false)` or with [`RENDER_GRAPH_ENV`]`=0`.
    #[must_use]
    pub fn graph_enabled() -> bool {
        GRAPH.load(Ordering::Relaxed)
    }
}

/// A plan's task graph over its phase-1 strips and the per-block state to run it.
#[derive(Default)]
pub(crate) struct StripGraph {
    pub(crate) graph: TaskGraph,
    pub(crate) run: GraphRun,
    /// Per phase-1 strip: the first table index of its layer. A key from a strip at or past it
    /// fills silence, as on the serial path (which fills a layer's keys before the layer runs).
    pub(crate) layer_start: Box<[u16]>,
    /// Per aux (table index less `phase1`): skipped this block because a key source missed (a
    /// phase-1 strip or an earlier skipped aux). Set by the serial tail each block.
    pub(crate) tail_skipped: Box<[bool]>,
}

/// Graph generations: one per table built (0: none).
static NEXT_GRAPH: AtomicU64 = AtomicU64::new(1);

/// The task graph of a plan with `phase1` phase-1 strips in `layers` (each layer's end) and the
/// key edges `keys`: an edge from a key source strip to its keyed strip when the source renders
/// in an earlier layer; with a skip flag per aux (`auxes` of them). Off the audio path
/// (allocates).
pub(crate) fn strip_graph(phase1: usize, auxes: usize, layers: &[u16], keys: &[KeySpec]) -> StripGraph {
    let mut layer_start = vec![0u16; phase1];
    let mut from = 0usize;
    for end in layers.iter().map(|&e| usize::from(e)).chain(std::iter::once(phase1)) {
        let end = end.min(phase1);
        for s in layer_start.iter_mut().take(end).skip(from) {
            *s = u16::try_from(from).unwrap_or(u16::MAX);
        }
        from = from.max(end);
    }
    let edges: Vec<(usize, usize)> = keys
        .iter()
        .filter_map(|k| match k.from {
            KeyEnd::Strip(j) => {
                let (j, keyed) = (usize::from(j), usize::from(k.keyed));
                (keyed < phase1 && j < usize::from(layer_start[keyed])).then_some((j, keyed))
            }
            KeyEnd::Bus(_) => None,
        })
        .collect();
    let generation = NEXT_GRAPH.fetch_add(1, Ordering::Relaxed);
    StripGraph {
        graph: TaskGraph::new(generation, phase1, &edges),
        run: GraphRun::new(phase1),
        layer_start: layer_start.into(),
        tail_skipped: vec![false; auxes].into(),
    }
}

impl StripGraph {
    /// The first table index of the layer of phase-1 strip `task` (0 past the table).
    pub(crate) fn layer_start(&self, task: usize) -> usize {
        self.layer_start.get(task).map_or(0, |&s| usize::from(s))
    }
}

/// A task's identity for its completion check (ADR 0029 amendment section 2): its index, the
/// identity of the processing chain it holds, and the hash of its input edges.
pub(crate) fn task_identity(task: usize, chain: u64, edge_hash: u64) -> u64 {
    fnv(fnv(fnv(0xcbf2_9ce4_8422_2325, u64::try_from(task).unwrap_or(u64::MAX)), chain), edge_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #174: the task graph is the default for keyed plans; `0` is the opt-out; anything else is
    /// logged and keeps the graph. Pure: no environment is touched.
    #[test]
    fn unset_and_1_are_the_graph_0_is_serial_and_anything_else_is_logged() {
        assert_eq!(render_graph_setting(None), RenderGraphSetting::Graph);
        for v in ["1", " 1 ", "1\n"] {
            assert_eq!(render_graph_setting(Some(v)), RenderGraphSetting::Graph, "{v:?}");
        }
        for v in ["0", " 0", "0 ", "0\n"] {
            assert_eq!(render_graph_setting(Some(v)), RenderGraphSetting::Serial, "{v:?}");
        }
        for v in ["", "  ", "2", "00", "01", "true", "false", "yes", "no", "off", "on", "graph", "-1", "0.0"] {
            assert_eq!(render_graph_setting(Some(v)), RenderGraphSetting::Unrecognized, "{v:?}");
        }
        assert_eq!(RENDER_GRAPH_ENV, "TRANSIT_RENDER_GRAPH");
    }

    /// #174 review: the switch's handling of the parsed value, on a switch of the test's own
    /// (starting on, as `GRAPH` does): `0` turns it off; unset, `1` and anything else leave it on.
    #[test]
    fn only_0_turns_the_switch_off() {
        let off = AtomicBool::new(true);
        apply_graph_setting(Some("0"), &off);
        assert!(!off.load(Ordering::Relaxed));
        for v in [None, Some("1"), Some(" 1 "), Some("2"), Some("off"), Some("")] {
            let on = AtomicBool::new(true);
            apply_graph_setting(v, &on);
            assert!(on.load(Ordering::Relaxed), "{v:?}");
        }
    }

    fn key(keyed: u16, from: KeyEnd) -> KeySpec {
        KeySpec { keyed, id: TrackId(u64::from(keyed) + 1), slot: 0, from, pre: false, ch: 2 }
    }

    /// Edges only from a strip of an earlier layer; bus keys and keys onto tracks past phase 1
    /// are not edges; every table gets a new generation.
    #[test]
    fn the_graph_has_the_key_edges_the_serial_layers_honor() {
        // Layers: [0, 1, 2) [2, 4) [4, 5).
        let layers = [2u16, 4];
        let keys = [
            key(2, KeyEnd::Strip(0)),
            key(3, KeyEnd::Strip(2)),
            key(4, KeyEnd::Strip(3)),
            key(1, KeyEnd::Strip(0)),
            key(4, KeyEnd::Bus(BusId(9))),
            key(6, KeyEnd::Strip(0)),
        ];
        let g = strip_graph(5, 0, &layers, &keys);
        assert_eq!((0..5).map(|t| g.layer_start(t)).collect::<Vec<_>>(), [0, 0, 2, 2, 4]);
        assert_eq!(g.graph.tasks(), 5);
        assert_eq!(g.graph.dependents(0), [2], "a key within the source's own layer is no edge");
        assert!(g.graph.dependents(2).is_empty(), "3 keyed from 2 in its own layer fills silence: no edge");
        assert_eq!(g.graph.dependents(3), [4]);
        assert_eq!(g.graph.edges(), 2);
        let h = strip_graph(5, 0, &layers, &keys);
        assert_ne!(g.graph.generation(), h.graph.generation(), "a new generation per table");
        assert_eq!(g.run.capacity(), 5);
        assert_ne!(task_identity(2, 7, g.graph.edge_hash(2)), task_identity(2, 7, g.graph.edge_hash(1)));
    }
}
