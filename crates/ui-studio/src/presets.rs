//! Presets of the built-in plug-ins. A preset is a small JSON file:
//!
//! ```json
//! {"plugin": "compressor", "version": 1, "params": {"threshold": -18.0, "ratio": 3.0}}
//! ```
//!
//! `params` maps parameter ids to values. Recalling a preset sets every parameter of the plug-in:
//! the ones it lists to its values, the rest to their defaults, so a preset sounds the same
//! whatever came before. Ids the plug-in does not have are ignored. MIDI effects (#202) have
//! factory presets too, recalled from the effect's row menu. Factory presets live in
//! `assets/presets/<plugin-id>/<name>.json` and are compiled in; user presets are the same files
//! in `<config dir>/presets/<plugin-id>/`.

use crate::StudioApp;
use crate::theme::TOKENS;
use egui::Ui;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use transit_dsp::PluginInfo;
use transit_model::{Insert, TrackId};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub plugin: String,
    pub version: u32,
    pub params: BTreeMap<String, f32>,
}

macro_rules! factory {
    ($($plugin:literal / $name:literal),* $(,)?) => {
        &[$(($plugin, $name, include_str!(concat!("../../../assets/presets/", $plugin, "/", $name, ".json")))),*]
    };
}

/// Every factory preset: plug-in id, name, file text.
pub static FACTORY: &[(&str, &str, &str)] = factory![
    "compressor" / "Vocal",
    "compressor" / "Drum Bus",
    "compressor" / "Gentle",
    "expander_gate" / "Tight Drums",
    "expander_gate" / "Light Gate",
    "eq_7band" / "Flat",
    "eq_7band" / "Low Cut",
    "plate_reverb" / "Vocal Plate",
    "plate_reverb" / "Bright Plate",
    "macro_oscillator" / "Init",
    "macro_oscillator" / "Analog Bass",
    "macro_oscillator" / "Glass Bell",
    "macro_oscillator" / "Plucked String",
    "macro_oscillator" / "Key Tracked Timbre",
    "filter" / "Pluck",
    "filter" / "Wah",
    "filter" / "Paraphonic Brass",
    "texture" / "Granular Cloud",
    "texture" / "Frozen Pad",
    "texture" / "Tape Delay",
    "time_stretcher" / "Octave Shimmer",
    "time_stretcher" / "Sub Doubler",
    "time_stretcher" / "Stretch Pad",
    "cross_modulator" / "Ring Bell",
    "cross_modulator" / "Vocoder",
    "cross_modulator" / "XOR Grit",
    "resonator" / "Struck Bell",
    "resonator" / "Sympathetic Harp",
    "resonator" / "Steel Pluck",
    "resonator" / "FM Mallet",
    "resonator" / "Strummed Chords",
    "resonator" / "Ambient Pluck",
    "modal_voice" / "Bowed Glass",
    "modal_voice" / "Breath Pipe",
    "modal_voice" / "Struck Bar",
    "modal_voice" / "Temple Bell",
    "modal_voice" / "Plucked Wire",
    "modal_voice" / "Noise Pad",
    "random_sequencer" / "Drifting Melody",
    "random_sequencer" / "Locked Loop",
    "random_sequencer" / "Sparse Random Bass",
    "texture_synth" / "Grain Pad",
    "texture_synth" / "Grain Pulse",
    "texture_synth" / "Frozen Cloud",
    "texture_synth" / "Tape Keys",
];

/// The factory presets of one plug-in, in table order.
pub fn factory_for(plugin: &str) -> Vec<(&'static str, Preset)> {
    FACTORY.iter().filter(|(p, ..)| *p == plugin).filter_map(|(_, name, text)| parse(text).ok().map(|p| (*name, p))).collect()
}

/// Parses a preset. A Macro Oscillator preset saved before #243 has its note-to-macro mapping
/// rewritten as matrix rows, as a session's is when it loads (`transit_model::legacy_mapping`).
pub fn parse(text: &str) -> Result<Preset, String> {
    let mut p: Preset = serde_json::from_str(text).map_err(|e| e.to_string())?;
    if p.version > VERSION {
        return Err(format!("preset version {} is newer than this build reads ({VERSION})", p.version));
    }
    if p.plugin == transit_model::legacy_mapping::MACRO_OSC_ID {
        transit_model::legacy_mapping::migrate_params(&mut p.params, |_| false);
    }
    Ok(p)
}

