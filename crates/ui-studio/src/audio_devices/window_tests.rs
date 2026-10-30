use super::*;

/// A device that runs a buffer other than the one asked for (PipeWire's quantum, a driver whose panel
/// fixes the size) shows both on the line under "Output open", on any backend; an honored request
/// shows nothing extra.
#[test]
fn the_window_shows_the_requested_buffer_next_to_the_one_the_device_runs() {
    use crate::audio_devices::requested_note;
    use transit_audio_backend as ab;
    assert_eq!(requested_note(Some(128), Some(256), None, 48_000).as_deref(), Some("Requested 128 · running at 256"));
    assert_eq!(requested_note(Some(128), Some(128), None, 48_000), None);
    assert_eq!(requested_note(None, Some(256), None, 48_000), None, "nothing was asked for");
    assert_eq!(
        requested_note(Some(128), Some(256), Some(44_100), 48_000).as_deref(),
        Some("Requested 128 · running at 256; Requested 44100 Hz · running at 48000 Hz")
    );
    let driver = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32))
        .named("ASIO", "Focusrite USB ASIO")
        .with_capture(2, true)
        .with_fixed_buffer(256);
    let again = driver.clone();
    let mut t = asio_rig(&driver, move |session, cfg| transit_playback::Player::open_fake_native_asio(session, cfg, &again).unwrap());
    t.app.ui.audio.buffer_size = Some(128);
    t.open_window();
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(lines[0].starts_with("Output open") && lines[0].contains("256-sample buffer"), "{lines:?}");
    assert_eq!(lines[1], "Requested 128 · running at 256", "{lines:?}");
    t.app.ui.audio.buffer_size = Some(256);
    t.frames(2);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(lines.iter().all(|l| !l.starts_with("Requested")), "an honored request adds nothing: {lines:?}");
}

#[test]
fn the_open_lines_end_with_the_workers_mmcss_state() {
    use crate::audio_devices::priority_detail;
    use transit_playback::scheduling::WorkerPriority as P;
    assert_eq!(priority_detail(P::MmcssProAudio), " · MMCSS Pro Audio");
    assert_eq!(priority_detail(P::Pending), " · MMCSS pending");
    assert_eq!(priority_detail(P::PromotionFailed), " · MMCSS refused");
    assert_eq!(priority_detail(P::Released), " · MMCSS released");
    assert_eq!(priority_detail(P::DriverRegistered), " · MMCSS by driver", "the ASIO driver registered its own thread (error 1552)");
    assert_eq!(priority_detail(P::Unobserved), "", "nothing where the backend observes none (a scripted driver, the host thread)");
    assert_eq!(priority_detail(P::NoWorker), "");
}

