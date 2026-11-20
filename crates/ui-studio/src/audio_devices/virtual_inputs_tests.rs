//! ADR 0015, amendment for #431, slice C in Audio Devices: Show virtual devices (rule 1), the
//! chosen input always shown and an absent one said by name (rules 3 and 4), the timing warning
//! (rule 7), the monitor label (rule 13), and Use the current ports on the I/O paths page (rule
//! 5). Driven like a user over stand-in devices and scripted JACK clients; no device is opened.

use super::*;
use crate::audio_devices::jack_inputs::{self, FEEDBACK_HOVER, SHOW_VIRTUAL, TIMING_WARNING, VIRTUAL_NOTE};
use transit_audio_backend as ab;
use transit_playback::Player;

/// A PipeWire desktop as the native listing gives it: two physical inputs, and the virtual
/// devices (a browser's stream and built-in audio's monitor, which also plays physically).
fn jack_listing() -> Value {
    json!({"systems": [{"name": "JACK", "is_default": true,
        "outputs": [{"name": "Headset", "is_default": true, "max_channels": 2}, {"name": "Built-in Audio", "max_channels": 2}],
        "inputs": [{"name": "Headset", "max_channels": 2}, {"name": "Mic", "max_channels": 1}],
        "virtual_inputs": [{"name": "Firefox", "max_channels": 2, "channel_names": ["output_FL", "output_FR"]},
                           {"name": "Built-in Audio", "max_channels": 2, "channel_names": ["monitor_FL", "monitor_FR"]}]}]})
}

fn jack_window(input: Option<&str>, show: bool) -> T {
    let mut t = T::new();
    t.control("audio.fake_devices", jack_listing()).unwrap();
    t.app.ui.audio = AudioConfig {
        system: Some("JACK".into()),
        output: Some("Headset".into()),
        input: input.map(str::to_owned),
        jack_show_virtual: show,
        ..Default::default()
    };
    t.open_window();
    t
}

/// The labels the Input list offers (after opening it).
fn offered(t: &mut T) -> Vec<String> {
    t.click(mid(t.control_rect("input")));
    let items = t.layout()["items"].clone();
    let labels = items.as_array().unwrap().iter().filter(|m| m["value"] == "input").map(|m| m["label"].as_str().unwrap().to_owned()).collect();
    t.click(mid(t.control_rect("input")));
    labels
}

fn control_text(t: &T, key: &str) -> String {
    let controls = t.layout()["controls"].clone();
    controls.as_array().unwrap().iter().find(|m| m["value"] == key).map(|m| m["label"].as_str().unwrap().to_owned()).unwrap_or_default()
}

/// Rule 1: off by default, today's list; on, the virtual devices too. The checkbox takes effect at
/// once and is kept with the preferences, with no pending change and no reopen.
#[test]
fn show_virtual_devices_adds_the_virtual_devices_to_the_input_list_only_when_on() {
    let mut t = jack_window(None, false);
    assert!(!AudioConfig::default().jack_show_virtual, "off by default");
    assert_eq!(control_text(&t, "jack_show_virtual"), SHOW_VIRTUAL);
    assert_eq!(control_text(&t, "jack_virtual_note"), VIRTUAL_NOTE);
    assert_eq!(control_text(&t, "jack_timing_warning"), TIMING_WARNING);
    assert_eq!(offered(&mut t), ["System default", "Headset · 2 ch", "Mic · 1 ch"], "today's list");
    t.click(mid(t.control_rect("jack_show_virtual")));
    assert!(t.app.ui.audio.jack_show_virtual, "kept at once");
    assert!(t.layout()["pending"].is_null(), "not a pending change");
    assert!(t.reopened().is_empty(), "nothing reopened");
    assert_eq!(
        offered(&mut t),
        ["System default", "Headset · 2 ch", "Mic · 1 ch", "Firefox · 2 ch", "System Audio (monitor of Built-in Audio) · 2 ch"]
    );
    assert_eq!(serde_json::to_value(&t.app.ui).unwrap()["audio"]["jack_show_virtual"], true, "saved with the preferences");
    t.click(mid(t.control_rect("jack_show_virtual")));
    assert_eq!(offered(&mut t), ["System default", "Headset · 2 ch", "Mic · 1 ch"]);
    // Choosing one is an ordinary input choice, applied by Apply.
    t.click(mid(t.control_rect("jack_show_virtual")));
    t.pick("input", "Firefox · 2 ch");
    assert_eq!(t.app.audio_devices.draft.input.as_deref(), Some("Firefox"));
    assert!(!t.layout()["pending"].is_null(), "an input choice waits for Apply");
}

