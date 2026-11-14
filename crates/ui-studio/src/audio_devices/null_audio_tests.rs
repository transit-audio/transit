//! `--null-audio`: the real Player on the silent device, paced by a stepped clock. A hosted plug-in
//! gets its instance and its editor is offered, as on a real device; the machine's audio devices are
//! never listed; the window offers no switching, Apply only saves, and a reopen is the silent device
//! again. The control (`null_audio` off with a player) is the existing #363 test's second half.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use transit_dsp::{Plugin, PluginEditor};
use transit_playback::backend::null::{self, StepClock};
use transit_playback::{Player, PluginTestHooks};
use transit_plugin_host::{Config, Host};

/// One silent-device block (512 frames at 48 kHz), rounded up to the nanosecond.
const BLOCK: Duration = Duration::from_nanos(10_666_667);

/// The built-in gain with an editor that has a window.
struct WithEditor(Box<dyn Plugin>);

impl Plugin for WithEditor {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.0.info()
    }
    fn prepare(&mut self, sr: f32, max: usize, ch: usize) {
        self.0.prepare(sr, max, ch);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.0.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.0.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.0.process(io, frames);
    }
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        Some(Box::new(Window(false)))
    }
}

struct Window(bool);

impl PluginEditor for Window {
    fn has_gui(&self) -> bool {
        true
    }
    fn open(&mut self) -> Result<(), String> {
        self.0 = true;
        Ok(())
    }
    fn close(&mut self) {
        self.0 = false;
    }
    fn is_open(&self) -> bool {
        self.0
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
}

/// A `--null-audio` app over a stepped silent device, no stand-in devices, with a machine lister
/// that counts each time it is asked.
fn null_app() -> (T, Arc<StepClock>, Arc<AtomicUsize>) {
    let engine = transit_engine::Engine::default();
    let clock = StepClock::new();
    let player = Player::with_null_device_clock(engine.session_arc(), Arc::clone(&clock) as Arc<dyn null::PaceClock>);
    clock.settle();
    let mut app = StudioApp::new(engine, Some(player), Services::default());
    app.null_audio = true;
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    let wasapi = transit_playback::devices::AudioSystem {
        name: "WASAPI".into(),
        is_default: true,
        outputs: Vec::new(),
        inputs: Vec::new(),
        diagnostics: Vec::new(),
    };
    app.audio_devices.machine = Some((
        vec![wasapi],
        Arc::new(move |_: Option<String>| {
            counted.fetch_add(1, Ordering::Relaxed);
            None
        }),
    ));
    app.diag.machine = Some(crate::diagnostics::Machine::default());
    let t = T { app, ctx: egui::Context::default(), time: 0.0, screen: egui::vec2(1400.0, 800.0), events: Vec::new() };
    (t, clock, asked)
}

/// `n` frames, the silent device one block further at each.
fn run(t: &mut T, clock: &StepClock, n: usize) {
    for _ in 0..n {
        clock.advance(BLOCK);
        t.frames(1);
    }
}

#[test]
fn a_null_audio_run_hosts_a_plugin_with_its_editor_and_lists_no_machine_device() {
    let (mut t, clock, asked) = null_app();
    let now = Instant::now();
    t.app.player.as_ref().unwrap().set_plugin_test_hooks(PluginTestHooks {
        host: Host::new(Config { inline: true, ..Config::default() }),
        factory: Arc::new(|spec: &transit_mix::InstanceSpec| {
            transit_mix::create_instance("gain", None, 48_000.0, 512, spec.ch)
                .map(|n| transit_mix::NewInstance { plugin: Box::new(WithEditor(n.plugin)), ..n })
        }),
        deferred: Arc::new(|id: &str| id == "clap:test.null-audio"),
        clock: Arc::new(move || now),
        slow_after: Duration::from_secs(10),
    });
    let r = t.app.run("track.new", json!({"kind": "audio"})).unwrap();
    let track = transit_model::TrackId(r["tracks"][0].as_u64().unwrap());
    t.app.engine.session_mut().track_mut(track).unwrap().mixer.inserts[0] = Some(transit_model::Insert::new("clap:test.null-audio"));
    t.app.focus(track);
    t.app.ui.show_mixer = true;
    t.app.ui.show_devices = true;
    run(&mut t, &clock, 8);
    let player = t.app.player.as_ref().unwrap();
    assert!(player.null_device() && !player.silent);
    assert!(player.live_name(track, 0).is_some(), "the hosted plug-in has its instance");
    assert!(player.has_editor(track, 0) && player.has_gui(track, 0), "its editor is there, with a window");
    assert!(t.app.mixer.devices.cards[0].editor_enabled, "Edit is offered, as on a real device");

    // Audio Devices: the silent device shows, nothing of the machine is listed, nothing switches.
    t.open_window();
    for key in ["section_io", "section_hardware", "section_system"] {
        t.section(key);
    }
    t.click(mid(t.control_rect("rescan")));
    t.click(mid(t.control_rect("copy_diagnostics")));
    run(&mut t, &clock, 10);
    assert_eq!(t.app.audio_devices.machine_listings.load(Ordering::Relaxed), 0, "no listing passed the gate");
    assert_eq!(asked.load(Ordering::Relaxed), 0, "the ASIO lister was never asked");
    assert_eq!(t.layout()["systems"], json!([]), "nothing listed");
    let opened = t.layout()["opened"].to_string();
    assert!(opened.contains("Output open: No hardware (silent) · 48000 Hz · 2 ch · 512-sample buffer"), "{opened}");
    assert!(t.app.diag.copied.contains(crate::diagnostics::NULL_AUDIO_DEVICES), "the report says why: {}", t.app.diag.copied);
}

#[test]
fn a_null_audio_run_saves_device_choices_and_reopens_only_the_silent_device() {
    let (mut t, clock, asked) = null_app();
    run(&mut t, &clock, 3);
    let speakers = AudioConfig { output: Some("Speakers (Realtek)".into()), ..AudioConfig::default() };
    t.app.apply_audio(speakers.clone()).unwrap();
    assert_eq!(t.app.status, crate::audio_devices::NULL_AUDIO_SAVED);
    assert_eq!(t.app.ui.audio, speakers, "the choice is saved for the next start");
    assert!(t.app.player.as_ref().unwrap().null_device(), "nothing reopened");
    // Audio › Reconnect Device: the replacement Player is the silent device again (the real-time
    // one; nothing here waits on its pace), whatever the saved choices name.
    t.app.reconnect_audio();
    let player = t.app.player.as_ref().unwrap();
    assert!(player.null_device() && !player.silent, "{:?}", player.initial_error());
    assert_eq!(player.device_name, null::DEVICE);
    assert!(player.open_input(&t.app.ui.audio).is_err_and(|e| e == null::NO_INPUT), "recording is unavailable, and says so");
    t.frames(5);
    assert_eq!(t.app.audio_devices.machine_listings.load(Ordering::Relaxed), 0);
    assert_eq!(asked.load(Ordering::Relaxed), 0);
}
