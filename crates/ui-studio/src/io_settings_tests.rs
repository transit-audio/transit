//! The machine's I/O tables per side (ADR 0015, amendment for #431): the app composes the engine's
//! table from the output record and the open input's record, and keeps both halves apart.
#![allow(clippy::unwrap_used)]

use super::*;
use std::sync::Arc;
use transit_audio_backend as ab;
use transit_model::{ChannelFormat, IoDirection, IoPath};
use transit_playback::{AudioConfig, Backend, Player};

fn tascam() -> IoMap {
    IoMap::for_device("ASIO", "TASCAM Model 12", 12, 10)
}

fn out(name: &str, channels: Vec<u16>) -> IoPath {
    let format = ChannelFormat::for_channels(channels.len());
    IoPath { live: false, name: name.into(), direction: IoDirection::Output, channels, format }
}

fn inputs(m: &IoMap) -> Vec<&str> {
    m.paths_of(IoDirection::Input).map(|p| p.name.as_str()).collect()
}

/// `--null-audio`: the silent device's table reaches the engine and the player, and neither half
/// is added to the saved tables in `ui.json`.
#[test]
fn a_null_audio_run_saves_no_table_for_the_silent_device() {
    use transit_playback::backend::null::{self, PaceClock, StepClock};
    let engine = transit_engine::Engine::default();
    let clock = StepClock::new();
    let player = Player::with_null_device_clock(engine.session_arc(), Arc::clone(&clock) as Arc<dyn PaceClock>);
    clock.settle();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.null_audio = true;
    assert!(app.player.as_ref().is_some_and(|p| p.null_device() && !p.silent));
    app.sync_io();
    assert_eq!(app.engine.io_map().key(), (null::SYSTEM, null::DEVICE), "the engine holds the silent device's table");
    assert_eq!(app.io_pushed, app.engine.io_revision(), "the table went to the player");
    assert!(app.ui.io_maps.is_empty(), "no output record for the silent device: {:?}", app.ui.io_maps);
    assert!(app.ui.input_maps.is_empty(), "no input record either: {:?}", app.ui.input_maps);
}

#[test]
fn an_unknown_device_gets_its_defaults_and_a_known_one_its_saved_outputs() {
    let mut saved = Vec::new();
    assert_eq!(adopt(&saved, &tascam()), tascam());
    let mut mine = tascam();
    mine.add_path(out("Cue", vec![8, 9])).unwrap();
    remember(&mut saved, &mine);
    assert!(adopt(&saved, &tascam()).path("Cue").is_some());
    assert_eq!(saved[0].inputs, 0, "a record a new build makes has no input half");
    assert!(inputs(&saved[0]).is_empty());
    let other = IoMap::for_device("ASIO", "Scarlett 2i2", 2, 2);
    assert_eq!(adopt(&saved, &other).paths.len(), other.paths.len());
}

#[test]
fn one_device_in_another_usb_mode_adapts_its_saved_outputs() {
    let mut mine = tascam();
    mine.add_path(out("Cue", vec![8, 9])).unwrap();
    mine.add_path(out("Pair", vec![2, 3])).unwrap();
    let mut saved = Vec::new();
    remember(&mut saved, &mine);
    let two = adopt(&saved, &IoMap::for_device("ASIO", "TASCAM Model 12", 2, 2));
    assert_eq!(two.outputs, 2, "the opened output count is stored inside the record");
    assert!(two.path("Cue").is_none() && two.path("Pair").is_none(), "output paths beyond the opened channels are dropped");
    assert!(two.path("Out 1-2").is_some());
}

#[test]
fn remembering_replaces_by_device_and_is_bounded() {
    let mut saved = Vec::new();
    remember(&mut saved, &tascam());
    remember(&mut saved, &tascam());
    assert_eq!(saved.len(), 1);
    for n in 0..(MAX_SAVED + 5) {
        remember(&mut saved, &IoMap::for_device("ASIO", &format!("device {n}"), 2, 2));
    }
    assert_eq!(saved.len(), MAX_SAVED);
    assert_eq!(saved.last().map(|m| m.device.as_str()), Some("device 68"));
    let mut records = Vec::new();
    for n in 0..(MAX_SAVED + 1) {
        remember_input(&mut records, InputMap { system: "JACK".into(), device: format!("group {n}"), ..InputMap::default() });
    }
    assert_eq!(records.len(), MAX_SAVED);
}

/// A scripted native WASAPI render endpoint with two inputs; its paired capture is set per open.
fn wasapi() -> ab::fake::FakeBackend {
    ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).named("WASAPI", "Speakers (Interface)").with_capture(2, true)
}

