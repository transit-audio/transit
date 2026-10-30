//! Info panel and help catalog tests (#178). Driven like a user: key presses and pointer moves
//! through the synthetic input queue, one frame at a time; assertions read what the panel shows
//! (`app.help.layout`) and what the app did, never how it drew it.

use crate::help::{self, Id, map};
use crate::{Services, StudioApp};
use egui::{Event, Key, Modifiers};
use serde_json::{Value, json};
use transit_playback::telemetry::{CallbackTelemetry, StatsBackend, StreamStats};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
}

fn mid(r: [f32; 4]) -> egui::Pos2 {
    egui::pos2((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

impl T {
    fn new() -> T {
        let app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0 };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                time: Some(self.time),
                ..Default::default()
            };
            let mut raw = raw;
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            out.textures_delta.clear();
        }
    }

    fn control(&mut self, method: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(method, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(10);
    }

    fn click(&mut self, at: egui::Pos2) {
        self.control("ui.click", json!({"x": at.x, "y": at.y}));
    }

    fn point_at(&mut self, at: egui::Pos2) {
        self.control("ui.move", json!({"x": at.x, "y": at.y}));
    }

    /// Shift+? as a keyboard reports it.
    fn toggle_key(&mut self) {
        let modifiers = Modifiers::SHIFT;
        for pressed in [true, false] {
            self.app.synthetic.push(Event::Key { key: Key::Questionmark, physical_key: None, pressed, repeat: false, modifiers });
        }
        self.frames(5);
    }

    fn panel_on(&mut self) {
        if !self.app.help.open {
            self.toggle_key();
        }
        assert!(self.app.help.open);
    }

    /// Open the Callback Telemetry window over a snapshot with the render worker figures.
    fn telemetry(&mut self) {
        let figure = transit_mix::StripFigure {
            track: 0,
            overruns_busy: 1,
            overruns_late: 2,
            overruns_unstarted: 3,
            cost_ns: 50_000,
            worst_ns: 90_000,
            ..Default::default()
        };
        let mut faults =
            transit_mix::StripFaults { workers: 3, deadline_percent: 150, deadline_frames: 64, deadline_rate: 48_000, ..Default::default() };
        faults.top_strips[0] = Some(figure);
        faults.lateness[1] = 4;
        faults.wake[2] = 5;
        self.app.telemetry.injected = Some(CallbackTelemetry {
            callbacks: 100,
            timed_callbacks: 100,
            frames: 6400,
            callback_frames: 64,
            sample_rate: 48_000,
            over_budget_callbacks: 2,
            strip_faults: Some(faults),
            ..Default::default()
        });
        self.app.telemetry.open = true;
        self.frames(30);
    }

    fn target(&self, key: &str) -> egui::Pos2 {
        let r = self.app.help.layout.targets.get(key).copied();
        mid(r.unwrap_or_else(|| panic!("no help target {key}: {:?}", self.app.help.layout.targets.keys().collect::<Vec<_>>())))
    }
}

#[test]
fn every_id_has_an_entry_in_its_area_and_the_catalog_has_no_extras() {
    for id in Id::ALL {
        let entry = help::entry(*id).unwrap_or_else(|| panic!("{} has no entry", id.key()));
        assert!(!entry.name.trim().is_empty() && !entry.text.trim().is_empty(), "{}: needs a name and a text", id.key());
        let area = id.key().split_once('.').map(|(a, _)| a.replace('_', "-"));
        assert_eq!(help::area_of(*id), area.as_deref(), "{}: lives in the file of its area", id.key());
    }
    assert_eq!(help::keys().count(), Id::ALL.len(), "an entry without an id");
    let mut keys: Vec<&str> = Id::ALL.iter().map(|i| i.key()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), Id::ALL.len(), "a key is used by two ids");
}

#[test]
fn an_entrys_key_is_a_key_of_the_shortcut_table() {
    let table: Vec<String> = crate::shortcuts::TABLE.iter().map(crate::shortcuts::text).collect();
    for id in Id::ALL {
        let Some(keys) = help::entry(*id).and_then(|e| e.key.clone()) else { continue };
        for alternative in keys.split(" or ") {
            assert!(table.iter().any(|t| t == alternative), "{}: {alternative} is not in the shortcut table", id.key());
        }
    }
}

