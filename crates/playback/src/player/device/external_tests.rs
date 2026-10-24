#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! External MIDI output (#61) through the real Player: the audio callback schedules a MIDI
//! track's clip into the output thread, which sends on the fake backend. No hardware MIDI.
use super::*;
use soundcraft_model::{ChannelFormat, Clip, ClipId, MidiOutChannel, MidiOutPort, MidiOutput, TrackKind};
use std::time::{Duration, Instant};
use transit_midi_io::fake::Fake;
use transit_rt::testing::measure;

const QUARTER: i64 = 24_000;

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "did not reach: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A MIDI track (no instrument) playing to `Synth OUT`: C at 0 for an eighth, E from the eighth
/// for a quarter, so E sounds across a one-beat loop's end.
fn session(fake: &Fake) -> (Session, TrackId, transit_midi_io::PortKey) {
    let key = fake.add_output("Synth OUT");
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let ppq = soundcraft_time::TICKS_PER_QUARTER;
    // Playback does not depend on the sequence crate; the clip's sequence comes from its JSON.
    let seq = serde_json::from_value(serde_json::json!({"notes": [
        {"pitch": 60, "velocity": 100, "release_velocity": 64, "channel": 4, "start": 0, "length": ppq / 2},
        {"pitch": 64, "velocity": 90, "release_velocity": 64, "channel": 4, "start": ppq / 2, "length": ppq},
    ], "ctrls": []}))
    .unwrap();
    let track = s.track_mut(t).unwrap();
    track.playlist_mut().unwrap().clips.push(Clip::midi(ClipId(1), "phrase", 0, 4 * QUARTER, seq));
    track.midi_output = MidiOutput::External {
        port: MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() },
        channel: MidiOutChannel::AsRecorded,
        offset_ms: 0.0,
    };
    (s, t, key)
}

#[test]
fn a_looped_midi_track_plays_its_clip_to_the_port_off_the_audio_thread_and_stop_releases() {
    let fake = Fake::default();
    let (s, track, key) = session(&fake);
    assert_eq!(s.sample_rate.hz(), 48_000);
    let (mut player, access, _clock) = Player::prepare(Arc::new(s), 48_000, soundcraft_model::IoMap::default()).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(fake.io());
    let mut callback = DeviceOutput::new(access, &player, 2);
    until("the destination opens", || {
        player.service().unwrap();
        player.midi_status().unwrap().output.track(track) == Some(&crate::player::midi_out::DestinationState::Connected)
    });
    // This thread plays the device callback, marked as the audio thread like a real one.
    crate::mark_audio_thread();
    let audio = std::thread::current().id();
    let mut block = [0.0f32; 1024];
    callback.fill_f32(&mut block);
    player.play(0, None, Some(soundcraft_time::Range { start: 0, end: QUARTER })).unwrap();
    // Real-time pacing (512 frames is 10.7 ms at 48 kHz): the fallback clock stamps blocks by
    // callback time, so callbacks must not run ahead of the wall clock. 80 blocks are 40 960
    // samples: lap 2 stops inside E.
    for _ in 0..80 {
        let started = Instant::now();
        let ((), heap) = measure(|| callback.fill_f32(&mut block));
        assert!(heap.is_zero(), "the callback allocated while scheduling external MIDI: {heap:?}");
        std::thread::sleep(Duration::from_micros(10_700).saturating_sub(started.elapsed()));
    }
    player.stop().unwrap();
    for _ in 0..4 {
        callback.fill_f32(&mut block);
        std::thread::sleep(Duration::from_millis(11));
    }
    soundcraft_mix::unmark_audio_thread();
    until("Stop's All Notes Off", || fake.sent(&key).contains(&vec![0xB4, 123, 0]));
    let sends = fake.sends(&key);
    let messages: Vec<Vec<u8>> = sends.iter().map(|s| s.message.clone()).collect();
    // Lap 1: C, then C off and E at the eighth. The wrap ends E (it sounds across the loop end);
    // lap 2 starts C again. Stop releases what is held and resets the channel.
    let expected_start: Vec<Vec<u8>> = vec![
        vec![0x94, 60, 100],
        vec![0x84, 60, 64],
        vec![0x94, 64, 90],
        vec![0x84, 64, 64],
        vec![0x94, 60, 100],
        vec![0x84, 60, 64],
        vec![0x94, 64, 90],
    ];
    assert_eq!(messages.get(..expected_start.len()), Some(&expected_start[..]), "{messages:x?}");
    assert_eq!(messages.get(messages.len() - 2..), Some(&[vec![0xB4, 64, 0], vec![0xB4, 123, 0]][..]), "{messages:x?}");
    assert!(messages.contains(&vec![0x84, 64, 64]));
    for send in &sends {
        assert_ne!(send.thread, audio, "a MIDI send ran on the audio thread");
    }
    let status = player.midi_status().unwrap().output;
    assert_eq!(status.audio_thread_sends, 0);
    assert_eq!(status.dropped, 0);
    // E is held at the stop: its off comes first, then the channel reset.
    assert_eq!(messages.get(expected_start.len()), Some(&vec![0x84, 64, 64]), "{messages:x?}");
}

