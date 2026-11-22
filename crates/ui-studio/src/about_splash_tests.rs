//! Help > About and the launch splash, driven like a user: synthetic clicks through the control
//! channel's input queue, one frame at a time.

use crate::plugin_scan::ScanProgress;
use crate::{Services, StudioApp};
use serde_json::{Value, json};

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    copied: Vec<String>,
    /// The screen in points (1400x800 unless a test sets it).
    screen: egui::Vec2,
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

impl T {
    fn new() -> T {
        let app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, copied: Vec::new(), screen: egui::vec2(1400.0, 800.0) };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)),
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
            for c in &out.platform_output.commands {
                if let egui::OutputCommand::CopyText(t) = c {
                    self.copied.push(t.clone());
                }
            }
        }
    }

    fn click(&mut self, at: (f32, f32)) {
        let (req, _rx) = crate::ControlRequest::new("ui.click", json!({"x": at.0, "y": at.1}));
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(30);
    }

    fn inspect(&self) -> Value {
        self.app.inspect(&self.ctx)
    }

    fn menu(&mut self, name: &str) {
        let r =
            self.inspect()["bar_menu"].as_array().and_then(|a| a.iter().find(|i| i["label"] == name).map(|i| i["rect"].clone())).unwrap_or_default();
        let r: [f32; 4] = serde_json::from_value(r).unwrap_or_else(|_| panic!("no menu item {name}"));
        self.click(mid(r));
    }
}