#[test]
fn the_window_shows_the_backend_loop_lines_under_the_open_lines_from_a_scripted_snapshot() {
    use transit_playback::telemetry::StreamStats;
    let mut hist = [0u64; 16];
    hist[0] = 90;
    hist[3] = 10; // 10 wakes of 64..127 us
    let output = StreamStats {
        wakes: 1200,
        skipped_wakes: 2,
        wake_late_us: hist,
        max_wake_late_us: 90,
        service_us: hist,
        max_service_us: 100,
        min_padding_frames: 96,
        ..Default::default()
    };
    let input = StreamStats { wakes: 800, timeouts: 1, max_packets_per_wake: 3, max_wake_late_us: 5, wake_late_us: hist, ..Default::default() };
    let mut t = T::new();
    t.app.audio_devices.injected_backend = Some((Some(output), Some(input)));
    t.open_window();
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[0].starts_with("Output open"));
    assert_eq!(lines[1], "Backend: gap over period p95 90 µs, max 90 µs; service p95 100 µs, max 100 µs; skipped 2, timeouts 0; min padding 96");
    assert!(lines[2].starts_with("Input open"));
    assert_eq!(
        lines[3], "Backend: late p95 5 µs, max 5 µs; service p95 0 µs, max 0 µs; skipped 0, timeouts 1; max packets 3",
        "the capture line ends with the most packets of a wake"
    );
    // Two samples a second apart give a rate, not a raw counter.
    t.time += 1.0;
    let (mut output, mut input) = (output, input);
    output.wakes += 100;
    input.wakes += 99;
    t.app.audio_devices.injected_backend = Some((Some(output), Some(input)));
    t.frames(1);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    // The first sample was taken at the window's first frame, so the span is a little over a second.
    assert!(lines[1].contains("wakes 6") || lines[1].contains("wakes 7") || lines[1].contains("wakes 8"), "{lines:?}");
    assert!(lines[1].starts_with("Backend: wakes ") && lines[1].contains("/s, gap over period p95"), "a rate, not a counter: {lines:?}");
    assert!(lines[3].starts_with("Backend: wakes ") && lines[3].contains("/s, late p95"), "{lines:?}");
    // A native ASIO driver's events, busy skips with the overloads, join the line once one happened.
    output.overloads = 1;
    output.busy_skips = 2;
    t.app.audio_devices.injected_backend = Some((Some(output), Some(input)));
    t.frames(1);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(lines[1].ends_with("min padding 96; overloads 1, busy skips 2, position jumps 0, resets 0"), "{lines:?}");
    t.app.audio_devices.injected_backend = None;
    t.frames(3);
    let plain = t.layout()["opened"].as_array().unwrap().len();
    assert_eq!(plain, 2, "no snapshot, no backend lines");
    // On ASIO: no padding, packets or skipped wakes (WASAPI's), the driver events always, and the
    // lateness named for what it is; one line for a duplex driver (#127), with each direction's
    // figures: a fault on the input alone shows.
    t.app.ui.audio.system = Some("ASIO".into());
    let mut input = StreamStats { wakes: output.wakes, faults: 1, overloads: 1, busy_skips: 2, ..input };
    t.app.audio_devices.injected_backend = Some((Some(output), Some(input)));
    t.frames(1);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(
        lines[1],
        "Backend (output / input, one ASIO driver): gap over period p95 90 / 5 µs, max 90 / 5 µs; service p95 100 / 0 µs, \
         max 100 / 0 µs; faults 0 / 1; timeouts 0; overloads 1, busy skips 2, position jumps 0, resets 0",
        "{lines:?}"
    );
    assert!(lines[2].starts_with("Input open"), "{lines:?}");
    // The render side stopped while capture runs: its count stays, capture's grows; past the 1%
    // rule the two get their own lines, capture's with capture's figures.
    t.time += 1.0;
    input.wakes += 750;
    input.max_service_us = 40;
    t.app.audio_devices.injected_backend = Some((Some(output), Some(input)));
    t.frames(1);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert!(lines[1].starts_with("Backend: wakes 0/s, gap over period"), "render's frozen figures on its own line: {lines:?}");
    assert!(
        lines[3].starts_with("Backend: wakes ") && lines[3].contains("max 40 µs") && lines[3].contains("; faults 1;"),
        "capture's own: {lines:?}"
    );
    // An ASIO output alone has a plain Backend line.
    t.app.audio_devices.injected_backend = Some((Some(output), None));
    t.frames(1);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(lines[1].starts_with("Backend: wakes ") && lines[1].contains("gap over period"), "{lines:?}");
}

#[test]
fn the_jack_and_wasapi_lines_name_the_gap_over_period_and_leave_out_a_wake_rate_not_yet_known() {
    use transit_playback::telemetry::StreamStats;
    let output = StreamStats { wakes: 1200, max_wake_late_us: 93_039, ..Default::default() };
    let mut t = T::new();
    t.app.audio_devices.injected_backend = Some((Some(output), None));
    t.open_window();
    t.app.ui.audio.system = Some("JACK".into());
    t.frames(1);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[1].starts_with("Backend: gap over period p95 "), "no dash, no 'late': {lines:?}");
    assert!(lines[1].contains("max 93039 µs") && !lines[1].contains("wakes") && !lines[1].contains("late p95"), "{lines:?}");
    t.app.ui.audio.system = Some("WASAPI".into());
    t.frames(1);
    let lines: Vec<String> = t.layout()["opened"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(lines[1].starts_with("Backend: gap over period p95 ") && !lines[1].contains("wakes"), "{lines:?}");
}