#[test]
fn live_input_on_an_armed_external_track_is_echoed_without_an_instrument() {
    let fake = Fake::default();
    let (mut s, track, key) = session(&fake);
    let input = fake.add_input("Keys");
    s.track_mut(track).unwrap().mixer.record_arm = true;
    s.track_mut(track).unwrap().midi_output = MidiOutput::External {
        port: MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() },
        channel: MidiOutChannel::Channel(1),
        offset_ms: 100.0,
    };
    let (mut player, access, _clock) = Player::prepare(Arc::new(s), 48_000, soundcraft_model::IoMap::default()).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(fake.io());
    let mut callback = DeviceOutput::new(access, &player, 2);
    until("the input route and the destination are ready", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        let status = player.midi_status().unwrap();
        status.routes == 1 && status.output.track(track) == Some(&crate::player::midi_out::DestinationState::Connected)
    });
    let played = Instant::now();
    assert!(fake.input_port(&input).unwrap().deliver(0, &[0x90, 60, 100]));
    until("the echo", || fake.sent(&key).contains(&vec![0x91, 60, 100]));
    let latency = fake.sends(&key).iter().find(|s| s.message == vec![0x91, 60, 100]).unwrap().at - played;
    // This is a stall bound, not a latency measurement (#87, E49). The wake-on-data property of
    // the ingress worker (#62) is asserted from the worker's own timestamps by
    // `an_idle_ingress_worker_wakes_on_the_input_callback_not_on_a_timer`. A 10 ms wall-clock
    // bound across two threads tripped on a loaded macOS runner (10.96 ms), so this one allows
    // 250 ms: over 10x the scheduling jitter seen on CI and 12x the worker's 20 ms idle safety
    // net, and still far below a lost wake-up or a stuck echo ring.
    assert!(latency < Duration::from_millis(250), "echo took {latency:?}");
    assert!(fake.input_port(&input).unwrap().deliver(1_000, &[0x80, 60, 0]));
    until("the echoed release", || fake.sent(&key).contains(&vec![0x81, 60, 0]));
}

