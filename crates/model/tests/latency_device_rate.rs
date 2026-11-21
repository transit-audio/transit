//! #709 (ADR 0015, Amendment 2026-11-16 on the #709 converter term, rules 5, 6 and 8; ADR 0020's
//! 2026-11-16 amendment): a measured hardware insert round trip or External Instrument latency
//! records the rate the audio device ran at. The field is additive: a record stored before it loads
//! with 0 ("not recorded"), shows "measure again at this rate" at every running rate, equal rates
//! included, and is used as stored; a recorded figure shows the hint only while the device runs at
//! another rate. Manual values never carry it (ADR 0020 `manual_ms` is not a record).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use transit_model::{ExternalLatency, HardwareLatency, IoMap, MidiOutPort, rate_hint};

/// A hardware insert record as a build before this change wrote it (`ui.json`).
const OLD_HARDWARE: &str =
    r#"{"send": "Out 3-4", "ret": "In 3-4", "round_trip_frames": 612, "rate": 48000, "measured_at": 7, "input_device": "Scarlett"}"#;
/// An External Instrument record as a build before this change wrote it.
const OLD_EXTERNAL: &str =
    r#"{"port_id": "", "port_name": "MODEL D", "ret": "In 1", "rate": 48000, "buffer": 64, "frames": 955, "input_device": "Scarlett"}"#;

#[test]
fn a_figure_stored_before_the_device_rate_was_recorded_shows_the_hint_at_every_rate() {
    let hw: HardwareLatency = serde_json::from_str(OLD_HARDWARE).unwrap();
    let ext: ExternalLatency = serde_json::from_str(OLD_EXTERNAL).unwrap();
    assert_eq!((hw.device_rate, ext.device_rate), (0, 0), "absent: not recorded");
    for running in [None, Some(48_000), Some(44_100), Some(96_000)] {
        assert!(hw.rate_hint(running), "{running:?}: rule 6, equal rates included");
        assert!(ext.rate_hint(running), "{running:?}");
    }
    // Used as stored: the table's lookup gives the stored frames, nothing added or removed.
    let mut io = IoMap::for_device("ASIO", "Scarlett", 4, 4);
    io.input_device = "Scarlett".into();
    io.set_hardware_latency(hw.clone());
    assert_eq!(io.hardware_latency("Out 3-4", "In 3-4", 48_000), Some(612));
    assert_eq!(io.hardware_figure("Out 3-4", "In 3-4", 48_000), Some(&hw));
    io.set_external_latency(ext);
    let port = MidiOutPort { name: "MODEL D".into(), ..MidiOutPort::default() };
    assert_eq!(io.external_measurement(&port, "In 1", 48_000, Some(64)).unwrap().frames, 955);
}

#[test]
fn a_recorded_figure_shows_the_hint_only_at_another_device_rate() {
    let hw = HardwareLatency { device_rate: 44_100, ..serde_json::from_str(OLD_HARDWARE).unwrap() };
    assert!(!hw.rate_hint(Some(44_100)), "measured at the running rate");
    assert!(hw.rate_hint(Some(48_000)), "rule 8: the hardware's delay can change with the rate");
    assert!(!hw.rate_hint(None), "no device open: nothing to compare against");
    let ext = ExternalLatency { device_rate: 48_000, ..serde_json::from_str(OLD_EXTERNAL).unwrap() };
    assert!(!ext.rate_hint(Some(48_000)) && ext.rate_hint(Some(44_100)));
    assert!(rate_hint(0, None) && !rate_hint(96_000, Some(96_000)));
}

#[test]
fn the_device_rate_round_trips_and_an_unrecorded_one_is_not_written() {
    let hw = HardwareLatency { device_rate: 44_100, ..serde_json::from_str(OLD_HARDWARE).unwrap() };
    let text = serde_json::to_string(&hw).unwrap();
    assert!(text.contains(r#""device_rate":44100"#), "{text}");
    assert_eq!(serde_json::from_str::<HardwareLatency>(&text).unwrap(), hw);
    let ext = ExternalLatency { device_rate: 88_200, ..serde_json::from_str(OLD_EXTERNAL).unwrap() };
    assert_eq!(serde_json::from_str::<ExternalLatency>(&serde_json::to_string(&ext).unwrap()).unwrap(), ext);
    // A record without one is written as before, so the file reads the same in an older build.
    let old: HardwareLatency = serde_json::from_str(OLD_HARDWARE).unwrap();
    assert!(!serde_json::to_string(&old).unwrap().contains("device_rate"));
}