#[test]
fn the_toggle_is_shift_question_mark_and_is_listed_with_its_convention() {
    use crate::shortcuts::{Action, TABLE, label, text};
    assert_eq!(label(Action::ToggleInfo), "Shift+?");
    let rows: Vec<_> = TABLE.iter().filter(|s| s.action == Action::ToggleInfo).collect();
    assert_eq!(rows.len(), 2, "Shift+? and Shift+/");
    assert!(rows.iter().all(|s| s.note.contains("Ableton") || s.note.contains("same key")), "{:?}", rows.iter().map(|s| text(s)).collect::<Vec<_>>());
    assert!(TABLE.last().is_some_and(|s| s.action == Action::ToggleInfo), "the Info row ends the table");
}

#[test]
fn shift_question_mark_turns_the_panel_on_and_off_and_never_dirties_the_document() {
    let mut t = T::new();
    assert!(!t.app.help.open);
    let rev = (t.app.engine.is_dirty(), t.app.engine.revision);
    t.toggle_key();
    assert!(t.app.help.open);
    assert_eq!(t.app.help.layout.name, "", "nothing hovered yet");
    assert!(t.app.help.layout.rect[2] > t.app.help.layout.rect[0], "the panel is drawn");
    // Bottom-left: the panel's left edge is the window's, its bottom is above the status bar.
    assert!(t.app.help.layout.rect[0] < 40.0 && t.app.help.layout.rect[3] < 900.0 - crate::theme::STATUS_H + 1.0);
    t.toggle_key();
    assert!(!t.app.help.open);
    assert_eq!(t.app.help.layout.rect, [0.0; 4], "an off panel draws nothing");
    assert_eq!((t.app.engine.is_dirty(), t.app.engine.revision), rev);
}

#[test]
fn the_help_menu_item_toggles_the_panel() {
    let mut t = T::new();
    let item = |t: &T, label: &str| t.app.arrange.bar_menu.iter().find(|m| m.label == label).map(|m| m.rect).unwrap_or_else(|| panic!("no {label}"));
    t.click(mid(item(&t, "Help")));
    t.click(mid(item(&t, "Info Panel")));
    assert!(t.app.help.open, "Help > Info Panel turns it on");
    t.click(mid(item(&t, "Help")));
    t.click(mid(item(&t, "Info Panel")));
    assert!(!t.app.help.open, "and off");
}

#[test]
fn the_panel_shows_the_entry_of_the_hovered_telemetry_row() {
    let mut t = T::new();
    t.telemetry();
    t.panel_on();
    t.frames(3);
    for key in [
        "telemetry.load_now",
        "telemetry.over_budget",
        "telemetry.overruns_late",
        "telemetry.upstream_missed",
        "telemetry.strip_row",
        "telemetry.strip_lateness",
    ] {
        let at = t.target(key);
        t.point_at(at);
        let entry = Id::ALL.iter().find(|i| i.key() == key).and_then(|i| help::entry(*i)).expect("entry");
        assert_eq!(t.app.help.layout.id.as_deref(), Some(key), "hovering {key}");
        assert_eq!(t.app.help.layout.name, entry.name);
        assert_eq!(t.app.help.layout.text, entry.text);
    }
}

#[test]
fn the_panel_shows_the_key_of_a_transport_button_and_keeps_the_entry_when_the_pointer_leaves() {
    let mut t = T::new();
    t.panel_on();
    t.frames(3);
    let play = t.target("transport.play");
    t.point_at(play);
    assert_eq!(t.app.help.layout.id.as_deref(), Some("transport.play"));
    assert_eq!(t.app.help.layout.key.as_deref(), Some("Space"));
    // The pointer goes to empty space, then to the panel itself: the entry stays so More can be pressed.
    t.point_at(egui::pos2(700.0, 300.0));
    assert_eq!(t.app.help.layout.id.as_deref(), Some("transport.play"));
    let rect = t.app.help.layout.rect;
    t.point_at(mid(rect));
    assert_eq!(t.app.help.layout.id.as_deref(), Some("transport.play"));
}

