//! The External Instrument card's Measure (ADR 0020 slice 2) with the Player's Measure faked:
//! the button, the progress and Cancel, the stored key, the lookup labels (manual wins, buffer
//! change, rate change), the refusals and Keep.

use crate::external_measure::{CANCELLED, FakeMeasure};
use crate::{Services, StudioApp};
use serde_json::{Value, json};
use soundcraft_model::{ExternalLatency, ExternalSource, TrackId};
use soundcraft_playback::external_probe::{MeasureError, Measured, Progress};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    track: TrackId,
}

impl T {
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 800.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn click(&mut self, r: [f32; 4]) {
        assert_ne!(r, [0.0; 4], "nothing to click");
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": (r[0] + r[2]) / 2.0, "y": (r[1] + r[3]) / 2.0}));
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        // The synthetic move, press and release reach egui over the next frames.
        self.frames(6);
    }

    fn card(&self) -> crate::mixer::DeviceCard {
        self.app.mixer.devices.cards[0].clone()
    }

    fn fake(&mut self) -> &mut FakeMeasure {
        self.app.external_measure.fake.as_mut().unwrap()
    }

    fn report(&self) -> soundcraft_mix::ExternalReport {
        self.app.external_reports().into_iter().find(|r| r.track == self.track).unwrap()
    }

    fn stored(&self) -> Vec<ExternalLatency> {
        self.app.engine.io_map().external_latency.clone()
    }
}

/// A Model D track: External Instrument on In 1 of a duplex Scarlett, MIDI to "MODEL D" channel 1,
/// the device chain showing its card, and a fake Measure on a device running 64 frames.
fn rig() -> T {
    let mut app = StudioApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.engine.set_io_map(soundcraft_model::IoMap::for_device("ASIO", "Focusrite USB ASIO", 2, 2));
    let r = app.run("track.new", json!({"kind": "instrument", "name": "Model D"})).unwrap();
    let track = TrackId(r["tracks"][0].as_u64().unwrap());
    app.run("instrument.external", json!({"track": track.0, "port": {"backend": "winmm", "name": "MODEL D"}, "channel": 1, "return": "In 1"}))
        .unwrap();
    app.external_measure.fake = Some(FakeMeasure { buffer: Some(64), ..FakeMeasure::default() });
    app.ui.show_devices = true;
    app.focus(track);
    let mut t = T { app, ctx: egui::Context::default(), time: 0.0, track };
    t.frames(3);
    t
}

fn measured(frames: u32, spread: f64) -> Result<Measured, MeasureError> {
    Ok(Measured { frames, spread, found: 8, sent: 8, latencies: vec![u64::from(frames); 8], backlog: None })
}

