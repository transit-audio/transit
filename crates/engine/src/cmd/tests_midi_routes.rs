//! ADR 0065 slice MR0 in the engine: `midi.input`'s `from` and `listen`, the refusals of decision
//! 1.7 and the paths of decision 1.5, deletion, freeze and import.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{Engine, EngineError};
use serde_json::{Value, json};
use transit_model::{
    ChannelFormat, Frozen, Insert, MidiFrom, MidiInput, MidiOutPort, MidiOutput, MidiPort, Rack, RouteState, Session, SourceId, TrackId, TrackKind,
};

fn port_out() -> MidiOutput {
    MidiOutput::External {
        port: MidiOutPort { backend: "fake".into(), name: "Synth".into(), ordinal: 0, id: String::new() },
        channel: Default::default(),
        offset_ms: 0.0,
    }
}

fn port_json() -> Value {
    json!({"backend": "fake", "name": "Synth"})
}

/// An engine with instrument tracks Keys and Bass.
fn engine() -> (Engine, TrackId, TrackId) {
    let mut s = Session::default();
    let keys = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let bass = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Bass"));
    (Engine::new(s), keys, bass)
}

fn from(e: &Engine, t: TrackId) -> Option<MidiFrom> {
    e.session().track(t).unwrap().midi_from
}

/// Runs a command that must be refused with exactly `message`, and checks it changed nothing.
fn refused(e: &mut Engine, id: &str, p: Value, message: &str) {
    let before = e.session().to_json().unwrap();
    let pushes = e.undo_pushes();
    match e.execute(id, &p) {
        Err(EngineError::BadParams(_, m)) => assert_eq!(m, message, "{p}"),
        other => panic!("{id} {p}: expected a refusal, got {other:?}"),
    }
    assert_eq!(e.session().to_json().unwrap(), before, "{id} {p} changed the session");
    assert_eq!(e.undo_pushes(), pushes);
}

#[test]
fn a_new_route_has_listen_on_and_listen_is_one_undo_step() {
    let (mut e, keys, bass) = engine();
    let pushes = e.undo_pushes();
    e.execute("midi.input", &json!({"track": bass.0, "from": "Keys"})).unwrap();
    assert_eq!(from(&e, bass), Some(MidiFrom { track: keys, listen: true }));
    assert_eq!(e.undo_pushes(), pushes + 1);
    e.execute("midi.input", &json!({"track": bass.0, "listen": false})).unwrap();
    assert_eq!(from(&e, bass), Some(MidiFrom { track: keys, listen: false }));
    assert_eq!(e.undo_pushes(), pushes + 2);
    assert!(e.undo());
    assert_eq!(from(&e, bass), Some(MidiFrom { track: keys, listen: true }), "undo restores Listen");
    assert!(e.redo());
    assert_eq!(from(&e, bass), Some(MidiFrom { track: keys, listen: false }));
    // `listen` given with `from` is the new route's.
    e.execute("midi.input", &json!({"track": bass.0, "from": null})).unwrap();
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0, "listen": false})).unwrap();
    assert_eq!(from(&e, bass), Some(MidiFrom { track: keys, listen: false }));
}

#[test]
fn clearing_restores_the_kept_port_and_a_port_replaces_the_route_each_one_undo_step() {
    let (mut e, keys, bass) = engine();
    let device = json!({"backend": "winmm", "name": "Keyboard", "ordinal": 0, "id": "usb"});
    e.execute("midi.input", &json!({"track": bass.0, "port": device, "channel": 2})).unwrap();
    let kept = e.session().track(bass).unwrap().midi_input.clone();
    let pushes = e.undo_pushes();
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0})).unwrap();
    assert_eq!(e.session().track(bass).unwrap().midi_input, kept, "the hardware choice is kept while the route is set");
    e.execute("midi.input", &json!({"track": bass.0, "from": null})).unwrap();
    assert_eq!((from(&e, bass), &e.session().track(bass).unwrap().midi_input), (None, &kept), "the track takes its kept choice again");
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0})).unwrap();
    e.execute("midi.input", &json!({"track": bass.0, "port": "none"})).unwrap();
    assert_eq!(from(&e, bass), None);
    assert_eq!(e.session().track(bass).unwrap().midi_input, MidiInput { port: MidiPort::None, channel: Some(1) });
    assert_eq!(e.undo_pushes(), pushes + 4);
    assert!(e.undo());
    assert_eq!(from(&e, bass), Some(MidiFrom { track: keys, listen: true }));
    // The channel filter applies to a routed track too, and keeps the route.
    e.execute("midi.input", &json!({"track": bass.0, "channel": 3})).unwrap();
    assert_eq!((from(&e, bass).map(|m| m.track), e.session().track(bass).unwrap().midi_input.channel), (Some(keys), Some(2)));
}