#[test]
fn hovering_with_the_panel_off_shows_nothing_and_a_resting_pointer_is_explained_once_it_is_on() {
    let mut t = T::new();
    let play = mid(t.app.arrange.layout.transport["play"]);
    t.point_at(play);
    assert!(!t.app.help.open && t.app.help.layout.id.is_none() && t.app.help.layout.rect == [0.0; 4]);
    assert!(t.app.help.shown.is_none(), "an off panel remembers nothing");
    t.panel_on();
    t.frames(3);
    assert_eq!(t.app.help.layout.id.as_deref(), Some("transport.play"), "the pointer is still on Play");
}

#[test]
fn more_opens_the_docs_page_of_the_entry() {
    let mut t = T::new();
    t.telemetry();
    t.panel_on();
    t.frames(3);
    let at = t.target("telemetry.over_budget");
    t.point_at(at);
    let more = t.app.help.layout.more;
    assert!(more[2] > more[0], "an entry with a link has a More button");
    t.click(mid(more));
    let expected = help::entry(Id::TelemetryOverBudget).and_then(|e| e.url());
    assert_eq!(t.app.help.opened_url, expected);
    assert!(expected.is_some_and(|u| u.ends_with("docs/troubleshooting.md#callback-telemetry-figures")));
}

#[test]
fn the_question_mark_in_a_window_header_opens_the_panel_on_the_windows_entry() {
    let mut t = T::new();
    t.telemetry();
    assert!(!t.app.help.open);
    let mark = t.app.help.layout.question_marks.get("telemetry.window").copied().expect("a question mark in the Callback Telemetry window");
    t.click(mid(mark));
    assert!(t.app.help.open, "pressing it turns the panel on");
    assert_eq!(t.app.help.layout.id.as_deref(), Some("telemetry.window"));
    assert_eq!(t.app.help.layout.name, "Callback Telemetry");
}

#[test]
fn every_diagnostics_window_has_a_question_mark_for_its_entry() {
    let mut t = T::new();
    t.control(
        "audio.fake_devices",
        json!({"systems": [{"name": "WASAPI", "is_default": true, "outputs": [{"name": "Speakers", "is_default": true}], "inputs": []}]}),
    );
    t.app.audio_devices.open = true;
    t.app.telemetry.injected = Some(CallbackTelemetry::default());
    t.app.telemetry.open = true;
    t.app.run("window.midi_monitor", json!({"value": true})).expect("monitor");
    t.app.run("window.midi_mappings", json!({"value": true})).ok();
    t.frames(30);
    let marks: Vec<String> = t.app.help.layout.question_marks.keys().cloned().collect();
    for want in ["audio_devices.window", "telemetry.window", "midi_monitor.window"] {
        assert!(marks.iter().any(|m| m == want), "{want} missing from {marks:?}");
    }
}

#[test]
fn every_telemetry_row_has_help_but_the_histogram_total() {
    let mut t = T::new();
    t.telemetry();
    let snap = t.app.telemetry.snap;
    let strips = [crate::telemetry::StripName { track: "Bass".into(), plugin: Some("Serum: 2".into()) }];
    let rows = crate::telemetry::detail_rows_named(&snap, &strips);
    let missing: Vec<&String> = rows.iter().skip(snap.duration_histogram.len() + 1).filter(|r| map::telemetry_row(r).is_none()).collect();
    assert!(missing.is_empty(), "rows with no help: {missing:?}");
    assert!(rows.iter().any(|r| map::telemetry_row(r) == Some(Id::TelemetryStripRow)), "a per-strip row maps to the strip entry: {rows:?}");
    assert_eq!(map::telemetry_row(&rows[snap.duration_histogram.len()]), None, "Histogram total");
}

fn joined(line: &str) -> String {
    map::status_segments(line).into_iter().map(|s| s.text).collect()
}

fn ids_of(line: &str) -> Vec<Id> {
    map::status_segments(line).into_iter().filter_map(|s| s.id).collect()
}

fn stats() -> StreamStats {
    StreamStats {
        wakes: 100,
        max_wake_late_us: 1172,
        max_service_us: 700,
        skipped_wakes: 1,
        timeouts: 2,
        min_padding_frames: 5,
        max_packets_per_wake: 3,
        faults: 1,
        overloads: 4,
        ..Default::default()
    }
}