fn wasapi_config() -> AudioConfig {
    AudioConfig { backend: Backend::NativeWasapi, system: Some("WASAPI".into()), ..AudioConfig::default() }
}

/// The app reopens on another Player, as Reconnect does.
fn reopen(app: &mut StudioApp, player: Player) {
    app.player = Some(player);
    app.io_adopt = true;
    app.sync_io();
}

/// Rule 8 and #689 on native WASAPI: one output, capture endpoint A, then B: two input records
/// keyed by the capture endpoints' identities (never the render endpoint's name), and a hardware
/// insert round trip measured through A is not applied through B, then applies again through A.
#[test]
fn two_paired_capture_endpoints_under_one_output_are_two_input_tables() {
    let endpoint = wasapi();
    let cfg = wasapi_config();
    let engine = transit_engine::Engine::default();
    let session = engine.session_arc();
    endpoint.pair_capture(Some(("{capture-a}", "Microphone (Interface)")));
    let player = Player::open_fake_native_wasapi(Arc::clone(&session), &cfg, &endpoint).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.sync_io();
    assert_eq!(app.engine.io_map().input_device, "{capture-a}");
    assert_eq!(app.io_input.as_ref().map(|i| i.name.as_str()), Some("Microphone (Interface)"));
    app.engine.execute("setup.io_map", &serde_json::json!({"action": "rename_path", "name": "In 2", "new_name": "Vox"})).unwrap();
    app.engine.store_hardware_latency("Out 1-2", "Vox", 1_234, 48_000);
    app.sync_io();
    endpoint.pair_capture(Some(("{capture-b}", "Line In (Interface)")));
    reopen(&mut app, Player::open_fake_native_wasapi(Arc::clone(&session), &cfg, &endpoint).unwrap());
    let keys: Vec<(&str, &str)> = app.ui.input_maps.iter().map(InputMap::key).collect();
    assert_eq!(keys, [("WASAPI", "{capture-a}"), ("WASAPI", "{capture-b}")], "two input records");
    assert_eq!(app.ui.io_maps.len(), 1, "one output record");
    assert_eq!(inputs(app.engine.io_map()), ["In 1", "In 2", "In 1-2"], "B starts from the defaults");
    let io = app.engine.io_map();
    assert_eq!(io.hardware_latency("Out 1-2", "Vox", 48_000), None, "not applied through B");
    assert!(io.hardware_latency_elsewhere("Out 1-2", "Vox", 48_000), "measure again");
    endpoint.pair_capture(Some(("{capture-a}", "Microphone (Interface)")));
    reopen(&mut app, Player::open_fake_native_wasapi(Arc::clone(&session), &cfg, &endpoint).unwrap());
    assert_eq!(inputs(app.engine.io_map()), ["In 1", "Vox", "In 1-2"], "A's paths again");
    assert_eq!(app.engine.io_map().hardware_latency("Out 1-2", "Vox", 48_000), Some(1_234), "and its round trip");
}

/// A scripted native ASIO driver: output, input and driver are one.
fn asio(inputs: u16) -> ab::fake::FakeBackend {
    ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).named("ASIO", "Scarlett 2i2 USB").with_capture(inputs, true)
}

fn asio_config() -> AudioConfig {
    AudioConfig { backend: Backend::NativeAsio, system: Some("ASIO".into()), ..AudioConfig::default() }
}

/// Rule 11 on ASIO: a both-halves record an older build saved loads with the same paths and Main,
/// its round trip applies (the return's identity is established), and saving keeps the old input
/// half exactly as it was read while the input record takes the edits.
#[test]
fn an_older_asio_table_loads_with_its_paths_and_keeps_its_input_half_on_disk() {
    let mut old = IoMap::for_device("ASIO", "Scarlett 2i2 USB", 2, 2);
    old.input_device.clear();
    old.add_path(IoPath { direction: IoDirection::Input, ..out("Guitar", vec![1]) }).unwrap();
    old.hardware_latency.push(transit_model::HardwareLatency {
        send: "Out 1-2".into(),
        ret: "Guitar".into(),
        round_trip_frames: 612,
        rate: 48_000,
        measured_at: 1,
        input_device: String::new(),
        input_ports: Vec::new(),
        device_rate: 0,
    });
    let read = serde_json::to_string(&old).unwrap();
    let driver = asio(2);
    let engine = transit_engine::Engine::default();
    let player = Player::open_fake_native_asio(engine.session_arc(), &asio_config(), &driver).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.ui.io_maps.push(old.clone());
    app.sync_io();
    let io = app.engine.io_map().clone();
    assert_eq!(io.paths, old.paths, "the same paths");
    assert_eq!(io.main, old.main);
    assert_eq!(io.hardware_latency("Out 1-2", "Guitar", 48_000), Some(612));
    app.engine.execute("setup.io_map", &serde_json::json!({"action": "remove_path", "name": "In 2"})).unwrap();
    app.sync_io();
    assert_eq!(serde_json::to_string(&app.ui.io_maps[0]).unwrap(), read, "the old record is written back as it was read");
    assert_eq!(app.ui.input_maps.len(), 1);
    assert!(app.ui.input_maps[0].paths.iter().all(|p| p.name != "In 2"), "the edit is the input record's");
}