#[test]
fn every_refusal_of_1_7_refuses_before_any_change_with_its_wording() {
    let (mut e, keys, bass) = engine();
    let s = e.session_mut();
    let seq = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Seq"));
    let sent = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Sent"));
    let ext = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Ext"));
    let rack = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some("Rack"));
    let gtr = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Gtr"));
    let ice = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Ice"));
    let seq_out = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("SeqOut"));
    s.track_mut(sent).unwrap().midi_output = port_out();
    s.track_mut(seq_out).unwrap().midi_output = port_out();
    s.track_mut(ext).unwrap().instrument = Some(Insert::external("In 1-2"));
    s.track_mut(ext).unwrap().midi_output = port_out();
    s.track_mut(rack).unwrap().rack = Some(Rack::default());
    s.track_mut(ice).unwrap().freeze = Some(Frozen { source: SourceId(999), start: 0, length: 10, hash: 0, revision: 0, latency: 0 });
    let to_bass = |src: TrackId| json!({"track": bass.0, "from": src.0});
    refused(&mut e, "midi.input", to_bass(seq), "Seq cannot send MIDI to other tracks: it is a MIDI track");
    refused(&mut e, "midi.input", to_bass(sent), "Sent cannot send MIDI to other tracks: it sends to an external port");
    refused(&mut e, "midi.input", to_bass(ext), "Ext cannot send MIDI to other tracks: it holds an External Instrument");
    refused(&mut e, "midi.input", to_bass(rack), "Rack cannot send MIDI to other tracks: it is a rack");
    refused(&mut e, "midi.input", to_bass(ice), "Ice cannot send MIDI to other tracks: it is frozen");
    let from_keys = |dest: TrackId| json!({"track": dest.0, "from": keys.0});
    refused(&mut e, "midi.input", from_keys(gtr), "Gtr cannot take MIDI from another track: it is not an instrument track");
    refused(&mut e, "midi.input", from_keys(seq), "Seq cannot take MIDI from another track: it is not an instrument track");
    refused(
        &mut e,
        "midi.input",
        from_keys(seq_out),
        "SeqOut cannot take MIDI from another track: it sends to an external port; routing to a hardware port is not available yet",
    );
    refused(
        &mut e,
        "midi.input",
        from_keys(ext),
        "Ext cannot take MIDI from another track: it holds an External Instrument; routing to a hardware port is not available yet",
    );
    refused(
        &mut e,
        "midi.input",
        from_keys(sent),
        "Sent cannot take MIDI from another track: it sends to an external port; routing to a hardware port is not available yet",
    );
    refused(&mut e, "midi.input", from_keys(rack), "Rack cannot take MIDI from another track: it is a rack");
    refused(&mut e, "midi.input", json!({"track": bass.0, "from": bass.0}), "that route would create a routing loop: Bass → Bass");
    refused(&mut e, "midi.input", json!({"track": bass.0, "listen": true}), "Bass takes no MIDI from another track");
    // A frozen track is a fine destination (decision 6.3).
    e.execute("midi.input", &json!({"track": ice.0, "from": keys.0})).unwrap();
    // A loop through two routes: Keys → Bass → Lead, then Lead → Keys.
    let lead = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0})).unwrap();
    e.execute("midi.input", &json!({"track": lead.0, "from": bass.0})).unwrap();
    refused(&mut e, "midi.input", json!({"track": keys.0, "from": lead.0}), "that route would create a routing loop: Keys → Bass → Lead → Keys");
    // Several destinations are checked together: one bad one refuses them all.
    refused(
        &mut e,
        "midi.input",
        json!({"tracks": [lead.0, gtr.0], "from": keys.0}),
        "Gtr cannot take MIDI from another track: it is not an instrument track",
    );
}