#[test]
fn backend_lines_split_into_figures_and_join_back_to_the_line() {
    let s = stats();
    let lines = [
        crate::telemetry::backend_line(&s, false, Some(375), StatsBackend::Wasapi),
        crate::telemetry::backend_line(&s, true, None, StatsBackend::Wasapi),
        crate::telemetry::backend_line(&s, false, Some(375), StatsBackend::Asio),
        crate::telemetry::backend_line(&s, false, Some(375), StatsBackend::Jack),
        format!("Backend (output / input, one ASIO driver): {}", StreamStats::asio_duplex_line(&s, &s, Some(("375/s", "375/s")))),
    ];
    for line in &lines {
        assert_eq!(&joined(line), line, "the pieces join back to the line");
        let segments = map::status_segments(line);
        assert!(segments.iter().all(|s| s.id.is_some()), "every piece of {line} has an entry: {segments:?}");
    }
    let wasapi_render = ids_of(&lines[0]);
    for id in [
        Id::AudioDevicesBackend,
        Id::AudioDevicesWakes,
        Id::AudioDevicesGapOverPeriod,
        Id::AudioDevicesService,
        Id::AudioDevicesSkipped,
        Id::AudioDevicesTimeouts,
        Id::AudioDevicesMinPadding,
        Id::AudioDevicesFaults,
    ] {
        assert!(wasapi_render.contains(&id), "{id:?} in {wasapi_render:?}");
    }
    assert!(ids_of(&lines[1]).contains(&Id::AudioDevicesLate) && ids_of(&lines[1]).contains(&Id::AudioDevicesMaxPackets));
    for id in [Id::AudioDevicesOverloads, Id::AudioDevicesBusySkips, Id::AudioDevicesPositionJumps, Id::AudioDevicesResets] {
        assert!(ids_of(&lines[2]).contains(&id), "{id:?} in the ASIO line");
    }
    // The p95 and max values stay with their figure; they are not figures of their own.
    let gap = map::status_segments(&lines[0]).into_iter().find(|s| s.id == Some(Id::AudioDevicesGapOverPeriod)).map(|s| s.text);
    assert!(gap.is_some_and(|g| g.contains("p95") && g.contains("max 1172")), "{lines:?}");
}

#[test]
fn open_lines_split_into_the_device_and_what_it_runs_at() {
    let line = "Output open: Speakers (Realtek) \u{b7} 48000 Hz \u{b7} 2 ch \u{b7} 256-sample buffer \u{b7} WASAPI* exclusive \u{b7} i24 \u{b7} 15.8 ms out \u{b7} MMCSS Pro Audio";
    assert_eq!(joined(line), line);
    assert_eq!(
        ids_of(line),
        vec![
            Id::AudioDevicesOutputOpen,
            Id::AudioDevicesDevice,
            Id::AudioDevicesRate,
            Id::AudioDevicesChannels,
            Id::AudioDevicesBuffer,
            Id::AudioDevicesHost,
            Id::AudioDevicesSampleFormat,
            Id::AudioDevicesLatency,
            Id::AudioDevicesPriority,
        ]
    );
    for line in [
        "Output open: none",
        "Input open: none yet (it opens when a track is armed or monitored)",
        "Requested 128 \u{b7} running at 256",
        "Recovery: driver reset, reopened automatically (resets 1, stalls 0, reopened 1)",
    ] {
        assert_eq!(joined(line), line);
        assert!(ids_of(line).iter().all(|_| true) && !ids_of(line).is_empty(), "{line}");
    }
}

#[test]
fn hovering_a_backend_figure_in_audio_devices_explains_it() {
    let mut t = T::new();
    t.control(
        "audio.fake_devices",
        json!({"systems": [{"name": "WASAPI", "is_default": true, "outputs": [{"name": "Speakers", "is_default": true}], "inputs": []}]}),
    );
    t.app.audio_devices.injected_backend = Some((Some(stats()), None));
    t.app.audio_devices.open = true;
    t.frames(30);
    t.panel_on();
    t.frames(3);
    let at = t.target("audio_devices.timeouts");
    t.point_at(at);
    assert_eq!(t.app.help.layout.id.as_deref(), Some("audio_devices.timeouts"));
    let entry = help::entry(Id::AudioDevicesTimeouts).expect("entry");
    assert_eq!(t.app.help.layout.text, entry.text);
    assert!(entry.text.contains("Healthy is 0"), "a healthy value is given: {}", entry.text);
}
