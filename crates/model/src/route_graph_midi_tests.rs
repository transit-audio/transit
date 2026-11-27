//! ADR 0065 decision 2.1 in the route graph: MIDI edges, their depth, the refusals and the
//! extended loop cut.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::super::{RouteEdge, hardware_problem, new_cycle, would_create_cycle};
use super::*;
use crate::{ChannelFormat, Insert, KeyFrom, KeySource, MidiFrom, Route, Tap, TrackId, TrackKind};

/// Six instrument tracks A, B, Keys, C, D, Bass: Keys at index 2 and Bass at index 5.
fn six() -> (Session, TrackId, TrackId) {
    let mut s = Session::default();
    let mut ids = Vec::new();
    for n in ["A", "B", "Keys", "C", "D", "Bass"] {
        ids.push(s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(n)));
    }
    let (keys, bass) = (ids[2], ids[5]);
    assert_eq!((s.track_index(keys), s.track_index(bass)), (Some(2), Some(5)));
    (s, keys, bass)
}

fn route(s: &mut Session, from: TrackId, to: TrackId) {
    s.track_mut(to).unwrap().midi_from = Some(MidiFrom { track: from, listen: true });
}

fn key(s: &mut Session, on: TrackId, from: TrackId) {
    let mut i = Insert::new("compressor");
    i.key = Some(KeySource { from: KeyFrom::Track(from), tap: Tap::Post });
    s.track_mut(on).unwrap().mixer.inserts[0] = Some(i);
}

fn midi_edges(s: &Session) -> Vec<RouteEdge> {
    s.route_graph().edges().iter().filter(|e| e.kind == EdgeKind::Midi).copied().collect()
}

fn names(s: &Session, ts: &[TrackId]) -> Vec<String> {
    ts.iter().map(|t| s.track(*t).unwrap().name.clone()).collect()
}

#[test]
fn a_route_in_force_is_an_edge_and_orders_its_destination_after_its_source() {
    let (mut s, keys, bass) = six();
    // Source after the destination in the table, then before it.
    route(&mut s, bass, keys);
    assert_eq!(midi_edges(&s), [RouteEdge { from: RouteNode::Track(5), to: RouteNode::Track(2), kind: EdgeKind::Midi }]);
    let l = s.route_graph().layering();
    assert!(l.cut.is_empty());
    assert_eq!((l.depth[5], l.depth[2]), (0, 1));
    let order = l.order();
    assert!(order.iter().position(|&i| i == 5) < order.iter().position(|&i| i == 2));
    s.track_mut(keys).unwrap().midi_from = None;
    route(&mut s, keys, bass);
    assert_eq!(s.route_graph().layering().depth[5], 1);
    // A chain of three routes: A → Keys → Bass → C.
    let (a, c) = (s.tracks[0].id, s.tracks[3].id);
    route(&mut s, a, keys);
    route(&mut s, bass, c);
    let l = s.route_graph().layering();
    assert_eq!((l.depth[0], l.depth[2], l.depth[5], l.depth[3]), (0, 1, 2, 3));
}

#[test]
fn layering_after_reserve_allocates_no_edge_storage_with_routes() {
    // Every track at its most edges: each send to a bus, each insert keyed, its output to a bus
    // and a route, so the edges fill the reserved storage exactly.
    let (mut s, _, _) = six();
    let ids: Vec<TrackId> = s.tracks.iter().map(|t| t.id).collect();
    let bus = s.add_bus("Sum", ChannelFormat::Stereo);
    for (k, &t) in ids.iter().enumerate() {
        let other = ids[(k + 1) % ids.len()];
        let tr = s.track_mut(t).unwrap();
        tr.mixer.output = Route::Bus(bus);
        for slot in tr.mixer.sends.iter_mut() {
            *slot = Some(crate::SendSlot::new(Route::Bus(bus)));
        }
        for slot in tr.mixer.inserts.iter_mut() {
            let mut i = Insert::new("compressor");
            i.key = Some(KeySource { from: KeyFrom::Track(other), tap: Tap::Post });
            *slot = Some(i);
        }
        tr.midi_from = Some(MidiFrom { track: other, listen: true });
    }
    let mut scratch = RouteScratch::new();
    scratch.reserve(&s);
    let cap = scratch.edges.capacity();
    scratch.layer(&s);
    assert_eq!(scratch.edges.len(), cap, "the session fills the reservation");
    assert_eq!(scratch.edges.capacity(), cap, "every send, key, output and route fits the reserved storage");
    assert_eq!(scratch.edges.iter().filter(|e| e.kind == EdgeKind::Midi).count(), ids.len());
}

#[test]
fn a_pure_midi_loop_cuts_the_route_into_the_first_track_and_names_it_on_open() {
    let (mut s, keys, bass) = six();
    route(&mut s, keys, bass);
    route(&mut s, bass, keys);
    let loaded = Session::from_json(&s.to_json().unwrap()).unwrap();
    let l = loaded.route_graph().layering();
    assert_eq!(l.cut, [RouteEdge { from: RouteNode::Track(5), to: RouteNode::Track(2), kind: EdgeKind::Midi }], "Bass → Keys is cut");
    assert_eq!((l.depth[2], l.depth[5]), (0, 1), "the surviving route orders Bass after Keys");
    let mut scratch = RouteScratch::new();
    scratch.reserve(&loaded);
    scratch.layer(&loaded);
    assert_eq!(scratch.dropped().copied().collect::<Vec<_>>(), l.cut, "the cut route is reported dropped");
    assert_eq!(loaded.load_notes, ["MIDI from Bass to Keys is disabled: it made a loop (Keys → Bass → Keys)."]);
    assert_eq!(loaded.track(keys).unwrap().midi_from, Some(MidiFrom { track: bass, listen: true }), "the field is kept");
    assert_eq!(loaded.to_json().unwrap(), s.to_json().unwrap());
}