#[test]
fn measure_runs_from_the_card_stores_the_median_under_its_key_and_the_lookup_follows() {
    let mut t = rig();
    assert_eq!(t.card().external_state, "Not measured: notes sound late by the round trip");
    assert_eq!((t.card().external_measure_text.as_str(), t.card().external_measure_blocked.as_str()), ("Measure", ""));
    t.click(t.card().external_measure);
    let track = t.track;
    assert_eq!(t.fake().started, [(track, vec![0])], "the return's device channels");
    assert_eq!(t.card().external_measure_text, "Measuring\u{2026} 0/8");
    t.fake().progress = Some(Progress { note: 3, of: 8, level: 0.2 });
    t.frames(2);
    assert_eq!(t.card().external_measure_text, "Measuring\u{2026} 3/8");
    assert!(t.stored().is_empty(), "nothing is stored while it runs");
    // The transport cannot start while it runs.
    assert_eq!(t.app.run("transport.play", json!({})).unwrap_err(), crate::external_measure::REFUSED_TRANSPORT);
    // 955 frames at 48 kHz / 64: the Model D on the Scarlett (the bench's figure); spread 0.4 ms.
    t.fake().result = Some(measured(955, 19.0));
    t.frames(3);
    let stored = t.stored();
    assert_eq!(stored.len(), 1);
    let m = &stored[0];
    assert_eq!(
        (m.port_name.as_str(), m.ret.as_str(), m.rate, m.buffer, m.frames, m.spread_frames, m.notes),
        ("MODEL D", "In 1", 48_000, 64, 955, 19, 8),
        "stored under (port, return, rate, buffer)"
    );
    assert!(t.app.engine.io_map().hardware_latency.is_empty(), "not the hardware insert table");
    assert!(t.app.status.contains("19.9 ms (955 frames), spread 0.4 ms, 8 of 8 notes"), "{}", t.app.status);
    let today = soundcraft_playback::devices::utc_today();
    assert_eq!(t.card().external_state, format!("Measured 19.9 ms on {today} at 48 kHz/64"));
    assert_eq!(t.card().external_measure_text, "Measure again");
    assert_eq!((t.report().latency, matches!(t.report().source, Some(ExternalSource::Measured(_)))), (Some(955), true));
    // A manual value wins while it is set.
    t.app.run("instrument.external", json!({"track": t.track.0, "manual_ms": 20.0})).unwrap();
    t.frames(2);
    assert_eq!((t.card().external_state.as_str(), t.report().latency), ("Manual: 20.0 ms", Some(960)));
    t.app.run("instrument.external", json!({"track": t.track.0, "manual_ms": null})).unwrap();
    t.frames(2);
    assert_eq!(t.report().latency, Some(955), "cleared: the measurement again");
    // The device now runs 128 frames: the measurement is used as is, labeled amber.
    t.fake().buffer = Some(128);
    t.frames(2);
    assert_eq!(t.card().external_state, "Measured at 48 kHz / 64, now 48 kHz / 128: measure again");
    assert!(t.card().external_stale);
    assert_eq!(t.report().latency, Some(955));
    // The session at 44.1 kHz: rescaled by time, still labeled.
    t.app.engine.session_mut().sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
    t.frames(2);
    assert_eq!(t.report().latency, Some(877), "955 at 48 kHz is 877 at 44.1 kHz");
    assert_eq!(t.card().external_state, "Measured at 48 kHz / 64, now 44.1 kHz / 128: measure again");
}

#[test]
fn measure_is_refused_in_the_adr_words_and_a_failed_run_stores_nothing() {
    let mut t = rig();
    t.app.engine.session_mut().track_mut(t.track).unwrap().mixer.record_arm = true;
    t.frames(2);
    assert_eq!(t.card().external_measure_blocked, "Disarm the track to measure: live input would play the synth.");
    t.click(t.card().external_measure);
    assert!(t.fake().started.is_empty());
    assert_eq!(t.app.status, "Disarm the track to measure: live input would play the synth.");
    t.app.engine.session_mut().track_mut(t.track).unwrap().mixer.record_arm = false;
    t.fake().unsettled = true;
    t.frames(2);
    assert_eq!(t.card().external_measure_blocked, "Timing approximate: device clock not settled");
    t.fake().unsettled = false;
    t.app.run("instrument.external", json!({"track": t.track.0, "return": "In 7"})).unwrap();
    t.frames(2);
    assert_eq!(t.card().external_measure_blocked, "Return In 7 is not on this interface");
    t.app.run("instrument.external", json!({"track": t.track.0, "return": "In 1"})).unwrap();
    t.frames(2);
    // Each refusal of decision 7's table, said where the state is; nothing stored.
    let cases = [
        (
            MeasureError::NoSound { found: 2, sent: 8 },
            "No sound came back on In 1 within 1 s. Check the synth's audio goes to In 1, its MIDI channel and its volume.",
        ),
        (MeasureError::NotQuiet { peak: 0.3 }, "The return is not quiet before the note: stop the synth's sound (held note, arpeggiator, drone)."),
        (MeasureError::Clipped { peak: 1.0 }, "The return clipped: lower the synth or the input gain and measure again."),
        (MeasureError::DeviceLost, "Measurement stopped: the audio device went away. Nothing was stored."),
        (MeasureError::InputAlignmentChanged { first: 256, last: 448 }, "Input alignment changed during the measurement: measure again."),
    ];
    for (error, said) in cases {
        t.click(t.card().external_measure);
        t.fake().result = Some(Err(error));
        t.frames(3);
        assert_eq!(t.card().external_state, said);
        assert_eq!(t.app.status, said);
        assert!(t.stored().is_empty(), "{said}");
    }
}

