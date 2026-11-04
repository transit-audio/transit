#![allow(clippy::unwrap_used)]
use super::*;
use crate::Services;
use transit_engine::Engine;
use transit_midi_io::TransportEnvelope;
use transit_model::{Mapping, MidiSource};
use transit_time::{FrameRate, Range};

fn app() -> StudioApp {
    StudioApp::new(Engine::default(), None, Services::default())
}

fn event(bytes: &[u8], stamp: u64) -> ButtonEvent {
    let mut raw = TransportEnvelope { bytes: [0; 13], len: bytes.len() as u8, host_arrival_ns: stamp, seq: stamp };
    raw.bytes[..bytes.len()].copy_from_slice(bytes);
    ButtonEvent { device: "Controller".into(), generation: 1, raw }
}

fn mmc(command: u8) -> [u8; 6] {
    [0xf0, 0x7f, 0, 6, command, 0xf7]
}

#[test]
fn every_mmc_command_and_both_admitted_ids_validate_before_dispatch() {
    let s = transit_model::Session::default();
    for (command, action) in [
        (1, Action::Stop),
        (2, Action::Play),
        (3, Action::Play),
        (4, Action::FastForward),
        (5, Action::Rewind),
        (6, Action::Strobe),
        (7, Action::Exit),
        (9, Action::Pause),
    ] {
        for id in [5, 127] {
            let mut bytes = mmc(command);
            bytes[2] = id;
            assert_eq!(decode(&bytes, 5, &s), Some(action));
            assert_eq!(decode(&bytes[..5], 5, &s), None);
        }
        let mut foreign = mmc(command);
        foreign[2] = 1;
        assert_eq!(decode(&foreign, 5, &s), None);
    }
    assert_eq!(decode(&mmc(8), 0, &s), None);
    assert_eq!(decode(&[0xf8], 0, &s), None);
    assert_eq!(decode(&[0xf2, 0], 0, &s), None);
    assert_eq!(decode(&[0xf2, 128, 0], 0, &s), None);
}

#[test]
fn locate_uses_session_rate_offset_and_rejects_invalid_timecode_labels() {
    let mut s = transit_model::Session::default();
    let mut wire = [0xf0, 0x7f, 0x7f, 6, 0x44, 6, 1, 0x60, 0, 10, 0, 99, 0xf7];
    for rate in [FrameRate::Fps30, FrameRate::Fps25] {
        s.frame_rate = rate;
        assert_eq!(decode(&wire, 0, &s), Some(Action::Locate(480_000)));
        s.timecode_start = 96_000;
        assert_eq!(decode(&wire, 0, &s), Some(Action::Locate(384_000)));
        s.timecode_start = 0;
    }
    assert_eq!(decode(&wire[..12], 0, &s), None);
    let mut missing_subframe = wire.to_vec();
    missing_subframe.remove(11);
    assert_eq!(decode(&missing_subframe, 0, &s), None);
    for (index, invalid) in [(5, 5), (6, 0), (7, 24), (8, 60), (9, 60), (10, 25), (11, 100)] {
        let mut bad = wire;
        bad[index] = invalid;
        assert_eq!(decode(&bad, 0, &s), None, "byte {index}");
    }
    s.frame_rate = FrameRate::Fps29_97Drop;
    wire[8] = 1;
    wire[9] = 0;
    wire[10] = 0;
    assert_eq!(decode(&wire, 0, &s), None, "00:01:00;00 does not exist");
    wire[10] = 2;
    assert_eq!(
        decode(&wire, 0, &s),
        Some(Action::Locate(Timecode { negative: false, hours: 0, minutes: 1, seconds: 0, frames: 2 }.to_samples(s.frame_rate, s.sample_rate)))
    );
}

