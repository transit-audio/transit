//! The route graph's MIDI edges (ADR 0065 decision 2.1): a route in force is an edge from its
//! source track to its destination track, so depth gains it and a destination renders in a later
//! layer than its source; a loaded loop is cut at its bus input, else its keys, else its MIDI
//! routes into its first track; a cut route is reported dropped, and opening the session names it.

use super::{EdgeKind, RouteChange, RouteGraph, RouteNode, RouteScratch, on_graph};
use crate::{Session, Track, midi_route};

/// The node the MIDI route of track `t` (at index `i`) comes from, `None` when it has no route in
/// force: no route, a missing source, the track itself, or an end that is not eligible (see
/// `midi_route.rs`). A `RouteChange::Midi` for `t` in `changes` replaces its field (the last wins).
pub(super) fn source_node(s: &Session, t: &Track, i: usize, changes: &[RouteChange]) -> Option<RouteNode> {
    let mut from = t.midi_from.and_then(|m| m.source());
    for c in changes {
        if let RouteChange::Midi { track, from: f } = c
            && *track == t.id
        {
            from = *f;
        }
    }
    // Track 0 is the disconnected sentinel, from a change as from the field.
    let k = s.track_index(from.filter(|id| id.0 != 0)?).filter(|&k| k != i)?;
    let src = s.tracks.get(k).filter(|x| on_graph(x))?;
    (midi_route::destination_problem(t).is_none() && midi_route::source_problem(src).is_none()).then_some(RouteNode::Track(k))
}

/// Whether two versions of a track route MIDI alike: the same source, and the same of everything
/// that decides whether a route is in force (the kind is compared by the caller).
pub(super) fn same_route(x: &Track, y: &Track) -> bool {
    x.midi_from.map(|m| m.track) == y.midi_from.map(|m| m.track)
        && x.is_rack() == y.is_rack()
        && x.midi_output.is_internal() == y.midi_output.is_internal()
        && x.external_instrument().is_some() == y.external_instrument().is_some()
        && x.freeze.is_some() == y.freeze.is_some()
}

impl RouteScratch {
    /// One round's last resort: cuts the edges of a kind `pick` accepts that run inside a loop
    /// into the loop's first track. Whether any was cut.
    pub(super) fn cut_into_first(&mut self, pick: impl Fn(EdgeKind) -> bool) -> bool {
        let mut any = false;
        for e in 0..self.edges.len() {
            let (u, v) = (self.from[e], self.to[e]);
            if self.cut[e] || self.comp[u] != self.comp[v] || !pick(self.edges[e].kind) {
                continue;
            }
            if self.comp_min[self.comp[u]] == v {
                self.cut[e] = true;
                any = true;
            }
        }
        any
    }
}

/// Decision 2.1's load note for each MIDI route a loaded loop cut, in edge order: "MIDI from Bass
/// to Keys is disabled: it made a loop (Keys → Bass → Keys)", the loop named from the route's
/// destination (the loop's first track).
pub(crate) fn midi_loop_notes(s: &Session) -> Vec<String> {
    let l = RouteGraph::new(s).layering();
    let name = |i: usize| s.tracks.get(i).map_or_else(String::new, |t| t.name.clone());
    let mut notes = Vec::new();
    for (e, cycle) in l.cut.iter().zip(&l.loops) {
        let (EdgeKind::Midi, RouteNode::Track(from), RouteNode::Track(to)) = (e.kind, e.from, e.to) else { continue };
        let mut tracks = cycle.tracks.clone();
        if let Some(p) = s.tracks.get(to).and_then(|t| tracks.iter().position(|x| *x == t.id)) {
            tracks.rotate_left(p);
        }
        notes.push(midi_route::loop_note(&name(from), &name(to), &crate::describe_cycle(s, &tracks)));
    }
    notes
}

#[cfg(test)]
#[path = "route_graph_midi_tests.rs"]
mod tests;
