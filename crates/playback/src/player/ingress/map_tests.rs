#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! MIDI Learn on the ingress worker (ADR 0028), through the fake backend and the real pump.
use super::*;
use crate::plan_bridge;
use crate::player::midi_map::{MapSpec, Wanted};
use soundcraft_mix::{DeviceGeneration, PlanLimits, RenderTarget};
use soundcraft_model::{Control, ControlTarget, MapMode, Mapping, MappingSet, MidiSource, Scope, SourceKind, Strip, TrackId, resolve};
use transit_midi_io::fake::FakeInputPort;
use transit_midi_io::{InputConfig, fake::Fake};
use transit_rt::testing::measure;

const PORT: u64 = 3;

struct Rig {
    worker: Pump,
    send: SyncSender<Update>,
    events: rtrb::Consumer<MapEvent>,
    port: FakeInputPort,
    wanted: Wanted,
    fake: Fake,
    key: transit_midi_io::PortKey,
    _handle: transit_midi_io::InputHandle,
    /// Keeps the registry, plan bridge and action lane alive for the routed rig.
    _keep: Box<dyn std::any::Any>,
}

fn rig() -> Rig {
    build(false)
}

/// `routed`: the port also feeds a track (a route that admits), as when the track is armed.
fn build(routed: bool) -> Rig {
    let (control, plans) = plan_bridge::pair(
        RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: 512, out_channels: 0, in_channels: 0 },
        PlanLimits::default(),
    );
    let (lane, audio) = action_lane::pair_with_result(&control);
    let (send, mut worker) = super::tests::pump(lane);
    let (out, events) = rtrb::RingBuffer::new(64);
    worker.map_out = out;
    let fake = Fake::default();
    let key = fake.add_input("KeyStep");
    let (handle, consumer) = fake.io().open_input(&key, InputConfig::default()).unwrap();
    let (mut registry, sources) = sources::pair(&control).unwrap();
    let routes = if routed {
        let token = registry.register(4).unwrap();
        vec![(registry.permit(token).unwrap(), super::tests::route())]
    } else {
        Vec::new()
    };
    send.send(Update::Input(Input { id: PORT, mon: Mon::default(), consumer, routes })).ok().unwrap();
    worker.step();
    let port = fake.input_port(&key).unwrap();
    Rig { worker, send, events, port, wanted: Wanted::new(), fake, key, _handle: handle, _keep: Box::new((registry, sources, control, audio, plans)) }
}

impl Rig {
    /// Hand the worker these mappings, with "KeyStep" open as input `PORT`.
    fn map(&mut self, specs: Vec<MapSpec>) {
        self.wanted.set(specs);
        let snapshot = self.wanted.rebuild(vec![Some(PORT); self.wanted.len()]).unwrap();
        self.send.send(Update::Maps(snapshot)).ok().unwrap();
        self.worker.step();
    }
    /// Deliver a CC and step; the continuous values Main would apply.
    fn cc(&mut self, cc: u8, value: u8) -> Vec<f32> {
        self.port.deliver(0, &[0xB0, cc, value]);
        self.worker.step();
        self.moves()
    }
    fn moves(&self) -> Vec<f32> {
        self.wanted.take_moves().into_iter().map(|(_, v, _)| v).collect()
    }
    fn note(&mut self, on: bool, key: u8) -> Vec<MapEvent> {
        self.port.deliver(0, &[if on { 0x90 } else { 0x80 }, key, 100]);
        self.worker.step();
        self.drain()
    }
    fn drain(&mut self) -> Vec<MapEvent> {
        let mut v = Vec::new();
        while let Ok(e) = self.events.pop() {
            v.push(e);
        }
        v
    }
    /// What Main reports for control 0: the value, and the publish count it has applied.
    fn control_is(&self, value: f32, applied: u32) {
        self.wanted.set_values(&[(value, applied)]);
    }
    fn admitted(&self) -> u64 {
        self.worker.shared.admitted.load(Ordering::Relaxed)
    }
}

fn spec(kind: SourceKind, number: u8, mode: MapMode, button: bool) -> MapSpec {
    MapSpec { port: "KeyStep".into(), channel: Some(0), kind, number, mode, min: 0.0, max: 1.0, toggle: true, button, pass_through: false }
}

