//! The editor window of built-in plug-ins (the device card's "Open editor" for a built-in), driven
//! the way a user drives it: synthetic clicks and drags, one frame at a time. No player.

use crate::{Services, StudioApp};
use serde_json::{Value, json};
use transit_model::TrackId;

struct T {
    app: StudioApp,
    ctx: egui::Context,
    time: f64,
    shapes: Vec<egui::epaint::ClippedShape>,
}

impl T {
    fn new() -> T {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, Services::default());
        app.ui.show_mixer = true;
        app.ui.show_devices = true;
        let mut t = T { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new() };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.time += 1.0 / 60.0;
            let mut raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                time: Some(self.time),
                ..Default::default()
            };
            self.app.raw_input_hook(&mut raw);
            let app = &mut self.app;
            let mut out = self.ctx.run_ui(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
            self.shapes = out.shapes;
            out.textures_delta.clear();
        }
    }

    fn send(&mut self, id: &str, p: Value) {
        let (req, _rx) = crate::ControlRequest::new(id, p);
        let ctx = egui::Context::default();
        let _ = crate::control::handle_for_test(&mut self.app, &ctx, &req);
        self.frames(12);
    }

    fn click(&mut self, at: (f32, f32), count: u32) {
        self.send("ui.click", json!({"x": at.0, "y": at.1, "count": count}));
    }

    fn track(&mut self, kind: &str) -> TrackId {
        let r = self.app.run("track.new", json!({"kind": kind})).unwrap();
        let t = TrackId(r["tracks"][0].as_u64().unwrap());
        self.app.focus(t);
        self.frames(4);
        t
    }

    fn undo_len(&mut self) -> usize {
        self.app.engine.execute("engine.history", &json!({})).unwrap()["undo"].as_array().unwrap().len()
    }

    fn window(&self, t: TrackId) -> crate::builtin_editor::EditorLayout {
        self.app.builtin_editor.windows.iter().find(|w| w.key.0 == t).cloned().expect("an editor window is open")
    }

    fn row(&self, t: TrackId, param: &str) -> crate::builtin_editor::RowLayout {
        self.window(t).rows.into_iter().find(|r| r.param == param).unwrap_or_else(|| panic!("a row for `{param}`"))
    }

    fn param(&self, t: TrackId, slot: Option<usize>, id: &str) -> Option<f32> {
        let tr = self.app.engine.session().track(t).unwrap();
        let ins = match slot {
            Some(s) => tr.mixer.inserts[s].as_ref(),
            None => tr.instrument.as_ref(),
        };
        ins.and_then(|i| i.params.get(id).copied())
    }

    fn assert_preset_label(&self, tr: TrackId, expected: &str) {
        fn contains(shape: &egui::Shape, rect: egui::Rect, expected: &str) -> bool {
            match shape {
                egui::Shape::Text(text) => rect.contains(text.pos) && text.galley.text() == expected,
                egui::Shape::Vec(shapes) => shapes.iter().any(|s| contains(s, rect, expected)),
                _ => false,
            }
        }
        let [x0, y0, x1, y1] = self.window(tr).preset;
        let rect = egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1));
        assert!(self.shapes.iter().any(|s| contains(&s.shape, rect, expected)), "preset dropdown should show {expected}");
    }

    /// How many painted texts read exactly `want`, and where the first one is.
    fn painted(&self, want: &str) -> (usize, Option<(f32, f32)>) {
        fn walk(shape: &egui::Shape, want: &str, out: &mut Vec<(f32, f32)>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == want => out.push((t.pos.x + t.galley.size().x / 2.0, t.pos.y + t.galley.size().y / 2.0)),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, want, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        self.shapes.iter().for_each(|s| walk(&s.shape, want, &mut found));
        (found.len(), found.first().copied())
    }

    /// Click the focused track's editor button on card `card`.
    fn open_editor(&mut self, card: usize) {
        let at = mid(self.app.mixer.devices.cards[card].editor_button);
        self.click(at, 1);
    }
}

fn mid(r: [f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

fn info(plugin: &str) -> &'static transit_dsp::PluginInfo {
    transit_dsp::plugin_info(plugin).unwrap()
}

fn default_of(plugin: &str, id: &str) -> f32 {
    info(plugin).param(id).unwrap().default
}

#[test]
fn the_editor_button_opens_a_window_for_a_built_in_instrument_and_effect() {
    let mut t = T::new();
    let tr = t.track("instrument");
    t.app.add_device(tr, "gain").unwrap();
    t.frames(4);
    assert!(t.app.mixer.devices.cards.iter().all(|c| c.editor_enabled), "built-in editors need no player");
    assert_eq!(t.app.mixer.devices.cards[0].plugin, "macro_oscillator");
    t.open_editor(0);
    let key = (tr, transit_mix::INSTRUMENT_SLOT);
    assert_eq!(t.app.ui.plugin_windows, vec![key]);
    let w = t.window(tr);
    assert_eq!(w.title, format!("{} · Macro Oscillator", t.app.engine.session().track(tr).unwrap().name));
    // One row per parameter of the module's own; the shaper and the matrix (#243) hide until used.
    assert_eq!(w.rows.len(), 12, "one row per parameter of the module");
    assert!(t.app.mixer.devices.cards[0].editor_open);
    t.open_editor(1);
    assert_eq!(t.app.ui.plugin_windows.len(), 2);
    assert_eq!(t.app.builtin_editor.windows.len(), 2);
    assert_eq!(t.app.builtin_editor.windows[1].title.rsplit(" · ").next(), Some("Gain"));
    // The button closes it again, and the window is gone.
    t.open_editor(0);
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, 0)]);
    assert_eq!(t.app.builtin_editor.windows.len(), 1);
}