#[test]
fn a_high_spread_is_shown_and_stored_only_on_keep() {
    let mut t = rig();
    t.click(t.card().external_measure);
    // 62 frames is 1.3 ms at 48 kHz.
    t.fake().result = Some(measured(1000, 62.0));
    t.frames(3);
    assert!(t.stored().is_empty(), "not saved without Keep");
    assert_eq!(
        t.card().external_state,
        "20.8 ms (1000 frames), spread 1.3 ms, 8 of 8 notes. Spread 1.3 ms is high (expected under 1 ms): the synth's attack or the driver is unsteady."
    );
    t.click(t.card().external_keep);
    assert_eq!(t.stored().len(), 1);
    assert_eq!(t.stored()[0].frames, 1000);
    assert_eq!(t.report().latency, Some(1000));
    assert_eq!(t.card().external_keep, [0.0; 4], "Keep is gone once kept");
}

#[test]
fn cancel_stops_the_run_and_a_late_result_is_not_collected() {
    let mut t = rig();
    t.click(t.card().external_measure);
    assert_eq!(t.card().external_measure_text, "Measuring\u{2026} 0/8");
    t.click(t.card().external_measure);
    assert_eq!(t.fake().cancelled, 1, "the Player's Measure is cancelled (it releases the note)");
    assert_eq!(t.card().external_state, CANCELLED);
    t.fake().result = Some(measured(955, 10.0));
    t.frames(3);
    assert!(t.stored().is_empty());
    assert!(t.app.external_measure.running.is_none(), "the transport is free again");
    t.click(t.card().external_measure);
    assert_eq!(t.fake().started.len(), 2, "Measure is available again");
}

#[test]
fn the_command_starts_reports_keeps_and_cancels() {
    let mut t = rig();
    let v: Value = t.app.run("instrument.measure_external", json!({"track": t.track.0})).unwrap();
    assert_eq!((v["running"].clone(), v["of"].clone()), (json!(true), json!(8)));
    assert!(t.app.run("instrument.measure_external", json!({"track": t.track.0})).unwrap_err().contains("already running"));
    t.fake().result = Some(measured(1000, 62.0));
    t.frames(3);
    let v = t.app.run("instrument.measure_external", json!({"track": t.track.0, "action": "status"})).unwrap();
    assert_eq!((v["running"].clone(), v["keep"].clone()), (json!(false), json!(true)));
    t.app.run("instrument.measure_external", json!({"track": t.track.0, "action": "keep"})).unwrap();
    assert_eq!(t.stored().len(), 1);
    assert!(t.app.run("instrument.measure_external", json!({"track": t.track.0, "action": "keep"})).is_err(), "nothing left to keep");
    t.app.run("instrument.measure_external", json!({"track": t.track.0})).unwrap();
    t.app.run("instrument.measure_external", json!({"track": t.track.0, "action": "cancel"})).unwrap();
    assert_eq!(t.fake().cancelled, 1);
    // The engine's refusal comes first when the track cannot be measured at all.
    let r = t.app.run("track.new", json!({"kind": "instrument"})).unwrap();
    let other = r["tracks"][0].as_u64().unwrap();
    assert!(t.app.run("instrument.measure_external", json!({"track": other})).unwrap_err().contains("no External Instrument"));
}

// ---- The card's rows and the Devices row height (issue #96) ----

use crate::external_instrument::{CARD_H, CARD_W, CHOOSE_FIRST, Rows};

fn overlap(a: egui::Rect, b: egui::Rect) -> bool {
    let i = a.intersect(b);
    i.width() > 0.0 && i.height() > 0.0
}

fn rect(r: [f32; 4]) -> egui::Rect {
    egui::Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]))
}