#[test]
fn a_cc_moves_a_fader_with_pickup_and_not_before() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Pickup, false)]);
    rig.control_is(0.8, 0);
    assert!(rig.cc(7, 0).is_empty(), "the knob is far below the fader: the fader does not jump");
    assert!(rig.cc(7, 60).is_empty(), "still below");
    assert!(rig.cc(7, 90).is_empty(), "still below 0.8 (101/127)");
    let caught = rig.cc(7, 110);
    assert_eq!(caught.len(), 1, "crossing the fader picks it up");
    assert!((caught[0] - 110.0 / 127.0).abs() < 1e-6);
    assert_eq!(rig.cc(7, 100).len(), 1, "then it follows");
    assert!(rig.cc(8, 100).is_empty(), "another CC is not mapped");
}

#[test]
fn jump_mode_moves_the_fader_to_the_knob_at_once() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    rig.control_is(0.8, 0);
    assert_eq!(rig.cc(7, 0), vec![0.0]);
    assert_eq!(rig.cc(7, 127), vec![1.0]);
}

#[test]
fn a_note_toggles_mute_on_note_on_only() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Note, 36, MapMode::Pickup, true)]);
    assert_eq!(rig.note(true, 36), vec![MapEvent::Press { epoch: rig.wanted.epoch(), index: 0, set: None }]);
    assert!(rig.note(false, 36).is_empty(), "note-off does nothing");
    assert!(rig.note(true, 37).is_empty(), "another key");
}

fn src(cc: u8) -> MidiSource {
    MidiSource { port: "KeyStep".into(), channel: Some(0), kind: SourceKind::Cc, number: cc }
}

#[test]
fn the_session_mapping_wins_a_conflict_with_the_controller_profile() {
    let tracks = [TrackId(10), TrackId(11)];
    let mut session = MappingSet::default();
    let mut profile = MappingSet::default();
    // The profile says CC 74 is strip 1's volume; this session says CC 74 is track 11's pan.
    profile.set(Mapping::new(src(74), ControlTarget { strip: Strip::Position(1), control: Control::Volume })).unwrap();
    session.set(Mapping::new(src(74), ControlTarget { strip: Strip::Track(TrackId(11)), control: Control::Pan })).unwrap();
    let resolved = resolve(&tracks, &session, &profile);
    let active: Vec<_> = resolved.iter().filter(|r| !r.overridden).collect();
    assert_eq!(active.len(), 1);
    assert_eq!((active[0].scope, active[0].track, active[0].control), (Scope::Session, TrackId(11), Control::Pan));
    let mut rig = rig();
    let mut specs: Vec<MapSpec> = active.iter().map(|r| MapSpec::from_resolved(r)).collect();
    specs[0].mode = MapMode::Jump;
    rig.map(specs);
    assert_eq!(rig.cc(74, 127), vec![1.0], "only the session's mapping answers CC 74");
}

#[test]
fn without_mappings_or_a_learn_the_worker_does_no_mapping_work() {
    let mut rig = rig();
    for n in 0..20 {
        rig.port.deliver(0, &[0xB0, 7, n]);
    }
    let (_, heap) = measure(|| rig.worker.step());
    assert!(heap.is_zero(), "{heap:?}");
    assert!(!rig.worker.map_pass_on);
    assert_eq!(rig.worker.inputs[0].mon.map_next, 0, "nothing was matched or even looked at");
    assert!(rig.drain().is_empty());
}

#[test]
fn matching_and_publishing_cost_the_worker_no_heap_work() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false), spec(SourceKind::Note, 36, MapMode::Pickup, true)]);
    for n in 0..30 {
        rig.port.deliver(0, &[0xB0, 7, n]);
        rig.port.deliver(0, &[0x90, 36, 100]);
    }
    let (_, heap) = measure(|| rig.worker.step());
    assert!(heap.is_zero(), "{heap:?}");
    assert_eq!(rig.moves().len(), 1, "the sweep is coalesced into its newest value");
    assert_eq!(rig.drain().len(), 30);
}