#[test]
fn dragging_a_slider_writes_the_value_in_one_undo_step() {
    let mut t = T::new();
    let tr = t.track("instrument");
    t.app.run("mix.instrument", json!({"track": tr.0, "plugin": "drum_synth"})).unwrap();
    t.frames(4);
    t.open_editor(0);
    let id = "snare_decay";
    let p = info("drum_synth").param(id).unwrap();
    let before = t.undo_len();
    let r = t.row(tr, id);
    let from = mid(r.control);
    t.send("ui.drag", json!({"x": from.0, "y": from.1, "to_x": r.control[0] + 6.0, "to_y": from.1}));
    let v = t.param(tr, None, id).expect("the drag wrote the parameter");
    assert!(v < p.default, "dragged toward the left end: {v}");
    assert_eq!(t.undo_len(), before + 1, "one undo step for the whole drag");
    assert_eq!(t.row(tr, id).value, p.format(v), "the row shows the value");
    t.app.run("edit.undo", json!({})).unwrap();
    assert!(t.param(tr, None, id).is_none_or(|u| (u - p.default).abs() < 1e-3), "undo restores the default");
}

#[test]
fn double_clicking_a_control_resets_it() {
    let mut t = T::new();
    let tr = t.track("instrument");
    t.app.run("mix.instrument", json!({"track": tr.0, "plugin": "drum_synth"})).unwrap();
    t.frames(4);
    t.app.run("mix.instrument_param", json!({"track": tr.0, "param": "snare_decay", "value": 100.0})).unwrap();
    t.frames(3);
    t.open_editor(0);
    t.click(mid(t.row(tr, "snare_decay").control), 2);
    assert_eq!(t.param(tr, None, "snare_decay"), Some(default_of("drum_synth", "snare_decay")));
}

#[test]
fn an_effects_editor_has_a_bypass_toggle_and_follows_the_insert_when_reordered() {
    let mut t = T::new();
    let tr = t.track("audio");
    for p in ["gain", "saturator"] {
        t.app.add_device(tr, p).unwrap();
    }
    t.frames(4);
    t.open_editor(0);
    let uid = t.window(tr).uid;
    assert_eq!(t.window(tr).key, (tr, 0));
    t.click(mid(t.window(tr).bypass), 1);
    assert!(t.app.engine.session().track(tr).unwrap().mixer.inserts[0].as_ref().unwrap().bypass);
    // Move the Gain behind the saturator: the same window (same uid) now belongs to slot 1.
    t.app.reorder_device(tr, 0, 1, false).unwrap();
    t.frames(4);
    assert_eq!(t.app.builtin_editor.windows.len(), 1);
    assert_eq!((t.window(tr).uid, t.window(tr).key), (uid, (tr, 1)));
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, 1)]);
    // Edits land on the insert it follows.
    let id = info("gain").params[0].id;
    t.app.run("mix.insert_param", json!({"track": tr.0, "slot": 1, "param": id, "value": info("gain").params[0].max})).unwrap();
    t.frames(2);
    t.click(mid(t.row(tr, id).control), 2);
    assert_eq!(t.param(tr, Some(1), id), Some(default_of("gain", id)));
}

