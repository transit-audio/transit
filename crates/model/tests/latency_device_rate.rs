//! #709 (ADR 0015, Amendment 2026-11-16 on the #709 converter term, rules 5, 6 and 8; ADR 0020's
//! 2026-11-16 amendment): a measured hardware insert round trip or External Instrument latency
//! records the rate the audio device ran at. The field is additive: a record stored before it loads
//! with 0 ("not recorded"), shows "measure again at this rate" at every running rate, equal rates
//! included, and is used as stored; a recorded figure shows the hint only while the device runs at
//! another rate. Manual values never carry it (ADR 0020 `manual_ms` is not a record).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use transit_model::{
    Alignment, ExternalLatency, HardwareLatency, IoMap, LatencyHint, MidiOutPort, converter_term, manual_hint, measured_hint, rate_hint,
};

/// Rule 2: the term of every pair the ADR lists, 0 at equal rates and when a rate is unknown.
#[test]
fn the_converter_term_is_the_adrs_figure_for_every_listed_pair() {
    for (session, device, term) in [
        (48_000, 44_100, 34),
        (44_100, 48_000, 32),
        (48_000, 96_000, 32),
        (96_000, 48_000, 62),
        (48_000, 88_200, 32),
        (44_100, 88_200, 32),
        (48_000, 176_400, 32),
        (48_000, 192_000, 32),
    ] {
        assert_eq!(converter_term(session, device), term, "{session} on {device}");
        // ceil(32·max(1, q) − q), q = S/D, in floating point as a cross-check.
        let q = f64::from(session) / f64::from(device);
        assert_eq!(f64::from(term), (32.0 * q.max(1.0) - q).ceil(), "{session} on {device}");
    }
    assert_eq!((converter_term(48_000, 48_000), converter_term(0, 48_000), converter_term(48_000, 0)), (0, 0, 0));
}

/// Rules 5 to 8 and 10 as one table: what a measured figure and a manual value say.
#[test]
fn the_hints_follow_the_provenance_the_rate_and_the_alignment() {
    use Alignment::{Invalid, None as Off, Valid};
    use LatencyHint::{MeasureAgain, TimingChanged};
    let at48 = Some(48_000);
    // (measured device rate, term provenance, running rate, alignment) -> hint
    for (measured, term, running, alignment, want) in [
        (48_000, true, at48, Valid, None),                  // the pair it ran at, the term in force
        (44_100, true, at48, Valid, Some(MeasureAgain)),    // rule 8: another device rate, provenance or not
        (48_000, true, at48, Off, Some(MeasureAgain)),      // separate streams: the term cannot apply
        (48_000, false, at48, Valid, Some(MeasureAgain)),   // rule 5: no provenance where the term applies
        (48_000, false, at48, Off, None),                   // measured and used on separate streams
        (48_000, true, at48, Invalid, Some(TimingChanged)), // rule 10
        (48_000, false, at48, Invalid, Some(TimingChanged)),
        (0, false, at48, Off, Some(MeasureAgain)), // rule 6: no device rate, equal rates included
        (0, false, at48, Valid, Some(MeasureAgain)),
    ] {
        assert_eq!(measured_hint(measured, term, running, alignment), want, "{measured} {term} {running:?} {alignment:?}");
    }
    assert_eq!((manual_hint(Valid), manual_hint(Off), manual_hint(Invalid)), (None, None, Some(TimingChanged)), "rule 7");
}

/// Rule 5: both values are stored; the term is applied only with provenance.
#[test]
fn a_figure_with_term_provenance_keeps_both_values_and_only_it_gains_the_running_term() {
    let hw = HardwareLatency { device_rate: 44_100, normalized_frames: Some(578), ..serde_json::from_str(OLD_HARDWARE).unwrap() };
    assert_eq!((hw.round_trip_frames, hw.aligned_frames(Some(44_100)), hw.aligned_frames(Some(48_000))), (612, 612, 578));
    assert_eq!(hw.aligned_frames(None), 612, "no running rate known: as measured");
    let text = serde_json::to_string(&hw).unwrap();
    assert!(text.contains(r#""normalized_frames":578"#), "{text}");
    assert_eq!(serde_json::from_str::<HardwareLatency>(&text).unwrap(), hw);
    let old: HardwareLatency = serde_json::from_str(OLD_HARDWARE).unwrap();
    assert_eq!(old.normalized_frames, None, "rule 6's default");
    assert_eq!(old.aligned_frames(Some(44_100)), 612, "never adjusted");
    assert!(!serde_json::to_string(&old).unwrap().contains("normalized"), "written as before");
    let ext = ExternalLatency { device_rate: 44_100, normalized_frames: Some(921), frames: 955, ..serde_json::from_str(OLD_EXTERNAL).unwrap() };
    assert_eq!((ext.aligned_frames_at(48_000, Some(48_000)), ext.aligned_frames_at(48_000, Some(96_000))), (921, 953));
    let old: ExternalLatency = serde_json::from_str(OLD_EXTERNAL).unwrap();
    assert_eq!((old.normalized_frames, old.aligned_frames_at(48_000, Some(44_100))), (None, 955));
    // Through the table: the plan's pair of figures with the running device rate.
    let mut io = IoMap::for_device("ASIO", "Scarlett", 4, 4);
    io.input_device = "Scarlett".into();
    io.set_hardware_latency(hw);
    io.device_rate = Some(48_000);
    assert_eq!(io.hardware_latencies("Out 3-4", "In 3-4", 48_000), Some((612, 578)));
}

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