#[test]
fn a_midi_and_key_loop_cuts_the_key_and_keeps_the_route() {
    let (mut s, keys, bass) = six();
    // A route Keys → Bass, and a key from Bass into an insert on Keys.
    route(&mut s, keys, bass);
    key(&mut s, keys, bass);
    let loaded = Session::from_json(&s.to_json().unwrap()).unwrap();
    let l = loaded.route_graph().layering();
    assert_eq!(l.cut, [RouteEdge { from: RouteNode::Track(5), to: RouteNode::Track(2), kind: EdgeKind::Key { slot: 0, pre_fader: false } }]);
    assert!(loaded.load_notes.is_empty(), "a cut key is not a route: {:?}", loaded.load_notes);
    assert_eq!((l.depth[2], l.depth[5]), (0, 1));
}

#[test]
fn a_loop_through_a_bus_keeps_todays_cut_with_a_route_elsewhere_in_the_session() {
    let mut s = Session::default();
    let synth = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Synth"));
    let fx = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("FX"));
    let bus = s.add_bus("FX In", ChannelFormat::Stereo);
    s.track_mut(fx).unwrap().mixer.input = Route::Bus(bus);
    s.track_mut(synth).unwrap().mixer.output = Route::Bus(bus);
    let gen_ = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Gen"));
    route(&mut s, gen_, synth);
    // FX feeds its own input bus: a loop through a bus.
    s.track_mut(fx).unwrap().mixer.output = Route::Bus(bus);
    let l = s.route_graph().layering();
    assert_eq!(l.cut.len(), 1);
    assert_eq!(l.cut[0].kind, EdgeKind::Output, "the feedback into the aux's bus, as today");
    assert_eq!(midi_edges(&s).len(), 1, "the route is untouched");
}

#[test]
fn a_route_that_closes_a_loop_is_refused_through_routes_and_keys() {
    let (mut s, keys, bass) = six();
    let a = s.tracks[0].id;
    // Two routes: A → Keys → Bass; Bass → A would close A → Keys → Bass → A.
    route(&mut s, a, keys);
    route(&mut s, keys, bass);
    let c = would_create_cycle(&s, &RouteChange::Midi { track: a, from: Some(bass) }).expect("loop through two routes");
    assert_eq!(names(&s, &c), ["A", "Keys", "Bass"]);
    assert_eq!(crate::describe_cycle(&s, &c), "A → Keys → Bass → A");
    assert!(would_create_cycle(&s, &RouteChange::Midi { track: a, from: None }).is_none());
    // A route and a key: a key from Bass into Keys closes Keys → Bass → Keys.
    let back = Some(KeySource { from: KeyFrom::Track(bass), tap: Tap::Pre });
    let c = would_create_cycle(&s, &RouteChange::Key { track: keys, slot: 0, key: back }).expect("loop through a route and a key");
    assert_eq!(names(&s, &c), ["Keys", "Bass"]);
    // And the other way: with the key in place, the route closes it.
    let (mut s, keys, bass) = six();
    key(&mut s, keys, bass);
    let c = would_create_cycle(&s, &RouteChange::Midi { track: bass, from: Some(keys) }).expect("loop through a key and a route");
    assert_eq!(crate::describe_cycle(&s, &c), "Bass → Keys → Bass");
    // An ineligible source is no edge, so it closes nothing (the command refuses it on its own).
    let seq = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Seq"));
    assert!(would_create_cycle(&s, &RouteChange::Midi { track: bass, from: Some(seq) }).is_none());
}

#[test]
fn an_edit_that_brings_a_route_into_force_inside_a_loop_is_a_new_cycle() {
    let (mut s, keys, bass) = six();
    route(&mut s, keys, bass);
    route(&mut s, bass, keys);
    // Keys has a port output: Keys cannot take a route, so the loop is not there yet.
    s.track_mut(keys).unwrap().midi_output = crate::MidiOutput::External {
        port: crate::MidiOutPort { backend: "fake".into(), name: "P".into(), ordinal: 0, id: String::new() },
        channel: Default::default(),
        offset_ms: 0.0,
    };
    assert!(s.route_graph().find_cycle().is_none());
    let mut after = s.clone();
    after.track_mut(keys).unwrap().midi_output = crate::MidiOutput::Internal;
    assert!(new_cycle(&s, &after).is_some(), "the routing comparison sees the output change");
}

#[test]
fn the_hardware_insert_check_does_not_follow_routes() {
    let mut s = Session::default();
    let keys = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let bass = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Bass"));
    s.track_mut(keys).unwrap().mixer.inserts[0] = Some(Insert::hardware("Out 3-4", "In 3-4"));
    route(&mut s, keys, bass);
    s.track_mut(bass).unwrap().mixer.output = Route::Hardware("Out 3-4".into());
    // Bass is not fed Keys' audio: its route to the path is an ordinary conflict, not feedback.
    let why = hardware_problem(&s).unwrap();
    assert_eq!(why, "`Bass` routes to `Out 3-4`, the send path of a hardware insert on `Keys`");
}