#[test]
fn the_rows_of_the_card_are_inside_it_and_never_overlap_at_the_least_width_or_a_wide_one() {
    for w in [CARD_W, 400.0] {
        let r = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(w, CARD_H));
        let rows = Rows::of(r, r.min.y + 22.0);
        let all = rows.all();
        for (i, a) in all.iter().enumerate() {
            assert!(r.contains_rect(*a), "row {i} leaves the card at width {w}: {a:?}");
            assert!(a.width() >= 40.0, "row {i} is too narrow for its text at width {w}");
            for (j, b) in all.iter().enumerate().skip(i + 1) {
                assert!(!overlap(*a, *b), "rows {i} and {j} overlap at width {w}: {a:?} {b:?}");
            }
        }
    }
}

#[test]
fn the_widgets_drawn_in_the_card_do_not_overlap_and_the_status_row_is_last() {
    let mut t = rig();
    t.app.run("midi.output", json!({"track": t.track.0, "port": "internal"})).unwrap();
    t.frames(3);
    let c = t.card();
    assert_eq!(c.external_state, "No MIDI port: choose one on the Out button");
    let widgets = [c.external_out, c.external_return, c.external_latency, c.external_measure, c.external_result, c.external_gain, c.external_status];
    for (i, a) in widgets.iter().enumerate() {
        assert_ne!(*a, [0.0; 4], "widget {i} is not drawn");
        assert!(rect(c.rect).contains_rect(rect(*a)), "widget {i} leaves the card");
        for (j, b) in widgets.iter().enumerate().skip(i + 1) {
            assert!(!overlap(rect(*a), rect(*b)), "widgets {i} and {j} overlap");
        }
    }
    let status = rect(c.external_status);
    let pane = rect(t.app.mixer.devices.area);
    assert!(pane.contains_rect(status), "the status row is inside the Devices row, not below it: {status:?} in {pane:?}");
    assert!(widgets[..6].iter().all(|w| rect(*w).max.y <= status.min.y), "the status row is below every other widget");
}

#[test]
fn measure_is_off_with_its_tooltip_until_a_port_and_a_return_are_both_set() {
    let mut t = rig();
    let track = t.track.0;
    let port = json!({"backend": "winmm", "name": "MODEL D"});
    for (has_port, has_return, enabled) in [(false, false, false), (true, false, false), (false, true, false), (true, true, true)] {
        t.app.run("midi.output", json!({"track": track, "port": if has_port { port.clone() } else { json!("internal") }})).unwrap();
        t.app.run("instrument.external", json!({"track": track, "return": if has_return { "In 1" } else { "" }})).unwrap();
        t.frames(3);
        let c = t.card();
        assert_ne!(c.external_measure, [0.0; 4], "Measure is drawn with port {has_port} and return {has_return}");
        assert_eq!(c.external_measure_text, "Measure");
        let why = if enabled { "" } else { CHOOSE_FIRST };
        assert_eq!(c.external_measure_blocked, why, "port {has_port}, return {has_return}");
        if !enabled {
            t.click(c.external_measure);
            assert!(t.app.external_measure.running.is_none(), "a disabled Measure does not start");
        }
    }
}

#[test]
fn the_devices_row_is_as_tall_as_its_tallest_card_and_shrinks_back_without_the_external_instrument() {
    let mut t = rig();
    let height = |t: &T| t.app.mixer.devices.area[3] - t.app.mixer.devices.area[1];
    let high = height(&t);
    assert_eq!(t.card().rect[3] - t.card().rect[1], CARD_H);
    let plugin = t.app.builtin_plugins().into_iter().find(|p| p.instrument).expect("a built-in instrument").id;
    t.app.set_instrument(t.track, &plugin).unwrap();
    t.frames(3);
    let low = height(&t);
    // The row is its card plus the lane padding: it grows by exactly what the card asks beyond the
    // stock card, and the stock row is the old one (the panel's own line takes a point).
    assert_eq!(high - low, CARD_H - (crate::devices::PANE_H - 16.0), "the External Instrument's card sets the row ({high} vs {low})");
    assert!((crate::devices::PANE_H - 2.0..=crate::devices::PANE_H).contains(&low), "stock cards keep the row of old: {low}");
}