#[test]
fn a_loop_through_a_key_and_a_route_is_refused() {
    let (mut e, keys, bass) = engine();
    let mut ins = Insert::new("compressor");
    ins.key = Some(transit_model::KeySource { from: transit_model::KeyFrom::Track(bass), tap: transit_model::Tap::Post });
    e.session_mut().track_mut(keys).unwrap().mixer.inserts[0] = Some(ins);
    refused(&mut e, "midi.input", json!({"track": bass.0, "from": keys.0}), "that route would create a routing loop: Bass → Keys → Bass");
}

#[test]
fn a_routed_track_refuses_a_port_output_and_an_external_instrument() {
    let (mut e, _keys, bass) = engine();
    e.execute("midi.input", &json!({"track": bass.0, "from": "Keys"})).unwrap();
    let message = "Bass takes MIDI from Keys; routing to a hardware port is not available yet";
    refused(&mut e, "midi.output", json!({"track": bass.0, "port": port_json()}), message);
    refused(&mut e, "instrument.external", json!({"track": bass.0, "port": port_json(), "return": "In 1-2"}), message);
    // Back to internal is no port: allowed.
    e.execute("midi.output", &json!({"track": bass.0, "port": "internal"})).unwrap();
}

#[test]
fn a_track_preset_with_an_external_instrument_is_refused_on_a_routed_destination() {
    let dir = transit_test_support::TestDir::new("midi-route-preset");
    std::fs::create_dir_all(&dir).unwrap();
    let (mut e, keys, bass) = engine();
    let synth = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Synth"));
    e.session_mut().track_mut(synth).unwrap().instrument = Some(Insert::external("In 1-2"));
    let path = dir.join("ext.tpreset").to_string_lossy().into_owned();
    e.execute("track.save_preset", &json!({"path": path, "track": synth.0})).unwrap();
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0})).unwrap();
    refused(
        &mut e,
        "track.load_preset",
        json!({"path": path, "track": bass.0}),
        "Bass takes MIDI from Keys; routing to a hardware port is not available yet",
    );
    // Without the route the same preset loads.
    e.execute("midi.input", &json!({"track": bass.0, "from": null})).unwrap();
    e.execute("track.load_preset", &json!({"path": path, "track": bass.0})).unwrap();
    assert!(e.session().track(bass).unwrap().external_instrument().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// No route into a track with an external MIDI output is in force (it would be a MIDI edge).
fn no_route_into_hardware(e: &Engine) {
    let s = e.session();
    for (i, t) in s.tracks.iter().enumerate() {
        if transit_model::sends_to_hardware(t) {
            assert!(!matches!(s.midi_route_state(i), RouteState::InForce { .. }), "{} takes a route in force", t.name);
        }
    }
}

#[test]
fn duplicating_and_undoing_across_route_changes_never_leave_a_route_into_hardware_in_force() {
    let (mut e, keys, bass) = engine();
    e.execute("midi.output", &json!({"track": bass.0, "port": port_json()})).unwrap();
    assert!(e.undo());
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0, "listen": false})).unwrap();
    e.execute("track.duplicate", &json!({"track": bass.0})).unwrap();
    let dup = e.session().track_by_name("Bass.dup").unwrap().id;
    assert_eq!(from(&e, dup), Some(MidiFrom { track: keys, listen: false }), "the copy takes the same route");
    refused(
        &mut e,
        "midi.output",
        json!({"track": dup.0, "port": port_json()}),
        "Bass.dup takes MIDI from Keys; routing to a hardware port is not available yet",
    );
    no_route_into_hardware(&e);
    while e.undo() {
        no_route_into_hardware(&e);
    }
    while e.redo() {
        no_route_into_hardware(&e);
    }
    assert_eq!(from(&e, dup), Some(MidiFrom { track: keys, listen: false }));
}