/// The values a recall writes: every parameter of `info`, from the preset or else its default.
pub fn recall_values(info: &PluginInfo, preset: &Preset) -> BTreeMap<String, f32> {
    info.params.iter().map(|p| (p.id.to_string(), p.clamp(preset.params.get(p.id).copied().unwrap_or(p.default)))).collect()
}

/// A file-safe name: path separators and other trouble characters become spaces.
pub fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " -_().".contains(c) { c } else { ' ' }).collect();
    s.trim().trim_matches('.').to_string()
}

fn dir_of(root: &Path, plugin: &str) -> PathBuf {
    root.join("presets").join(plugin)
}

/// The names of the user presets of `plugin` under the config dir `root`, sorted.
pub fn user_names(root: &Path, plugin: &str) -> Vec<String> {
    // The Sampler's user presets hold the retired one-sample instrument's parameters (#455):
    // one log line each at startup (`retired_user_preset_lines`), never listed until a patch preset exists.
    if plugin == RETIRED_SAMPLER_PRESETS {
        return Vec::new();
    }
    let mut v: Vec<String> = std::fs::read_dir(dir_of(root, plugin))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.path().file_stem().filter(|_| e.path().extension().is_some_and(|x| x == "json")).map(|s| s.to_string_lossy().into_owned()))
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

pub fn save_user(root: &Path, info: &PluginInfo, name: &str, params: &BTreeMap<String, f32>) -> Result<String, String> {
    let name = file_name(name);
    if name.is_empty() {
        return Err("Name the preset first".into());
    }
    let dir = dir_of(root, info.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let preset = Preset { plugin: info.id.to_string(), version: VERSION, params: params.clone() };
    let text = serde_json::to_string_pretty(&preset).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{name}.json")), text).map_err(|e| e.to_string())?;
    Ok(name)
}

pub fn load_user(root: &Path, plugin: &str, name: &str) -> Result<Preset, String> {
    let text = std::fs::read_to_string(dir_of(root, plugin).join(format!("{}.json", file_name(name)))).map_err(|e| e.to_string())?;
    let p = parse(&text)?;
    if p.plugin != plugin {
        return Err(format!("that preset is for `{}`", p.plugin));
    }
    Ok(p)
}

pub fn delete_user(root: &Path, plugin: &str, name: &str) -> Result<(), String> {
    std::fs::remove_file(dir_of(root, plugin).join(format!("{}.json", file_name(name)))).map_err(|e| e.to_string())
}

/// The plug-in id whose user presets hold only the retired one-sample Sampler's 19 parameters.
const RETIRED_SAMPLER_PRESETS: &str = "sampler";

/// The retired device a user preset naming `plugin` was for: an inherited effect, or the
/// one-sample Sampler (#455).
fn retired_device(plugin: &str) -> Option<&'static str> {
    transit_model::legacy_effects::retired_name(plugin).or((plugin == RETIRED_SAMPLER_PRESETS).then_some("one-sample Sampler"))
}

/// One line per user preset file under the config dir `root` that names a retired built-in device
/// (the inherited effects retired in v0.6.0, #264, ADR 0040, and the one-sample Sampler, #455), in
/// a folder of its own or filed under another device's. The file stays where it is, and nothing
/// lists or loads it: the device is gone.
pub fn retired_user_preset_lines(root: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let mut dirs: Vec<PathBuf> =
        std::fs::read_dir(root.join("presets")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs {
        let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for f in files {
            let named = std::fs::read_to_string(&f)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get("plugin").and_then(serde_json::Value::as_str).map(str::to_string));
            let device = named.as_deref().and_then(retired_device).or_else(|| retired_device(&folder));
            if let Some(device) = device {
                let name = f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                lines.push(format!("presets: user preset \"{name}\" skipped: it is for the retired {device} (presets/{folder})"));
            }
        }
    }
    lines
}

