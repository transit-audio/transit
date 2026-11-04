#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::super::{
    Audio, Command,
    tests::{fill, fixture_session, make, recording_clock},
};
use super::*;
use crate::player::Control;
use crate::record::start;
use std::sync::{Arc, atomic::Ordering};
use transit_mix::{EngineEpoch, Message, NoteId, SlotBinding, Source};
use transit_model::{ChannelFormat, Insert, Session, TrackKind};
use transit_rt::testing::measure;

struct Rig {
    control: Control<Command, ActionResult>,
    registry: sources::Control,
    lane: action_lane::Control<Packet, ActionResult>,
    audio: Audio,
    token: sources::Token,
    target: SlotBinding,
    capture: crate::midi_capture::Capture,
}

impl Rig {
    fn new(device: u32) -> Self {
        let mut session = Session::default();
        let track = session.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        session.track_mut(track).unwrap().instrument = Some(Insert::new("macro_oscillator"));
        let (mut control, mut audio, _) = fixture_session(device, Arc::new(session));
        let (mut registry, sources) = sources::pair(&control.sessions.bridge).unwrap();
        let token = registry.register(4).unwrap();
        audio.sources = sources;
        let (lane, midi) = action_lane::pair_with_result(&control.sessions.bridge);
        audio.set_midi(midi);
        fill(&mut audio, &mut []);
        control.service(&mut make);
        let target = control.sessions.active_bindings()[0];
        recording_clock(&mut audio, device);
        audio.plans.install_event_tap_for_test(512);
        let (capture, sink) = crate::midi_capture::pair(crate::midi_capture::CAPACITY);
        audio.set_midi_capture(sink);
        Self { control, registry, lane, audio, token, target, capture }
    }
    fn packet(&self, output: u64, sequence: u64, release: bool) -> Packet {
        let audio = &self.audio;
        let host = audio.transport.clock_map.unwrap().frame_to_host(audio.transport.clock_key, output, audio.transport.clock_now.unwrap()).unwrap();
        Packet {
            engine: EngineEpoch(1),
            target: self.target,
            source: Source { port: self.token.port(), generation: self.token.generation(), channel: 0 },
            message: if release {
                Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE }
            } else {
                Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE }
            },
            raw: transit_midi_io::InputEvent {
                host_arrival_ns: host,
                backend_time_us: 987,
                seq: sequence,
                bytes: [if release { 0x80 } else { 0x90 }, 60, 100],
                len: 3,
            },
            lookahead: 0,
            deliver: true,
        }
    }
    fn send(&mut self, output: u64, sequence: u64, release: bool) -> Packet {
        let packet = self.packet(output, sequence, release);
        self.lane.send_note(packet, if release { action_lane::NoteKind::Off } else { action_lane::NoteKind::On }).unwrap();
        packet
    }
    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut output = vec![0.0; frames * 2];
        let (_, heap) = measure(|| fill(&mut self.audio, &mut output));
        assert!(heap.is_zero(), "{heap:?}");
        output
    }
}

#[test]
fn live_notes_sound_without_a_clock_map_and_keep_their_original_arrival() {
    for device in [44_100, 48_000, 96_000] {
        let mut rig = Rig::new(device);
        let mut packet = rig.packet(100, 0, false);
        packet.lookahead = 37;
        rig.audio.transport.clock_map = None;
        rig.lane.send_note(packet, action_lane::NoteKind::On).unwrap();
        let output = rig.render(256);
        let tap = rig.audio.plans.engine().event_tap();
        assert_eq!(tap.len(), 1, "unavailable clock must not silence live playing");
        assert_eq!(tap[0].due, Some(RenderFrame((37 * 48_000 + u64::from(device) / 2) / u64::from(device))));
        assert_eq!(tap[0].arrival, Arrival { host_ns: packet.raw.host_arrival_ns, sequence: 0 });
        assert!(output.iter().any(|sample| sample.abs() > 0.000_001), "the actual instrument must sound");
        assert_eq!(rig.lane.reclaim().unwrap().handler_result, Some(ActionResult::MidiBufferedApproximate));
    }
}