#[test]
fn a_wake_rate_is_the_counter_delta_over_the_time_delta() {
    use crate::audio_devices::WakeRate;
    let mut w = WakeRate::default();
    assert_eq!(w.update(10.0, 1_000), None, "one sample is no rate");
    assert_eq!(w.update(10.1, 1_010), None, "too soon: still no rate");
    assert_eq!(w.update(11.0, 1_100), Some(100));
    assert_eq!(w.update(11.1, 1_110), Some(100), "too soon: the previous rate stays");
    assert_eq!(w.update(13.0, 1_300), Some(100), "200 wakes over 2 s");
    assert_eq!(w.update(14.0, 5), None, "a counter that went down is a new stream");
}

#[test]
fn the_calibration_command_wraps_inside_the_window_and_has_a_copy_button() {
    let line = calibration_line(&AudioConfig::default(), "Speakers", Some("Mic"), 48_000, "ui.json");
    let command = calibration_command(&line).expect("an uncalibrated line carries its command");
    assert!(command.starts_with("audio-bench --backend wasapi") && command.ends_with("--settings \"ui.json\""), "{command}");
    let measured = "measured 300 frames \u{b1}2 (today) \u{b7} shared";
    assert_eq!(calibration_command(measured), None, "a measured line has nothing to copy");
    if !transit_playback::devices::NATIVE_WASAPI_COMPILED {
        return;
    }
    let mut t = T::new();
    t.open_window();
    t.pick("backend", Backend::NativeWasapi.label());
    let window: [f32; 4] = serde_json::from_value(t.layout()["window"].clone()).unwrap();
    let text = t.control_rect("calibration_text");
    let copy = t.control_rect("calibration_copy");
    assert!(text[2] <= window[2] + 0.5, "the wrapped command stays inside the window: {text:?} in {window:?}");
    assert!(text[3] - text[1] > 20.0, "the long command wraps onto more than one line: {text:?}");
    assert!(copy[2] <= window[2], "the Copy button is inside the window");
}

#[test]
fn only_the_selected_asio_driver_says_why_it_did_not_load() {
    let mut t = T::new();
    t.control(
        "audio.fake_devices",
        json!({"systems": [
            {"name": "WASAPI", "is_default": true, "outputs": [{"name": "Speakers (Realtek)", "is_default": true}], "inputs": []},
            {"name": "ASIO",
             "outputs": [{"name": "Good ASIO", "max_channels": 2, "is_default": true}, {"name": "Move", "max_channels": 2}, {"name": "Overbridge 1", "max_channels": 2}, {"name": "Maschine", "max_channels": 2}],
             "inputs": [{"name": "Good ASIO", "max_channels": 2, "is_default": true}],
             "diagnostics": [
                "Move: ASIO driver \"Move\": init failed: No device is connected to the PC.",
                "Overbridge 1: ASIO driver \"Overbridge 1\": init failed: No device is connected to the PC.",
                "Maschine: ASIO driver \"Maschine\": init failed: No device is connected to the PC."]}
        ]}),
    )
    .unwrap();
    t.open_window();
    t.pick("system", "ASIO");
    assert!(t.layout()["diagnostics"].as_array().unwrap().is_empty(), "a loaded driver is selected: nothing is said");
    t.pick("output", "Move · 2 ch");
    assert_eq!(t.layout()["diagnostics"], json!(["Move: init failed: No device is connected to the PC."]));
    t.pick("output", "Good ASIO · 2 ch");
    assert!(t.layout()["diagnostics"].as_array().unwrap().is_empty());
}

/// Two systems whose buffer lists differ: the pending-change tests pick between them.
fn two_backends() -> Value {
    json!({"systems": [
        {"name": "ALSA", "is_default": true, "outputs": [{"name": "hw:0", "is_default": true, "buffer_min": 64, "buffer_max": 2048}]},
        {"name": "JACK", "outputs": [{"name": "system", "is_default": true, "buffer_min": 128, "buffer_max": 256}]}
    ]})
}