/// Refutation B1, through the Player: a key and the sustain pedal held through the echo when the
/// track stops echoing (`disarm`) or its controller is unplugged (`unplug`).
#[test]
fn the_midi_monitor_sees_input_routed_or_not_and_every_message_sent() {
    use crate::player::monitor::{Dir, MonitorEvent};
    let fake = Fake::default();
    let (mut s, track, key) = session(&fake);
    let keys = fake.add_input("Keys");
    let other = fake.add_input("Unused pads");
    s.track_mut(track).unwrap().mixer.record_arm = true;
    s.track_mut(track).unwrap().midi_input = soundcraft_model::MidiInput {
        port: soundcraft_model::MidiPort::Device {
            backend: keys.backend.as_str().into(),
            name: keys.name.clone(),
            ordinal: keys.ordinal,
            id: keys.id.clone(),
        },
        channel: None,
    };
    let (mut player, access, _clock) = Player::prepare(Arc::new(s), 48_000, soundcraft_model::IoMap::default()).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(fake.io());
    player.set_midi_monitor(true, false);
    let mut callback = DeviceOutput::new(access, &player, 2);
    until("both inputs open and the destination connects", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        let status = player.midi_status().unwrap();
        status.open_inputs == 2 && status.routes == 1 && status.output.track(track) == Some(&crate::player::midi_out::DestinationState::Connected)
    });
    assert!(fake.input_port(&keys).unwrap().deliver(0, &[0x90, 60, 100]));
    assert!(fake.input_port(&other).unwrap().deliver(0, &[0x99, 36, 90]));
    let mut events: Vec<MonitorEvent> = Vec::new();
    let mut report = Default::default();
    until("the monitor holds both inputs and the echo", || {
        player.service().unwrap();
        report = player.take_midi_monitor(&mut events, 1000);
        events.len() >= 3
    });
    let name = |e: &MonitorEvent| report.ports.iter().find(|p| p.dir == e.dir && p.id == e.port).map(|p| p.name.clone()).unwrap_or_default();
    let seen = |dir, status, data1, port: &str| events.iter().any(|e| e.dir == dir && e.status == status && e.data1 == data1 && name(e) == port);
    assert!(seen(Dir::In, 0x90, 60, "Keys"), "{events:?} {report:?}");
    assert!(seen(Dir::In, 0x99, 36, "Unused pads"), "the unrouted port is shown too: {events:?}");
    assert!(seen(Dir::Out, 0x90, 60, "Synth OUT"), "the echo is shown as sent: {events:?} {report:?}");
    assert_eq!(player.midi_status().unwrap().output.dropped, 0);
    assert_eq!((report.dropped_in, report.dropped_out), (0, 0));
    // Off: nothing is copied.
    player.set_midi_monitor(false, false);
    let before = events.len();
    assert!(fake.input_port(&keys).unwrap().deliver(0, &[0x90, 61, 100]));
    until("the echo of the second note", || fake.sent(&key).iter().any(|m| m.get(1) == Some(&61)));
    player.take_midi_monitor(&mut events, 1000);
    assert_eq!(events.len(), before);
}

fn held_echo_is_released(unplug: bool) {
    let fake = Fake::default();
    let (mut s, track, key) = session(&fake);
    let input = fake.add_input("Keys");
    s.track_mut(track).unwrap().mixer.record_arm = true;
    let (mut player, access, _clock) = Player::prepare(Arc::new(s.clone()), 48_000, soundcraft_model::IoMap::default()).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(fake.io());
    let mut callback = DeviceOutput::new(access, &player, 2);
    until("ready", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        let status = player.midi_status().unwrap();
        status.routes == 1 && status.output.track(track) == Some(&crate::player::midi_out::DestinationState::Connected)
    });
    assert!(fake.input_port(&input).unwrap().deliver(0, &[0x90, 60, 100, 0xB0, 64, 127]));
    until("echo", || fake.sent(&key).contains(&vec![0x90, 60, 100]) && fake.sent(&key).contains(&vec![0xB0, 64, 127]));
    if unplug {
        fake.remove_input(&input);
    } else {
        s.track_mut(track).unwrap().mixer.record_arm = false;
        player.update_session(Arc::new(s)).unwrap();
    }
    until("the held note and pedal end on the port", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        let sent = fake.sent(&key);
        sent.contains(&vec![0x80, 60, 64]) && sent.contains(&vec![0xB0, 64, 0])
    });
    assert_eq!(player.midi_status().unwrap().output.audio_thread_sends, 0);
}