/// Rule 8 and 9: a failed input open makes no record and changes none; the composed table then has
/// no input half (the page says why), and the saved record is untouched.
#[test]
fn a_failed_input_open_changes_no_record() {
    let driver = asio(2);
    let engine = transit_engine::Engine::default();
    let session = engine.session_arc();
    let player = Player::open_fake_native_asio(Arc::clone(&session), &asio_config(), &driver).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.sync_io();
    app.engine.execute("setup.io_map", &serde_json::json!({"action": "remove_path", "name": "In 2"})).unwrap();
    app.sync_io();
    let before = app.ui.input_maps.clone();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].defaults_for, 2);
    // The next open's input fails (the capture opens first, and its failure is soft).
    driver.fail_next_open(ab::OpenError::Other("the input is busy".into()));
    let failed = Player::open_fake_native_asio(Arc::clone(&session), &asio_config(), &driver).unwrap();
    assert!(!failed.holds_input());
    reopen(&mut app, failed);
    assert_eq!(app.ui.input_maps, before, "no record changed, defaults_for included");
    assert_eq!((app.engine.io_map().inputs, inputs(app.engine.io_map()).len()), (0, 0));
    assert!(app.engine.io_map().input_device.is_empty());
    assert_eq!(app.ui.io_maps[0].inputs, 0, "the output record has no input half");
}

/// #682 on JACK: the input is another port group; its record is keyed by that group, and the
/// output record by the output group.
#[test]
fn a_jack_input_group_has_its_own_record() {
    let client = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32))
        .named("JACK", "HDMI")
        .with_capture(0, true)
        .with_capture_device("USB Mic", 1);
    let cfg = AudioConfig { backend: Backend::NativeJack, system: Some("JACK".into()), input: Some("USB Mic".into()), ..AudioConfig::default() };
    let engine = transit_engine::Engine::default();
    let player = Player::open_fake_native_jack(engine.session_arc(), &cfg, &client).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.sync_io();
    assert_eq!(app.engine.io_map().key(), ("JACK", "HDMI"));
    assert_eq!((app.engine.io_map().input_device.as_str(), app.engine.io_map().inputs), ("USB Mic", 1));
    assert_eq!(inputs(app.engine.io_map()), ["In 1"]);
    assert_eq!(app.ui.input_maps.iter().map(InputMap::key).collect::<Vec<_>>(), [("JACK", "USB Mic")]);
}

/// A separately opened input (cpal: ALSA) with a saved "Guitar" on channel 5.
fn separate_input_app(inputs: u16) -> (StudioApp, ab::fake::FakeBackend) {
    let driver = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).named("ALSA", "pipewire").with_capture(inputs, false);
    let cfg = AudioConfig { system: Some("ALSA".into()), output: Some("pipewire".into()), ..AudioConfig::default() };
    let engine = transit_engine::Engine::default();
    let player = Player::open_fake_native_asio(engine.session_arc(), &cfg, &driver).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.ui.audio = cfg;
    app.ui.input_maps.push(InputMap {
        system: "ALSA".into(),
        device: "pipewire".into(),
        inputs: 8,
        defaults_for: 8,
        paths: vec![IoPath { direction: IoDirection::Input, ..out("Guitar", vec![4]) }],
        ports: Vec::new(),
    });
    app.sync_io();
    (app, driver)
}

