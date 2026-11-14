//! #683: on JACK a loopback calibration is keyed by the output and the input port group that
//! opened (one client records from any group, #682). A calibration measured through one group does
//! not apply after a switch to another; a record saved before the key named the group (no input)
//! applies to the output's own group only. Other systems keep their key: the pairing.

use super::*;
use crate::backend::native::NativeKind;

/// A JACK-like duplex client: the output group "Headset" with two inputs of its own, and the
/// groups "Mic" (mono) and "Line" (stereo) that only record.
fn client(system: &str) -> ab::fake::FakeBackend {
    fake_factory(false).named(system, "Headset").with_capture(2, true).with_capture_device("Mic", 1).with_capture_device("Line", 2)
}

/// Open the output on a fresh client of `system` with the input `input` and the saved
/// `calibrations`; the Player and the calibration each stream was asked to open with.
fn open(system: &str, input: Option<&str>, calibrations: &[crate::LoopbackCalibration]) -> (Player, Vec<Option<ab::Calibration>>) {
    let native = client(system);
    let cfg = AudioConfig { input: input.map(str::to_owned), calibrations: calibrations.to_vec(), ..Default::default() };
    let player =
        Player::open_native_kind(Arc::new(Session::default()), &cfg, &native, &fake_factory(false), NativeKind::Jack).expect("the output opens");
    let asked = native.opens().into_iter().map(|(_, request)| request.calibration).collect();
    (player, asked)
}

/// A loop measured under `key`: 480 frames (10 ms at 48 kHz) with a 25 frame bound.
fn measured(key: &crate::devices::CalibrationKey) -> crate::LoopbackCalibration {
    key.record(480, 25, "2026-11-14")
}

#[test]
fn a_calibration_measured_through_one_input_group_does_not_apply_after_a_switch_to_another() {
    let (player, _) = open("JACK", Some("Mic"), &[]);
    let key = player.calibration_key.clone().expect("a duplex JACK open has a calibration key");
    assert_eq!((key.device.as_str(), key.input.as_deref()), ("Headset", Some("Mic")), "the key names the output and the group that opened");
    let record = measured(&key);
    let saved = [record.clone()];
    // The same group again: the calibration reaches both streams and the Player.
    let (same, asked) = open("JACK", Some("Mic"), &saved);
    assert_eq!(same.calibration, record.to_backend());
    assert_eq!(asked, [record.to_backend(), record.to_backend()], "the input and the output open calibrated");
    // Another group: nothing applies, and the record that did not is what the app reports.
    let (other, asked) = open("JACK", Some("Line"), &saved);
    let other_key = other.calibration_key.clone().expect("a key");
    assert_eq!(other_key.input.as_deref(), Some("Line"));
    assert_eq!(other.calibration, None, "a calibration through Mic is not Line's");
    assert_eq!(asked, [None, None], "neither stream opens with it");
    let cfg = AudioConfig { calibrations: saved.to_vec(), ..Default::default() };
    assert_eq!(cfg.jack_calibration_elsewhere(&other_key), Some(&record), "it is reported as needing a new measurement");
    assert_eq!(cfg.jack_calibration_elsewhere(&key), None, "nothing to report where it applies");
}

#[test]
fn a_record_without_an_input_applies_to_the_outputs_own_group_only() {
    // Saved before the key named the input group: the input was then always the output's own.
    let (player, _) = open("JACK", None, &[]);
    let key = player.calibration_key.clone().expect("a key");
    assert_eq!(key.input.as_deref(), Some("Headset"), "the output's own group is named too");
    let old = crate::LoopbackCalibration { input: None, ..measured(&key) };
    let (own, asked) = open("JACK", None, std::slice::from_ref(&old));
    assert_eq!((own.calibration, asked), (old.to_backend(), vec![old.to_backend(), old.to_backend()]), "it measured this group");
    let (mic, asked) = open("JACK", Some("Mic"), std::slice::from_ref(&old));
    assert_eq!((mic.calibration, asked), (None, vec![None, None]), "it did not measure Mic");
}

#[test]
fn other_systems_keep_the_pairing_key_whatever_input_opened() {
    // The same client under another system name: the key is the pairing (none here), as before
    // #683, so an unnamed record applies even with another group open and the key names no input.
    let (player, _) = open("Fake", Some("Mic"), &[]);
    let key = player.calibration_key.clone().expect("a key");
    assert_eq!(key.input, None, "no pairing, no input in the key");
    let record = measured(&key);
    let (mic, asked) = open("Fake", Some("Mic"), std::slice::from_ref(&record));
    assert_eq!((mic.calibration, asked), (record.to_backend(), vec![record.to_backend(), record.to_backend()]));
}
