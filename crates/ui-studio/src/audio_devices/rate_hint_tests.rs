//! #709 (ADR 0015, Amendment 2026-11-16 on the #709 converter term, rules 5 to 8 and 10): a
//! hardware insert Measure records the rate the device ran at and whether the term was in force
//! (with the normalized figure); a figure measured with the device at another rate, stored before
//! the rate was recorded, or with another input timing, says "measure again at this rate" in the
//! Audio Devices table and on the insert's card, and every measured figure says "timing changed"
//! while the input's alignment is not valid. A figure with term provenance is shown and used with
//! the running term while the term is in force; otherwise the measured total.

use super::io_paths::{item, with_window};
use super::*;
use crate::hardware_measure::{FakeProbe, MEASURE_DEVICE_CHANGED};
use crate::hardware_ui::{MEASURE_AGAIN_RATE, TIMING_CHANGED, card_text};
use transit_mix::HardwareState;
use transit_model::{Alignment, LatencyHint};
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

fn finish_with(t: &mut T, frames: u32, term: bool) {
    t.app.audio_devices.fake_probe.as_mut().unwrap().result =
        Some(Ok(Measured { round_trip_frames: frames, spread_frames: 1, found: 8, sent: 8, term }));
    t.frames(3);
}

fn finish(t: &mut T, frames: u32) {
    finish_with(t, frames, false);
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
    assert_eq!(stored[0].normalized_frames, None, "measured without the term: no term provenance");
    assert_eq!(row(&t), json!(["Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms)"]), "at the rate it was measured at: no hint");
    // The device now runs at 48 kHz: the figure is used as measured, with the hint.
    t.app.audio_devices.fake_device_rate = Some(48_000);
    t.frames(2);
    assert_eq!(
        row(&t),
        json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), measured with the device at 44.1 kHz: {MEASURE_AGAIN_RATE}")])
    );
    assert_eq!(t.app.engine.io_map().hardware_latency("Out 3-4", "In 3-4", 48_000), Some(612), "not adjusted");
    assert_eq!(t.app.hardware_hint("Out 3-4", "In 3-4"), Some(LatencyHint::MeasureAgain));
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
        normalized_frames: None,
    });
    t.app.engine.set_io_map(io);
    t.frames(2);
    assert_eq!(row(&t), json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), device rate not recorded: {MEASURE_AGAIN_RATE}")]));
    assert!(t.app.hardware_hint("Out 3-4", "In 3-4").is_some(), "the device runs at the session's rate and still: rule 6");
    // On a paired device with the term in force too: no term is added to it (rule 6).
    t.app.audio_devices.fake_alignment = Some(Alignment::Valid);
    t.frames(2);
    assert_eq!(t.app.engine.io_map().hardware_latency("Out 3-4", "In 3-4", 48_000), Some(612), "used as stored");
    let mut io = t.app.engine.io_map().clone();
    io.device_rate = Some(44_100);
    assert_eq!(io.hardware_latencies("Out 3-4", "In 3-4", 48_000), Some((612, 612)), "a figure without provenance never gains the term");
    t.app.audio_devices.fake_alignment = None;
    // Measuring again replaces it with a figure that records the rate, and the hint goes.
    measure(&mut t);
    finish(&mut t, 610);
    assert_eq!(row(&t), json!(["Out 3-4 \u{2192} In 3-4: 610 frames (12.7 ms)"]));
    assert_eq!(t.app.hardware_hint("Out 3-4", "In 3-4"), None);
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
    let again = Some(LatencyHint::MeasureAgain);
    assert_eq!(card_text(&HardwareState::Ready, Some(612), 48_000, again), format!("12.8 ms: {MEASURE_AGAIN_RATE}"));
    assert_eq!(card_text(&HardwareState::Ready, Some(612), 48_000, None), "12.8 ms");
    assert_eq!(card_text(&HardwareState::Unmeasured, None, 48_000, again), "Unmeasured", "no figure: nothing to measure again at");
    assert_eq!(card_text(&HardwareState::Ready, Some(612), 48_000, Some(LatencyHint::TimingChanged)), format!("12.8 ms: {TIMING_CHANGED}"));
}