/// A saved buffer the device does not offer (#230: the tester's 4096 on a server that allows 64 to
/// 2048) never leaves the list showing a value it cannot take: the window uses the nearest offered
/// size, says so under the list, and Apply saves it.
#[test]
fn a_saved_buffer_the_device_does_not_offer_snaps_to_the_nearest_and_says_so() {
    let mut t = T::new();
    t.control("audio.fake_devices", two_backends()).unwrap();
    t.app.ui.audio.system = Some("ALSA".into());
    t.app.ui.audio.buffer_size = Some(4096);
    t.open_window();
    let lay = t.layout();
    assert_eq!(lay["draft"]["buffer_size"], json!(2048), "{}", lay["draft"]);
    let notes: Vec<&str> =
        lay["controls"].as_array().unwrap().iter().filter(|c| c["value"] == "buffer_snapped").map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(notes, ["Saved buffer 4096 is outside this device's range (64-2048); using 2048. Apply saves it."]);
    assert!(lay["pending"].is_string(), "Apply is offered: {}", lay["pending"]);
    assert!(buffer_labels(&mut t).iter().all(|l| l != "4096 samples"), "the list offers only the device's sizes");
    // An offered saved size is left alone and says nothing.
    t.app.ui.audio.buffer_size = Some(256);
    t.frames(3);
    let lay = t.layout();
    assert_eq!(lay["draft"]["buffer_size"], json!(256));
    assert!(lay["controls"].as_array().unwrap().iter().all(|c| c["value"] != "buffer_snapped"));
}

fn buffer_labels(t: &mut T) -> Vec<String> {
    t.click(mid(t.control_rect("buffer")));
    let labels =
        t.layout()["items"].as_array().unwrap().iter().filter(|i| i["value"] == "buffer").map(|i| i["label"].as_str().unwrap().to_string()).collect();
    t.click((2.0, 2.0));
    labels
}

#[test]
fn a_system_note_without_a_device_prefix_shows_once_whichever_output_is_selected() {
    let note = "Native JACK (PipeWire 1.0.7) runs at 48000 Hz (the server's rate) with a 128-sample buffer";
    let dev = |n: &str, d: bool| json!({"name": n, "is_default": d, "buffer_min": 128, "buffer_max": 128});
    let mut t = T::new();
    t.control(
        "audio.fake_devices",
        json!({"systems": [{"name": "JACK", "is_default": true,
            "outputs": [dev("system", true), dev("Scarlett", false), dev("Monitor", false)],
            "diagnostics": [note, "Monitor: port group busy"]}]}),
    )
    .unwrap();
    t.open_window();
    t.pick("system", "JACK");
    let count = |t: &mut T| t.layout()["controls"].as_array().unwrap().iter().filter(|c| c["value"] == "system_note" && c["label"] == note).count();
    assert_eq!(count(&mut t), 1);
    for output in ["Scarlett", "Monitor", "system"] {
        t.pick("output", &format!("{output} · 2 ch"));
        assert_eq!(count(&mut t), 1, "{output}");
    }
}

#[test]
fn picking_another_driver_shows_the_pending_line_highlights_apply_and_lists_the_selected_backends_choices() {
    let mut t = T::new();
    t.control("audio.fake_devices", two_backends()).unwrap();
    t.open_window();
    assert_eq!(t.layout()["pending"], Value::Null);
    assert_eq!(t.layout()["apply_highlighted"], json!(false));
    assert!(buffer_labels(&mut t).contains(&"64 samples".to_string()), "the default system offers 64");
    t.pick("system", "JACK");
    assert_eq!(t.layout()["pending"], json!("Driver change pending: press Apply to switch to JACK."));
    assert_eq!(t.layout()["apply_highlighted"], json!(true));
    assert_eq!(buffer_labels(&mut t), ["System default", "128 samples", "256 samples"], "the selected backend's sizes, before Apply");
    assert!(t.reopened().is_empty(), "a draft change opens nothing");
    let note = t.layout()["controls"].as_array().unwrap().iter().find(|c| c["value"] == "jack_buffer_note").cloned().unwrap_or_default();
    assert_eq!(note["label"], json!("Changes the JACK server's buffer size for all clients."));
    t.click(mid(t.control_rect("apply")));
    assert_eq!(t.reopened().last().and_then(|c| c.system.clone()).as_deref(), Some("JACK"));
    assert_eq!(t.layout()["pending"], Value::Null, "Apply clears the pending line");
    assert_eq!(t.layout()["apply_highlighted"], json!(false));
}

#[test]
fn revert_clears_the_draft_without_switching() {
    let mut t = T::new();
    t.control("audio.fake_devices", two_backends()).unwrap();
    t.open_window();
    t.pick("system", "JACK");
    assert!(t.layout()["pending"].is_string());
    t.click(mid(t.control_rect("revert")));
    assert_eq!(t.layout()["pending"], Value::Null);
    assert_eq!(t.app.audio_devices.draft, t.app.ui.audio);
    assert!(t.reopened().is_empty(), "Revert reopens nothing");
}

