//! ADR 0065 slice MR0 in the model: the `midi_from` field, `needs`, eligibility, inert routes and
//! their load notes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::{ChannelFormat, Frozen, Insert, MidiOutPort, MidiOutput, Rack, SourceId};

/// The `KNOWN_NEEDS` of the last build before MIDI routes.
const BEFORE_ROUTES: [&str; 8] =
    ["clip_anchor", "clip_loop", "clip_scale_mode", "device_pages", "midi_hidden_notes", "pre_count", "sampler_patch", "sampler_slicing_modes"];

pub(crate) fn instrument(s: &mut Session, name: &str) -> TrackId {
    s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(name))
}

pub(crate) fn route(s: &mut Session, from: TrackId, to: TrackId, listen: bool) {
    s.track_mut(to).unwrap().midi_from = Some(MidiFrom { track: from, listen });
}

fn port() -> MidiOutput {
    MidiOutput::External {
        port: MidiOutPort { backend: "fake".into(), name: "Synth".into(), ordinal: 0, id: String::new() },
        channel: Default::default(),
        offset_ms: 0.0,
    }
}

fn freeze(t: &mut Track) {
    t.freeze = Some(Frozen { source: SourceId(999), start: 0, length: 10, hash: 0, revision: 0, latency: 0 });
}

#[test]
fn midi_from_round_trips_with_listen_and_is_absent_when_unset() {
    let mut s = Session::default();
    let keys = instrument(&mut s, "Keys");
    let bass = instrument(&mut s, "Bass");
    let plain = s.to_json().unwrap();
    assert!(!plain.contains("midi_from") && !plain.contains("\"needs\""), "{plain}");
    for listen in [true, false] {
        route(&mut s, keys, bass, listen);
        let text = s.to_json().unwrap();
        let back = Session::from_json(&text).unwrap();
        assert_eq!(back.track(bass).unwrap().midi_from, Some(MidiFrom { track: keys, listen }));
        assert_eq!(back.track(keys).unwrap().midi_from, None);
        assert!(back.load_notes.is_empty(), "{:?}", back.load_notes);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["needs"], serde_json::json!(["midi_routes"]));
        assert_eq!(s.needs(), vec!["midi_routes"]);
    }
    // Cleared again, the session serializes exactly as it did before the route.
    s.track_mut(bass).unwrap().midi_from = None;
    assert_eq!(s.to_json().unwrap(), plain);
    assert!(s.needs().is_empty());
}

#[test]
fn a_build_without_the_entry_warns_and_drops_the_field() {
    let mut s = Session::default();
    let keys = instrument(&mut s, "Keys");
    let bass = instrument(&mut s, "Bass");
    route(&mut s, keys, bass, true);
    let v: serde_json::Value = serde_json::from_str(&s.to_json().unwrap()).unwrap();
    let old = crate::unknown_needs(&v, &BEFORE_ROUTES);
    assert_eq!(old, vec!["midi_routes".to_string()]);
    assert!(crate::unknown_needs_note(&old).unwrap().contains("midi_routes"));
    assert!(Session::KNOWN_NEEDS.contains(&"midi_routes"));
    // A track reader ignores a field it does not know (it is a field, not a `MidiPort` variant),
    // so an older build loads the track with its kept hardware choice.
    let mut bass_json = v["tracks"].as_array().unwrap().iter().find(|t| t["name"] == "Bass").unwrap().clone();
    bass_json.as_object_mut().unwrap().insert("a_field_from_a_later_build".into(), serde_json::json!({"track": 1}));
    let read: Track = serde_json::from_value(bass_json).unwrap();
    assert_eq!(read.midi_input, crate::MidiInput::default());
}

#[test]
fn a_route_stored_without_its_fields_loads_disconnected_with_listen_on() {
    let from: MidiFrom = serde_json::from_str("{}").unwrap();
    assert_eq!(from, MidiFrom { track: TrackId(0), listen: true });
    let mut s = Session::default();
    let bass = instrument(&mut s, "Bass");
    s.track_mut(bass).unwrap().midi_from = Some(from);
    assert_eq!(s.midi_route_state(0), RouteState::Disconnected, "no session allocates track 0");
}