fn rect(v: &Value) -> [f32; 4] {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn the_help_menu_opens_about_with_wordmark_version_and_credit() {
    let mut t = T::new();
    t.app.about.app_version = "9.9.9-test".into();
    assert_eq!(t.inspect()["about"]["open"], false);
    t.menu("Help");
    t.menu("About transit…");
    let about = t.inspect()["about"].clone();
    assert_eq!(about["open"], true);
    assert_eq!(about["version"], "transit 9.9.9-test");
    assert_eq!(about["credit"], "Forked from SoundCraft by storytold, October 2026");
    assert_eq!(about["repo"], "https://github.com/transit-audio/transit");
    assert_eq!(about["licenses"], "MIT OR Apache-2.0");
    // ADR 0033: the ported instrument's original author and the port's author, with their notices.
    assert_eq!(about["third_party"], crate::about::THIRD_PARTY);
    assert!(crate::about::THIRD_PARTY.contains("Émilie Gillet") && crate::about::THIRD_PARTY.contains("Oliver Rockstedt"));
    let notices = crate::about::THIRD_PARTY_NOTICES;
    assert!(notices.contains("Copyright (c) 2022 Oliver Rockstedt") && notices.contains("Copyright 2016 Emilie Gillet"));
    assert!(notices.contains("Copyright 2012 Emilie Gillet"), "the stmlib notice");
    // #241: the Texture insert's credit and notices.
    assert!(crate::about::THIRD_PARTY.contains("Texture and Texture Synth: a port of Émilie Gillet's texture synthesizer firmware"));
    assert!(notices.contains("Third-party licenses: transit-texture") && notices.contains("Copyright 2017 Emilie Gillet"));
    // #199: the Time Stretcher insert's credit and notices.
    assert!(crate::about::THIRD_PARTY.contains("Time Stretcher: a port of Émilie Gillet's texture processor firmware"));
    assert!(notices.contains("Third-party licenses: transit-granular") && notices.contains("Copyright 2014 Emilie Gillet"));
    // #200: the Cross Modulator's credit and notices.
    assert!(crate::about::THIRD_PARTY.contains("Cross Modulator: a port of Émilie Gillet's meta-modulator firmware"));
    assert!(notices.contains("Third-party licenses: transit-modulator") && notices.contains("Copyright 2014 Emilie Gillet"));
    // #198: the Resonator's credit and notices.
    assert!(crate::about::THIRD_PARTY.contains("Resonator: a port of Émilie Gillet's resonator firmware"));
    assert!(notices.contains("Third-party licenses: transit-resonator") && notices.contains("Copyright 2015 Emilie Gillet"));
    // #198 slice 2: the Modal Voice's credit and notices.
    assert!(crate::about::THIRD_PARTY.contains("Modal Voice: a port of Émilie Gillet's modal synthesizer firmware"));
    assert!(notices.contains("Third-party licenses: transit-modal-voice") && notices.contains("Copyright 2014 Emilie Gillet"));
    // #202: the Random Sequencer's credit and notices.
    assert!(crate::about::THIRD_PARTY.contains("Random Sequencer: a port of Émilie Gillet's random sampler firmware"));
    assert!(notices.contains("Third-party licenses: transit-random-sequencer") && notices.contains("Copyright 2015 Emilie Gillet"));
    // #238: libopus's credit and BSD notice.
    assert!(crate::about::THIRD_PARTY.contains("Opus export and import: libopus 1.6.1"));
    assert!(notices.contains("Third-party licenses: transit-opus") && notices.contains("Copyright 2001-2023 Xiph.Org"));
    // #737: the trademark notices, worded as the owners' guidelines ask.
    assert_eq!(about["trademarks"], crate::about::TRADEMARKS);
    assert!(crate::about::TRADEMARKS.contains("VST is a registered trademark of Steinberg Media Technologies GmbH."));
    assert!(crate::about::TRADEMARKS.contains("ASIO is a registered trademark of Steinberg Media Technologies GmbH."));
    let w = rect(&about["wordmark"]);
    assert!((w[2] - w[0] - 260.0).abs() < 1.5, "260 points wide: {w:?}");
    assert!(((w[2] - w[0]) / (w[3] - w[1]) - 3.0).abs() < 0.05, "3:1, never stretched");
    let win = rect(&about["window"]);
    let cx = (win[0] + win[2]) / 2.0;
    assert!((cx - 700.0).abs() < 2.0, "centered: {win:?}");
}

/// #737: the About window, the README, `ATTRIBUTION.md` and the site footer carry the same notice lines.
#[test]
fn the_trademark_notices_match_on_every_public_surface() {
    let surfaces = [
        ("README.md", include_str!("../../../README.md")),
        ("ATTRIBUTION.md", include_str!("../../../ATTRIBUTION.md")),
        ("site footer", include_str!("../../../site/layouts/_partials/site-footer.html")),
    ];
    for line in crate::about::TRADEMARKS.lines() {
        for (name, text) in surfaces {
            assert!(text.contains(line), "{name} lacks the notice line: {line}");
        }
    }
}

#[test]
fn the_wordmark_follows_the_theme_without_tinting() {
    let mut t = T::new();
    t.app.about.open = true;
    t.frames(2);
    let dark = crate::icons::rasterized(&t.ctx);
    t.ctx.set_theme(egui::Theme::Light);
    t.frames(3);
    assert!(crate::icons::rasterized(&t.ctx) > dark, "the graphite artwork is a second texture");
    let n = crate::icons::rasterized(&t.ctx);
    t.frames(5);
    assert_eq!(crate::icons::rasterized(&t.ctx), n, "cached, not rasterized every frame");
}

#[test]
fn copy_version_info_puts_the_facts_on_the_clipboard_and_close_closes() {
    let mut t = T::new();
    t.app.about.app_version = "9.9.9-test".into();
    t.app.about.open = true;
    t.frames(2);
    let copy = rect(&t.inspect()["about"]["copy"]);
    t.click(mid(copy));
    let text = t.copied.last().expect("a copy command").clone();
    assert!(
        text.starts_with(
            "transit 9.9.9-test
"
        ),
        "{text}"
    );
    assert!(text.contains("https://github.com/transit-audio/transit"), "{text}");
    assert!(text.contains(&format!("OS: {}", std::env::consts::OS)), "{text}");
    assert!(text.contains("Audio: no device open"), "{text}");
    let close = rect(&t.inspect()["about"]["close"]);
    t.click(mid(close));
    assert_eq!(t.inspect()["about"]["open"], false);
}

fn scanning(index: usize, total: usize) -> Option<ScanProgress> {
    Some(ScanProgress { format: "VST3", index, total, folder: "VST3".into() })
}

#[test]
fn the_splash_shows_scan_counts_then_hides_when_the_scan_completes() {
    let mut t = T::new();
    assert_eq!(t.inspect()["splash"]["visible"], false, "unarmed: never shows");
    t.app.splash.arm();
    t.app.splash.scan_override = Some(scanning(12, 40));
    t.frames(3);
    let s = t.inspect()["splash"].clone();
    assert_eq!(s["visible"], true);
    assert!(s["status"].as_str().unwrap().starts_with("Scanning plug-ins… 12 of 40"), "{s}");
    assert!((s["fraction"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert!(rect(&s["wordmark"])[2] > 0.0 && rect(&s["bar"])[2] > 0.0 && rect(&s["skip"])[2] > 0.0);
    t.app.splash.scan_override = Some(scanning(40, 40));
    t.frames(2);
    assert_eq!(t.inspect()["splash"]["visible"], true);
    t.app.splash.scan_override = None; // the global scan state: nothing running
    t.frames(2);
    assert_eq!(t.inspect()["splash"]["visible"], false);
    t.app.splash.scan_override = Some(scanning(1, 2));
    t.frames(2);
    assert_eq!(t.inspect()["splash"]["visible"], false, "it does not come back");
}

#[test]
fn a_click_or_skip_hides_the_splash_early_and_the_device_line_shows() {
    let mut t = T::new();
    t.app.splash.arm();
    t.app.splash.opening_device = Some("Focusrite USB ASIO".into());
    t.frames(3);
    assert_eq!(t.inspect()["splash"]["status"], "Opening Focusrite USB ASIO…");
    t.click((300.0, 400.0));
    assert_eq!(t.inspect()["splash"]["visible"], false, "a click anywhere hides it");

    let mut t = T::new();
    t.app.splash.arm();
    t.app.splash.scan_override = Some(None);
    t.frames(3);
    assert_eq!(t.inspect()["splash"]["status"], "Scanning plug-ins…");
    let skip = rect(&t.inspect()["splash"]["skip"]);
    t.click(mid(skip));
    assert_eq!(t.inspect()["splash"]["visible"], false);
}

#[test]
fn the_splash_reports_the_device_stage_before_the_scan_stage() {
    let mut t = T::new();
    let opened = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let seen = std::rc::Rc::clone(&opened);
    t.app.services.open_audio = Some(Box::new(move |session, _| {
        seen.set(seen.get() + 1);
        transit_playback::Player::prepare_device_free(session).unwrap()
    }));
    t.app.splash.arm();
    t.app.splash.scan_override = Some(scanning(1, 4));
    t.app.splash.defer_open(transit_playback::AudioConfig { output: Some("Focusrite USB ASIO".into()), ..Default::default() });
    t.frames(1);
    assert_eq!(opened.get(), 0, "the first frame only announces the open");
    assert_eq!(t.inspect()["splash"]["status"], "Opening Focusrite USB ASIO…");
    t.frames(2);
    assert_eq!(opened.get(), 1, "the next frame opens the device");
    assert!(t.app.player.is_some());
    let s = t.inspect()["splash"].clone();
    assert!(s["status"].as_str().unwrap().starts_with("Scanning plug-ins…"), "{s}");
    // The device-free player counts as not opened: its failure shows for exactly one frame.
    assert_eq!(t.app.splash.stages, ["device", "failed", "scan"], "the device stage comes before the scan stage");
}

/// The dialog layout gate (#735) on About transit, at 1x and 1.5x.
#[test]
fn the_about_window_keeps_its_buttons_whole() {
    let layer = crate::layout_gate::window("About transit");
    for zoom in crate::layout_gate::SCALES {
        let mut t = T::new();
        crate::layout_gate::watch(&t.ctx, layer);
        t.app.ui.ui_zoom = zoom;
        t.app.about.app_version = "9.9.9-a-version-string-that-is-longer-than-any-release".into();
        t.menu("Help");
        t.menu("About transit…");
        assert_eq!(t.ctx.pixels_per_point(), zoom);
        assert_eq!(t.inspect()["about"]["open"], true);
        crate::layout_gate::assert_clean(&t.ctx, layer);
    }
}

/// Reads, at the end of every pass, how many rows each text shape on one layer painted (keyed by its text).
struct RowCounter {
    layer: egui::LayerId,
    rows: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>>,
}

impl egui::Plugin for RowCounter {
    fn debug_name(&self) -> &'static str {
        "about_row_counter"
    }

    fn on_end_pass(&mut self, ui: &mut egui::Ui) {
        fn walk(s: &egui::Shape, out: &mut std::collections::HashMap<String, usize>) {
            match s {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                egui::Shape::Text(t) => {
                    out.insert(t.galley.job.text.clone(), t.galley.rows.len());
                }
                _ => {}
            }
        }
        let mut found = std::collections::HashMap::new();
        ui.ctx().graphics(|g| {
            if let Some(list) = g.get(self.layer) {
                list.all_entries().for_each(|e| walk(&e.shape, &mut found));
            }
        });
        *self.rows.lock().unwrap() = found;
    }
}

/// Count the rows the About window paints for each paragraph: install before the frames that draw it.
fn count_rows(ctx: &egui::Context, layer: egui::LayerId) -> std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, usize>>> {
    let rows = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    ctx.add_plugin(RowCounter { layer, rows: rows.clone() });
    rows
}

/// The About window is a fixed 560 points of content wide (the window about 594 with the body padding), its
/// small print wraps to that width, and the whole window fits a 1366x768 screen at 1x and a 1920x1080 screen at 150
/// percent (1280x720 points) with the third-party notices collapsed and expanded.
#[test]
fn the_about_window_is_wide_and_fits_small_screens() {
    for (zoom, screen) in [(1.0, egui::vec2(1366.0, 768.0)), (1.5, egui::vec2(1280.0, 720.0))] {
        for expanded in [false, true] {
            let layer = crate::layout_gate::window("About transit");
            let mut t = T::new();
            let rows = count_rows(&t.ctx, layer);
            crate::layout_gate::watch(&t.ctx, layer);
            t.screen = screen;
            t.app.ui.ui_zoom = zoom;
            t.app.about.app_version = "9.9.9-a-version-string-that-is-longer-than-any-release".into();
            t.menu("Help");
            t.menu("About transit…");
            if expanded {
                let h = rect(&t.inspect()["about"]["notices"]);
                t.click(mid(h));
            }
            let what = format!("zoom {zoom}, expanded {expanded}");
            let about = t.inspect()["about"].clone();
            let w = rect(&about["window"]);
            // 560 of content plus the tool window's 16 a side and the 1 point edge: about 594 (it was 340).
            assert!((w[2] - w[0] - 594.0).abs() < 2.0, "{what}: {w:?}");
            assert!(w[0] >= 0.0 && w[1] >= 0.0 && w[2] <= screen.x && w[3] <= screen.y, "{what}: {w:?} must fit {screen:?}");
            let wm = rect(&about["wordmark"]);
            assert!(((wm[0] + wm[2]) / 2.0 - (w[0] + w[2]) / 2.0).abs() < 2.0, "{what}: the wordmark stays centered");
            // Before the widening the credits took 18 rows, the trademark lines 6 and the privacy line 2.
            let rows = rows.lock().unwrap();
            assert!(rows[crate::about::THIRD_PARTY] <= 10, "{what}: credits rows {}", rows[crate::about::THIRD_PARTY]);
            assert_eq!(rows[crate::about::TRADEMARKS], 3, "{what}: one row a trademark line");
            assert_eq!(rows[crate::update::PRIVACY], 1, "{what}: the privacy line on one row");
            drop(rows);
            crate::layout_gate::assert_clean(&t.ctx, layer);
        }
    }
}
