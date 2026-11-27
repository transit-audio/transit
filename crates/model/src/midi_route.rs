//! MIDI routing between tracks (ADR 0065): a destination instrument track takes its MIDI input
//! from one source track ([`Track::midi_from`]). This module holds the session field, which tracks
//! can be a source or a destination (decision 1.5), the refusal wordings of decision 1.7, a
//! route's state as a loaded session or a restored snapshot holds it, and the load notes of
//! decision 8. The route graph's MIDI edges are in `route_graph_midi.rs`.
//!
//! A route is **in force** when its source exists, is another track, and both ends are eligible;
//! only a route in force is an edge of the route graph. Any other route is **inert**: the field
//! stays as it is (and saves unchanged), it orders nothing and carries nothing, and opening the
//! session says why in a load note. A route in force can still be cut by a loaded loop (decision
//! 2.1); that cut is the route graph's.

use crate::{Session, Track, TrackId, TrackKind};

/// Where a destination takes its MIDI from (ADR 0065 decision 1.6). While it is set, the track's
/// hardware choice in [`crate::MidiInput::port`] is kept and not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct MidiFrom {
    /// The source track. A file without it names track 0, the disconnected sentinel (see
    /// [`MidiFrom::source`]): the destination loads disconnected (decision 6.6).
    #[serde(default)]
    pub track: TrackId,
    /// The route's Listen switch (decision 5.2): saved, an undo step, on for a new route, and on
    /// for a route a file stores without it.
    #[serde(default = "listen_on")]
    pub listen: bool,
}

fn listen_on() -> bool {
    true
}

impl MidiFrom {
    /// The source track, `None` for track 0: a route stored without its source names no track,
    /// even in a hand-edited file that holds a track with id 0, so it is disconnected (decision
    /// 6.6). Every reading of a route's source goes through here.
    #[must_use]
    pub fn source(&self) -> Option<TrackId> {
        (self.track.0 != 0).then_some(self.track)
    }
}

/// The suffix of a refusal that keeps a hardware port out of routing until the follow-up record
/// "Routed MIDI to external ports" (decision 7).
pub const HARDWARE_NOT_YET: &str = "routing to a hardware port is not available yet";

/// Why `t` cannot send MIDI to other tracks (decision 1.5), `None` when it can. The reasons are
/// decision 1.7's, except "it is not an instrument track" for the kinds 1.7 gives no reason for
/// (audio, aux, master, VCA, a plain folder): the record lists them as not sources without a
/// wording, and this is the destination list's wording for the same fact.
#[must_use]
pub fn source_problem(t: &Track) -> Option<&'static str> {
    if t.is_rack() {
        Some("it is a rack")
    } else if t.kind == TrackKind::Midi {
        Some("it is a MIDI track")
    } else if t.external_instrument().is_some() {
        Some("it holds an External Instrument")
    } else if t.midi_output.is_external() {
        Some("it sends to an external port")
    } else if t.kind != TrackKind::Instrument {
        Some("it is not an instrument track")
    } else if t.freeze.is_some() {
        Some("it is frozen")
    } else {
        None
    }
}

/// Why `t` cannot take MIDI from another track (decision 1.5), `None` when it can. A frozen
/// track can be a destination (decision 6.3).
#[must_use]
pub fn destination_problem(t: &Track) -> Option<&'static str> {
    if t.is_rack() {
        Some("it is a rack")
    } else if t.external_instrument().is_some() {
        Some("it holds an External Instrument; routing to a hardware port is not available yet")
    } else if t.midi_output.is_external() {
        Some("it sends to an external port; routing to a hardware port is not available yet")
    } else if t.kind != TrackKind::Instrument {
        Some("it is not an instrument track")
    } else {
        None
    }
}

/// Whether `t` has an external MIDI output in the sense of decision 1.5 (a port output or an
/// External Instrument): such a track is not a route destination.
#[must_use]
pub fn sends_to_hardware(t: &Track) -> bool {
    t.midi_output.is_external() || t.external_instrument().is_some()
}

/// Decision 1.7: "<track> cannot send MIDI to other tracks: <reason>".
#[must_use]
pub fn source_refusal(name: &str, reason: &str) -> String {
    format!("{name} cannot send MIDI to other tracks: {reason}")
}

/// Decision 1.7: "<track> cannot take MIDI from another track: <reason>".
#[must_use]
pub fn destination_refusal(name: &str, reason: &str) -> String {
    format!("{name} cannot take MIDI from another track: {reason}")
}

/// Decision 1.5: "Bass takes MIDI from Keys; routing to a hardware port is not available yet",
/// for every path that would put an external output or an External Instrument on a routed track.
#[must_use]
pub fn takes_route_refusal(s: &Session, destination: &Track) -> String {
    let from = destination.midi_from.map_or_else(String::new, |m| source_name(s, m));
    format!("{} takes MIDI from {from}; {HARDWARE_NOT_YET}", destination.name)
}

/// Decision 6.3: "Keys sends MIDI to Bass; remove that route or freeze Bass instead", naming the
/// first destination in session order. `None` when no track takes MIDI from `source`.
#[must_use]
pub fn freeze_refusal(s: &Session, source: TrackId) -> Option<String> {
    let dest = s.midi_destinations(source).next()?;
    Some(format!("{} sends MIDI to {}; remove that route or freeze {} instead", track_name(s, source), dest.name, dest.name))
}