#[test]
fn eligibility_follows_decision_1_5_with_the_1_7_reasons() {
    let mut s = Session::default();
    let keys = instrument(&mut s, "Keys");
    let t = |s: &Session, id| s.track(id).unwrap().clone();
    assert_eq!((source_problem(&t(&s, keys)), destination_problem(&t(&s, keys))), (None, None));
    // An empty instrument slot is fine at both ends.
    assert!(t(&s, keys).instrument.is_none());
    let midi = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Seq"));
    assert_eq!(source_problem(&t(&s, midi)), Some("it is a MIDI track"));
    assert_eq!(destination_problem(&t(&s, midi)), Some("it is not an instrument track"));
    s.track_mut(midi).unwrap().midi_output = port();
    assert_eq!(destination_problem(&t(&s, midi)), Some("it sends to an external port; routing to a hardware port is not available yet"));
    let sent = instrument(&mut s, "Sent");
    s.track_mut(sent).unwrap().midi_output = port();
    assert_eq!(source_problem(&t(&s, sent)), Some("it sends to an external port"));
    assert_eq!(destination_problem(&t(&s, sent)), Some("it sends to an external port; routing to a hardware port is not available yet"));
    let ext = instrument(&mut s, "Ext");
    s.track_mut(ext).unwrap().instrument = Some(Insert::external("In 1-2"));
    s.track_mut(ext).unwrap().midi_output = port();
    assert_eq!(source_problem(&t(&s, ext)), Some("it holds an External Instrument"));
    assert_eq!(destination_problem(&t(&s, ext)), Some("it holds an External Instrument; routing to a hardware port is not available yet"));
    let rack = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some("Rack"));
    s.track_mut(rack).unwrap().rack = Some(Rack::default());
    assert_eq!((source_problem(&t(&s, rack)), destination_problem(&t(&s, rack))), (Some("it is a rack"), Some("it is a rack")));
    let audio = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    assert_eq!(destination_problem(&t(&s, audio)), Some("it is not an instrument track"));
    assert_eq!(source_problem(&t(&s, audio)), Some("it is not an instrument track"));
    freeze(s.track_mut(keys).unwrap());
    assert_eq!(source_problem(&t(&s, keys)), Some("it is frozen"));
    assert_eq!(destination_problem(&t(&s, keys)), None, "a frozen track can be a destination (6.3)");
    assert_eq!(source_refusal("Keys", "it is frozen"), "Keys cannot send MIDI to other tracks: it is frozen");
    assert_eq!(destination_refusal("Bass", "it is a rack"), "Bass cannot take MIDI from another track: it is a rack");
}

/// Builds a session, saves it, opens it and saves it again: the route fields are unchanged, the
/// canonical text is the same, and the notes are returned.
fn reopen(s: &Session) -> (Session, Vec<String>) {
    let text = s.to_json().unwrap();
    let back = Session::from_json(&text).unwrap();
    assert_eq!(back.to_json().unwrap(), text, "saving the loaded session writes its canonical serialization unchanged");
    for (a, b) in back.tracks.iter().zip(&s.tracks) {
        assert_eq!(a.midi_from, b.midi_from, "the field is kept");
    }
    let notes = back.load_notes.clone();
    (back, notes)
}

#[test]
fn a_missing_source_loads_disconnected_with_its_note() {
    let mut s = Session::default();
    let bass = instrument(&mut s, "Bass");
    route(&mut s, TrackId(4242), bass, true);
    let (back, notes) = reopen(&s);
    assert_eq!(back.midi_route_state(back.track_index(bass).unwrap()), RouteState::Disconnected);
    assert_eq!(notes, ["Bass took MIDI from a track that is not in this session. Its input is disconnected until you choose one."]);
    assert!(back.route_graph().edges().iter().all(|e| e.kind != crate::EdgeKind::Midi));
}