#[test]
fn disarming_while_a_key_and_the_pedal_are_held_releases_them_on_the_port() {
    held_echo_is_released(false);
}

#[test]
fn unplugging_the_controller_while_a_key_and_the_pedal_are_held_releases_them_on_the_port() {
    held_echo_is_released(true);
}

/// ADR 0020 decision 6: an External Instrument whose MIDI port is missing plays silent with no
/// latency. The Player follows the output thread's view of the port and prepares a plan that
/// leaves the device out while the port is gone, and one that has it back when it returns.
#[test]
fn an_external_instruments_missing_port_takes_it_out_of_the_plan_until_the_port_returns() {
    let fake = Fake::default();
    let key = fake.add_output("MODEL D");
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
    let track = s.track_mut(t).unwrap();
    let mut ins = soundcraft_model::Insert::external("In 1");
    ins.external.as_mut().unwrap().manual_ms = Some(19.5);
    track.instrument = Some(ins);
    track.midi_output = MidiOutput::External {
        port: MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() },
        channel: MidiOutChannel::Channel(0),
        offset_ms: 0.0,
    };
    let (mut player, access, _clock) = Player::prepare(Arc::new(s), 48_000, soundcraft_model::IoMap::default()).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(fake.io());
    let mut callback = DeviceOutput::new(access, &player, 2);
    let state = |player: &Player| player.midi_status().unwrap().output.track(t).cloned();
    until("the destination opens", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        state(&player) == Some(crate::player::midi_out::DestinationState::Connected)
    });
    assert!(player.external_off.borrow().is_empty(), "a present port keeps the device in the plan");
    fake.remove_output(&key);
    until("the plan leaves the device out", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        *player.external_off.borrow() == [t]
    });
    // The synth is plugged in again on the same interface path.
    fake.add_output_with_id("MODEL D", &key.id);
    until("the device is back", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        player.external_off.borrow().is_empty()
    });
}

/// M1 of the slice review: a port that is present but will not open (Failed, another program
/// holds it) takes the device out of the plan just as a missing one does; MIDI unavailable as a
/// whole does too, since no note can reach the synth. A port not looked at yet, or open, keeps it;
/// a bare MIDI-out track is never listed.
#[test]
fn a_failed_or_missing_port_or_no_midi_at_all_takes_an_external_instrument_out_of_the_plan() {
    use crate::player::midi_out::{DestinationState, OutputStatus, TrackOutput};
    let mut s = Session::default();
    let bare = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
    let device = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
    s.track_mut(device).unwrap().instrument = Some(soundcraft_model::Insert::external("In 1"));
    for t in [bare, device] {
        s.track_mut(t).unwrap().midi_output =
            MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::Channel(0), offset_ms: 0.0 };
    }
    let status = |state: Option<DestinationState>| OutputStatus {
        tracks: state
            .map(|state| [bare, device].map(|track| TrackOutput { track, port: "MODEL D".into(), state: state.clone() }).to_vec())
            .unwrap_or_default(),
        ..OutputStatus::default()
    };
    assert_eq!(external_unavailable(&s, Some(&status(None))), [], "not looked at yet: present");
    assert_eq!(external_unavailable(&s, Some(&status(Some(DestinationState::Pending)))), []);
    assert_eq!(external_unavailable(&s, Some(&status(Some(DestinationState::Connected)))), []);
    assert_eq!(external_unavailable(&s, Some(&status(Some(DestinationState::Missing)))), [device]);
    assert_eq!(external_unavailable(&s, Some(&status(Some(DestinationState::Failed("held by another program".into()))))), [device]);
    assert_eq!(external_unavailable(&s, None), [device], "MIDI unavailable: the device cannot play");
}