#[test]
fn clock_expiry_keeps_releases_working_and_recovery_restores_fitted_placement() {
    let mut rig = Rig::new(48_000);
    let now = rig.audio.transport.clock_now;
    let mut off = rig.packet(20, 1, true);
    off.lookahead = 7;
    rig.send(10, 0, false);
    rig.render(32);
    assert_eq!(rig.lane.reclaim().unwrap().handler_result, Some(ActionResult::MidiBuffered));
    rig.audio.transport.clock_now = Some(u64::MAX); // Present but expired map.
    rig.lane.send_note(off, action_lane::NoteKind::Off).unwrap();
    rig.render(64);
    let tap = rig.audio.plans.engine().event_tap();
    assert_eq!(tap.len(), 2);
    assert_eq!(tap[1].due, Some(RenderFrame(39)));
    assert_eq!(tap[1].arrival.sequence, 1);
    assert_eq!(tap[1].outcome, transit_mix::EventOutcome::Queued);
    assert_eq!(rig.lane.reclaim().unwrap().handler_result, Some(ActionResult::MidiBufferedApproximate));
    assert!(rig.registry.admits(rig.token));
    rig.audio.transport.clock_now = now;
    rig.send(150, 2, false);
    rig.render(128);
    assert_eq!(rig.audio.plans.engine().event_tap()[2].due, Some(RenderFrame(150)));
    assert_eq!(rig.lane.reclaim().unwrap().handler_result, Some(ActionResult::MidiBuffered));
}

#[test]
fn switching_timing_modes_cannot_reorder_buffered_note_and_pedal_edges() {
    let mut rig = Rig::new(48_000);
    let on = rig.packet(200, 0, false);
    let mut pedal = rig.packet(210, 1, false);
    pedal.message = Message::Control { cc: 64, value: 127 };
    pedal.raw.bytes = [0xb0, 64, 127];
    let off = rig.packet(220, 2, true);
    assert_eq!(rig.audio.midi_inbox.admit(on, 200, false, &rig.audio.sources), ActionResult::MidiBuffered);
    assert_eq!(rig.audio.midi_inbox.admit(pedal, 100, true, &rig.audio.sources), ActionResult::MidiBufferedApproximate);
    assert_eq!(rig.audio.midi_inbox.admit(off, 50, false, &rig.audio.sources), ActionResult::MidiBufferedApproximate);
    rig.render(128);
    assert!(rig.audio.plans.engine().event_tap().is_empty());
    rig.render(128);
    let tap = rig.audio.plans.engine().event_tap();
    assert_eq!(tap.len(), 3);
    assert_eq!(tap.iter().map(|event| event.arrival.sequence).collect::<Vec<_>>(), [0, 1, 2]);
    assert!(tap.iter().all(|event| event.due == Some(RenderFrame(200))));
}

#[test]
fn stop_and_disconnect_cancel_approximate_events_before_dispatch() {
    for disconnect in [false, true] {
        let mut rig = Rig::new(48_000);
        let mut packet = rig.packet(100, 0, false);
        packet.lookahead = 200;
        rig.audio.transport.clock_map = None;
        rig.lane.send_note(packet, action_lane::NoteKind::On).unwrap();
        rig.render(32);
        assert_eq!(rig.audio.midi_inbox.count, 1);
        if disconnect {
            rig.registry.permit(rig.token).unwrap().invalidate();
        } else {
            rig.registry.cancel_before(rig.token, 1).unwrap();
            rig.control.stop().unwrap();
        }
        rig.render(256);
        assert!(rig.audio.plans.engine().event_tap().is_empty());
        assert_eq!(rig.audio.midi_inbox.count, 0);
    }
}

#[test]
fn a_cancelled_approximate_predecessor_cannot_delay_new_input_after_stop() {
    let mut rig = Rig::new(48_000);
    let mut old = rig.packet(100, 0, false);
    old.lookahead = 200;
    let mut new = rig.packet(110, 1, false);
    new.lookahead = 0;
    rig.audio.transport.clock_map = None;
    rig.lane.send_note(old, action_lane::NoteKind::On).unwrap();
    rig.render(32);
    rig.registry.cancel_before(rig.token, 1).unwrap();
    rig.control.stop().unwrap();
    rig.lane.send_note(new, action_lane::NoteKind::On).unwrap();
    rig.render(32);
    let tap = rig.audio.plans.engine().event_tap();
    assert_eq!(tap.len(), 1);
    assert_eq!(tap[0].arrival.sequence, 1);
    assert_eq!(tap[0].due, Some(RenderFrame(32)));
}