/// Rule 5 through Measure: a run with the term in force stores the normalized figure beside the
/// measured total. Used at another device rate with the term in force, the plan's figure is the
/// normalized one plus the running term (the table lists the measured total and says "measure
/// again at this rate", rule 8); with the alignment invalid (rule 10) the measured total is used
/// and the table says "timing changed"; on separate streams (no alignment) the provenance differs
/// and the table says so.
#[test]
fn a_term_measurement_stores_both_figures_and_the_hints_follow_the_alignment() {
    let mut t = with_fake_probe(44_100);
    t.app.audio_devices.fake_alignment = Some(Alignment::Valid);
    measure(&mut t);
    finish_with(&mut t, 612, true);
    let stored = t.app.engine.io_map().hardware_latency[0].clone();
    // T = 34 at 48 kHz on 44.1 kHz.
    assert_eq!((stored.round_trip_frames, stored.normalized_frames, stored.device_rate), (612, Some(578), 44_100));
    assert_eq!(row(&t), json!(["Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms)"]), "the pair it ran at, the term in force: no hint");
    let mut io = t.app.engine.io_map().clone();
    io.device_rate = Some(48_000);
    assert_eq!(io.hardware_latencies("Out 3-4", "In 3-4", 48_000), Some((612, 578)), "at equal rates the term is 0");
    io.device_rate = Some(96_000);
    assert_eq!(io.hardware_latencies("Out 3-4", "In 3-4", 48_000), Some((612, 610)), "48 on 96: T = 32");
    // Another device rate: rule 8's hint, term provenance included.
    t.app.audio_devices.fake_device_rate = Some(48_000);
    t.frames(2);
    assert_eq!(
        row(&t),
        json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), measured with the device at 44.1 kHz: {MEASURE_AGAIN_RATE}")])
    );
    // The alignment is invalid (another speed since the last stop at 1x): "timing changed".
    t.app.audio_devices.fake_device_rate = Some(44_100);
    t.app.audio_devices.fake_alignment = Some(Alignment::Invalid);
    t.frames(2);
    assert_eq!(row(&t), json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), {TIMING_CHANGED}")]));
    assert_eq!(t.app.hardware_hint("Out 3-4", "In 3-4"), Some(LatencyHint::TimingChanged));
    // The card shows the figure in use: the measured total while the alignment is invalid, the
    // term figure while it is valid.
    assert_eq!(t.app.shown_latency(Some(612), Some(578)), Some(612));
    t.app.audio_devices.fake_alignment = Some(Alignment::Valid);
    assert_eq!(t.app.shown_latency(Some(612), Some(578)), Some(578));
    // Separate streams: the term cannot apply, so the provenance differs.
    t.app.audio_devices.fake_alignment = Some(Alignment::None);
    t.frames(2);
    assert_eq!(row(&t), json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), measured with another input timing: {MEASURE_AGAIN_RATE}")]));
}

/// A figure measured without the term where it is now in force says so (rule 5: the provenance
/// differs), and gains no term.
#[test]
fn a_figure_without_term_provenance_says_measure_again_where_the_term_is_in_force() {
    let mut t = with_fake_probe(48_000);
    measure(&mut t);
    finish_with(&mut t, 612, false);
    assert_eq!(row(&t), json!(["Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms)"]));
    t.app.audio_devices.fake_alignment = Some(Alignment::Valid);
    t.frames(2);
    assert_eq!(row(&t), json!([format!("Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms), measured with another input timing: {MEASURE_AGAIN_RATE}")]));
}

/// #709 round 3: at equal rates (the device at the session's rate) "timing changed" never shows,
/// with the alignment invalid: a figure measured at another device rate says "measure again at this
/// rate" (rule 8), one measured at equal rates says nothing, one without a device rate keeps rule 6's
/// hint: each exactly what it says with a valid alignment. The figure in use is unchanged (the
/// measured total while the alignment is not claimed). At a converted rate "timing changed" stays.
#[test]
fn at_equal_rates_an_invalid_alignment_shows_no_timing_hint() {
    let mut t = with_fake_probe(48_000);
    t.app.audio_devices.fake_alignment = Some(Alignment::Invalid);
    let set = |t: &mut T, device_rate: u32, normalized: Option<u32>| {
        let mut io = t.app.engine.io_map().clone();
        io.set_hardware_latency(transit_model::HardwareLatency {
            send: "Out 3-4".into(),
            ret: "In 3-4".into(),
            round_trip_frames: 612,
            rate: 48_000,
            measured_at: 1,
            input_device: io.input_device.clone(),
            input_ports: Vec::new(),
            device_rate,
            normalized_frames: normalized,
        });
        t.app.engine.set_io_map(io);
        t.frames(2);
    };
    let plain = "Out 3-4 \u{2192} In 3-4: 612 frames (12.8 ms)";
    // Measured at 44.1 kHz with the term: rule 8, and the measured total in use.
    set(&mut t, 44_100, Some(578));
    assert_eq!(row(&t), json!([format!("{plain}, measured with the device at 44.1 kHz: {MEASURE_AGAIN_RATE}")]));
    assert_eq!(t.app.hardware_hint("Out 3-4", "In 3-4"), Some(LatencyHint::MeasureAgain));
    assert_eq!(t.app.shown_latency(Some(612), Some(578)), Some(612), "the plan's figure is not changed");
    // Measured at equal rates with the term: nothing to say.
    set(&mut t, 48_000, Some(612));
    assert_eq!(row(&t), json!([plain]));
    assert_eq!(t.app.hardware_hint("Out 3-4", "In 3-4"), None);
    // No device rate recorded: rule 6's own hint.
    set(&mut t, 0, None);
    assert_eq!(row(&t), json!([format!("{plain}, device rate not recorded: {MEASURE_AGAIN_RATE}")]));
    // The same figures with the device at 44.1 kHz (T = 34 in force when valid): "timing changed".
    set(&mut t, 48_000, Some(612));
    t.app.audio_devices.fake_device_rate = Some(44_100);
    t.frames(2);
    assert_eq!(row(&t), json!([format!("{plain}, {TIMING_CHANGED}")]));
    assert_eq!(t.app.hardware_hint("Out 3-4", "In 3-4"), Some(LatencyHint::TimingChanged));
}