impl StudioApp {
    /// Recalls `preset` on the effect or instrument at `key` as one undo step.
    pub(crate) fn apply_preset(&mut self, key: (TrackId, usize), info: &PluginInfo, name: &str, preset: &Preset) {
        let values = recall_values(info, preset);
        let (t, slot) = key;
        // A recall is one instantaneous gesture on each parameter it changes (automation capture).
        let current = self.builtin_at(key).map(|i| i.params).unwrap_or_default();
        for (id, v) in &values {
            let was = current.get(id).copied().or_else(|| info.param(id).map(|p| p.default));
            if was.is_none_or(|c| (c - v).abs() > f32::EPSILON) {
                self.touch_param(t, slot, id, false);
            }
        }
        let r = if slot == transit_mix::INSTRUMENT_SLOT {
            // No batch command for the instrument: one merge key makes the writes one step.
            self.engine.end_merge();
            let mut r = Ok(());
            for (id, v) in &values {
                r = r.and(
                    self.execute_ui_merged(
                        "mix.instrument_param",
                        &json!({"track": t.0, "param": id, "value": v, "preset": name}),
                        &format!("preset:{}", t.0),
                    )
                    .map(|_| ()),
                );
            }
            self.engine.end_merge();
            r
        } else {
            self.execute_ui("mix.insert_params", &json!({"track": t.0, "slot": slot, "params": values, "preset": name})).map(|_| ())
        };
        if let Err(e) = r {
            self.status = e.to_string();
        }
    }
}

/// What the user chose in a preset list this frame.
#[derive(Default)]
pub(crate) struct Picked {
    pub pick: Option<(String, Result<Preset, String>)>,
    pub delete: Option<String>,
    pub save: Option<String>,
}

/// The preset list (the dropdown's and the device title bar's, 326-M): factory and user presets,
/// Save As and Delete. `name` is the Save As field's text.
pub(crate) fn list(ui: &mut Ui, ins: &Insert, info: &'static PluginInfo, root: Option<&Path>, user: &[String], name: &mut String) -> Picked {
    let mut out = Picked::default();
    let factory = factory_for(info.id);
    if !factory.is_empty() {
        ui.label(egui::RichText::new("Factory").size(10.5).color(TOKENS.text_dim));
    }
    for (n, p) in factory {
        if crate::chrome_pick::choice_label(ui, ins.preset == n, n).clicked() {
            out.pick = Some((n.to_string(), Ok(p)));
        }
    }
    if !user.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("User").size(10.5).color(TOKENS.text_dim));
    }
    for n in user {
        if crate::chrome_pick::choice_label(ui, ins.preset == *n, n).clicked() {
            out.pick = Some((n.clone(), root.map_or(Err("no config folder".into()), |r| load_user(r, info.id, n))));
        }
    }
    ui.separator();
    ui.horizontal(|ui| {
        let field = ui.add(egui::TextEdit::singleline(name).hint_text("Save As…").desired_width(100.0));
        let can_save = root.is_some() && !file_name(name).is_empty();
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_enabled(can_save, egui::Button::new("Save")).clicked() || (can_save && entered) {
            out.save = Some(name.clone());
        }
    });
    if user.contains(&ins.preset) && ui.button(format!("Delete \"{}\"", ins.preset)).clicked() {
        out.delete = Some(ins.preset.clone());
    }
    // The list holds an editable field, so it closes on a click outside only (see
    // `chrome_pick::dropdown_with_fields`); what ends the choice closes it here.
    if out.pick.is_some() || out.save.is_some() || out.delete.is_some() {
        ui.close();
    }
    out
}

/// Carries out what the user chose in a preset list: recall, Save As (and clear the field), Delete.
/// Whether `info`'s device has presets: a built-in preset holds parameter values, so a device with
/// none (the Sampler, whose kit presets come later) has nothing to save, recall or list.
pub fn available(info: &PluginInfo) -> bool {
    !info.params.is_empty()
}