#[test]
fn ineligible_sources_and_destinations_load_inert_with_their_notes() {
    let mut s = Session::default();
    let keys = instrument(&mut s, "Keys");
    let bass = instrument(&mut s, "Bass");
    let seq = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Seq"));
    let lead = instrument(&mut s, "Lead");
    let ext = instrument(&mut s, "Ext");
    let sent = instrument(&mut s, "Sent");
    let audio = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    freeze(s.track_mut(keys).unwrap());
    route(&mut s, keys, bass, true);
    route(&mut s, seq, lead, false);
    s.track_mut(ext).unwrap().instrument = Some(Insert::external("In 1-2"));
    route(&mut s, lead, ext, true);
    s.track_mut(sent).unwrap().midi_output = port();
    route(&mut s, lead, sent, true);
    route(&mut s, lead, audio, true);
    let (back, notes) = reopen(&s);
    assert_eq!(
        notes,
        [
            "Bass takes MIDI from Keys, but Keys cannot send MIDI to other tracks: it is frozen.",
            "Lead takes MIDI from Seq, but Seq cannot send MIDI to other tracks: it is a MIDI track.",
            "Ext takes MIDI from Lead, but routing to a hardware port is not available yet.",
            "Sent takes MIDI from Lead, but routing to a hardware port is not available yet.",
            "Gtr takes MIDI from Lead, but Gtr cannot take MIDI from another track: it is not an instrument track.",
        ]
    );
    assert!(back.route_graph().edges().iter().all(|e| e.kind != crate::EdgeKind::Midi), "no inert route is an edge");
    let state = |id| back.midi_route_state(back.track_index(id).unwrap());
    assert!(matches!(state(bass), RouteState::BadSource { reason: "it is frozen", .. }));
    assert!(matches!(state(ext), RouteState::BadDestination(_)));
    assert_eq!(state(keys), RouteState::None);
}

#[test]
fn a_track_naming_itself_loads_inert_as_a_loop_of_one() {
    let mut s = Session::default();
    let bass = instrument(&mut s, "Bass");
    route(&mut s, bass, bass, true);
    let (back, notes) = reopen(&s);
    assert_eq!(back.midi_route_state(0), RouteState::OwnSource);
    assert_eq!(notes, ["MIDI from Bass to Bass is disabled: it made a loop (Bass → Bass)."]);
}

#[test]
fn a_route_in_force_loads_without_notes() {
    let mut s = Session::default();
    let keys = instrument(&mut s, "Keys");
    let bass = instrument(&mut s, "Bass");
    route(&mut s, keys, bass, false);
    let (back, notes) = reopen(&s);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(back.midi_route_state(back.track_index(bass).unwrap()), RouteState::InForce { source: back.track_index(keys).unwrap() });
    assert_eq!(back.midi_destinations(keys).map(|t| t.id).collect::<Vec<_>>(), [bass]);
}

#[test]
fn record_enable_and_input_monitoring_are_still_not_saved_on_a_routed_destination() {
    let mut s = Session::default();
    let keys = instrument(&mut s, "Keys");
    let bass = instrument(&mut s, "Bass");
    route(&mut s, keys, bass, true);
    let m = &mut s.track_mut(bass).unwrap().mixer;
    m.record_arm = true;
    m.input_monitor = true;
    let text = s.to_json().unwrap();
    assert!(!text.contains("record_arm") && !text.contains("input_monitor"), "{text}");
    let back = Session::from_json(&text).unwrap();
    let m = &back.track(bass).unwrap().mixer;
    assert!(!m.record_arm && !m.input_monitor);
    assert_eq!(back.track(bass).unwrap().midi_from, Some(MidiFrom { track: keys, listen: true }), "Listen is saved");
}

#[test]
fn the_backstop_reports_a_new_external_output_on_a_routed_track_only() {
    let mut s = Session::default();
    let keys = instrument(&mut s, "Keys");
    let bass = instrument(&mut s, "Bass");
    route(&mut s, keys, bass, true);
    let mut after = s.clone();
    after.track_mut(bass).unwrap().midi_output = port();
    assert_eq!(new_hardware_route(&s, &after).as_deref(), Some("Bass takes MIDI from Keys; routing to a hardware port is not available yet"));
    let mut ei = s.clone();
    ei.track_mut(bass).unwrap().instrument = Some(Insert::external("In 1-2"));
    assert!(new_hardware_route(&s, &ei).is_some());
    // A combination the session already held (a loaded file) is not reported again, and an
    // external output without a route is not this rule's.
    assert_eq!(new_hardware_route(&after, &after), None);
    let mut other = s.clone();
    other.track_mut(keys).unwrap().midi_output = port();
    assert_eq!(new_hardware_route(&s, &other), None);
}
