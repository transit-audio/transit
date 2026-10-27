use super::*;

fn calibration(device: &str, input: Option<&str>, rate: u32, period: u32) -> transit_playback::LoopbackCalibration {
    transit_playback::LoopbackCalibration {
        device: device.into(),
        input: input.map(str::to_string),
        mode: "shared".into(),
        rate,
        period,
        round_trip_frames: 480,
        spread_frames: 25,
        measured_at: "2026-10-21".into(),
    }
}

#[test]
fn the_calibration_line_reports_the_measured_pair_or_the_command_that_measures_it() {
    use crate::audio_devices::calibration_line;
    let out = "Speakers (Focusrite USB Audio)";
    let saved = AudioConfig {
        input_pairing: Some(SCARLETT_IN.into()),
        buffer_size: Some(128),
        calibrations: vec![calibration(out, Some(SCARLETT_IN), 48_000, 128)],
        share_device: true,
        ..Default::default()
    };
    assert_eq!(calibration_line(&saved, out, None, 48_000, "ui.json"), "measured 480 frames \u{b1}25 (2026-10-21) \u{b7} shared");
    let unmeasured = calibration_line(&saved, out, None, 96_000, "C:/Users/me/transit/ui.json");
    assert_eq!(
        unmeasured,
        format!(
            "not calibrated \u{b7} shared: run audio-bench --backend wasapi --loopback --out-device \"{out}\" --in-device \"{SCARLETT_IN}\" --period 128 \
             --rate 96000 --settings \"C:/Users/me/transit/ui.json\" (with this app closed)"
        )
    );
    // Exclusive is the default: its calibration is another key, and its command measures exclusive.
    let exclusive = AudioConfig { share_device: false, ..saved.clone() };
    let line = calibration_line(&exclusive, out, None, 48_000, "ui.json");
    assert!(line.starts_with("not calibrated \u{b7} exclusive: run audio-bench --backend wasapi --loopback"), "{line}");
    assert!(line.contains("--mode exclusive "), "{line}");
    // Paired input on Automatic: a record that named the input the pairing resolved to applies.
    let automatic = AudioConfig { input_pairing: None, ..saved.clone() };
    assert!(calibration_line(&automatic, out, None, 48_000, "ui.json").starts_with("not calibrated"));
    assert!(calibration_line(&automatic, out, Some("Line (Other)"), 48_000, "ui.json").starts_with("not calibrated"));
    assert_eq!(
        calibration_line(&automatic, out, Some(&SCARLETT_IN.to_uppercase()), 48_000, "ui.json"),
        "measured 480 frames \u{b1}25 (2026-10-21) \u{b7} shared"
    );
    let measured = transit_playback::LoopbackCalibration {
        mode: "exclusive".into(),
        round_trip_frames: 960,
        ..calibration(out, Some(SCARLETT_IN), 48_000, 128)
    };
    let both = AudioConfig { calibrations: vec![calibration(out, Some(SCARLETT_IN), 48_000, 128), measured], ..exclusive };
    assert_eq!(calibration_line(&both, out, None, 48_000, "ui.json"), "measured 960 frames \u{b1}25 (2026-10-21) \u{b7} exclusive");
    let other_period = AudioConfig { buffer_size: Some(64), share_device: true, ..both };
    assert!(
        calibration_line(&other_period, out, None, 48_000, "ui.json").starts_with("not calibrated \u{b7} shared: run "),
        "a calibration is for one buffer size"
    );
}