pub(crate) fn act(app: &mut StudioApp, ui: &Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, picked: Picked, name_id: egui::Id) {
    if !available(info) {
        return;
    }
    let root = app.services.config_dir.clone();
    if let Some((n, p)) = picked.pick {
        match p {
            Ok(p) => app.apply_preset(key, info, &n, &p),
            Err(e) => app.status = format!("Preset: {e}"),
        }
    }
    let Some(root) = root else { return };
    if let Some(n) = picked.save {
        let values: BTreeMap<String, f32> =
            info.params.iter().map(|p| (p.id.to_string(), p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)))).collect();
        match save_user(&root, info, &n, &values) {
            Ok(n) => {
                // The name shows in the header; a note-only recall of the same values.
                if key.1 != transit_mix::INSTRUMENT_SLOT {
                    let _ = app.execute_ui("mix.insert_params", &json!({"track": key.0.0, "slot": key.1, "params": {}, "preset": n}));
                }
                ui.data_mut(|d| d.insert_temp(name_id, String::new()));
            }
            Err(e) => app.status = format!("Preset: {e}"),
        }
    }
    if let Some(n) = picked.delete
        && let Err(e) = delete_user(&root, info.id, &n)
    {
        app.status = format!("Preset: {e}");
    }
}

/// The preset dropdown in an editor window: factory and user presets, Save As and Delete.
pub fn control(app: &mut StudioApp, ui: &mut Ui, key: (TrackId, usize), ins: &Insert, info: &'static PluginInfo, out: &mut [f32; 4]) {
    let root = app.services.config_dir.clone();
    let user = root.as_deref().map(|r| user_names(r, info.id)).unwrap_or_default();
    let current = if ins.preset.is_empty() { "Preset" } else { ins.preset.as_str() };
    let name_id = ui.id().with(("preset_name", ins.uid.get()));
    let mut name: String = ui.data_mut(|d| d.get_temp(name_id)).unwrap_or_default();
    let mut picked = Picked::default();
    let r = crate::chrome_pick::Combo::new(("builtin_preset", ins.uid.get()), current)
        .width(150.0)
        .dense()
        .keep_open()
        .show(ui, |ui| picked = list(ui, ins, info, root.as_deref(), &user, &mut name));
    *out = [r.response.rect.min.x, r.response.rect.min.y, r.response.rect.max.x, r.response.rect.max.y];
    ui.data_mut(|d| d.insert_temp(name_id, name));
    act(app, ui, key, ins, info, picked, name_id);
}

#[cfg(test)]
mod retired_tests {
    use super::*;

    #[test]
    fn a_user_preset_for_a_retired_device_is_reported_and_the_others_are_not() {
        let root = std::env::temp_dir().join(format!("transit-retired-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (dir, file, text) in [
            ("chorus", "Wide", r#"{"plugin": "chorus", "version": 1, "params": {"rate": 1.0}}"#),
            ("compressor", "Mine", r#"{"plugin": "compressor", "version": 1, "params": {}}"#),
            ("compressor", "Strayed", r#"{"plugin": "maximizer", "version": 1, "params": {}}"#),
        ] {
            std::fs::create_dir_all(root.join("presets").join(dir)).unwrap();
            std::fs::write(root.join("presets").join(dir).join(format!("{file}.json")), text).unwrap();
        }
        let lines = retired_user_preset_lines(&root);
        assert_eq!(
            lines,
            [
                "presets: user preset \"Wide\" skipped: it is for the retired Chorus (presets/chorus)",
                "presets: user preset \"Strayed\" skipped: it is for the retired Maximizer (presets/compressor)",
            ]
        );
        assert!(retired_user_preset_lines(&root.join("none")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_sampler_user_presets_of_the_retired_one_sample_instrument_are_reported_and_not_listed() {
        let root = std::env::temp_dir().join(format!("transit-retired-sampler-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("presets").join("sampler");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Plucky.json");
        std::fs::write(&file, r#"{"plugin": "sampler", "version": 1, "params": {"root_note": 48.0}}"#).unwrap();
        assert_eq!(
            retired_user_preset_lines(&root),
            ["presets: user preset \"Plucky\" skipped: it is for the retired one-sample Sampler (presets/sampler)"]
        );
        assert!(user_names(&root, "sampler").is_empty(), "the Sampler lists no preset");
        assert!(file.exists(), "the file is left on disk");
        let _ = std::fs::remove_dir_all(&root);
    }
}