#[test]
fn deleting_a_source_disconnects_its_destinations_and_undo_reconnects_them() {
    let (mut e, keys, bass) = engine();
    let lead = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0, "listen": false})).unwrap();
    e.execute("midi.input", &json!({"track": lead.0, "from": keys.0})).unwrap();
    e.execute("track.delete", &json!({"tracks": [keys.0]})).unwrap();
    let state = |e: &Engine, t| e.session().midi_route_state(e.session().track_index(t).unwrap());
    for t in [bass, lead] {
        assert_eq!(state(&e, t), RouteState::Disconnected);
        assert_eq!(from(&e, t).map(|m| m.track), Some(keys), "the field still names the deleted track");
    }
    assert!(e.undo());
    assert!(matches!(state(&e, bass), RouteState::InForce { .. }));
    assert_eq!(from(&e, bass), Some(MidiFrom { track: keys, listen: false }), "with its Listen");
    assert_eq!(from(&e, lead), Some(MidiFrom { track: keys, listen: true }));
}

#[test]
fn a_source_cannot_be_frozen() {
    let (mut e, keys, bass) = engine();
    let lead = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
    e.execute("midi.input", &json!({"track": bass.0, "from": keys.0})).unwrap();
    e.execute("midi.input", &json!({"track": lead.0, "from": bass.0})).unwrap();
    refused(&mut e, "track.freeze", json!({"tracks": [keys.0]}), "Keys sends MIDI to Bass; remove that route or freeze Bass instead");
    // Also when it is a destination itself, and through Flatten.
    refused(&mut e, "track.freeze", json!({"tracks": [bass.0]}), "Bass sends MIDI to Lead; remove that route or freeze Lead instead");
    refused(&mut e, "track.flatten", json!({"tracks": [keys.0]}), "Keys sends MIDI to Bass; remove that route or freeze Bass instead");
}

#[test]
fn an_imported_route_follows_its_source_or_is_disconnected() {
    let dir = transit_test_support::TestDir::new("midi-route-import");
    std::fs::create_dir_all(&dir).unwrap();
    let mut donor = Session::default();
    let keys = donor.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
    let bass = donor.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Bass"));
    let lead = donor.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Lead"));
    donor.track_mut(bass).unwrap().midi_from = Some(MidiFrom { track: keys, listen: false });
    donor.track_mut(lead).unwrap().midi_from = Some(MidiFrom { track: keys, listen: true });
    let path = dir.join("donor.transit");
    std::fs::write(&path, donor.to_json().unwrap()).unwrap();
    // The receiver holds a track whose id is the donor's Keys id: it must never become a source.
    let mut s = Session { next_id: keys.0, ..Session::default() };
    let decoy = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Decoy"));
    assert_eq!(decoy, keys);
    let mut e = Engine::new(s);
    let p = path.to_string_lossy().into_owned();
    e.execute("file.import_session_data", &json!({"path": p, "tracks": ["Keys", "Bass"]})).unwrap();
    e.execute("file.import_session_data", &json!({"path": p, "tracks": ["Lead"]})).unwrap();
    let s = e.session();
    let id = |n: &str| s.track_by_name(n).unwrap().id;
    assert_eq!(s.track(id("Bass")).unwrap().midi_from, Some(MidiFrom { track: id("Keys"), listen: false }), "followed to the copy of its source");
    assert_ne!(id("Keys"), decoy);
    let lead_from = s.track(id("Lead")).unwrap().midi_from.unwrap();
    assert!(lead_from.listen && s.track(lead_from.track).is_none(), "its source did not come along: disconnected");
    let _ = std::fs::remove_dir_all(&dir);
}