#[test]
fn real_audio_path_preserves_original_arrival_and_partition_independent_due_frames() {
    for device in [44_100, 48_000, 96_000] {
        for partitions in [vec![256], vec![17, 31, 64, 1, 143]] {
            let mut rig = Rig::new(device);
            let mut packet = rig.packet(100, 0, false);
            packet.lookahead = 37;
            rig.lane.send_note(packet, action_lane::NoteKind::On).unwrap();
            for frames in partitions {
                rig.render(frames);
            }
            let tap = rig.audio.plans.engine().event_tap();
            assert_eq!(tap.len(), 1);
            assert_eq!(tap[0].due, Some(RenderFrame((137 * 48_000 + u64::from(device) / 2) / u64::from(device))));
            assert_eq!(tap[0].arrival, Arrival { host_ns: packet.raw.host_arrival_ns, sequence: 0 });
            assert_eq!(tap[0].source, packet.source);
            assert_eq!(tap[0].outcome, transit_mix::EventOutcome::Queued);
        }
    }
}

#[test]
fn future_note_is_projected_after_speed_changes_without_moving_its_device_deadline() {
    let mut rig = Rig::new(48_000);
    rig.send(200, 0, false);
    rig.render(64);
    assert!(rig.audio.plans.engine().event_tap().is_empty());
    assert_eq!(rig.audio.midi_inbox.count, 1);
    rig.audio.shared.speed.store(2000, Ordering::Relaxed);
    rig.render(160);
    let tap = rig.audio.plans.engine().event_tap();
    assert_eq!(tap.len(), 1);
    assert_eq!(tap[0].due, Some(RenderFrame(64 + (200 - 64) * 2)));
}

#[test]
fn stop_cutoff_cancels_a_future_note_already_buffered_on_audio() {
    let mut rig = Rig::new(48_000);
    rig.send(200, 0, false);
    rig.render(64);
    rig.registry.cancel_before(rig.token, 1).unwrap();
    rig.control.stop().unwrap();
    rig.render(256);
    assert!(rig.audio.plans.engine().event_tap().is_empty());
    assert_eq!(rig.audio.midi_inbox.count, 0);
    rig.send(400, 1, false);
    rig.render(128);
    assert_eq!(rig.audio.plans.engine().event_tap().len(), 1, "new raw offers after Stop remain eligible");
}

#[test]
fn later_arrival_does_not_hide_an_earlier_deadline_on_another_route() {
    let mut rig = Rig::new(48_000);
    rig.send(400, 0, false);
    rig.send(20, 1, false);
    rig.render(64);
    let tap = rig.audio.plans.engine().event_tap();
    assert_eq!(tap.len(), 1);
    assert_eq!(tap[0].arrival.sequence, 1);
    assert_eq!(tap[0].due, Some(RenderFrame(20)));
    assert_eq!(rig.audio.midi_inbox.count, 1);
}

#[test]
fn deferred_capacity_reserves_releases_and_unknown_releases_do_not_revoke_other_notes() {
    let mut rig = Rig::new(48_000);
    for sequence in 0..(CAPACITY - RELEASE_RESERVE) {
        assert_eq!(rig.audio.midi_inbox.admit(rig.packet(400, sequence as u64, false), 400, false, &rig.audio.sources), ActionResult::MidiBuffered);
    }
    assert_eq!(rig.audio.midi_inbox.admit(rig.packet(400, 900, false), 400, false, &rig.audio.sources), ActionResult::MidiFull);
    for sequence in 0..RELEASE_RESERVE {
        assert_eq!(
            rig.audio.midi_inbox.admit(rig.packet(400, 1000 + sequence as u64, true), 400, false, &rig.audio.sources),
            ActionResult::MidiBuffered
        );
    }
    assert_eq!(rig.audio.midi_inbox.admit(rig.packet(400, 1200, true), 400, false, &rig.audio.sources), ActionResult::MidiFull);
    let mut rig = Rig::new(48_000);
    rig.send(10, 0, true);
    rig.render(64);
    assert!(rig.registry.admits(rig.token));
    assert_eq!(rig.audio.plans.engine().event_tap()[0].outcome, transit_mix::EventOutcome::Unmatched);
}