/// Review of 652509f3, must-fix 3: opening the input composes its saved table before capture is
/// attached, so a take planned at once (no frame, no `sync_io` in between) reads the saved path.
#[test]
fn a_take_planned_right_after_the_input_opens_reads_its_saved_path() {
    let (mut app, _driver) = separate_input_app(8);
    assert!(app.engine.io_map().input_path("Guitar").is_none(), "no input yet");
    let t = app.engine.session_mut().add_track(transit_model::TrackKind::Audio, ChannelFormat::Mono, Some("Gtr"));
    let track = app.engine.session_mut().track_mut(t).unwrap();
    track.mixer.input = transit_model::Route::Hardware("Guitar".into());
    track.mixer.record_arm = true;
    app.ensure_input();
    let r = app.recorder.as_ref().expect("the input opened");
    assert_eq!(app.engine.io_map().available_input("Guitar").map(|p| p.channels.clone()), Some(vec![4]), "composed before any frame");
    assert_eq!(app.player.as_ref().unwrap().io_map().input_path("Guitar").map(|p| p.channels.clone()), Some(vec![4]), "and the plan has it");
    assert_eq!(transit_engine::io::input_refusal(&app.engine, r.channels, r.system_channel()), None);
    let dir = transit_test_support::TestDir::new("take-after-open");
    let plan = transit_engine::io::plan_takes_with(&app.engine, &dir, r.channels, r.system_channel());
    assert_eq!(plan[0].1, [4], "channel 5, never channel 1");
}

/// Must-fix 3 across a reopen: the new device's table is composed with its input before that input
/// is attached (as Reconnect does: replace the player, mark the table for adoption, take the input).
#[test]
fn after_a_reopen_the_input_is_attached_with_its_composed_table() {
    let (mut app, driver) = separate_input_app(8);
    app.ensure_input();
    app.close_input();
    let fresh = Player::open_fake_native_asio(app.engine.session_arc(), &app.ui.audio, &driver).unwrap();
    app.player = Some(fresh);
    app.io_adopt = true;
    app.ensure_input();
    assert!(app.recorder.is_some());
    assert!(app.player.as_ref().unwrap().io_map().available_input("Guitar").is_some(), "the new plan has the saved path when capture attaches");
    assert!(!app.io_adopt, "adopted with the input");
}

/// First-record admission for a separate input: its first open makes its record (keyed by its own
/// identity) with the defaults for its width; the output record keeps no input half.
#[test]
fn a_separate_inputs_first_open_makes_its_record() {
    let driver = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).named("ALSA", "pipewire").with_capture(2, false);
    let cfg = AudioConfig { system: Some("ALSA".into()), output: Some("pipewire".into()), ..AudioConfig::default() };
    let engine = transit_engine::Engine::default();
    let player = Player::open_fake_native_asio(engine.session_arc(), &cfg, &driver).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.ui.audio = cfg;
    app.sync_io();
    assert!(app.ui.input_maps.is_empty(), "no input yet, no record");
    app.ensure_input();
    let keys: Vec<(&str, &str)> = app.ui.input_maps.iter().map(InputMap::key).collect();
    assert_eq!(keys, [("ALSA", "pipewire")]);
    assert_eq!(app.ui.input_maps[0].defaults_for, 2);
    assert_eq!(inputs(app.engine.io_map()), ["In 1", "In 2", "In 1-2"]);
    assert_eq!(app.ui.io_maps[0].inputs, 0);
}

/// Must-fix 4 in the app: an input without an identity (two devices of one name) gets this open's
/// defaults, reads no saved record, writes none, and no measurement applies through it.
#[test]
fn an_input_without_an_identity_saves_nothing_and_takes_no_measurement() {
    let endpoint = wasapi();
    endpoint.pair_capture(Some(("", "USB Audio")));
    let engine = transit_engine::Engine::default();
    let player = Player::open_fake_native_wasapi(engine.session_arc(), &wasapi_config(), &endpoint).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.ui.input_maps.push(InputMap {
        system: "WASAPI".into(),
        device: String::new(),
        inputs: 2,
        defaults_for: 2,
        paths: Vec::new(),
        ports: Vec::new(),
    });
    app.sync_io();
    assert_eq!(inputs(app.engine.io_map()), ["In 1", "In 2", "In 1-2"], "this open's defaults, not a saved empty record");
    assert!(!app.engine.store_hardware_latency("Out 1-2", "In 1", 500, 48_000), "nothing is stored without an identity");
    app.sync_io();
    assert_eq!(app.engine.io_map().hardware_latency("Out 1-2", "In 1", 48_000), None, "no measurement applies without an identity");
    assert_eq!(app.ui.input_maps.len(), 1, "nothing written for it");
    assert!(app.ui.input_maps[0].paths.is_empty(), "the old record untouched");
}

