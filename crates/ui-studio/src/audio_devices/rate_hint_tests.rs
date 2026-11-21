//! #709 (ADR 0015, Amendment 2026-11-16 on the #709 converter term, rules 5, 6 and 8): a hardware
//! insert Measure records the rate the device ran at; a figure measured with the device at another
//! rate, or stored before the rate was recorded, is used as measured and says "measure again at
//! this rate" in the Audio Devices table and on the insert's card.

use super::io_paths::{item, with_window};
use super::*;
use crate::hardware_measure::{FakeProbe, MEASURE_DEVICE_CHANGED};
use crate::hardware_ui::{MEASURE_AGAIN_RATE, card_text};
use transit_mix::HardwareState;
use transit_playback::hardware_probe::Measured;

fn with_fake_probe(device_rate: u32) -> T {
    let mut t = with_window();
    t.section("section_hardware");
    t.app.audio_devices.fake_probe = Some(FakeProbe { progress: (0, 8), ..Default::default() });
    t.app.audio_devices.fake_device_rate = Some(device_rate);
    t.frames(3);
    t
}

fn measure(t: &mut T) {
    t.click(mid(item(t, "hw_send", "")));
    t.click(mid(item(t, "hw_send_entry", "Out 3-4")));
    t.click(mid(item(t, "hw_ret", "")));
    t.click(mid(item(t, "hw_ret_entry", "In 3-4")));
    t.click(mid(item(t, "hw_measure", "Measure")));
}

fn finish(t: &mut T, frames: u32) {
    t.app.audio_devices.fake_probe.as_mut().unwrap().result = Some(Ok(Measured { round_trip_frames: frames, spread_frames: 1, found: 8, sent: 8 }));
    t.frames(3);
}

/// The one listed row of the Audio Devices table.
fn row(t: &T) -> Value {
    t.layout()["hardware"]["rows"].clone()
}

#[test]
fn measure_records_the_device_rate_and_the_table_says_measure_again_at_another_rate() {
    let mut t = with_fake_probe(44_100);
    measure(&mut t);
    finish(&mut t, 612);
    let stored = t.app.engine.io_map().hardware_latency.clone();
    assert_eq!((stored.len(), stored[0].device_rate, stored[0].round_trip_frames), (1, 44_100, 612), "the running device rate is kept");
    assert_eq!(row(&t), json!(["Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms)"]), "at the rate it was measured at: no hint");
    // The device now runs at 48 kHz: the figure is used as measured, with the hint.
    t.app.audio_devices.fake_device_rate = Some(48_000);
    t.frames(2);
    assert_eq!(
        row(&t),
        json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), measured with the device at 44.1 kHz: {MEASURE_AGAIN_RATE}")])
    );
    assert_eq!(t.app.engine.io_map().hardware_latency("Out 3-4", "In 3-4", 48_000), Some(612), "not adjusted");
    assert!(t.app.hardware_rate_hint("Out 3-4", "In 3-4"));
}

#[test]
fn a_figure_stored_without_the_device_rate_says_measure_again_at_this_rate_at_equal_rates_too() {
    let mut t = with_fake_probe(48_000);
    let mut io = t.app.engine.io_map().clone();
    io.set_hardware_latency(transit_model::HardwareLatency {
        send: "Out 3-4".into(),
        ret: "In 3-4".into(),
        round_trip_frames: 612,
        rate: 48_000,
        measured_at: 1,
        input_device: io.input_device.clone(),
        input_ports: Vec::new(),
        device_rate: 0,
    });
    t.app.engine.set_io_map(io);
    t.frames(2);
    assert_eq!(row(&t), json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), device rate not recorded: {MEASURE_AGAIN_RATE}")]));
    assert!(t.app.hardware_rate_hint("Out 3-4", "In 3-4"), "the device runs at the session's rate and still: rule 6");
    assert_eq!(t.app.engine.io_map().hardware_latency("Out 3-4", "In 3-4", 48_000), Some(612), "used as stored");
    // Measuring again replaces it with a figure that records the rate, and the hint goes.
    measure(&mut t);
    finish(&mut t, 610);
    assert_eq!(row(&t), json!(["Out 3-4 \u{2192} In 3-4: 610 frames (12.7 ms)"]));
    assert!(!t.app.hardware_rate_hint("Out 3-4", "In 3-4"));
}

/// As a change of input or output does: a result that arrives after the device's rate changed
/// is not stored under the new rate.
#[test]
fn a_measurement_that_finishes_at_another_device_rate_stores_nothing() {
    let mut t = with_fake_probe(44_100);
    measure(&mut t);
    t.app.audio_devices.fake_device_rate = Some(48_000);
    finish(&mut t, 612);
    assert!(t.app.engine.io_map().hardware_latency.is_empty());
    assert_eq!(t.layout()["hardware"]["result"], MEASURE_DEVICE_CHANGED);
}

#[test]
fn the_card_adds_the_hint_to_a_ready_inserts_latency_only() {
    assert_eq!(card_text(&HardwareState::Ready, Some(612), 48_000, true), format!("12.8 ms: {MEASURE_AGAIN_RATE}"));
    assert_eq!(card_text(&HardwareState::Ready, Some(612), 48_000, false), "12.8 ms");
    assert_eq!(card_text(&HardwareState::Unmeasured, None, 48_000, true), "Unmeasured", "no figure: nothing to measure again at");
}
