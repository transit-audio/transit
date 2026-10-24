#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The External Instrument's Measure (ADR 0020 decision 3) through the real Player: a fake duplex
//! device and a fake MIDI port. The "synth" answers each note-on `C` plan frames after the block
//! the note went out on; the Player's audio side sends the notes through the MIDI output thread and
//! records the return, and the median comes back as `C`. No hardware audio or MIDI.
use super::*;
use crate::external_probe::{self, MeasureError};
use ab::fake::{SliceSink, SliceSource};
use soundcraft_model::{ChannelFormat, MidiOutChannel, MidiOutPort, MidiOutput, TrackKind};
use std::time::{Duration, Instant};
use transit_midi_io::fake::Fake;

const BLOCK: usize = 192;
/// The Model D on a Scarlett at 48 kHz / 64 (`docs/benchmarks/2026-10-24-...-with-compensation.md`).
const C: usize = 955;
/// Blocks pumped before the measurement arms (the plan and the input ring settle).
const ARM_AT: usize = 20;

fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "did not reach: {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

struct Rig {
    player: Player,
    audio: ab::fake::FakeBackend,
    midi: Fake,
    key: transit_midi_io::PortKey,
    track: TrackId,
    _recorder: record::Recorder,
}

/// An instrument track holding an External Instrument returning on In 1, sending to `MODEL D` on
/// MIDI channel 3; a duplex fake device at 48 kHz, its input feeding the plan.
fn rig(armed: bool) -> Rig {
    let midi = Fake::default();
    let key = midi.add_output("MODEL D");
    let mut s = Session::default();
    let track = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Model D"));
    let t = s.track_mut(track).unwrap();
    t.instrument = Some(soundcraft_model::Insert::external("In 1"));
    t.midi_output = MidiOutput::External {
        port: MidiOutPort { backend: key.backend.as_str().into(), name: key.name.clone(), ordinal: key.ordinal, id: key.id.clone() },
        channel: MidiOutChannel::Channel(2),
        offset_ms: 0.0,
    };
    t.mixer.record_arm = armed;
    let audio = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).with_capture(2, true);
    let fallback = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32));
    let cfg = AudioConfig::default();
    let mut player = Player::open_native_or_cpal(Arc::new(s), &cfg, &audio, &fallback).unwrap();
    player.midi.get_mut().as_mut().unwrap().enable(midi.io());
    let recorder = player.open_input(&cfg).expect("the duplex input");
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    let rig = Rig { player, audio, midi, key, track, _recorder: recorder };
    let (mut output, input) = (vec![0.0f32; BLOCK * 2], vec![0.0f32; BLOCK * 2]);
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    until("the destination opens", || {
        rig.audio.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 2 });
        rig.audio.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 });
        rig.player.service().unwrap();
        rig.player.midi_status().unwrap().output.track(track) == Some(&crate::player::midi_out::DestinationState::Connected)
    });
    rig._recorder.monitor.clear_monitor();
    rig
}

/// What a pumped run saw: the result (if it finished) and the blocks pumped.
struct Pumped {
    result: Option<Result<external_probe::Measured, MeasureError>>,
}

/// Pump capture and render in lockstep. The probe arms after [`ARM_AT`] blocks; the synth
/// answers each note-on, which goes out at plan frame `on_at(k)` after arming (a block boundary
/// at this block size), `C` frames later on input 1. `stop(block, player)` may end the pumping.
fn pump(rig: &Rig, blocks: usize, mut stop: impl FnMut(usize, &Player) -> bool) -> Pumped {
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let (mut output, mut input) = (vec![0.0f32; BLOCK * 2], vec![0.0f32; BLOCK * 2]);
    let on_at = |k: u64| 48 * (100 + 1500 * k);
    let arm = (ARM_AT * BLOCK) as u64;
    let answers: Vec<u64> = (0..=external_probe::NOTES as u64).map(|k| arm + on_at(k) + C as u64).collect();
    let mut result = None;
    for i in 0..blocks {
        if i == ARM_AT {
            rig.player.start_external_measure(rig.track, &[0]).unwrap_or_else(|e| panic!("{e}"));
        }
        for f in 0..BLOCK {
            let n = (i * BLOCK + f) as u64;
            // A fast attack: one loud sample, then a quieter tone for 100 ms.
            let x = answers.iter().fold(0.0f32, |x, &a| match n.checked_sub(a) {
                Some(0) => x + 0.9,
                Some(d) if d < 4_800 => x + if d.is_multiple_of(2) { 0.05 } else { -0.05 },
                _ => x,
            });
            input[f * 2] = x;
            input[f * 2 + 1] = 0.0;
        }
        assert!(rig.audio.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 2 }));
        if !rig.audio.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 }) {
            break;
        }
        rig.player.service().unwrap();
        if i > ARM_AT
            && let Some(r) = rig.player.external_measure_result()
        {
            result = Some(r);
            break;
        }
        if stop(i, &rig.player) {
            break;
        }
    }
    Pumped { result }
}

fn messages(rig: &Rig) -> Vec<Vec<u8>> {
    rig.midi.sent(&rig.key)
}

/// Whether the last note-on at the port is followed by its note-off (any release velocity) and
/// All Notes Off on channel 3.
fn released(sent: &[Vec<u8>]) -> bool {
    let Some(last_on) = sent.iter().rposition(|m| *m == vec![0x92, 60, 100]) else { return false };
    let after = &sent[last_on..];
    after.iter().any(|m| m[..2] == [0x82, 60]) && after.contains(&vec![0xB2, 123, 0])
}