#[test]
fn an_unrouted_port_maps_without_being_popped() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    rig.port.deliver(0, &[0xB0, 7, 100]);
    rig.worker.step();
    assert_eq!(rig.moves().len(), 1);
    assert_eq!(rig.worker.inputs[0].consumer.len(), 1, "a route attached later still finds its backlog");
    rig.worker.step();
    assert!(rig.moves().is_empty(), "and the message is not matched twice");
}

#[test]
fn learn_forwards_exactly_the_next_candidate_once() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    // Armed: the first message is the candidate; the worker disarms itself and forwards no more.
    rig.worker.shared.learn.store(true, Ordering::Release);
    rig.port.deliver(0, &[0xB0, 21, 5]);
    rig.port.deliver(0, &[0xB0, 22, 6]);
    rig.port.deliver(0, &[0x90, 40, 100]);
    rig.worker.step();
    let events = rig.drain();
    assert_eq!(events, vec![MapEvent::Candidate { port: PORT, channel: 0, kind: SourceKind::Cc, number: 21 }], "the first message only");
    assert!(!rig.worker.shared.learn.load(Ordering::Acquire), "the worker disarms itself");
    rig.worker.step();
    assert!(rig.drain().is_empty(), "nothing more is forwarded");
    // Re-armed: a note is the candidate; non-candidates (pitch bend, CC 123) are skipped over.
    rig.worker.shared.learn.store(true, Ordering::Release);
    rig.port.deliver(0, &[0xE0, 0, 64]);
    rig.port.deliver(0, &[0xB0, 123, 0]);
    rig.worker.step();
    assert!(rig.drain().is_empty());
    rig.port.deliver(0, &[0x93, 50, 90]);
    rig.worker.step();
    assert_eq!(rig.drain(), vec![MapEvent::Candidate { port: PORT, channel: 3, kind: SourceKind::Note, number: 50 }]);
    // A candidate does not also move the mapped control.
    rig.worker.shared.learn.store(true, Ordering::Release);
    assert!(rig.cc(7, 99).is_empty());
    assert!(matches!(rig.drain()[..], [MapEvent::Candidate { .. }]));
}

// ---- consume by default (ADR 0028) ----

#[test]
fn a_mapped_cc_moves_the_fader_and_is_not_delivered_to_the_track() {
    let mut rig = build(true);
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    assert_eq!(rig.cc(7, 100).len(), 1, "the fader moves");
    assert_eq!(rig.admitted(), 0, "and the track's route never sees it");
    // Another CC on the same port still reaches the track.
    rig.port.deliver(0, &[0xB0, 74, 20]);
    rig.worker.step();
    assert_eq!(rig.admitted(), 1);
}

#[test]
fn with_pass_through_a_mapped_cc_does_both() {
    let mut rig = build(true);
    let mut sp = spec(SourceKind::Cc, 7, MapMode::Jump, false);
    sp.pass_through = true;
    rig.map(vec![sp]);
    assert_eq!(rig.cc(7, 100).len(), 1);
    assert_eq!(rig.admitted(), 1, "the track gets it too");
}

#[test]
fn a_note_mapped_to_mute_toggles_and_does_not_sound_and_its_note_off_goes_with_it() {
    let mut rig = build(true);
    rig.map(vec![spec(SourceKind::Note, 36, MapMode::Pickup, true)]);
    assert_eq!(rig.note(true, 36).len(), 1, "the press toggles mute");
    rig.note(false, 36);
    assert_eq!(rig.admitted(), 0, "neither the note-on nor its note-off reaches the track");
    rig.note(true, 37);
    assert_eq!(rig.admitted(), 1, "an unmapped key still plays");
}

#[test]
fn a_note_held_while_a_mapping_is_added_still_gets_its_note_off_through() {
    let mut rig = build(true);
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    // The note-on arrives before the mapping of key 36 exists, so it plays.
    rig.note(true, 36);
    assert_eq!(rig.admitted(), 1);
    rig.map(vec![spec(SourceKind::Note, 36, MapMode::Pickup, true)]);
    rig.note(false, 36);
    assert_eq!(rig.admitted(), 2, "the note-off is not swallowed: the note would hang");
}