/// A tiny mono 16-bit WAV.
fn wav(path: &std::path::Path) {
    let samples: Vec<i16> = (0..4800).map(|i| ((i as f32 * 0.1).sin() * 8000.0) as i16).collect();
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(48_000u32.to_le_bytes());
    b.extend(96_000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data_len.to_le_bytes());
    for s in samples {
        b.extend(s.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

#[test]
fn the_sampler_window_has_a_sample_picker_and_names_the_sample() {
    let mut t = T::new();
    let tr = t.track("instrument");
    t.app.run("mix.instrument", json!({"track": tr.0, "plugin": "sampler"})).unwrap();
    t.frames(4);
    let dir = std::env::temp_dir().join(format!("transit-sampler-editor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hit.wav");
    wav(&file);
    let path = file.to_string_lossy().into_owned();
    t.app.services.pick_open = Some(Box::new(move |purpose: &str| (purpose == "mix.sampler_load").then(|| path.clone())));
    t.open_editor(0);
    let w = t.window(tr);
    assert_eq!(w.sample, "No sample");
    assert!(w.picker != [0.0; 4], "the picker is shown");
    t.click(mid(w.picker), 1);
    let s = t.window(tr).sample;
    assert!(s.starts_with("hit"), "the loaded sample is named: {s}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_drum_synth_lists_its_parameters_by_pad() {
    let mut t = T::new();
    let tr = t.track("instrument");
    t.app.run("mix.instrument", json!({"track": tr.0, "plugin": "drum_synth"})).unwrap();
    t.frames(4);
    t.open_editor(0);
    let w = t.window(tr);
    assert_eq!(w.groups, ["Output", "Kick", "Snare", "Closed Hat", "Open Hat", "Toms", "Cymbals"]);
    assert_eq!(w.rows.len(), info("drum_synth").params.len());
}

#[test]
fn a_factory_instrument_preset_applies_without_a_config_directory() {
    let mut t = T::new();
    let (tr, _) = open_plugin(&mut t, "macro_oscillator");
    t.app.services.config_dir = None;
    assert_ne!(t.param(tr, None, "harmonics"), Some(30.0));
    let before = t.undo_len();
    let previous = t.app.builtin_at((tr, transit_mix::INSTRUMENT_SLOT)).unwrap();
    let preset = t.window(tr).preset;
    t.click(mid(preset), 1);
    // The menu has its Factory heading, Init, then Analog Bass.
    t.click(((preset[0] + preset[2]) / 2.0, preset[3] + 52.0), 1);
    assert_eq!(t.param(tr, None, "harmonics"), Some(30.0));
    assert_eq!(t.row(tr, "harmonics").value, info("macro_oscillator").param("harmonics").unwrap().format(30.0));
    t.assert_preset_label(tr, "Analog Bass");
    assert_eq!(t.undo_len(), before + 1);
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    assert_eq!(t.app.builtin_at((tr, transit_mix::INSTRUMENT_SLOT)).unwrap(), previous);
    t.assert_preset_label(tr, "<factory default>");
    t.app.run("edit.redo", json!({})).unwrap();
    t.frames(3);
    t.assert_preset_label(tr, "Analog Bass");
}

/// A track with `plugin` open in its editor: an instrument for the synth, an insert otherwise.
fn open_plugin(t: &mut T, plugin: &str) -> (TrackId, Option<usize>) {
    if info(plugin).is_instrument {
        let tr = t.track("instrument");
        t.app.run("mix.instrument", json!({"track": tr.0, "plugin": plugin})).unwrap();
        t.frames(4);
        t.open_editor(0);
        (tr, None)
    } else {
        let tr = t.track("audio");
        t.app.add_device(tr, plugin).unwrap();
        t.frames(4);
        t.open_editor(0);
        (tr, Some(0))
    }
}

#[test]
fn each_bespoke_panel_opens_and_lists_every_parameter() {
    for plugin in ["compressor", "expander_gate", "eq_7band", "plate_reverb", "macro_oscillator"] {
        let mut t = T::new();
        let (tr, _) = open_plugin(&mut t, plugin);
        let r = t.window(tr).addition_rect;
        if r != [0.0; 4] {
            // The Macro Oscillator's shaper and matrix (#243) show after a click on their line.
            t.click(((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0), 1);
            t.frames(3);
        }
        let w = t.window(tr);
        let mut got: Vec<String> = w.rows.iter().map(|r| r.param.clone()).collect();
        got.sort();
        let mut want: Vec<String> = info(plugin).params.iter().map(|p| p.id.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "{plugin}");
        assert!(w.preset != [0.0; 4], "{plugin} has a preset control");
    }
    // The built-ins without a layout keep the generic rows and have no preset control.
    let mut t = T::new();
    let (tr, _) = open_plugin(&mut t, "gain");
    assert_eq!(t.window(tr).preset, [0.0; 4]);
}

#[test]
fn the_macro_oscillator_panel_credits_the_original_and_drives_its_controls() {
    let mut t = T::new();
    let (tr, slot) = open_plugin(&mut t, "macro_oscillator");
    let w = t.window(tr);
    assert_eq!(w.title.split(" · ").last(), Some("Macro Oscillator"));
    assert_eq!(w.credit, crate::panel::MACRO_OSC_CREDIT);
    assert!(w.credit.contains("Émilie Gillet"));
    assert_eq!(t.row(tr, "model").value, "Virtual analog", "the default model, named by what it does");
    assert!(t.window(tr).rows.iter().all(|r| r.param != "hardware_tuning"), "no tuning switch: the port plays concert pitch");
    let before = t.undo_len();
    t.click(mid(t.row(tr, "mod_wheel").control), 1);
    t.frames(2);
    assert_eq!(t.undo_len(), before, "opening a menu writes nothing");
    let from = mid(t.row(tr, "timbre").control);
    t.send("ui.drag", json!({"x": from.0, "y": from.1, "to_x": from.0, "to_y": from.1 - 30.0}));
    assert!(t.param(tr, slot, "timbre").unwrap() > 50.0, "dragging Timbre up raises it");
}

/// One second of the instrument built from the session's parameters, playing middle C.
fn render_instrument(t: &T, tr: TrackId) -> Vec<f32> {
    let ins = t.app.engine.session().track(tr).unwrap().instrument.clone().unwrap();
    let mut m = transit_dsp::create("macro_oscillator").unwrap();
    for (id, v) in &ins.params {
        m.set_param(id, *v);
    }
    m.prepare(48_000.0, 512, 2);
    m.note_on(0, 60, 100);
    let mut out = Vec::new();
    let mut io = vec![vec![0.0; 512]; 2];
    for _ in 0..40 {
        m.process(&mut io, 512);
        out.extend_from_slice(&io[0]);
    }
    out
}

/// The Model dropdown applies its choice (its list was closing under the MIDI-learn menu, which
/// shared the dropdown button's popup id), and "Model" is the section header only.
#[test]
fn the_macro_oscillator_model_combo_applies_its_choice_and_the_panel_says_model_once() {
    use transit_dsp::plugins::macro_osc::MODELS;
    let mut t = T::new();
    let (tr, slot) = open_plugin(&mut t, "macro_oscillator");
    assert_eq!(t.painted("Model").0, 1, "one Model text: the section header, not a second control label");
    assert_eq!(t.row(tr, "model").value, MODELS[8]);
    let before = render_instrument(&t, tr);
    assert!(before.iter().any(|v| v.abs() > 1e-4), "the default model sounds");
    t.click(mid(t.row(tr, "model").control), 1);
    t.frames(3);
    let (n, at) = t.painted(MODELS[2]);
    assert_eq!(n, 1, "the open dropdown lists {}", MODELS[2]);
    t.click(at.unwrap(), 1);
    t.frames(3);
    assert_eq!(t.param(tr, slot, "model"), Some(2.0), "choosing a model writes the model parameter");
    assert_eq!(t.row(tr, "model").value, MODELS[2], "the panel shows the chosen model");
    assert_eq!(t.painted("Model").0, 1, "still one Model text");
    let after = render_instrument(&t, tr);
    assert_ne!(before, after, "the instrument renders the chosen model");
}

/// The voices and the voice lights (#196), which read the instrument's tap; the line that labels
/// the shaper and the matrix as transit's addition, and the matrix as a grid below the shaper
/// (#243): a row's cells side by side, rows 1 to 8 down the first column, 9 to 16 down the second.
#[test]
fn the_macro_oscillator_panel_shows_voices_the_shaper_the_matrix_grid_and_voice_lights() {
    let mut t = T::new();
    let tr = t.track("instrument");
    t.app.run("mix.instrument", json!({"track": tr.0, "plugin": "macro_oscillator"})).unwrap();
    t.frames(4);
    let hub = std::sync::Arc::new(transit_mix::ScopeHub::default());
    let tap = std::sync::Arc::new(transit_mix::ScopeTap::new());
    hub.register(tr, transit_mix::INSTRUMENT_UID, &tap);
    t.app.builtin_editor.hub = Some(hub);
    t.open_editor(0);
    assert!(tap.enabled(), "the open panel switches the instrument's tap on");
    assert_eq!(t.row(tr, "voices").value, "8");
    assert_eq!(t.window(tr).lights, "--------");
    assert_eq!(t.window(tr).addition, crate::panel::MACRO_OSC_ADDITION);
    assert!(t.window(tr).rows.iter().all(|r| r.param != "shaper"), "not in use: the shaper and the matrix are hidden");
    let r = t.window(tr).addition_rect;
    t.click(((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0), 1);
    t.frames(3);
    let cell = |id: &str| t.row(tr, id).control;
    let row1: Vec<[f32; 4]> = ["mod1_source", "mod1_via", "mod1_dest", "mod1_curve", "mod1_amount"].iter().map(|id| cell(id)).collect();
    for w in row1.windows(2) {
        assert!(w[1][0] > w[0][2] - 1.0 && (w[1][1] - w[0][1]).abs() < 6.0, "a row's cells side by side: {row1:?}");
    }
    let (s1, s2, s9) = (cell("mod1_source"), cell("mod2_source"), cell("mod9_source"));
    assert!(s2[1] > s1[3] - 1.0 && (s2[0] - s1[0]).abs() < 1.0, "row 2 under row 1");
    assert!(s9[0] > row1[4][2] && (s9[1] - s1[1]).abs() < 1.0, "row 9 heads the second column");
    assert!(s1[1] > cell("shaper")[3] && s1[1] > cell("lfo2_amount")[3], "the matrix sits below the shaper");
    // The window re-fits around the shown rows (#232), and stays a size a 1080p screen holds.
    let w = t.window(tr);
    let (right, bottom) = w.rows.iter().fold((0.0f32, 0.0f32), |(r, b), x| (r.max(x.control[2]), b.max(x.control[3])));
    assert!(w.rect[2] >= right && w.rect[2] - right <= 2.0 * crate::theme::SPACE + 40.0, "{:?} around {right}", w.rect);
    assert!(w.rect[3] >= bottom, "{:?} covers {bottom}", w.rect);
    assert!(w.rect[2] - w.rect[0] <= 1300.0 && w.rect[3] - w.rect[1] <= 800.0, "{:?}", w.rect);
    assert_eq!(t.row(tr, "mod1_source").value, "Off");
    assert_eq!(t.row(tr, "mod16_amount").value, "0%");
    assert_eq!(t.row(tr, "shaper").value, "Off");
    t.app.run("mix.instrument_param", json!({"track": tr.0, "param": "mod3_dest", "value": 7.0})).unwrap();
    t.frames(2);
    assert_eq!(t.row(tr, "mod3_dest").value, "Cutoff");
    t.app.run("mix.instrument_param", json!({"track": tr.0, "param": "voices", "value": 2.0})).unwrap();
    tap.push_voices(0b011 | 0b001 << 16);
    t.frames(3);
    assert_eq!(t.window(tr).lights, "HR-", "three voices: one held, one releasing, one free");
    let r = t.window(tr).lights_rect;
    assert!(r[2] > r[0] && r[3] > r[1]);
}

#[test]
fn turning_a_knob_is_one_undo_step_and_double_click_resets() {
    let mut t = T::new();
    let (tr, slot) = open_plugin(&mut t, "compressor");
    let p = info("compressor").param("threshold").unwrap();
    let before = t.undo_len();
    let r = t.row(tr, "threshold");
    let from = mid(r.control);
    t.send("ui.drag", json!({"x": from.0, "y": from.1, "to_x": from.0, "to_y": from.1 - 40.0}));
    let v = t.param(tr, slot, "threshold").expect("the drag wrote the threshold");
    assert!(v > p.default, "dragged up raises it: {v}");
    assert_eq!(t.undo_len(), before + 1, "one undo step for the whole drag");
    t.click(mid(t.row(tr, "threshold").control), 2);
    assert_eq!(t.param(tr, slot, "threshold"), Some(p.default));
    // A toggle is a button.
    t.click(mid(t.row(tr, "limiter").control), 1);
    assert_eq!(t.param(tr, slot, "limiter"), Some(1.0));
}

fn temp_config(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("transit-presets-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_user_preset_saves_loads_lists_and_deletes_as_one_undo_step() {
    use crate::presets;
    let dir = temp_config("user");
    let mut t = T::new();
    t.app.services.config_dir = Some(dir.clone());
    let (tr, slot) = open_plugin(&mut t, "compressor");
    let key = (tr, 0);
    let comp = info("compressor");
    t.app.run("mix.insert_params", json!({"track": tr.0, "slot": 0, "params": {"threshold": -30.0, "ratio": 8.0}})).unwrap();
    let values: std::collections::BTreeMap<String, f32> =
        comp.params.iter().map(|p| (p.id.to_string(), t.param(tr, slot, p.id).unwrap_or(p.default))).collect();
    assert_eq!(presets::save_user(&dir, comp, "My: Bus/Comp", &values).unwrap(), "My  Bus Comp");
    assert_eq!(presets::user_names(&dir, "compressor"), ["My  Bus Comp"]);
    assert!(presets::user_names(&dir, "eq_7band").is_empty());
    // Change things, then recall: a preset with an id the plug-in lacks still loads.
    t.app.run("mix.insert_params", json!({"track": tr.0, "slot": 0, "params": {"threshold": -5.0, "ratio": 2.0, "makeup": 9.0}})).unwrap();
    let mut p = presets::load_user(&dir, "compressor", "My  Bus Comp").unwrap();
    p.params.insert("no_such_parameter".into(), 1.0);
    let before = t.undo_len();
    t.app.apply_preset(key, comp, "My  Bus Comp", &p);
    assert_eq!(t.undo_len(), before + 1, "a recall is one undo step");
    assert_eq!(t.param(tr, slot, "threshold"), Some(-30.0));
    assert_eq!(t.param(tr, slot, "ratio"), Some(8.0));
    assert_eq!(t.param(tr, slot, "makeup"), Some(0.0), "what the preset leaves out returns to its default");
    t.frames(3);
    assert_eq!(t.app.engine.session().track(tr).unwrap().mixer.inserts[0].as_ref().unwrap().preset, "My  Bus Comp");
    t.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(t.param(tr, slot, "threshold"), Some(-5.0));
    // Another plug-in's file is refused; a deleted preset is gone.
    assert!(presets::load_user(&dir, "eq_7band", "My  Bus Comp").is_err());
    presets::delete_user(&dir, "compressor", "My  Bus Comp").unwrap();
    assert!(presets::user_names(&dir, "compressor").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_instrument_preset_name_survives_session_save_load_and_undo_restores_the_previous_name() {
    let mut t = T::new();
    let (tr, _) = open_plugin(&mut t, "macro_oscillator");
    let key = (tr, transit_mix::INSTRUMENT_SLOT);
    let synth = info("macro_oscillator");
    let factory = crate::presets::factory_for(synth.id);
    let soft = &factory.iter().find(|(name, _)| *name == "Analog Bass").unwrap().1;
    let bass = &factory.iter().find(|(name, _)| *name == "Glass Bell").unwrap().1;
    t.app.apply_preset(key, synth, "Analog Bass", soft);
    let previous = t.app.builtin_at(key).unwrap();
    let before = t.undo_len();
    t.app.apply_preset(key, synth, "Glass Bell", bass);
    assert_eq!(t.undo_len(), before + 1);
    t.app.run("edit.undo", json!({})).unwrap();
    t.frames(3);
    assert_eq!(t.app.builtin_at(key).unwrap(), previous);
    t.assert_preset_label(tr, "Analog Bass");

    let dir = temp_config("instrument-session");
    let path = dir.join("preset.transit");
    t.app.engine.execute("session.save_as", &json!({"path": path, "replace": true})).unwrap();
    t.app.engine.execute("session.new", &json!({})).unwrap();
    t.app.engine.execute("session.open", &json!({"path": path})).unwrap();
    t.frames(3);
    let loaded = t.app.builtin_at(key).unwrap();
    assert_eq!(loaded.preset, "Analog Bass");
    assert_eq!(loaded.params, previous.params);
    t.assert_preset_label(tr, "Analog Bass");

    // Older sessions may omit the existing optional preset field.
    let mut old: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    old["tracks"][0]["instrument"].as_object_mut().unwrap().remove("preset");
    let old = transit_model::Session::from_json(&old.to_string()).unwrap();
    assert_eq!(old.track(tr).unwrap().instrument.as_ref().unwrap().preset, "");
    assert_eq!(old.track(tr).unwrap().instrument.as_ref().unwrap().params, previous.params);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn every_factory_preset_file_parses_belongs_to_its_plugin_and_applies_in_one_step() {
    use crate::presets;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/presets");
    let mut files = 0;
    for dir in std::fs::read_dir(&root).unwrap().flatten() {
        let plugin = dir.file_name().to_string_lossy().into_owned();
        let comp = transit_dsp::plugin_info(&plugin).unwrap_or_else(|| panic!("`{plugin}` is a built-in"));
        for f in std::fs::read_dir(dir.path()).unwrap().flatten() {
            files += 1;
            let name = f.path().file_stem().unwrap().to_string_lossy().into_owned();
            let p = presets::parse(&std::fs::read_to_string(f.path()).unwrap()).unwrap_or_else(|e| panic!("{plugin}/{name}: {e}"));
            assert_eq!(p.plugin, plugin, "{name}");
            for (id, v) in &p.params {
                let pi = comp.param(id).unwrap_or_else(|| panic!("{plugin}/{name}: no parameter `{id}`"));
                assert!((pi.min..=pi.max).contains(v), "{plugin}/{name}: `{id}` = {v} is outside its range");
            }
            assert!(presets::FACTORY.iter().any(|(pl, n, _)| *pl == plugin && *n == name), "{plugin}/{name} is missing from presets::FACTORY");
            let mut t = T::new();
            let (tr, slot) = open_plugin(&mut t, &plugin);
            let before = t.undo_len();
            t.app.apply_preset((tr, slot.unwrap_or(transit_mix::INSTRUMENT_SLOT)), comp, &name, &p);
            assert_eq!(t.undo_len(), before + 1, "{plugin}/{name}: one undo step");
            let key = (tr, slot.unwrap_or(transit_mix::INSTRUMENT_SLOT));
            assert_eq!(t.app.builtin_at(key).unwrap().preset, name, "{plugin}/{name}: preset name");
            for (id, v) in presets::recall_values(comp, &p) {
                assert_eq!(t.param(tr, slot, &id).unwrap_or(comp.param(&id).unwrap().default), v, "{plugin}/{name}: {id}");
            }
        }
    }
    assert_eq!(files, presets::FACTORY.len(), "every table entry has a file");
}

/// A tap in a headless hub for the insert in `slot` of `tr`.
fn tap_for(t: &mut T, tr: TrackId, slot: usize) -> std::sync::Arc<transit_mix::ScopeTap> {
    let uid = t.app.engine.session().track(tr).unwrap().mixer.inserts[slot].as_ref().unwrap().uid.get();
    let hub = std::sync::Arc::new(transit_mix::ScopeHub::default());
    let tap = std::sync::Arc::new(transit_mix::ScopeTap::new());
    hub.register(tr, uid, &tap);
    t.app.builtin_editor.hub = Some(hub);
    tap
}

#[test]
fn a_compressor_panel_shows_a_scope_and_gain_reduction_and_drives_its_tap() {
    let mut t = T::new();
    let tr = t.track("audio");
    t.app.add_device(tr, "compressor").unwrap();
    t.frames(4);
    let tap = tap_for(&mut t, tr, 0);
    assert!(!tap.enabled(), "a closed panel leaves the tap off");
    t.open_editor(0);
    assert!(tap.enabled(), "opening the panel switches the tap on");
    let w = t.window(tr);
    let (sw, sh) = (w.scope[2] - w.scope[0], w.scope[3] - w.scope[1]);
    assert_eq!((sw, sh), (crate::scope_strip::SCOPE_W, crate::scope_strip::SCOPE_H), "the scope strip has its rect");
    assert!(w.scope[1] < w.rows[0].control[1], "the strip is above the knobs");
    assert_eq!(w.level, "-inf dB");
    assert_eq!(w.gr, "GR 0.0 dB", "the gain reduction readout");
    assert!(w.gr_bar[2] > w.gr_bar[0]);
    t.open_editor(0);
    assert!(t.app.builtin_editor.windows.is_empty());
    t.frames(2);
    assert!(!tap.enabled(), "closing the panel switches it off");
}

#[test]
fn a_panel_without_dynamics_has_a_scope_and_no_gain_reduction() {
    let mut t = T::new();
    let tr = t.track("audio");
    t.app.add_device(tr, "eq_7band").unwrap();
    t.frames(4);
    let tap = tap_for(&mut t, tr, 0);
    t.open_editor(0);
    assert!(tap.enabled());
    let w = t.window(tr);
    assert!(w.scope[2] > w.scope[0]);
    assert!(w.gr.is_empty() && w.gr_bar == [0.0; 4]);
}

impl T {
    fn strip_row(&self, t: TrackId, slot: usize) -> (f32, f32) {
        let s = self.app.mixer.layout.strips.iter().find(|s| s.track == t.0).expect("the strip is drawn");
        mid(s.inserts.iter().find(|r| r.slot == slot).expect("the insert row").rect)
    }
}

#[test]
fn double_click_on_a_built_in_mixer_insert_slot_opens_its_editor_without_a_player() {
    let mut t = T::new();
    let tr = t.track("audio");
    t.app.add_device(tr, "gain").unwrap();
    t.frames(4);
    assert!(t.app.player.is_none());
    let at = t.strip_row(tr, 0);
    t.click(at, 2);
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, 0)]);
    assert_eq!(t.app.builtin_editor.windows.len(), 1);
}

#[test]
fn the_insert_slot_context_menu_offers_open_and_close_editor() {
    let mut t = T::new();
    let tr = t.track("audio");
    t.app.add_device(tr, "gain").unwrap();
    t.frames(4);
    let at = t.strip_row(tr, 0);
    t.send("ui.click", json!({"x": at.0, "y": at.1, "button": "right"}));
    let item = t.app.mixer.layout.menu.iter().find(|m| m.value == "editor").cloned().expect("an editor entry");
    assert_eq!(item.label, "Open editor");
    t.click(mid(item.rect), 1);
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, 0)]);
}

#[test]
fn double_clicking_a_card_title_opens_the_editor_and_the_button_reads_edit() {
    let mut t = T::new();
    let tr = t.track("audio");
    t.app.add_device(tr, "gain").unwrap();
    t.frames(4);
    let card = t.app.mixer.devices.cards.iter().position(|c| c.plugin == "gain").unwrap();
    t.click(mid(t.app.mixer.devices.cards[card].drag_handle), 2);
    assert_eq!(t.app.ui.plugin_windows, vec![(tr, 0)]);
    assert!(t.app.mixer.devices.cards[card].editor_open);
}

/// #232: a built-in's window takes its panel's size, has no resize handle, and re-fits when the
/// panel changes (the Macro Oscillator's Voices section).
#[test]
fn a_built_in_window_fits_its_panel_and_has_no_resize_handle() {
    for scale in [1.0_f32, 1.5] {
        for plugin in ["macro_oscillator", "compressor", "gain"] {
            let mut t = T::new();
            t.ctx.set_pixels_per_point(scale);
            let (tr, _) = open_plugin(&mut t, plugin);
            t.frames(4);
            let w = t.window(tr);
            let rect = egui::Rect::from_min_max(egui::pos2(w.rect[0], w.rect[1]), egui::pos2(w.rect[2], w.rect[3]));
            // The window encloses every control row with only the content margin to spare.
            let (mut right, mut bottom) = (0.0_f32, 0.0_f32);
            // The rows, and the line that shows the Macro Oscillator's shaper and matrix (#243).
            for c in w.rows.iter().map(|r| r.control).chain(std::iter::once(w.addition_rect)) {
                right = right.max(c[2]);
                bottom = bottom.max(c[3]);
            }
            let panel = crate::panel::layout(plugin).is_some();
            // A generic parameter list keeps its 360 pt minimum width.
            assert!(panel || rect.width() >= 360.0 - 0.5, "{plugin} @{scale}: width {}", rect.width());
            assert!(!panel || rect.right() - right <= 2.0 * crate::theme::SPACE + 40.0, "{plugin} @{scale}: right slack {}", rect.right() - right);
            assert!(
                !panel || rect.bottom() - bottom <= 2.0 * crate::theme::SPACE + 60.0,
                "{plugin} @{scale}: bottom slack {}",
                rect.bottom() - bottom
            );
            assert!(rect.right() >= right && rect.bottom() >= bottom, "{plugin} @{scale}: the window covers its controls");
            // No resize handle: dragging the bottom-right corner changes nothing.
            let corner = rect.max - egui::vec2(2.0, 2.0);
            t.send("ui.drag", json!({"x": corner.x, "y": corner.y, "to_x": corner.x + 120.0, "to_y": corner.y + 90.0}));
            t.frames(4);
            let after = t.window(tr).rect;
            assert_eq!(after, w.rect, "{plugin} @{scale}: the size is fixed");
            // Stable across frames.
            t.frames(3);
            assert_eq!(t.window(tr).rect, w.rect, "{plugin} @{scale}: stable");
        }
    }
    // Content that changes size re-fits on the next frames: Voices off the panel's own toggle.
    let mut t = T::new();
    let tr = t.track("instrument");
    t.app.run("mix.instrument", json!({"track": tr.0, "plugin": "macro_oscillator"})).unwrap();
    t.frames(4);
    t.open_editor(0);
    t.frames(3);
    let before = t.window(tr).rect;
    t.app.run("mix.instrument_param", json!({"track": tr.0, "param": "voices", "value": 2.0})).unwrap();
    t.frames(4);
    let after = t.window(tr).rect;
    println!("voices 8 -> 2: {before:?} -> {after:?}");
    assert!(after[0] == before[0] && after[1] == before[1], "the position is kept");
}

#[test]
fn the_filter_is_in_the_insert_picker_and_its_panel_draws_the_response_above_the_knobs() {
    let mut t = T::new();
    let picker = t.app.builtin_plugins();
    assert!(picker.iter().any(|p| p.id == "filter" && p.name == "Filter" && !p.instrument), "the Filter is an insert effect");
    let tr = t.track("instrument");
    t.app.add_device(tr, "filter").unwrap();
    t.frames(4);
    // Card 0 is the instrument, card 1 the Filter.
    t.open_editor(1);
    let w = t.window(tr);
    assert_eq!(w.title.rsplit(" · ").next(), Some("Filter"));
    let (cw, ch) = (w.curve[2] - w.curve[0], w.curve[3] - w.curve[1]);
    assert_eq!((cw, ch), (crate::filter_curve::CURVE_W, crate::filter_curve::CURVE_H), "the response curve has its rect");
    assert!(w.curve[1] < w.rows[0].control[1], "the curve is above the knobs");
    assert_eq!(w.rows.len(), info("filter").params.len(), "every parameter has a control");
    assert!(w.preset[2] > w.preset[0], "the preset dropdown");
}

/// A Macro Oscillator preset saved before the matrix (#243) parses into matrix rows: the
/// note-to-macro mapping's ids become a row, as in a session that loads.
#[test]
fn a_macro_oscillator_preset_from_before_the_matrix_parses_into_rows() {
    let old =
        r#"{"plugin": "macro_oscillator", "version": 1, "params": {"model": 8.0, "key_curve": 2.0, "key_timbre": 50.0, "velocity_morph": 0.0}}"#;
    let p = crate::presets::parse(old).unwrap();
    assert!(!p.params.keys().any(|k| transit_model::legacy_mapping::is_old_param(k)), "{:?}", p.params);
    for (id, v) in [("model", 8.0), ("mod1_source", 6.0), ("mod1_dest", 3.0), ("mod1_amount", 50.0), ("mod1_curve", 2.0)] {
        assert_eq!(p.params.get(id), Some(&v), "{id}");
    }
    assert!(!p.params.contains_key("mod2_source"), "a zero depth makes no row");
}

/// The Macro Oscillator's window with the shaper and the matrix shown fits a 1080p screen at 125%
/// scaling with its title bar (864 pt of height there, less the title bar and a taskbar): at most
/// 800 pt tall and 1300 pt wide at 100% (#243, #232). Collapsed, it keeps the module's own size
/// (556 x 358 pt, as before the shaper and the matrix).
#[test]
fn the_expanded_macro_oscillator_window_fits_1080p_at_125_percent() {
    let mut t = T::new();
    let (tr, _) = open_plugin(&mut t, "macro_oscillator");
    t.frames(3);
    let c = t.window(tr).rect;
    let (cw, ch) = (c[2] - c[0], c[3] - c[1]);
    assert!((cw - 556.0).abs() < 1.0 && (ch - 358.125).abs() < 1.0, "collapsed {cw} x {ch} pt");
    let r = t.window(tr).addition_rect;
    t.click(((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0), 1);
    t.frames(3);
    let w = t.window(tr);
    assert!(w.rows.iter().any(|r| r.param == "mod16_amount") && w.rows.iter().any(|r| r.param == "lfo2_amount"), "all of it shows");
    let (width, height) = (w.rect[2] - w.rect[0], w.rect[3] - w.rect[1]);
    assert!(height <= 800.0 && width <= 1300.0, "expanded {width} x {height} pt");
}