fn recording(rig: &mut Rig) {
    rig.audio.transport.playing = true;
    rig.audio.transport.recording = true;
}

#[test]
fn a_recording_pass_captures_admitted_events_at_the_frames_the_instrument_gets_them_without_allocating() {
    for device in [44_100, 48_000] {
        let mut rig = Rig::new(device);
        recording(&mut rig);
        rig.send(100, 0, false);
        rig.send(300, 1, true);
        let mut pedal = rig.packet(320, 2, false);
        pedal.message = Message::Control { cc: 64, value: 127 };
        pedal.raw.bytes = [0xb0, 64, 127];
        rig.lane.send_note(pedal, action_lane::NoteKind::On).unwrap();
        rig.render(512);
        rig.capture.poll();
        let got: Vec<_> = rig.capture.events().iter().map(|e| (e.frame, e.track, e.channel, e.approximate)).collect();
        assert_eq!(got, vec![(100, rig.target.0, 0, false), (300, rig.target.0, 0, false), (320, rig.target.0, 0, false)]);
        assert!(matches!(rig.capture.events()[0].message, Message::NoteOn { key: 60, velocity: 100, .. }));
        assert!(matches!(rig.capture.events()[1].message, Message::NoteOff { key: 60, .. }));
        assert!(matches!(rig.capture.events()[2].message, Message::Control { cc: 64, value: 127 }));
        assert_eq!(rig.capture.dropped(), 0);
        // The instrument got the same events.
        assert_eq!(rig.audio.plans.engine().event_tap().len(), 3);
    }
}

#[test]
fn an_unclocked_event_is_captured_at_its_approximate_placement_and_flagged() {
    let mut rig = Rig::new(48_000);
    recording(&mut rig);
    let mut packet = rig.packet(100, 0, false);
    packet.lookahead = 37;
    rig.audio.transport.clock_map = None;
    let first = rig.audio.transport.timeline.next_output();
    rig.lane.send_note(packet, action_lane::NoteKind::On).unwrap();
    rig.render(128);
    rig.capture.poll();
    assert_eq!(rig.capture.events().len(), 1);
    assert_eq!((rig.capture.events()[0].frame, rig.capture.events()[0].approximate), (first + 37, true));
}

#[test]
fn nothing_is_captured_before_recording_starts_but_the_count_in_captures_pickups() {
    let mut rig = Rig::new(48_000);
    rig.send(100, 0, false);
    rig.render(256);
    rig.audio.transport.playing = true;
    rig.send(400, 1, false);
    rig.render(256);
    rig.capture.poll();
    assert!(rig.capture.events().is_empty(), "playing is not recording");
    // A count-in under way (#327): what is played over the click is captured at its frame, as a
    // pickup for the take, while the transport stays parked and not recording.
    let (mut start, _publisher) = start::pair(EngineEpoch(1));
    let ticket = start.prepare().unwrap().ticket();
    rig.audio.transport.count_in = Some(super::super::Counting {
        ticket,
        from: 0,
        end: None,
        looped: None,
        plan: super::super::CountIn { frames: 96_000, beat: 24_000, per_bar: 4 },
        left: 96_000,
    });
    rig.audio.transport.playing = false;
    rig.send(700, 2, false);
    rig.render(512);
    rig.capture.poll();
    assert_eq!(rig.capture.events().iter().map(|e| e.frame).collect::<Vec<_>>(), [700], "the count-in captures pickups");
    assert_eq!(rig.audio.plans.engine().event_tap().len(), 3, "the instrument sounds them all");
}

