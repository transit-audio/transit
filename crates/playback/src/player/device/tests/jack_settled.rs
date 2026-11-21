//! ADR 0021, amendment for #760, slice 1, from the Player's side: on JACK the buffer the started
//! client settled on is the device's and keys the loopback calibration, resolved after the starts
//! and handed to both directions; a record an earlier build stored is kept and not applied; a
//! latched rebuild leaves the Player as a driver's reset request does, until the reopen builds a
//! new one.

use super::*;
use crate::backend::native::NativeKind;

/// A JACK-like duplex client (the output group "Headset" with two inputs of its own) whose started
/// streams report the settled buffer `settled`.
fn client(settled: u32) -> ab::fake::FakeBackend {
    let fake = fake_factory(false).named("JACK", "Headset").with_capture(2, true).with_capture_device("Mic", 1);
    fake.set_started_period(Some(settled));
    fake
}

fn open(client: &ab::fake::FakeBackend, buffer: Option<u32>, calibrations: &[crate::LoopbackCalibration]) -> Player {
    let cfg = AudioConfig { buffer_size: buffer, calibrations: calibrations.to_vec(), ..Default::default() };
    Player::open_native_kind(Arc::new(Session::default()), &cfg, client, &fake_factory(false), NativeKind::Jack).expect("the output opens")
}

fn handed(client: &ab::fake::FakeBackend) -> Vec<(ab::Direction, Option<ab::Calibration>)> {
    client.live_calibrations()
}

#[test]
fn the_settled_buffer_is_the_devices_and_keys_the_calibration() {
    // 256 was asked for; the started client settled on 1024.
    let jack = client(1024);
    let player = open(&jack, Some(256), &[]);
    assert_eq!(player.device_buffer, Some(1024), "the settled buffer, not the request or the provisional figure");
    assert_eq!(player.with_held_input(|recorder| recorder.buffer_frames), Some(Some(1024)), "the held input shows the settled buffer");
    let key = player.calibration_key.clone().expect("a duplex JACK open has a key");
    assert_eq!((key.period, key.running_period), (1024, true), "keyed by the running buffer, and says so");
    drop(player);
    // A loop measured under that key applies after the start, to both directions.
    let record = key.record(480, 25, "2026-11-20");
    assert!(record.running_period, "a record stored under the key carries the marker");
    let jack = client(1024);
    let player = open(&jack, Some(256), std::slice::from_ref(&record));
    assert_eq!(player.calibration, record.to_backend());
    assert_eq!(handed(&jack), [(ab::Direction::Output, record.to_backend()), (ab::Direction::Input, record.to_backend())]);
    assert!(jack.opens().iter().all(|(_, request)| request.calibration.is_none()), "no open request carries it");
    drop(player);
    // Control: the same record does not apply where the client settled on another buffer.
    let other = client(512);
    let player = open(&other, Some(256), std::slice::from_ref(&record));
    assert_eq!((player.calibration, handed(&other)), (None, vec![]));
}

#[test]
fn a_record_an_earlier_build_stored_is_kept_not_applied_and_asks_to_be_measured_again() {
    let jack = client(1024);
    let key = open(&jack, None, &[]).calibration_key.clone().expect("a key");
    // Earlier builds keyed JACK records by the requested buffer (0 for none) without the marker,
    // whatever the server ran: neither one at the running period nor one at 0 applies.
    for period in [1024, 0] {
        let old = crate::LoopbackCalibration { period, running_period: false, ..key.record(480, 25, "2026-11-01") };
        let jack = client(1024);
        let cfg_calibrations = [old.clone()];
        let player = open(&jack, None, &cfg_calibrations);
        assert_eq!((player.calibration, handed(&jack)), (None, vec![]), "not applied (period {period})");
        let mut cfg = AudioConfig { calibrations: cfg_calibrations.to_vec(), ..Default::default() };
        assert_eq!(cfg.jack_calibration_unproven(&key), Some(&old), "kept, and it asks to be measured again (period {period})");
        assert_eq!(cfg.calibrations, std::slice::from_ref(&old), "it stays in the settings");
        // Measuring again under the key: the new record applies and nothing asks any more.
        cfg.store_calibration(key.record(470, 20, "2026-11-20"));
        assert!(cfg.jack_calibration(&key).is_some());
        assert_eq!(cfg.jack_calibration_unproven(&key), None);
    }
    // The marker is read with a default and written only when set.
    let json = r#"{"device":"Headset","input":"Headset","mode":"shared","rate":48000,"period":0,"round_trip_frames":480,"spread_frames":25,"measured_at":"2026-11-01"}"#;
    let old: crate::LoopbackCalibration = serde_json::from_str(json).unwrap();
    assert!(!old.running_period, "a record of an earlier build reads as unproven");
    assert!(!serde_json::to_string(&old).unwrap().contains("running_period"), "other systems' records are written as before");
    assert!(serde_json::to_string(&key.record(480, 25, "2026-11-20")).unwrap().contains("\"running_period\":true"));
}

#[test]
fn a_latched_rebuild_waits_for_the_reopen_and_the_reopened_player_shares_nothing() {
    let jack = client(1024);
    jack.set_needs_rebuild(true);
    let key = open(&client(1024), None, &[]).calibration_key.clone().expect("a key");
    let record = key.record(480, 25, "2026-11-20");
    let latched = open(&jack, None, std::slice::from_ref(&record));
    assert_eq!(latched.reopen_cause(), Some(crate::reopen::ReopenCause::DriverReset), "the app reopens it, Play and Record refused meanwhile");
    assert_eq!(latched.trouble(), Some(OutputTrouble::ResetRequested));
    assert!(!latched.is_playing());
    assert_eq!((latched.calibration_key.as_ref(), latched.calibration, handed(&jack)), (None, None, vec![]), "nothing keyed or applied");
    // The reopen builds a new Player at the running rate: nothing of the provisional one is kept.
    jack.set_needs_rebuild(false);
    let rebuilt = open(&jack, None, std::slice::from_ref(&record));
    assert_eq!(rebuilt.reopen_cause(), None);
    assert_ne!(rebuilt.epoch, latched.epoch, "a new engine and clock generation");
    let (a, b) = (latched.loop_probe.as_ref().unwrap(), rebuilt.loop_probe.as_ref().unwrap());
    assert!(!Arc::ptr_eq(a, b), "a new loop probe");
    assert_eq!(rebuilt.calibration, record.to_backend());
}

#[test]
fn a_declined_buffer_request_is_kept_for_the_page() {
    let jack = client(1024);
    assert!(!open(&jack, Some(256), &[]).device_buffer_declined);
    jack.set_period_declined(true);
    assert!(open(&jack, Some(256), &[]).device_buffer_declined);
}

/// The `buffer=N` of an open line.
fn buffer_of(line: &str) -> Option<&str> {
    line.split_whitespace().find_map(|word| word.strip_prefix("buffer="))
}

#[test]
fn one_open_reports_one_buffer_size_for_output_and_input() {
    // The open's provisional figure is the request (256); the started client settled on 1024
    // (#760: the output line said 1024 and the input line 2048 in one open).
    let jack = client(1024);
    let player = open(&jack, Some(256), &[]);
    let [output, input] = player.open_lines.as_slice() else { panic!("an output and an input line: {:?}", player.open_lines) };
    assert!(output.contains("audio output opened") && input.contains("audio input opened"), "{output} / {input}");
    assert_eq!((buffer_of(output), buffer_of(input)), (Some("1024"), Some("1024")), "{output} / {input}");
    assert_eq!(player.device_buffer, Some(1024));
}