#[test]
fn the_paired_input_and_calibration_line_exist_only_under_the_native_backend() {
    let mut t = T::new();
    t.open_window();
    let calibration_shown = |t: &T| !t.layout()["calibration"].is_null();
    assert!(!has_control(&t, "paired_input") && !calibration_shown(&t), "the default backend has no pairing");
    if !transit_playback::devices::NATIVE_WASAPI_COMPILED {
        return;
    }
    t.pick("backend", Backend::NativeWasapi.label());
    assert!(has_control(&t, "paired_input") && calibration_shown(&t), "the native backend pairs an input");
    let line = t.layout()["calibration"].as_str().unwrap().to_string();
    assert!(
        line.starts_with("not calibrated \u{b7} shared: run audio-bench --backend wasapi --loopback --out-device \"Speakers (Realtek)\""),
        "{line}"
    );
    t.pick("paired_input", &format!("{SCARLETT_IN} \u{b7} 2 ch"));
    assert_eq!(t.app.audio_devices.draft.input_pairing.as_deref(), Some(SCARLETT_IN));
    assert!(t.layout()["calibration"].as_str().unwrap().contains(&format!("--in-device \"{SCARLETT_IN}\"")));
    let apply = t.control_rect("apply");
    t.click(mid(apply));
    assert_eq!(t.reopened().last().and_then(|c| c.input_pairing.clone()).as_deref(), Some(SCARLETT_IN), "the pairing is applied and saved");
    t.pick("backend", Backend::Auto.label());
    assert!(!has_control(&t, "paired_input") && !calibration_shown(&t), "back to the default backend");
    t.pick("backend", Backend::NativeWasapi.label());
    t.pick("paired_input", PAIRED_AUTOMATIC);
    assert_eq!(t.app.audio_devices.draft.input_pairing, None);
}

#[test]
fn the_share_checkbox_follows_the_native_backend_and_applies_with_the_devices() {
    let mut t = T::new();
    t.open_window();
    assert!(!has_control(&t, "share_device"), "the default backend has no sharing choice");
    if !transit_playback::devices::NATIVE_WASAPI_COMPILED {
        return;
    }
    t.pick("backend", Backend::NativeWasapi.label());
    assert!(has_control(&t, "share_device"), "the native backend offers it");
    assert_eq!(SHARE_LABEL, "Take exclusive control of the device");
    assert!(t.app.audio_devices.draft.share_device, "unchecked: shared is the default");
    assert!(t.layout()["calibration"].as_str().unwrap().contains("shared"), "the Calibration line names the mode");
    t.click(mid(t.control_rect("share_device")));
    assert!(!t.app.audio_devices.draft.share_device, "checking it asks for exclusive control");
    assert!(t.layout()["calibration"].as_str().unwrap().contains("exclusive"));
    assert!(t.app.ui.audio.share_device, "nothing is applied until Apply");
    t.click(mid(t.control_rect("apply")));
    assert_eq!(t.reopened().last().map(|c| c.share_device), Some(false), "Apply reopens the devices exclusive");
    assert!(!t.app.ui.audio.share_device);
    t.click(mid(t.control_rect("share_device")));
    assert!(t.app.audio_devices.draft.share_device);
    t.click(mid(t.control_rect("apply")));
    assert_eq!(t.reopened().last().map(|c| c.share_device), Some(true));
    t.pick("backend", Backend::Auto.label());
    assert!(!has_control(&t, "share_device"), "back to the default backend");
}

#[test]
fn a_saved_calibration_is_shown_for_its_pair_and_survives_apply() {
    if !transit_playback::devices::NATIVE_WASAPI_COMPILED {
        return;
    }
    let mut t = T::new();
    let saved = AudioConfig {
        backend: Backend::NativeWasapi,
        system: Some("WASAPI".into()),
        output: Some("Speakers (Focusrite USB Audio)".into()),
        sample_rate: Some(48_000),
        calibrations: vec![calibration("Speakers (Focusrite USB Audio)", None, 48_000, 0)],
        share_device: true,
        ..Default::default()
    };
    t.app.apply_audio(saved).unwrap();
    t.open_window();
    assert_eq!(t.layout()["calibration"], json!("measured 480 frames \u{b1}25 (2026-10-21) \u{b7} shared"));
    t.pick("buffer", "128 samples");
    assert!(t.layout()["calibration"].as_str().unwrap().starts_with("not calibrated"), "another buffer size is another key");
    let apply = t.control_rect("apply");
    t.click(mid(apply));
    assert_eq!(t.app.ui.audio.calibrations.len(), 1, "Apply keeps the saved measurements");
}