#[test]
fn start_is_exact_cursor_continue_is_retained_and_spp_never_starts() {
    for cycle in [false, true] {
        for preroll in [false, true] {
            let mut a = app();
            let edit = &mut a.engine.session_mut().edit;
            edit.loop_playback = cycle;
            edit.loop_range = Range::new(50_000, 900_000);
            edit.pre_post_roll = preroll;
            edit.pre_roll = 4000;
            a.set_cursor(12_345, None);
            a.dispatch_midi_buttons(vec![event(&[0xfa], 0)]);
            assert_eq!(a.sim.unwrap().0, 12_345, "Play must bypass cycle start and pre-roll");
            a.engine.transport.position = 45_678;
            a.sim.as_mut().unwrap().0 = 45_678;
            a.dispatch_midi_buttons(vec![event(&[0xfa], 1)]);
            assert_eq!(a.sim.unwrap().0, 45_678, "duplicate Start never restarts");
            a.dispatch_midi_buttons(vec![event(&[0xfc], 2)]);
            assert!(!a.is_playing());
            assert_eq!(a.midi_transport.retained, 45_678);
            a.set_cursor(999, None);
            a.dispatch_midi_buttons(vec![event(&[0xfc], 3)]);
            assert_eq!(a.cursor(), 999, "stopped Stop must not return to zero");
            a.dispatch_midi_buttons(vec![event(&[0xfb], 4)]);
            assert_eq!(a.sim.unwrap().0, 45_678, "Continue ignores the cursor");
            a.dispatch_midi_buttons(vec![event(&mmc(9), 5)]);
            a.dispatch_midi_buttons(vec![event(&[0xf2, 8, 0], 6)]);
            assert!(!a.is_playing());
            let at = a.engine.session().tempo.tick_to_samples(1920, a.engine.session().sample_rate);
            assert_eq!(a.midi_transport.retained, at);
            a.set_cursor(123, None);
            a.dispatch_midi_buttons(vec![event(&[0xfb], 7)]);
            assert_eq!(a.sim.unwrap().0, at);
            a.dispatch_midi_buttons(vec![event(&[0xf2, 12, 0], 8)]);
            assert!(a.is_playing());
            assert_eq!(a.sim.unwrap().0, a.engine.session().tempo.tick_to_samples(2880, a.engine.session().sample_rate));
        }
    }
}

#[test]
fn queued_pairs_and_stop_start_stop_preserve_order_and_pending_count_in() {
    for order in [false, true] {
        let mut a = app();
        a.set_cursor(321, None);
        let mut pair = vec![event(&mmc(2), 0), event(&[0xfa], 1)];
        if order {
            pair.reverse();
        }
        a.dispatch_midi_buttons(pair);
        assert_eq!(a.sim.unwrap().0, 321);
        assert_eq!(a.status, "Transport from Controller: Play");
        a.dispatch_midi_buttons(vec![event(&[0xfc], 2), event(&[0xfa], 3), event(&[0xfc], 4)]);
        assert!(!a.is_playing());
        a.sim_count_in = Some((96_000, (321, None, None)));
        a.dispatch_midi_buttons(vec![event(&mmc(3), 5), event(&mmc(2), 6)]);
        assert!(a.sim_count_in.is_some());
        a.dispatch_midi_buttons(vec![event(&mmc(1), 7)]);
        assert!(a.sim_count_in.is_none());
        assert_eq!(a.cursor(), 321);
    }
}

#[test]
fn record_refusal_uses_device_text_lifetime_and_arrival_rate_limit() {
    let mut a = app();
    a.last_frame = Some(10.0);
    a.dispatch_midi_buttons(vec![event(&mmc(6), 0)]);
    assert_eq!(a.record_hint(), Some("Record from Controller: no armed track"));
    assert_eq!(a.status, "Record from Controller: no armed track");
    assert!(!a.recording_active());
    a.last_frame = Some(13.999);
    assert!(a.record_hint().is_some());
    a.last_frame = Some(14.0);
    assert!(a.record_hint().is_none());
    let mut state = State::default();
    assert_eq!(state.line("Controller", 0, "first".into()), Some("first".into()));
    assert_eq!(state.line("Controller", 999_999_999, "suppressed".into()), None);
    assert_eq!(state.line("Controller", 1_000_000_000, "next".into()), Some("next".into()));
    assert!(state.line("Other", 0, "independent".into()).is_some());
    a.engine.transport.recording = true;
    a.engine.transport.playing = true;
    a.sim = Some((99, None, None));
    a.dispatch_midi_buttons(vec![event(&mmc(6), 2_000_000_000)]);
    assert!(a.recording_active(), "Strobe cannot toggle off a take");
    a.dispatch_midi_buttons(vec![event(&mmc(7), 3_000_000_000)]);
    assert!(!a.recording_active() && !a.is_playing(), "Exit ends and stops");
}