#[test]
fn a_mapping_removed_while_its_note_is_held_still_consumes_that_note_off() {
    let mut rig = build(true);
    rig.map(vec![spec(SourceKind::Note, 36, MapMode::Pickup, true)]);
    rig.note(true, 36);
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    rig.note(false, 36);
    assert_eq!(rig.admitted(), 0, "the track never heard the note-on, so it does not get the note-off");
}

#[test]
fn a_mapped_message_arriving_between_the_scan_and_the_pop_is_still_matched() {
    // S1: on a routed port the match happens where the event is popped. CC 74 is unmapped and
    // routed; while the worker handles it, CC 7 (mapped) arrives, after the scan at the step's start.
    let mut rig = build(true);
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    let late = rig.fake.input_port(&rig.key).unwrap();
    let mut once = Some(());
    rig.worker.between_check_and_push = Some(Box::new(move || {
        if once.take().is_some() {
            late.deliver(0, &[0xB0, 7, 99]);
        }
    }));
    rig.port.deliver(0, &[0xB0, 74, 1]);
    rig.worker.step();
    assert_eq!(rig.moves(), vec![99.0 / 127.0], "the late message moved the fader");
    assert_eq!(rig.admitted(), 1, "only CC 74 reached the track");
}

// ---- a slow UI loses nothing (B2) ----

#[test]
fn a_thousand_ccs_a_second_on_two_ports_against_a_sixty_hertz_apply_loop_lose_no_final_value() {
    let mut rig = rig();
    let fake2 = Fake::default();
    let key2 = fake2.add_input("Second");
    let (_h2, consumer2) = fake2.io().open_input(&key2, InputConfig::default()).unwrap();
    rig.send.send(Update::Input(Input { id: PORT + 1, mon: Mon::default(), consumer: consumer2, routes: Vec::new() })).ok().unwrap();
    rig.worker.step();
    let port2 = fake2.input_port(&key2).unwrap();
    let mut s2 = spec(SourceKind::Cc, 9, MapMode::Jump, false);
    s2.port = "Second".into();
    rig.wanted.set(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false), s2]);
    let snapshot = rig.wanted.rebuild(vec![Some(PORT), Some(PORT + 1)]).unwrap();
    rig.send.send(Update::Maps(snapshot)).ok().unwrap();
    rig.worker.step();
    // One second of simulated time: 1000 messages per port per second, the UI applies at 60 Hz.
    let (mut last_a, mut last_b) = (0.0f32, 0.0f32);
    let mut applied = Vec::new();
    for frame in 0..60u32 {
        for n in 0..17u32 {
            let tick = frame * 17 + n;
            let (a, b) = ((tick % 128) as u8, ((tick * 3) % 128) as u8);
            rig.port.deliver(0, &[0xB0, 7, a]);
            port2.deliver(0, &[0xB0, 9, b]);
            (last_a, last_b) = (f32::from(a) / 127.0, f32::from(b) / 127.0);
        }
        rig.worker.step();
        applied.extend(rig.wanted.take_moves());
    }
    let newest = |i: u32| applied.iter().rev().find(|m| m.0 == i).map(|m| m.1);
    assert_eq!(newest(0), Some(last_a), "the fader lands where the knob stopped");
    assert_eq!(newest(1), Some(last_b));
    assert_eq!(rig.worker.shared.map_dropped.load(Ordering::Relaxed), 0);
}

#[test]
fn presses_that_overflow_the_ring_are_counted_not_silent() {
    let mut rig = rig();
    let (out, events) = rtrb::RingBuffer::new(4);
    rig.worker.map_out = out;
    rig.events = events;
    rig.map(vec![spec(SourceKind::Note, 36, MapMode::Pickup, true)]);
    for _ in 0..10 {
        rig.port.deliver(0, &[0x90, 36, 100]);
        rig.port.deliver(0, &[0x80, 36, 0]);
    }
    rig.worker.step();
    assert_eq!(rig.drain().len(), 4);
    assert_eq!(rig.worker.shared.map_dropped.load(Ordering::Relaxed), 6, "the six that did not fit are counted");
    assert_eq!(rig.worker.shared.map_dropped.load(Ordering::Relaxed), 6);
}
