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
    _handle: transit_midi_io::InputHandle,
    _fake: Fake,
}

fn rig() -> Rig {
    let (control, _plans) = plan_bridge::pair(
        RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: 512, out_channels: 0, in_channels: 0 },
        PlanLimits::default(),
    );
    let (lane, _audio) = action_lane::pair_with_result(&control);
    let (send, mut worker) = super::tests::pump(lane);
    let (out, events) = rtrb::RingBuffer::new(64);
    worker.map_out = out;
    let fake = Fake::default();
    let key = fake.add_input("KeyStep");
    let (handle, consumer) = fake.io().open_input(&key, InputConfig::default()).unwrap();
    send.send(Update::Input(Input { id: PORT, mon: Mon::default(), consumer, routes: Vec::new() })).ok().unwrap();
    worker.step();
    let port = fake.input_port(&key).unwrap();
    Rig { worker, send, events, port, wanted: Wanted::default(), _handle: handle, _fake: fake }
}

impl Rig {
    /// Hand the worker these mappings, with "KeyStep" open as input `PORT`.
    fn map(&mut self, specs: Vec<MapSpec>) {
        self.wanted.set(specs);
        let snapshot = self.wanted.rebuild(vec![Some(PORT); self.wanted.len()]).unwrap();
        self.send.send(Update::Maps(snapshot)).ok().unwrap();
        self.worker.step();
    }
    fn cc(&mut self, cc: u8, value: u8) -> Vec<MapEvent> {
        self.port.deliver(0, &[0xB0, cc, value]);
        self.worker.step();
        self.drain()
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
    /// What Main reports for control 0: the value, and how many events it has applied.
    fn control_is(&self, value: f32, applied: u32) {
        self.wanted.set_values(&[(value, applied)]);
    }
}

fn spec(kind: SourceKind, number: u8, mode: MapMode, button: bool) -> MapSpec {
    MapSpec { port: "KeyStep".into(), channel: Some(0), kind, number, mode, min: 0.0, max: 1.0, toggle: true, button }
}
fn moves(events: &[MapEvent]) -> Vec<f32> {
    events.iter().filter_map(|e| if let MapEvent::Move { value, .. } = e { Some(*value) } else { None }).collect()
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
    assert_eq!(moves(&caught).len(), 1, "crossing the fader picks it up");
    assert!((moves(&caught)[0] - 110.0 / 127.0).abs() < 1e-6);
    assert!(matches!(caught[0], MapEvent::Move { index: 0, .. }));
    assert_eq!(moves(&rig.cc(7, 100)).len(), 1, "then it follows");
    assert!(rig.cc(8, 100).is_empty(), "another CC is not mapped");
}

#[test]
fn jump_mode_moves_the_fader_to_the_knob_at_once() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    rig.control_is(0.8, 0);
    assert_eq!(moves(&rig.cc(7, 0)), vec![0.0]);
    assert_eq!(moves(&rig.cc(7, 127)), vec![1.0]);
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
    let events = rig.cc(74, 127);
    assert_eq!(events.len(), 1, "only the session's mapping answers CC 74");
    assert!(matches!(events[0], MapEvent::Move { index: 0, value, .. } if value == 1.0));
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
    assert!(rig.drain().len() >= 30);
}

#[test]
fn an_unrouted_port_maps_without_being_popped() {
    let mut rig = rig();
    rig.map(vec![spec(SourceKind::Cc, 7, MapMode::Jump, false)]);
    rig.port.deliver(0, &[0xB0, 7, 100]);
    rig.worker.step();
    assert_eq!(moves(&rig.drain()).len(), 1);
    assert_eq!(rig.worker.inputs[0].consumer.len(), 1, "a route attached later still finds its backlog");
    rig.worker.step();
    assert!(rig.drain().is_empty(), "and the message is not matched twice");
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
    assert!(rig.cc(7, 99).iter().all(|e| matches!(e, MapEvent::Candidate { .. })));
}