/// A route's source by name, or `track <id>` when it names no track of the session.
fn source_name(s: &Session, m: MidiFrom) -> String {
    m.source().and_then(|id| s.track(id)).map_or_else(|| format!("track {}", m.track), |t| t.name.clone())
}

/// A track's name, or `track <id>` when the session has no such track.
fn track_name(s: &Session, id: TrackId) -> String {
    s.track(id).map_or_else(|| format!("track {id}"), |t| t.name.clone())
}

/// The state of a track's route (decision 8), apart from a loop cut, which is the route graph's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteState {
    /// The track takes no MIDI from another track.
    None,
    /// The route is in force: `source` is the index of its source in [`Session::tracks`].
    InForce { source: usize },
    /// The source is not in the session (deleted, or a hand-edited file): no routed input and no
    /// hardware input (decision 6.6).
    Disconnected,
    /// The track names itself (a hand-edited file): a loop of one track, inert.
    OwnSource,
    /// The destination cannot take a route (decision 1.5, a track with an external output
    /// included): inert.
    BadDestination(&'static str),
    /// The source cannot send (decision 1.5, a frozen source included): inert.
    BadSource { source: usize, reason: &'static str },
}

impl Session {
    /// The state of the route of the track at index `i` (see [`RouteState`]).
    #[must_use]
    pub fn midi_route_state(&self, i: usize) -> RouteState {
        let Some(t) = self.tracks.get(i) else { return RouteState::None };
        let Some(from) = t.midi_from else { return RouteState::None };
        let Some(k) = from.source().and_then(|id| self.track_index(id)) else { return RouteState::Disconnected };
        if k == i {
            return RouteState::OwnSource;
        }
        if let Some(why) = destination_problem(t) {
            return RouteState::BadDestination(why);
        }
        match self.tracks.get(k).and_then(source_problem) {
            Some(reason) => RouteState::BadSource { source: k, reason },
            None => RouteState::InForce { source: k },
        }
    }

    /// The tracks that take MIDI from `source` (by their field, in force or not), in session order.
    pub fn midi_destinations(&self, source: TrackId) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(move |t| t.id != source && t.midi_from.is_some_and(|m| m.source() == Some(source)))
    }

    /// Whether any track takes MIDI from another track (the `midi_routes` need, decision 8).
    #[must_use]
    pub fn has_midi_routes(&self) -> bool {
        self.tracks.iter().any(|t| t.midi_from.is_some())
    }
}

/// The load notes of decision 8 for every inert route and every route a loaded loop cut, in
/// session order of the destinations (inert routes first, then loop cuts in edge order). Empty for
/// a session without routes.
#[must_use]
pub fn route_load_notes(s: &Session) -> Vec<String> {
    if !s.has_midi_routes() {
        return Vec::new();
    }
    let mut notes = Vec::new();
    for (i, t) in s.tracks.iter().enumerate() {
        let src = |k: usize| s.tracks.get(k).map_or_else(String::new, |x| x.name.clone());
        match s.midi_route_state(i) {
            RouteState::None | RouteState::InForce { .. } => {}
            RouteState::Disconnected => {
                notes.push(format!("{} took MIDI from a track that is not in this session. Its input is disconnected until you choose one.", t.name));
            }
            RouteState::OwnSource => notes.push(loop_note(&t.name, &t.name, &crate::describe_cycle(s, &[t.id]))),
            RouteState::BadDestination(_) if sends_to_hardware(t) => {
                let from = t.midi_from.map_or_else(String::new, |m| source_name(s, m));
                notes.push(format!("{} takes MIDI from {from}, but {HARDWARE_NOT_YET}.", t.name));
            }
            RouteState::BadDestination(why) => {
                let from = t.midi_from.map_or_else(String::new, |m| source_name(s, m));
                notes.push(format!("{} takes MIDI from {from}, but {}.", t.name, destination_refusal(&t.name, why)));
            }
            RouteState::BadSource { source, reason } => {
                notes.push(format!("{} takes MIDI from {}, but {}.", t.name, src(source), source_refusal(&src(source), reason)));
            }
        }
    }
    notes.extend(crate::route_graph::midi_loop_notes(s));
    notes
}

/// Decision 2.1: "MIDI from Bass to Keys is disabled: it made a loop (Keys → Bass → Keys)."
pub(crate) fn loop_note(from: &str, to: &str, cycle: &str) -> String {
    format!("MIDI from {from} to {to} is disabled: it made a loop ({cycle}).")
}

/// The reason an edit from `before` to `after` put an external MIDI output or an External
/// Instrument on a track that takes a route (decision 1.5), `None` when it did not. The engine's
/// edit validation calls it as the backstop for paths that write the session directly (a track
/// preset, a duplicate, a kind change); a combination `before` already held (a loaded file) is
/// not reported again.
#[must_use]
pub fn new_hardware_route(before: &Session, after: &Session) -> Option<String> {
    let held = |t: &Track| t.midi_from.is_some() && sends_to_hardware(t);
    after.tracks.iter().filter(|t| held(t)).find(|t| !before.track(t.id).is_some_and(held)).map(|t| takes_route_refusal(after, t))
}

#[cfg(test)]
#[path = "midi_route_tests.rs"]
mod tests;