#[test]
fn a_sample_rate_change_is_pending_until_apply_reopens() {
    let mut t = T::new();
    t.open_window();
    t.pick("rate", "48000 Hz");
    assert_eq!(t.layout()["pending"], json!("Sample rate change pending: press Apply to reopen at 48000 Hz."));
    t.click(mid(t.control_rect("apply")));
    assert_eq!(t.layout()["pending"], Value::Null);
}

#[test]
fn a_driver_that_must_be_open_to_be_described_marks_its_lists_stale() {
    let mut t = T::new();
    t.control(
        "audio.fake_devices",
        json!({"systems": [{"name": "ASIO", "is_default": true, "outputs": [{"name": "Driver A", "is_default": true, "max_channels": 0}], "inputs": []}]}),
    )
    .unwrap();
    t.open_window();
    for key in ["stale_rate", "stale_buffer"] {
        let note = t.layout()["controls"].as_array().unwrap().iter().find(|c| c["value"] == key).cloned().unwrap_or_default();
        assert_eq!(note["label"], json!("updates after Apply"), "{key}");
    }
}

/// The sidebar sections and what each must show (#107).
const SECTIONS: [&str; 3] = ["section_system", "section_io", "section_hardware"];

fn inside(outer: [f32; 4], inner: [f32; 4]) -> bool {
    inner[0] >= outer[0] - 0.5 && inner[1] >= outer[1] - 0.5 && inner[2] <= outer[2] + 0.5 && inner[3] <= outer[3] + 0.5
}

#[test]
fn at_1366_by_768_every_control_of_every_section_is_inside_the_window_and_the_window_inside_the_screen() {
    assert_sections_fit(egui::vec2(1366.0, 768.0), 1.0);
}

#[test]
fn at_1920_by_1080_with_150_percent_interface_zoom_every_audio_devices_section_fits() {
    assert_sections_fit(egui::vec2(1920.0, 1080.0), 1.5);
}

fn assert_sections_fit(physical_screen: egui::Vec2, zoom: f32) {
    let mut t = T::new();
    t.app.ui.ui_zoom = zoom;
    t.screen = physical_screen / zoom;
    t.frames(3);
    assert_eq!(t.ctx.zoom_factor(), zoom);
    assert_eq!(t.ctx.pixels_per_point(), zoom);
    t.app.engine.set_io_map(transit_model::IoMap::for_device("ASIO", "TASCAM Model 12", 12, 10));
    t.open_window();
    for key in SECTIONS {
        t.section(key);
        let layout = t.layout();
        let window: [f32; 4] = serde_json::from_value(layout["window"].clone()).unwrap();
        assert!(inside([0.0, 0.0, t.screen.x, t.screen.y], window), "{key}: the window {window:?} exceeds the screen");
        let controls = layout["controls"].as_array().unwrap();
        assert!(controls.len() > 7, "{key}: the section and the row show their controls");
        // Rows outside a scroll area's clip are not painted or clickable.
        for c in controls.iter().filter(|c| c["visible"] == true) {
            let rect: [f32; 4] = serde_json::from_value(c["rect"].clone()).unwrap();
            assert!(inside(window, rect), "{key}: {} {rect:?} is outside the window {window:?}", c["value"]);
        }
        if key == "section_io" {
            // Scroll the section, then the input paths' nested list, through real wheel input.
            t.control("ui.scroll", json!({"x": window[2] - 30.0, "y": window[1] + 100.0, "dy": -500.0})).unwrap();
            let layout = t.layout();
            let input = layout["controls"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["visible"] == true && c["value"].as_str().is_some_and(|v| v.starts_with("io_channels:In ")))
                .expect("scrolling the section exposes the input paths");
            let at = mid(serde_json::from_value(input["rect"].clone()).unwrap());
            t.control("ui.scroll", json!({"x": at.0, "y": at.1, "dy": -500.0})).unwrap();
            let layout = t.layout();
            let last = layout["controls"].as_array().unwrap().iter().find(|c| c["value"] == "io_channels:In 11-12").unwrap();
            assert_eq!(last["visible"], true, "the final input path is reachable by scrolling");
            assert!(inside(window, serde_json::from_value(last["rect"].clone()).unwrap()));
        }
    }
}