/// #20, K31: the native ASIO Calibration line words what is saved for the driver at this buffer
/// size and rate, what is saved elsewhere, and nothing saved.
#[test]
fn the_asio_calibration_line_names_the_buffer_size_and_rate_it_was_measured_at() {
    use crate::audio_devices::{asio_calibration_line, period_rate_text};
    let driver = "Focusrite USB ASIO";
    let at = |period: u32, rate: u32, frames: u32| transit_playback::LoopbackCalibration {
        device: driver.into(),
        input: None,
        mode: "asio".into(),
        rate,
        period,
        round_trip_frames: frames,
        spread_frames: 7,
        measured_at: "2026-10-23".into(),
    };
    // The requested buffer does not matter: the line keys on the buffer the open driver runs.
    let cfg = AudioConfig { buffer_size: Some(128), calibrations: vec![at(256, 48_000, 1_196), at(64, 48_000, 333)], ..Default::default() };
    assert_eq!(asio_calibration_line(&cfg, driver, Some((256, 48_000))), "calibrated: 1196 frames (24.9 ms) \u{b1}7 at 256 / 48 kHz (2026-10-23)");
    assert_eq!(
        asio_calibration_line(&cfg, driver, Some((512, 48_000))),
        "not calibrated at the buffer size and rate the driver runs (measured at 256 / 48 kHz, 64 / 48 kHz): the driver's own latency figures are used; Measure checks it again"
    );
    assert_eq!(
        asio_calibration_line(&cfg, driver, None),
        "measured at 256 / 48 kHz, 64 / 48 kHz: whether it applies is known once the driver runs these choices (Apply)"
    );
    assert!(asio_calibration_line(&cfg, "ModelMixer", None).starts_with("not calibrated: the driver's own latency figures are used"));
    assert_eq!((period_rate_text(128, 44_100), period_rate_text(0, 96_000)), ("128 / 44.1 kHz".to_string(), "driver buffer / 96 kHz".to_string()));
}