/// Review of 5e15a5d7, must-fix: no measurement is taken or stored through an input without an
/// identity (two connected inputs share its name). An existing figure for the same pair and rate is
/// not replaced; when one device leaves and the name is unique again, no figure from the ambiguous
/// time applies to the one that stays.
#[test]
fn no_measurement_is_stored_through_an_input_without_an_identity() {
    let endpoint = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).named("WASAPI", "USB Audio").with_capture(2, true);
    let cfg = wasapi_config();
    let engine = transit_engine::Engine::default();
    let session = engine.session_arc();
    endpoint.pair_capture(Some(("", "USB Audio")));
    let player = Player::open_fake_native_wasapi(std::sync::Arc::clone(&session), &cfg, &endpoint).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    let other = transit_model::HardwareLatency {
        send: "Out 1-2".into(),
        ret: "In 1".into(),
        round_trip_frames: 300,
        rate: 48_000,
        measured_at: 1,
        input_device: "Other Mic".into(),
        input_ports: Vec::new(),
        device_rate: 0,
    };
    app.ui.io_maps.push({
        let mut record = IoMap::for_device("WASAPI", "USB Audio", 0, 2);
        record.hardware_latency.push(other.clone());
        record
    });
    app.sync_io();
    assert!(app.engine.io_map().input_device.is_empty(), "two inputs of one name: no identity");
    let refused = app.engine.hardware_measurement("Out 1-2", "In 1");
    assert!(matches!(&refused, Err(transit_engine::EngineError::BadParams(_, why)) if why == transit_engine::NO_INPUT_IDENTITY), "{refused:?}");
    assert!(!app.engine.store_hardware_latency("Out 1-2", "In 1", 500, 48_000));
    app.sync_io();
    assert_eq!(app.engine.io_map().hardware_latency, std::slice::from_ref(&other), "the existing figure is not replaced");
    assert_eq!(app.ui.io_maps[0].hardware_latency, [other], "nor in the saved record");
    // One device leaves: the name is unique, the input has its identity, and nothing measured
    // while it was ambiguous applies.
    endpoint.pair_capture(Some(("USB Audio", "USB Audio")));
    reopen(&mut app, Player::open_fake_native_wasapi(session, &cfg, &endpoint).unwrap());
    assert_eq!(app.engine.io_map().input_device, "USB Audio");
    assert_eq!(app.engine.io_map().hardware_latency("Out 1-2", "In 1", 48_000), None);
}

/// Slice A with slice B on one reconnect over native JACK: the links other programs made are
/// snapshotted before the old client closes and restored on the new one, and the reopened input is
/// attached only with its saved table composed (Guitar on its channel), with no frame between.
#[test]
fn a_jack_reconnect_restores_the_links_and_attaches_the_input_with_its_table() {
    let client = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32))
        .named("JACK", "HDMI")
        .with_capture(0, true)
        .with_capture_device("USB Mic", 2);
    let cfg = AudioConfig { backend: Backend::NativeJack, system: Some("JACK".into()), input: Some("USB Mic".into()), ..AudioConfig::default() };
    let engine = transit_engine::Engine::default();
    let player = Player::open_fake_native_jack(engine.session_arc(), &cfg, &client).unwrap();
    let mut app = StudioApp::new(engine, Some(player), crate::Services::default());
    app.ui.audio = cfg.clone();
    app.ui.input_maps.push(InputMap {
        system: "JACK".into(),
        device: "USB Mic".into(),
        inputs: 2,
        defaults_for: 2,
        paths: vec![IoPath { direction: IoDirection::Input, ..out("Guitar", vec![1]) }],
        ports: Vec::new(),
    });
    let t = app.engine.session_mut().add_track(transit_model::TrackKind::Audio, ChannelFormat::Mono, Some("Gtr"));
    let track = app.engine.session_mut().track_mut(t).unwrap();
    track.mixer.input = transit_model::Route::Hardware("Guitar".into());
    track.mixer.input_monitor = true;
    app.sync_io();
    app.ensure_input();
    client.set_links(&["player:out_l -> transit:in_1"]);
    let reopened = app.reconnect_audio_using(false, |session, cfg| Player::open_fake_native_jack(session, cfg, &client).unwrap());
    assert_eq!(reopened, Ok(true));
    assert_eq!(client.restored_links(), ["player:out_l -> transit:in_1"], "slice B: the link came back on the new client");
    assert!(app.recorder.is_some(), "the input reopened for the monitored track");
    let plan = app.player.as_ref().unwrap().io_map();
    assert_eq!(plan.input_device, "USB Mic");
    assert_eq!(plan.available_input("Guitar").map(|p| p.channels.clone()), Some(vec![1]), "slice A: composed before capture attached");
    assert!(!app.io_adopt);
}