#[test]
fn the_status_lines_and_the_apply_row_stay_in_every_section() {
    let mut t = T::new();
    t.open_window();
    for key in SECTIONS {
        t.section(key);
        for control in ["apply", "revert", "reconnect", "rescan"] {
            assert!(has_control(&t, control), "{key}: {control}");
        }
        assert!(!t.layout()["opened"].as_array().unwrap().is_empty(), "{key}: the open lines");
    }
    t.section("section_system");
    t.pick("system", "ASIO");
    t.section("section_io");
    assert!(t.layout()["pending"].is_string(), "the pending line is in every section");
}

#[test]
fn the_last_section_is_remembered_in_the_ui_state_not_the_session() {
    let mut t = T::new();
    t.open_window();
    assert_eq!(t.layout()["section"], json!("section_system"), "the first time: System");
    t.section("section_hardware");
    assert_eq!(t.app.ui.audio_devices_section, "section_hardware");
    let saved = serde_json::to_value(&t.app.ui).unwrap();
    assert_eq!(saved["audio_devices_section"], json!("section_hardware"), "saved with the preferences in ui.json");
    // A later run reads it back.
    let mut later = T::new();
    later.app.ui = serde_json::from_value(saved).unwrap();
    later.open_window();
    assert_eq!(later.layout()["section"], json!("section_hardware"));
    // The session file does not carry it.
    assert!(!format!("{:?}", t.app.engine.session()).contains("audio_devices_section"));
}

#[test]
fn ctrl_tab_and_ctrl_shift_tab_cycle_the_sections() {
    let mut t = T::new();
    t.open_window();
    let chord = |t: &mut T, shift: bool| {
        let mut modifiers = egui::Modifiers::COMMAND;
        modifiers.shift = shift;
        t.events.push(egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers });
        t.frames(3);
        t.layout()["section"].as_str().unwrap().to_string()
    };
    assert_eq!(chord(&mut t, false), "section_io");
    assert_eq!(chord(&mut t, false), "section_hardware");
    assert_eq!(chord(&mut t, false), "section_system", "wraps around");
    assert_eq!(chord(&mut t, true), "section_hardware", "Shift goes back");
    assert_eq!(chord(&mut t, true), "section_io");
}

#[test]
fn escape_reverts_the_draft_but_not_while_a_text_field_has_focus() {
    let mut t = T::new();
    t.control("audio.fake_devices", two_backends()).unwrap();
    t.open_window();
    t.pick("system", "JACK");
    let id = egui::Id::new("a_text_field");
    t.ctx.memory_mut(|m| m.request_focus(id));
    t.frames(1);
    t.ctx.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { escape: true, ..Default::default() }));
    t.events.push(egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
    t.frames(2);
    assert_ne!(t.app.audio_devices.draft, t.app.ui.audio, "Escape in a focused field leaves the draft");
}

/// Copy diagnostics (#114): the button is in the status area of every section, copies the report
/// built from what the window shows, and says "Copied" for two seconds.
#[test]
fn copy_diagnostics_is_in_every_section_and_copies_the_report() {
    let mut t = T::new();
    t.app.diag.machine = Some(crate::diagnostics::Machine { os: "Test OS".into(), cores: 4, ..Default::default() });
    t.open_window();
    for key in SECTIONS {
        t.section(key);
        assert!(has_control(&t, "copy_diagnostics"), "{key}: the Copy diagnostics button");
    }
    t.section("section_io");
    t.click(mid(t.control_rect("copy_diagnostics")));
    let text = t.app.diag.copied.clone();
    for want in [
        "== System ==",
        "OS: Test OS",
        "== Audio devices ==",
        "Speakers (Realtek)",
        SCARLETT_IN,
        "== Settings ==",
        "== Open devices ==",
        "Output open: none",
        "== Callback telemetry ==",
        "== MIDI ==",
        crate::diagnostics::NO_PLAYER_MIDI,
        "MIDI Learn mappings: 0 session, 0 controller profile (0 in force)",
        "== Log ==",
    ] {
        assert!(text.contains(want), "missing {want:?}\n{text}");
    }
    assert!(t.app.diagnostics_copied(t.time - 1.0), "Copied shows");
    t.frames(150);
    assert!(!t.app.diagnostics_copied(t.time), "and goes after two seconds");
}