/// #20, K31: Measure on the Calibration line over native ASIO, with the real Player over a
/// scripted driver whose output 2 is cabled to input 2, 1065 device frames later. The result is
/// stored under the open key (256 frames, 48 kHz), shown with its spread, handed to the running
/// streams, and the chosen paths are remembered; another buffer size is "not calibrated" again.
#[test]
fn measure_on_the_asio_calibration_line_times_the_loop_and_stores_it_for_this_buffer_size() {
    use transit_audio_backend as ab;
    if !transit_playback::devices::NATIVE_ASIO_COMPILED {
        return;
    }
    const BLOCK: usize = 256;
    const LOOP: u64 = 1_065;
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let attempt = ab::OpenRequest {
        format,
        period: ab::Period::Frames(BLOCK as u32),
        mode: ab::Mode::Shared,
        origin: ab::HostOrigin::default(),
        calibration: None,
    };
    // The driver reports 5 ms each way (480 frames in all), far from the 1065-frame loop: the
    // result is stored and says so with a warning.
    let driver = ab::fake::FakeBackend::new(format)
        .named("ASIO", "Focusrite USB ASIO")
        .with_capture(2, true)
        .with_attempts(vec![attempt])
        .with_latency(std::time::Duration::from_millis(5));
    let cfg = AudioConfig {
        backend: Backend::NativeAsio,
        system: Some("ASIO".into()),
        output: Some("Focusrite USB ASIO".into()),
        sample_rate: Some(48_000),
        buffer_size: Some(256),
        ..Default::default()
    };
    let mut t = T::new();
    let player = transit_playback::Player::open_fake_native_asio(t.app.engine.session_arc(), &cfg, &driver).unwrap();
    t.app.engine.set_io_map(player.io_map().as_ref().clone());
    t.app.player = Some(player);
    t.app.ui.audio = cfg;
    // The measurement's give-up wait reads this frozen age, not the clock (E49, #106).
    t.app.audio_devices.measure_elapsed = Some(std::time::Duration::ZERO);
    t.open_window();
    let line = t.layout()["calibration"].as_str().unwrap().to_string();
    assert!(line.starts_with("not calibrated: "), "{line}");
    let m = t.layout()["calibration_measure"].clone();
    assert_eq!(
        (m["blocked"].as_str(), m["send"].as_str(), m["ret"].as_str()),
        (Some(""), Some("Out 1-2"), Some("In 1")),
        "Main's path and the first input"
    );
    t.click(mid(t.control_rect("cal_ret")));
    t.click(mid(T::rect(&t.layout()["items"], Some("In 2"), "cal_ret_entry")));
    assert_eq!(t.layout()["calibration_measure"]["ret"], "In 2");
    t.click(mid(t.control_rect("cal_measure")));
    assert!(t.app.audio_devices.measuring.is_some(), "it started: {}", t.app.status);
    assert_eq!(t.layout()["calibration_measure"]["progress"], "Measuring\u{2026} 0/8 clicks");
    t.section("section_hardware");
    assert_eq!(t.layout()["hardware"]["blocked"], "The calibration measurement is running.");
    t.section("section_system");
    assert_eq!((t.app.ui.audio.calibration_send.as_deref(), t.app.ui.audio.calibration_return.as_deref()), (Some("Out 1-2"), Some("In 2")));
    // The driver: each switch captures (input 2 is output 2, LOOP frames late) then renders.
    let mut sent = std::collections::BTreeMap::new();
    let mut position = 48_000u64;
    let (mut output, mut input) = (vec![0.0f32; BLOCK * 2], vec![0.0f32; BLOCK * 2]);
    for _ in 0..800 {
        for f in 0..BLOCK {
            input[f * 2 + 1] = (position + f as u64).checked_sub(LOOP).and_then(|k| sent.get(&k)).copied().unwrap_or(0.0);
        }
        std::thread::scope(|s| {
            s.spawn(|| {
                driver.pump_capture_at(BLOCK, ab::Stamp::INVALID, Some(position), format, &mut ab::fake::SliceSource { data: &input, channels: 2 });
                driver.pump_render_at(BLOCK, ab::Stamp::INVALID, Some(position), &mut ab::fake::SliceSink { data: &mut output, channels: 2 });
            });
        });
        for (f, frame) in output.as_chunks::<2>().0.iter().enumerate() {
            sent.insert(position + f as u64, frame[1]);
        }
        position += BLOCK as u64;
        t.frames(1);
        if t.app.audio_devices.measuring.is_none() {
            break;
        }
    }
    assert!(t.app.audio_devices.measuring.is_none(), "the measurement finished");
    t.frames(2);
    let saved = t.app.ui.audio.calibrations.clone();
    assert_eq!(saved.len(), 1, "{saved:?}");
    let c = &saved[0];
    assert_eq!(
        (c.device.as_str(), c.input.as_deref(), c.mode.as_str(), c.rate, c.period, c.round_trip_frames, c.spread_frames),
        ("Focusrite USB ASIO", None, "asio", 48_000, 256, 1_065, 1)
    );
    let said = "1065 frames (22.2 ms) \u{b1}1 at 256 / 48 kHz";
    assert_eq!(t.layout()["calibration"], json!(format!("calibrated: {said} ({})", c.measured_at)));
    let warned = format!(
        "{said}; driver figures 480 frames (+585). Warning: the measured loop is 585 frames from the driver's figures, more than a buffer: check the cable and the paths, and Measure again"
    );
    assert_eq!(t.layout()["calibration_measure"]["result"], json!(warned));
    assert!(t.app.status.starts_with(&format!("Calibrated: {said}; driver figures 480 frames (+585). Warning: ")), "{}", t.app.status);
    let live = driver.live_calibrations();
    assert_eq!(live.len(), 2, "both running streams took it: {live:?}");
    assert!(live.iter().all(|(_, cal)| *cal == c.to_backend()));
    assert_eq!(t.app.player.as_ref().unwrap().calibration, c.to_backend());
    assert_eq!(t.app.audio_devices.draft.calibrations, saved, "the window's choices keep it, so Apply keeps it");
    // Another buffer size is another key: not calibrated there, and Measure waits for Apply.
    t.pick("buffer", "64 samples");
    let line = t.layout()["calibration"].as_str().unwrap().to_string();
    assert_eq!(line, "measured at 256 / 48 kHz: whether it applies is known once the driver runs these choices (Apply)");
    assert_eq!(t.layout()["calibration_measure"]["blocked"], "Apply the changes first: Measure times the driver as it is open.");
}