/// The MIDI-unavailable edge of the slice review: with no MIDI runtime the Player still follows
/// the External Instrument and prepares a plan that leaves it out, so its C is not held in delay
/// compensation while no note can be sent.
#[test]
fn without_midi_an_external_instrument_is_left_out_of_the_plan() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
    let track = s.track_mut(t).unwrap();
    let mut ins = soundcraft_model::Insert::external("In 1");
    ins.external.as_mut().unwrap().manual_ms = Some(19.5);
    track.instrument = Some(ins);
    track.midi_output = MidiOutput::External { port: MidiOutPort::default(), channel: MidiOutChannel::Channel(0), offset_ms: 0.0 };
    let (mut player, access, _clock) = Player::prepare(Arc::new(s), 48_000, soundcraft_model::IoMap::default()).unwrap();
    // As when the MIDI runtime failed to start.
    drop(player.midi.get_mut().take());
    let mut callback = DeviceOutput::new(access, &player, 2);
    until("the plan leaves the device out", || {
        player.service().unwrap();
        callback.fill_f32(&mut [0.0f32; 256]);
        *player.external_off.borrow() == [t]
    });
}

/// #81: Cycle turned on while E already sounds (no loop at Play). The wrap the new loop brings ends
/// E at the loop end and lap 2 starts C again, as a loop set before Play does: no stuck note.
#[test]
fn a_loop_set_mid_note_while_playing_ends_the_held_note_at_its_wrap() {
    let fake = Fake::default();
    let (s, track, key) = session(&fake);
    let (mut player, access, _clock) = Player::prepare(Arc::new(s), 48_000, soundcraft_model::IoMap::default()).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(fake.io());
    let mut callback = DeviceOutput::new(access, &player, 2);
    until("the destination opens", || {
        player.service().unwrap();
        player.midi_status().unwrap().output.track(track) == Some(&crate::player::midi_out::DestinationState::Connected)
    });
    crate::mark_audio_thread();
    let mut block = [0.0f32; 1024];
    callback.fill_f32(&mut block);
    player.play(0, None, None).unwrap();
    let mut wrapped = false;
    for k in 0..80 {
        let started = Instant::now();
        callback.fill_f32(&mut block);
        // E sounds from 12 000; the loop goes on at about 15 000, ahead of its 24 000 end.
        if k == 29 {
            let at = player.position();
            assert!((QUARTER / 2..QUARTER).contains(&at), "{at}");
            player.set_loop(Some(soundcraft_time::Range { start: 0, end: QUARTER })).unwrap();
        }
        wrapped |= k > 29 && player.position() < QUARTER / 2;
        std::thread::sleep(Duration::from_micros(10_700).saturating_sub(started.elapsed()));
    }
    assert!(wrapped, "the transport never wrapped");
    player.stop().unwrap();
    for _ in 0..4 {
        callback.fill_f32(&mut block);
        std::thread::sleep(Duration::from_millis(11));
    }
    soundcraft_mix::unmark_audio_thread();
    until("Stop's All Notes Off", || fake.sent(&key).contains(&vec![0xB4, 123, 0]));
    let messages: Vec<Vec<u8>> = fake.sends(&key).iter().map(|s| s.message.clone()).collect();
    // Lap 1: C, C off, E. The wrap ends E; lap 2 starts C again.
    let expected_start: Vec<Vec<u8>> = vec![
        vec![0x94, 60, 100],
        vec![0x84, 60, 64],
        vec![0x94, 64, 90],
        vec![0x84, 64, 64],
        vec![0x94, 60, 100],
        vec![0x84, 60, 64],
        vec![0x94, 64, 90],
    ];
    assert_eq!(messages.get(..expected_start.len()), Some(&expected_start[..]), "{messages:x?}");
    // Every note-on is ended by Stop at the latest.
    for pitch in [60u8, 64] {
        let ons = messages.iter().filter(|m| m[..2] == [0x94, pitch]).count();
        let offs = messages.iter().filter(|m| m[..2] == [0x84, pitch]).count();
        assert_eq!(ons, offs, "pitch {pitch} left sounding: {messages:x?}");
    }
}