#[test]
fn measure_finds_the_round_trip_in_plan_frames_and_releases_on_the_port() {
    let rig = rig(false);
    let duration = rig.player.external_measure_duration();
    assert!((duration.as_secs_f64() - 13.22).abs() < 0.01, "{duration:?}");
    let blocks = (duration.as_secs_f64() * 48_000.0 / BLOCK as f64) as usize + ARM_AT + 50;
    let mut progress = Vec::new();
    let run = pump(&rig, blocks, |_, p| {
        if let Some(at) = p.external_measure_progress() {
            progress.push(at.note);
        }
        false
    });
    let m = run.result.expect("the measurement finished").expect("every note came back");
    assert_eq!(m.frames, C as u32, "the median is C: {m:?}");
    assert_eq!((m.found, m.sent), (8, 8));
    assert_eq!(m.spread, 0.0);
    assert!(progress.windows(2).all(|w| w[0] <= w[1]) && progress.contains(&3), "{progress:?}");
    assert!(!rig.player.external_measure_running());
    // Nine notes on (the warm-up and eight) and off on channel 3, then All Notes Off, all sent by
    // the output thread.
    until("All Notes Off", || messages(&rig).contains(&vec![0xB2, 123, 0]));
    let sent = messages(&rig);
    assert_eq!(sent.iter().filter(|m| **m == vec![0x92, 60, 100]).count(), 9, "{sent:x?}");
    assert_eq!(sent.iter().filter(|m| **m == vec![0x82, 60, 0]).count(), 9, "{sent:x?}");
    assert_eq!(sent.last(), Some(&vec![0xB2, 123, 0]));
    let status = rig.player.midi_status().unwrap().output;
    assert_eq!((status.audio_thread_sends, status.dropped), (0, 0));
}

#[test]
fn cancel_releases_the_held_note_and_measures_nothing() {
    let rig = rig(false);
    // Into the second measured note's hold: 100 ms + 2 × 1500 ms + 40 ms after arming.
    let held = ARM_AT + (48 * 3140) / BLOCK;
    let run = pump(&rig, held + 20, |i, p| {
        if i == held {
            p.cancel_external_measure();
        }
        i > held + 10
    });
    assert!(run.result.is_none(), "nothing measured");
    assert!(!rig.player.external_measure_running());
    until("the note-off and All Notes Off after the last note-on", || released(&messages(&rig)));
    // The warm-up and the first note went out; the second either went out and was released, or
    // was still waiting for its instant and was dropped with the track's scheduled playback.
    let ons = messages(&rig).iter().filter(|m| **m == vec![0x92, 60, 100]).count();
    assert!((2..=3).contains(&ons), "{:x?}", messages(&rig));
    assert!(rig.player.external_measure_result().is_none());
}

#[test]
fn a_device_lost_mid_measure_releases_the_note_and_refuses() {
    let rig = rig(false);
    let held = ARM_AT + (48 * 1640) / BLOCK;
    pump(&rig, held + 1, |i, _| i >= held);
    assert!(rig.player.external_measure_running());
    until("the first measured note is sent", || messages(&rig).iter().filter(|m| **m == vec![0x92, 60, 100]).count() == 2);
    assert!(rig.audio.inject_error(ab::Direction::Output, ab::StreamError::DeviceNotAvailable));
    // The callbacks stop; control sees the loss.
    assert_eq!(rig.player.external_measure_result(), Some(Err(MeasureError::DeviceLost)));
    assert!(!rig.player.external_measure_running());
    until("the output thread releases the track", || released(&messages(&rig)));
}

#[test]
fn measure_is_refused_while_playing_armed_without_a_port_or_input_and_one_at_a_time() {
    let rig = rig(true);
    assert!(rig.player.start_external_measure(rig.track, &[0]).unwrap_err().contains("disarm"), "an armed track");
    let rig = rig_unarmed();
    assert!(rig.player.start_external_measure(rig.track, &[5]).unwrap_err().contains("channel"));
    assert!(rig.player.start_external_measure(TrackId(9_999), &[0]).is_err());
    rig.player.start_hardware_probe(&[0], &[0]).unwrap();
    assert!(rig.player.start_external_measure(rig.track, &[0]).unwrap_err().contains("already running"));
    rig.player.cancel_hardware_probe();
    rig.player.start_external_measure(rig.track, &[0]).unwrap();
    assert!(rig.player.start_hardware_probe(&[0], &[0]).unwrap_err().contains("already running"), "one measurement at a time");
    assert!(rig.player.start_external_measure(rig.track, &[0]).unwrap_err().contains("already running"));
    rig.player.cancel_external_measure();
    // Playing.
    let rig = rig_unarmed();
    rig.player.play(0, None, None).unwrap();
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let (mut output, input) = (vec![0.0f32; BLOCK * 2], vec![0.0f32; BLOCK * 2]);
    until("playing", || {
        rig.audio.pump_capture(BLOCK, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 2 });
        rig.audio.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 2 });
        rig.player.service().unwrap();
        rig.player.is_playing()
    });
    assert!(rig.player.start_external_measure(rig.track, &[0]).unwrap_err().contains("stop the transport"));
    // No live input: an output-only device.
    let audio = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32));
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(soundcraft_model::Insert::external("In 1"));
    let player = Player::open_native_or_cpal(Arc::new(s), &AudioConfig::default(), &audio, &audio).unwrap();
    assert!(player.start_external_measure(t, &[0]).unwrap_err().contains("duplex"));
}

fn rig_unarmed() -> Rig {
    rig(false)
}