/// #20: Cancel stops a calibration measurement and stores nothing; a loop with no cable says
/// where to look and stores nothing.
#[test]
fn a_cancelled_or_silent_asio_calibration_stores_nothing_and_says_why() {
    use transit_audio_backend as ab;
    if !transit_playback::devices::NATIVE_ASIO_COMPILED {
        return;
    }
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let attempt =
        ab::OpenRequest { format, period: ab::Period::Frames(256), mode: ab::Mode::Shared, origin: ab::HostOrigin::default(), calibration: None };
    let driver = ab::fake::FakeBackend::new(format).named("ASIO", "Focusrite USB ASIO").with_capture(2, true).with_attempts(vec![attempt]);
    let cfg = AudioConfig {
        backend: Backend::NativeAsio,
        system: Some("ASIO".into()),
        output: Some("Focusrite USB ASIO".into()),
        sample_rate: Some(48_000),
        buffer_size: Some(256),
        ..Default::default()
    };
    let mut t = T::new();
    let player = transit_playback::Player::open_fake_native_asio(t.app.engine.session_arc(), &cfg, &driver).unwrap();
    t.app.engine.set_io_map(player.io_map().as_ref().clone());
    t.app.player = Some(player);
    t.app.ui.audio = cfg;
    // The measurement's give-up wait reads this frozen age, not the clock (E49, #106).
    t.app.audio_devices.measure_elapsed = Some(std::time::Duration::ZERO);
    t.open_window();
    t.click(mid(t.control_rect("cal_measure")));
    assert!(t.app.audio_devices.measuring.is_some());
    t.click(mid(t.control_rect("cal_cancel")));
    assert!(t.app.audio_devices.measuring.is_none() && !t.app.player.as_ref().unwrap().loop_calibration_running());
    assert_eq!(t.layout()["calibration_measure"]["result"], "Measurement cancelled.");
    assert_eq!(t.layout()["hardware"]["result"], "", "said where it was started");
    // No cable: silence comes back.
    t.click(mid(t.control_rect("cal_measure")));
    let (mut output, input) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    let mut position = 0u64;
    for _ in 0..800 {
        std::thread::scope(|s| {
            s.spawn(|| {
                driver.pump_capture_at(256, ab::Stamp::INVALID, Some(position), format, &mut ab::fake::SliceSource { data: &input, channels: 2 });
                driver.pump_render_at(256, ab::Stamp::INVALID, Some(position), &mut ab::fake::SliceSink { data: &mut output, channels: 2 });
            });
        });
        position += 256;
        t.frames(1);
        if t.app.audio_devices.measuring.is_none() {
            break;
        }
    }
    t.frames(2);
    assert_eq!(
        t.layout()["calibration_measure"]["result"],
        "the click came back on 0 of 8 tries: check the cable from Out 1-2 to In 1 and the input gain"
    );
    assert!(t.app.ui.audio.calibrations.is_empty() && driver.live_calibrations().is_empty());
}

/// #106: a measurement that never completes is given up on when its age passes the wait, and
/// stores nothing. The age is injected, so the test does not depend on how fast the machine is.
#[test]
fn an_asio_calibration_that_outlives_its_wait_is_given_up_and_stores_nothing() {
    use transit_audio_backend as ab;
    if !transit_playback::devices::NATIVE_ASIO_COMPILED {
        return;
    }
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let attempt =
        ab::OpenRequest { format, period: ab::Period::Frames(256), mode: ab::Mode::Shared, origin: ab::HostOrigin::default(), calibration: None };
    let driver = ab::fake::FakeBackend::new(format).named("ASIO", "Focusrite USB ASIO").with_capture(2, true).with_attempts(vec![attempt]);
    let cfg = AudioConfig {
        backend: Backend::NativeAsio,
        system: Some("ASIO".into()),
        output: Some("Focusrite USB ASIO".into()),
        sample_rate: Some(48_000),
        buffer_size: Some(256),
        ..Default::default()
    };
    let mut t = T::new();
    let player = transit_playback::Player::open_fake_native_asio(t.app.engine.session_arc(), &cfg, &driver).unwrap();
    t.app.engine.set_io_map(player.io_map().as_ref().clone());
    t.app.player = Some(player);
    t.app.ui.audio = cfg;
    t.app.audio_devices.measure_elapsed = Some(std::time::Duration::ZERO);
    t.open_window();
    t.click(mid(t.control_rect("cal_measure")));
    t.frames(3);
    assert!(t.app.audio_devices.measuring.is_some(), "no block has come and the wait has not passed: still measuring");
    t.app.audio_devices.measure_elapsed = Some(std::time::Duration::from_secs(60));
    t.frames(2);
    assert!(t.app.audio_devices.measuring.is_none() && !t.app.player.as_ref().unwrap().loop_calibration_running());
    assert_eq!(t.layout()["calibration_measure"]["result"], "the audio device did not answer: no measurement");
    assert!(t.app.ui.audio.calibrations.is_empty() && driver.live_calibrations().is_empty());
}