/// Send a real record count-in of `frames` project samples from `from` (a MIDI-only pass) and
/// apply it with one callback.
fn counting_in(rig: &mut Rig, from: i64, frames: i64) -> (start::Control, start::Reader) {
    let (mut start, publisher) = start::pair(EngineEpoch(1));
    rig.audio.transport.record_start = publisher;
    let reader = start.prepare().unwrap();
    let plan = super::super::CountIn { frames, beat: frames / 4, per_bar: 4 };
    let command = Command::Record { ticket: reader.ticket(), from: Some(from), end: None, looped: None, count_in: Some(plan) };
    rig.control.send_action(command, true).unwrap();
    rig.render(64);
    assert!(rig.audio.transport.count_in.is_some() && !rig.audio.transport.playing && !rig.audio.transport.recording);
    (start, reader)
}

/// Run callbacks until the count-in hands over to Play and the latch.
fn latched(rig: &mut Rig, reader: &start::Reader) -> start::Point {
    for _ in 0..400 {
        if let start::Status::Started(point) = reader.status() {
            return point;
        }
        rig.render(64);
    }
    panic!("never latched: {:?}", reader.status());
}

/// The pass's events as the take places them: session samples since the latch.
fn offsets(rig: &mut Rig, point: start::Point) -> Vec<i64> {
    rig.capture.poll();
    let take =
        crate::MidiTake { tracks: vec![rig.target.0], start: Some(point), events: rig.capture.events().to_vec(), dropped: 0, device_rate: 48_000 };
    take.timed(48_000).iter().map(|(offset, _)| *offset).collect()
}

#[test]
fn a_pickup_played_during_the_count_in_lands_before_the_record_start_by_the_count_in_left() {
    let mut rig = Rig::new(48_000);
    let (_start, reader) = counting_in(&mut rig, 96_000, 4_800);
    // The count-in has rendered ahead of the callbacks; the device frame of its next sample and
    // what is left of it then describe the same instant.
    let left = rig.audio.transport.count_in.unwrap().left;
    let now = rig.audio.transport.timeline.next_output();
    rig.send(now + 1_000, 0, false);
    rig.send(now + 1_600, 1, true);
    let point = latched(&mut rig, &reader);
    assert_eq!(point.output_frame, now + u64::try_from(left).unwrap(), "the latch is where the count-in ran out");
    assert_eq!(point.position.sample, 96_000);
    assert!(rig.audio.transport.playing && rig.audio.transport.recording);
    // A note after the latch (the pass itself, or its pre-roll) keeps its place.
    rig.send(point.output_frame + 2_400, 2, false);
    rig.render(64);
    rig.render(4_096);
    let n = left - 1_000;
    assert_eq!(offsets(&mut rig, point), [-n, -n + 600, 2_400], "pickup at record_start - {n}");
}

#[test]
fn a_pickup_before_the_session_start_is_clamped_at_sample_zero() {
    let mut rig = Rig::new(48_000);
    // Record start 1,000 samples into the session, after a 4,800-sample count-in.
    let (_start, reader) = counting_in(&mut rig, 1_000, 4_800);
    let now = rig.audio.transport.timeline.next_output();
    rig.send(now, 0, false);
    let point = latched(&mut rig, &reader);
    assert_eq!(offsets(&mut rig, point), [-1_000], "clamped at the session start, never before it");
}

#[test]
fn a_full_capture_queue_drops_and_counts_and_playing_goes_on() {
    let mut rig = Rig::new(48_000);
    let (capture, sink) = crate::midi_capture::pair(8);
    rig.audio.set_midi_capture(sink);
    rig.capture = capture;
    recording(&mut rig);
    for sequence in 0..6 {
        rig.send(50 + sequence * 10, sequence, false);
    }
    rig.send(200, 6, true);
    rig.render(512);
    rig.capture.poll();
    // Half of a queue this small is kept for releases: four ons fit, two are dropped, the off fits.
    assert_eq!(rig.capture.events().len(), 5);
    assert_eq!(rig.capture.dropped(), 2);
    assert!(matches!(rig.capture.events()[4].message, Message::NoteOff { .. }));
    assert_eq!(rig.audio.plans.engine().event_tap().len(), 7, "the instrument gets every event");
}