/// Rule 3: the chosen input is shown under its own name with the preference off, and a chosen
/// input that is not present says so, hardware and virtual alike.
#[test]
fn the_chosen_input_is_always_shown_and_an_absent_one_says_no_input_opens() {
    let t = jack_window(Some("Firefox"), false);
    assert_eq!(control_text(&t, "input"), "Firefox · 2 ch", "shown with the preference off");
    let mut t = t;
    assert!(offered(&mut t).contains(&"Firefox · 2 ch".to_owned()), "the active choice is never hidden");
    for absent in ["OBS", "Scarlett 2i2"] {
        let t = jack_window(Some(absent), true);
        assert_eq!(control_text(&t, "input"), format!("{absent} (not present: no input opens)"));
    }
}

/// Rule 13 and rule 7: a monitor group gets the System Audio label, with ", transit's output" and
/// the feedback hover when it monitors the output transit plays to; every virtual device carries
/// the timing warning on its hover.
#[test]
fn the_monitor_label_names_transits_output_and_virtual_devices_carry_the_warning() {
    let mut t = jack_window(None, true);
    t.pick("output", "Built-in Audio · 2 ch");
    assert!(offered(&mut t).contains(&"System Audio (monitor of Built-in Audio), transit's output · 2 ch".to_owned()));
    let listing = t.app.audio_devices.systems.clone().unwrap();
    let jack = listing.iter().find(|s| s.name == "JACK").unwrap();
    let draft = AudioConfig { jack_show_virtual: true, ..Default::default() };
    let entries = jack_inputs::entries(jack, &draft, Some("Built-in Audio"));
    let monitor = entries.iter().find(|e| e.name == "Built-in Audio").unwrap();
    assert_eq!(monitor.hover.as_deref(), Some(format!("{TIMING_WARNING} {FEEDBACK_HOVER}").as_str()));
    assert_eq!(entries.iter().find(|e| e.name == "Firefox").unwrap().hover.as_deref(), Some(TIMING_WARNING));
    assert_eq!(entries.iter().find(|e| e.name == "Mic").unwrap().hover, None, "a physical group carries no warning");
    let other = jack_inputs::entries(jack, &draft, Some("Headset"));
    assert_eq!(other.iter().find(|e| e.name == "Built-in Audio").unwrap().label, "System Audio (monitor of Built-in Audio) · 2 ch");
}

/// The preference through the control channel: `ui.set {audio: {jack_show_virtual}}` sets it,
/// the reply reads it back, and it reopens nothing.
#[test]
fn the_preference_is_set_and_read_through_the_control_channel_without_a_reopen() {
    let mut t = T::new();
    let reply = t.control("ui.set", json!({"audio": {"jack_show_virtual": true}})).unwrap();
    assert_eq!(reply["ui"]["audio"]["jack_show_virtual"], true);
    assert!(t.app.ui.audio.jack_show_virtual);
    assert!(t.reopened().is_empty(), "a listing preference reopens nothing");
    t.control("ui.set", json!({"audio": {"jack_show_virtual": false, "input": SCARLETT_IN}})).unwrap();
    assert_eq!(t.reopened().len(), 1, "another choice in the same patch still reopens");
}

/// A JACK-like client: the output group "Headset" (no inputs of its own) and the virtual device
/// "Firefox" with `ports`.
fn client(ports: &[&str]) -> ab::fake::FakeBackend {
    ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32))
        .named("JACK", "Headset")
        .with_capture(0, true)
        .with_capture_device("Mic", 1)
        .with_virtual_device("Firefox", ports)
}

fn jack_cfg(input: &str) -> AudioConfig {
    AudioConfig {
        backend: Backend::NativeJack,
        system: Some("JACK".into()),
        output: Some("Headset".into()),
        input: Some(input.into()),
        jack_show_virtual: true,
        ..Default::default()
    }
}

/// Opens the client's player for `cfg` in the app and composes its input table, as an open does.
fn reopen_on(t: &mut T, client: &ab::fake::FakeBackend, cfg: &AudioConfig) {
    let player = Player::open_fake_native_jack(t.app.engine.session_arc(), cfg, client).unwrap();
    t.app.player = Some(player);
    t.app.ui.audio = cfg.clone();
    t.app.io_adopt = true;
    t.frames(2);
}

/// Rule 4 in the app: a chosen input that is absent opens no input, and the app says so by name.
#[test]
fn an_absent_chosen_input_opens_no_input_and_the_app_names_it() {
    let mut t = T::new();
    t.control("audio.fake_devices", jack_listing()).unwrap();
    let cfg = jack_cfg("OBS");
    reopen_on(&mut t, &client(&["output_FL", "output_FR"]), &cfg);
    t.app.audio_devices.fake = None;
    t.app.ensure_input();
    let failure = t.app.input_failure.clone().unwrap_or_default();
    assert!(failure.contains(&ab::absent_input("OBS")), "{failure}");
    assert!(t.app.status.contains("The chosen input \"OBS\" is not present; no input opened. Use Reconnect when it is back."), "{}", t.app.status);
    assert_eq!(t.app.engine.io_map().inputs, 0, "no other group opened");
}