#[test]
fn learned_buttons_are_momentary_machine_preferences_and_override_session_sources() {
    let mut a = app();
    a.midi_learn_feed(0.0);
    let track = a.run("track.new", json!({"kind":"instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    let source = json!({"port":"Controller", "kind":"cc", "number":20, "channel":1});
    a.run("midi.map", json!({"target":format!("track:{track}:mute"), "source":source})).unwrap();
    a.run("midi.map", json!({"target":"transport:cycle", "scope":"profile", "source":source})).unwrap();
    a.midi_learn_feed(0.0);
    assert!(crate::midi_learn::rows(&a).iter().any(|r| r.scope == transit_model::Scope::Session && r.overridden));
    assert_eq!(a.engine.session().midi_mappings.len(), 1, "the overridden mapping stays stored");
    let saved = serde_json::to_value(&a.ui).unwrap();
    assert_eq!(saved["midi_transport"]["Controller"]["mappings"][0]["target"], "transport:cycle");
    a.dispatch_midi_buttons(vec![event(&[0xb0, 20, 64], 0), event(&[0xb0, 20, 127], 1)]);
    assert!(a.engine.session().edit.loop_playback);
    a.dispatch_midi_buttons(vec![event(&[0xb0, 20, 0], 2)]);
    assert!(a.engine.session().edit.loop_playback);
    a.dispatch_midi_buttons(vec![event(&[0xb0, 20, 64], 3)]);
    assert!(!a.engine.session().edit.loop_playback);
    a.ui.midi_transport.get_mut("Controller").unwrap().transport_buttons = false;
    a.dispatch_midi_buttons(vec![event(&[0xfa], 4)]);
    assert!(!a.is_playing());
    let prefs = TransportPrefs {
        mappings: vec![Mapping::new(
            MidiSource { port: "Controller".into(), channel: Some(0), kind: SourceKind::Note, number: 60 },
            TransportControl::Record.target(),
        )],
        ..Default::default()
    };
    let mut state = State::default();
    assert_eq!(state.learned(&event(&[0x90, 60, 1], 0), &prefs), Some(Action::Record));
    assert_eq!(state.learned(&event(&[0x90, 60, 127], 1), &prefs), None);
    assert_eq!(state.learned(&event(&[0x90, 60, 0], 2), &prefs), None);
    assert_eq!(state.learned(&event(&[0x90, 60, 1], 3), &prefs), Some(Action::Record));
    assert_eq!(state.learned(&event(&[0x80, 60, 64], 4), &prefs), None);
}

#[test]
fn continue_waits_for_applied_stop_position_on_the_real_player_path() {
    let engine = Engine::default();
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut a = StudioApp::new(engine, Some(player), Services::default());
    let step = |a: &mut StudioApp| {
        a.player.as_ref().unwrap().step_device_free();
        a.apply_editor_edits();
        a.poll_player();
        a.handle_transport(0.0);
    };
    for _ in 0..3 {
        step(&mut a);
    }
    a.set_cursor(1000, None);
    a.dispatch_midi_buttons(vec![event(&[0xfa], 0)]);
    for _ in 0..12 {
        step(&mut a);
    }
    assert!(a.is_playing());
    assert!(a.player.as_ref().unwrap().position() > 1000);
    a.dispatch_midi_buttons(vec![event(&[0xfc], 1), event(&[0xfb], 2)]);
    assert!(!a.play_pending(), "Continue must not submit before Stop acknowledges");
    assert_eq!(a.midi_transport.waiting.len(), 1);
    for _ in 0..8 {
        step(&mut a);
    }
    let stopped = a.player.as_ref().unwrap().position();
    assert!(stopped > 1000);
    assert_eq!(a.midi_transport.retained, stopped);
    a.set_cursor(10, None);
    a.dispatch_midi_buttons(Vec::new());
    assert!(a.play_pending());
    assert_eq!(a.midi_transport.retained, stopped, "the admitted start is the applied Stop position");
}

#[test]
fn record_preparation_failure_is_not_overwritten_by_success_text() {
    let mut a = app();
    let track = a.run("track.new", json!({"kind":"instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
    a.run("mix.record_arm", json!({"tracks":[track], "value":true})).unwrap();
    a.dispatch_midi_buttons(vec![event(&mmc(6), 0)]);
    assert_eq!(a.status, "Cannot record: no audio device is open to record MIDI");
    assert!(!a.recording_active());
}

#[test]
fn loss_discards_starts_through_stop_acknowledgment() {
    let engine = Engine::default();
    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
    let mut a = StudioApp::new(engine, Some(player), Services::default());
    a.dispatch_midi_buttons(vec![event(&[0xfa], 0)]);
    a.midi_transport_loss();
    a.dispatch_midi_buttons(vec![event(&[0xfa], 1)]);
    assert!(!a.play_pending());
    assert!(a.midi_transport.waiting.is_empty());
    for _ in 0..8 {
        a.player.as_ref().unwrap().step_device_free();
        a.apply_editor_edits();
        a.poll_player();
        a.handle_transport(0.0);
    }
    assert!(!a.midi_stop_pending());
    a.dispatch_midi_buttons(vec![event(&[0xfa], 2)]);
    assert!(!a.play_pending(), "offers from before acknowledgment are discarded");
    a.dispatch_midi_buttons(vec![event(&[0xfa], 3)]);
    assert!(a.play_pending(), "a fresh post-recovery Start is admitted");
}

#[test]
fn fast_forward_rewind_pause_and_learned_return_use_the_normal_commands() {
    let mut a = app();
    a.run("transport.locate", json!({"at": 96_000})).unwrap();
    a.handle_transport(0.0);
    a.dispatch_midi_buttons(vec![event(&mmc(4), 0)]);
    assert_eq!(a.cursor(), 144_000);
    a.dispatch_midi_buttons(vec![event(&mmc(5), 1)]);
    assert_eq!(a.cursor(), 96_000);
    a.dispatch_midi_buttons(vec![event(&[0xfa], 2), event(&mmc(9), 3)]);
    assert!(!a.is_playing());
    assert_eq!(a.midi_transport.retained, 96_000);
    a.ui.midi_transport
        .entry("Controller".into())
        .or_default()
        .set(Mapping::new(
            MidiSource { port: "Controller".into(), channel: None, kind: SourceKind::Cc, number: 20 },
            TransportControl::ReturnToZero.target(),
        ))
        .unwrap();
    a.dispatch_midi_buttons(vec![event(&[0xb0, 20, 127], 4)]);
    assert_eq!(a.cursor(), 0);
    assert_eq!(a.midi_transport.retained, 0);
}

#[test]
fn record_strobe_uses_the_same_armed_count_in_and_pre_roll_plan_as_ui_record() {
    for cycle in [false, true] {
        for count_in in [false, true] {
            for pre_roll in [false, true] {
                let make = || {
                    let engine = Engine::default();
                    let player = transit_playback::Player::prepare_device_free(engine.session_arc()).unwrap();
                    let mut a = StudioApp::new(engine, Some(player), Services::default());
                    let track = a.run("track.new", json!({"kind":"midi"})).unwrap()["tracks"][0].as_u64().unwrap();
                    a.run("mix.record_arm", json!({"tracks":[track],"value":true})).unwrap();
                    a.set_cursor(96_000, None);
                    let edit = &mut a.engine.session_mut().edit;
                    edit.loop_playback = cycle;
                    edit.loop_range = Range::new(48_000, 192_000);
                    edit.pre_post_roll = pre_roll;
                    edit.pre_roll = 4000;
                    a.ui.count_in = count_in;
                    a
                };
                let mut button = make();
                let mut ui = make();
                ui.run("transport.record", json!({})).unwrap();
                ui.handle_transport(0.0);
                button.dispatch_midi_buttons(vec![event(&mmc(6), 0)]);
                assert!(button.recording_active(), "{}", button.status);
                assert_eq!(button.record_start, ui.record_start);
                assert_eq!(button.count_in_plan.map(|p| p.frames), ui.count_in_plan.map(|p| p.frames));
                assert_eq!(button.take_plan.as_ref().map(|p| (p.start, p.pass)), ui.take_plan.as_ref().map(|p| (p.start, p.pass)));
                let pending = button.player_requests.recording;
                button.dispatch_midi_buttons(vec![event(&mmc(6), 1)]);
                assert_eq!(button.player_requests.recording, pending, "Strobe cannot cancel a pending Record");
                button.dispatch_midi_buttons(vec![event(&mmc(7), 2)]);
                assert!(button.midi_stop_pending());
                assert!(!button.play_pending());
                ui.stop_play();
            }
        }
    }
}