/// Rule 5 on the I/O paths page: the first open makes the baseline; the same device back in
/// another order shows the warning and Use the current ports, whose paths are unavailable with
/// why; the action does nothing while recording and, stopped, makes the current order the
/// baseline (saved with the input record), after which the action is gone.
#[test]
fn use_the_current_ports_shows_only_on_a_mismatch_and_works_only_while_stopped() {
    let mut t = T::new();
    let jack = client(&["output_FL", "output_FR"]);
    reopen_on(&mut t, &jack, &jack_cfg("Firefox"));
    let record = |t: &T| t.app.ui.input_maps.iter().find(|r| r.device == "Firefox").map(|r| r.ports.clone()).unwrap_or_default();
    assert_eq!(record(&t), ["output_FL", "output_FR"], "the first creation sets the baseline");
    t.open_window();
    t.section("section_io");
    assert_eq!(control_text(&t, "io_use_ports"), "", "no mismatch, no action");
    jack.set_input_ports("Firefox", Some(&["output_FR", "output_FL"]));
    reopen_on(&mut t, &jack, &jack_cfg("Firefox"));
    assert_eq!(record(&t), ["output_FL", "output_FR"], "a reopen never replaces the baseline");
    assert_eq!(control_text(&t, "io_use_ports"), jack_inputs::USE_CURRENT_PORTS);
    assert_eq!(control_text(&t, "io_ports_changed"), jack_inputs::PORTS_CHANGED);
    assert_eq!(control_text(&t, "io_ports_detail"), "Paths read: output_FL, output_FR. Open now: output_FR, output_FL.");
    assert!(control_text(&t, "io_unavailable:In 1-2").contains("ports changed"), "{}", control_text(&t, "io_unavailable:In 1-2"));
    t.app.engine.transport.recording = true;
    t.click(mid(t.control_rect("io_use_ports")));
    assert!(t.app.engine.io_map().ports_mismatch(), "disabled while recording");
    t.app.engine.transport.recording = false;
    t.click(mid(t.control_rect("io_use_ports")));
    assert!(!t.app.engine.io_map().ports_mismatch());
    assert!(t.app.engine.io_map().available_input("In 1-2").is_some());
    assert_eq!(record(&t), ["output_FR", "output_FL"], "the new baseline is kept with the record");
    assert_eq!(control_text(&t, "io_use_ports"), "", "the action is gone");
}

/// Rule 13: a JACK loopback calibration measured through a virtual input with one port order is
/// not applied after the order changes, and the notes and the status line say "measure again".
#[test]
fn a_calibration_through_a_virtual_input_says_measure_again_after_its_ports_changed() {
    let mut t = T::new();
    t.control("audio.fake_devices", jack_listing()).unwrap();
    let jack = client(&["output_FL", "output_FR"]);
    let first = Player::open_fake_native_jack(t.app.engine.session_arc(), &jack_cfg("Firefox"), &jack).unwrap();
    let record = first.calibration_key.as_ref().expect("a key").record(480, 25, "2026-11-20");
    drop(first);
    jack.set_input_ports("Firefox", Some(&["output_FR", "output_FL"]));
    let cfg = AudioConfig { calibrations: vec![record], ..jack_cfg("Firefox") };
    let player = Player::open_fake_native_jack(t.app.engine.session_arc(), &cfg, &jack).unwrap();
    assert_eq!(player.calibration, None, "no stale correction");
    t.app.player = Some(player);
    t.app.ui.audio = cfg;
    t.frames(2);
    let note = "The loopback calibration of “Headset” was measured through other ports of input “Firefox”: measure again.";
    assert_eq!(t.app.audio_devices.notes, [note], "and no input note: Firefox is listed, as a virtual device");
    assert_eq!(t.app.status, note);
}

/// Rules 2 and 4 in the device notes: on JACK a chosen virtual device counts as listed, and one
/// that is not listed says no input opens (never that the default records instead).
#[test]
fn the_jack_input_note_counts_virtual_devices_and_says_no_input_opens() {
    let t = jack_window(None, false);
    let systems = t.app.audio_devices.systems.clone().unwrap();
    let cfg = |input: &str| AudioConfig { system: Some("JACK".into()), input: Some(input.into()), ..Default::default() };
    assert!(fallback_notes(&cfg("Firefox"), Some(&systems), &Opened::default()).is_empty(), "a virtual device is listed");
    assert_eq!(fallback_notes(&cfg("OBS"), Some(&systems), &Opened::default()), ["Input “OBS” is not present; no input opens."]);
}
