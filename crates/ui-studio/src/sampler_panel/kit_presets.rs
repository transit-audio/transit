//! The Sampler's kit presets in the device title bar (ADR 0041, amendment 2026-11-11 "kit
//! presets", #467). The preset browser lists the kit files (`.tkit`) of the Sampler's kit folder,
//! `<config dir>/presets/sampler/`, through the engine's kit helper
//! (`transit_engine::io::kit_presets`), recalls one with `mix.sampler_kit_load`, saves the kit with
//! `mix.sampler_kit_save` and deletes a listed kit with the helper. It never calls the parameter
//! preset code (`crate::presets`).
//!
//! - **Recall** (a pick from the list, previous, next) asks first only when the kit has a zone or a
//!   loop AND has changed since this window's last recall or save; with no such record any zone or
//!   loop counts as changed. The question is the destructive-action modal: `Replace the kit?` and
//!   `12 loaded pads will be replaced by "808".`, Replace (Enter) or Cancel (Escape, nothing sent).
//!   The command carries `replace` explicitly.
//! - **Previous and next** wrap through the list; from a kit not in it, next is the first and
//!   previous the last (`title_bar::stepped`). Each step is sent with `Engine::execute_merged` under
//!   one key per track, so a run of steps with nothing else between them is one undo step back to
//!   the kit before the run; any edit ends the run. The question can only come at a run's first
//!   step: a step leaves the kit as recalled, so the next one finds it unchanged.
//! - **Save** writes over the kit file this window last recalled or saved, while the field still
//!   shows it; otherwise it opens the list, whose Save As names a new kit file. **Delete** removes
//!   the shown kit's file.
//! - **The field** reads the kit last recalled or saved in this window while the slot's preset
//!   label is still what it was then (an undo past the recall restores the old label), else the
//!   slot's label, else `<factory default>`. There is no modified mark in v0.7.0.
//! - **After a recall** the view, the bank and the selected pad stay; a selected zone the new kit
//!   does not have is cleared, and so is the Slices view's selection. The engine's notice goes to
//!   the status line, and to the panel's note row when samples are missing; a refusal shows there
//!   in the engine's words and leaves the kit.

use super::view;
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::device_editor::title_bar::{self, FACTORY_DEFAULT, FIELD_W, TitleLayout, button, paint_glyph, stepped};
use crate::help::{self, Id};
use crate::theme::TOKENS;
use crate::tokens;
use crate::widgets;
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use transit_engine::io::kit_presets as files;
use transit_model::sampler_patch::PatchV1;

const LOAD: &str = "mix.sampler_kit_load";
const SAVE: &str = "mix.sampler_kit_save";
/// The question's default button.
pub const REPLACE: &str = "Replace";
/// The question.
pub const QUESTION: &str = "Replace the kit?";

/// The kit as it stood after this window's last recall or save.
#[derive(Debug, Clone)]
pub struct Baseline {
    /// The kit file and its name (the file stem).
    pub file: PathBuf,
    pub name: String,
    /// The patch right after the recall or save: the kit has changed when the patch differs.
    pub patch: PatchV1,
    /// The slot's preset label then: the field shows `name` while the label is unchanged.
    pub label: String,
}

/// A recall waiting for the answer to the question.
#[derive(Debug, Clone)]
pub struct Pending {
    pub file: PathBuf,
    pub name: String,
    /// Previous or next (sent merged into the run of steps).
    pub step: bool,
    /// What the question says will be replaced.
    pub detail: String,
}

/// One Sampler window's kit preset state (UI state for the session).
#[derive(Debug, Clone, Default)]
pub struct KitState {
    pub baseline: Option<Baseline>,
    pub pending: Option<Pending>,
}

/// The question's id for instance `uid` (its layout is read by `chrome::modal_layout`).
pub fn modal_id(uid: u64) -> egui::Id {
    egui::Id::new(("sampler_kit_replace", uid))
}

/// Whether a run of previous and next steps goes on: the document is still the one its last step
/// left. While it does, the frame's idle `end_merge` waits, so the run's clicks stay one undo step;
/// any other change to the document ends the run (the engine ends the merge on it too).
pub fn stepping(app: &StudioApp) -> bool {
    let run = app.builtin_editor.devices.sampler.step_run.as_ref();
    run.is_some_and(|w| std::ptr::eq(w.as_ptr(), std::sync::Arc::as_ptr(&app.engine.session_arc())))
}

/// The merge key of track `t`'s previous and next steps.
fn step_key(t: u64) -> String {
    format!("sampler_kit_step:{t}")
}

/// Whether the kit is loaded: any zone or the slice loop (the amendment's item 11.3).
pub fn loaded(p: &PatchV1) -> bool {
    !p.zones.is_empty() || p.slicing.is_some()
}

/// Whether replacing `current` asks first: loaded, and changed since `baseline` (no record counts
/// as changed).
pub fn asks(baseline: Option<&Baseline>, current: &PatchV1) -> bool {
    loaded(current) && baseline.is_none_or(|b| b.patch != *current)
}

/// What the question says will be replaced by kit `name`: the loaded pads, else the zones, else
/// the slice loop.
pub fn detail(current: &PatchV1, name: &str) -> String {
    let pads = current.pads.iter().filter(|q| q.zone.is_some()).count();
    let zones = current.zones.len();
    let what = match (pads, zones) {
        (1, _) => "1 loaded pad".to_string(),
        (0, 0) => "The slice loop".to_string(),
        (0, 1) => "1 zone".to_string(),
        (0, n) => format!("{n} zones"),
        (n, _) => format!("{n} loaded pads"),
    };
    format!("{what} will be replaced by \"{name}\".")
}

/// The user's kits: names and files, as the list shows them.
fn kits(app: &StudioApp) -> Vec<(String, PathBuf)> {
    app.services.config_dir.as_deref().map(files::entries).unwrap_or_default()
}

/// The window's baseline while the field still shows it (the slot's label is unchanged since).
fn current_baseline<'a>(app: &'a StudioApp, dev: Dev) -> Option<&'a Baseline> {
    let b = app.builtin_editor.devices.sampler.views.get(&dev.uid())?.kit.baseline.as_ref()?;
    (b.label == dev.ins.preset).then_some(b)
}

/// The name the field shows.
pub(crate) fn shown(app: &StudioApp, dev: Dev) -> String {
    match current_baseline(app, dev) {
        Some(b) => b.name.clone(),
        None if !dev.ins.preset.is_empty() => dev.ins.preset.clone(),
        None => FACTORY_DEFAULT.to_string(),
    }
}

/// The browser: previous and next step through the kit list, the field lists the kits (with Save
/// As and Delete), save writes the current kit file; the question, while one is open.
pub(crate) fn browser(app: &mut StudioApp, ui: &mut Ui, dev: Dev, [prev, field, next, save]: [Rect; 4], tl: &mut TitleLayout) {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let id = |s: &str| egui::Id::new(("macro_osc_title", dev.uid(), s.to_string()));
    let kits = kits(app);
    let names: Vec<(bool, String)> = kits.iter().map(|(n, _)| (false, n.clone())).collect();
    let current = shown(app, dev);
    for (r, which, step, help_id) in [(prev, "prev", -1, Id::SamplerKitsPrevious), (next, "next", 1, Id::SamplerKitsNext)] {
        let resp = button(ui, id(which), r, !names.is_empty());
        help::hover(ui, &resp, help_id);
        if resp.clicked()
            && let Some(i) = stepped(&names, &current, step)
        {
            request(app, dev, &kits[i], true);
        }
        paint_glyph(&p, r.center(), which, t.text_muted);
    }
    field_ui(app, ui, dev, field, &kits, &current, tl);
    let resp = button(ui, id("save"), save, app.services.config_dir.is_some());
    help::hover(ui, &resp, Id::SamplerKitsSave);
    if resp.clicked() {
        save_current(app, ui, dev);
    }
    paint_glyph(&p, save.center(), "save", t.text_muted);
    ask(app, ui.ctx(), dev);
}

/// The field's popup id (`Popup::default_response_id` of the field's response).
fn popup_id(uid: u64) -> egui::Id {
    egui::Id::new(("macro_osc_preset", uid)).with("popup")
}

/// What the user chose in the kit list this frame.
#[derive(Default)]
struct Picked {
    pick: Option<usize>,
    save: Option<String>,
    delete: bool,
}

/// The name field: `surface.inset`, `User · ` in `text.faint` before a listed kit's name, the
/// chevron; a click opens the kit list.
fn field_ui(app: &mut StudioApp, ui: &mut Ui, dev: Dev, field: Rect, kits: &[(String, PathBuf)], current: &str, tl: &mut TitleLayout) {
    let t = &TOKENS;
    let resp = ui.interact(field, egui::Id::new(("macro_osc_preset", dev.uid())), Sense::click());
    help::hover(ui, &resp, Id::SamplerKitsField);
    let p = ui.painter().clone();
    p.rect(
        field,
        title_bar::BUTTON_RADIUS,
        t.surface_inset,
        Stroke::new(1.0, if resp.hovered() { t.line_strong } else { t.line }),
        StrokeKind::Inside,
    );
    let listed = kits.iter().any(|(n, _)| n == current);
    let group = if listed { "User" } else { "" };
    let mut x = field.min.x + 12.0;
    if listed {
        let r = widgets::role_text(
            &p,
            &tokens::chrome::BUTTON_FONT,
            &format!("{group} · "),
            t.text_faint,
            Align2::LEFT_CENTER,
            pos2(x, field.center().y),
        );
        x = r.max.x;
    }
    widgets::role_text(&p, &tokens::chrome::BUTTON_FONT, current, t.text, Align2::LEFT_CENTER, pos2(x, field.center().y));
    crate::device_editor::chevron(&p, pos2(field.max.x - 16.0, field.center().y), 9.0, t.text_faint);
    (tl.preset_group, tl.preset_name) = (group.to_string(), current.to_string());
    let name_id = egui::Id::new(("sampler_kit_name", dev.uid()));
    let mut name: String = ui.data_mut(|d| d.get_temp(name_id)).unwrap_or_default();
    let can_save = app.services.config_dir.is_some();
    let mut picked = Picked::default();
    crate::chrome_pick::dropdown_with_fields(&resp).width(FIELD_W).show(|ui| picked = list(ui, kits, current, can_save, &mut name));
    if picked.save.is_some() {
        name.clear();
    }
    ui.data_mut(|d| d.insert_temp(name_id, name));
    if let Some(i) = picked.pick {
        request(app, dev, &kits[i], false);
    }
    if let Some(n) = picked.save {
        save_as(app, dev, &n);
    }
    if picked.delete
        && let Some((_, file)) = kits.iter().find(|(n, _)| n == current)
    {
        delete(app, dev, file);
    }
}

/// The kit list: the user's kits under `User`, Save As, and Delete for a listed kit shown in the
/// field. `name` is the Save As field's text.
fn list(ui: &mut Ui, kits: &[(String, PathBuf)], current: &str, can_save: bool, name: &mut String) -> Picked {
    let mut out = Picked::default();
    if !kits.is_empty() {
        ui.label(egui::RichText::new("User").size(10.5).color(TOKENS.text_dim));
    }
    for (i, (n, _)) in kits.iter().enumerate() {
        if crate::chrome_pick::choice_label(ui, current == n, n).clicked() {
            out.pick = Some(i);
        }
    }
    if !kits.is_empty() {
        ui.separator();
    }
    ui.horizontal(|ui| {
        let field = ui.add(egui::TextEdit::singleline(name).hint_text("Save As…").desired_width(100.0));
        let can = can_save && !crate::presets::file_name(name).is_empty();
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let save = ui.add_enabled(can, egui::Button::new("Save"));
        help::hover(ui, &save, Id::SamplerKitsSaveAs);
        if save.clicked() || (can && entered) {
            out.save = Some(name.clone());
        }
    });
    if kits.iter().any(|(n, _)| n == current) {
        let del = ui.button(format!("Delete \"{current}\""));
        help::hover(ui, &del, Id::SamplerKitsDelete);
        out.delete = del.clicked();
    }
    // The list holds an editable field, so it closes on a click outside only; what ends the choice
    // closes it here.
    if out.pick.is_some() || out.save.is_some() || out.delete {
        ui.close();
    }
    out
}

/// A recall of kit `(name, file)`: asked first when the rule says so, else sent at once.
fn request(app: &mut StudioApp, dev: Dev, (name, file): &(String, PathBuf), step: bool) {
    let current = super::patch_at(app.engine.session(), dev.key);
    let baseline = view(app, dev.uid()).kit.baseline.clone();
    match current.filter(|c| asks(baseline.as_ref(), c)) {
        Some(c) => {
            let detail = detail(&c, name);
            view(app, dev.uid()).kit.pending = Some(Pending { file: file.clone(), name: name.clone(), step, detail });
        }
        None => recall(app, dev, file, step),
    }
}

/// The question, while one is open: drawn every frame until answered.
fn ask(app: &mut StudioApp, ctx: &egui::Context, dev: Dev) {
    let Some(pending) = view(app, dev.uid()).kit.pending.clone() else { return };
    let ask = crate::chrome::Ask { question: QUESTION.into(), detail: Some(pending.detail.clone()), default: REPLACE, cancel: "Cancel", third: None };
    match crate::chrome::modal(ctx, modal_id(dev.uid()), &ask) {
        Some(crate::chrome::Choice::Default) => {
            view(app, dev.uid()).kit.pending = None;
            recall(app, dev, &pending.file, pending.step);
        }
        Some(_) => view(app, dev.uid()).kit.pending = None,
        None => {}
    }
}

/// Sends the recall (the user's choice is made: `replace` is stated for a loaded kit) and takes in
/// the answer.
fn recall(app: &mut StudioApp, dev: Dev, file: &Path, step: bool) {
    let t = dev.key.0.0;
    let replace = super::patch_at(app.engine.session(), dev.key).is_some_and(|p| loaded(&p));
    let params = json!({"track": t, "path": file, "replace": replace});
    let reply = if step { app.execute_ui_merged(LOAD, &params, &step_key(t)).map_err(|e| e.to_string()) } else { app.run(LOAD, params) };
    if step {
        app.builtin_editor.devices.sampler.step_run = Some(std::sync::Arc::downgrade(&app.engine.session_arc()));
    }
    let uid = dev.uid();
    let refused = |app: &mut StudioApp, why: String| {
        app.status = why.clone();
        view(app, uid).note = Some(why);
    };
    let reply = match reply {
        Ok(r) if r.get("zones").is_some() => r,
        Ok(r) => {
            let why = r["notice"].as_str().map_or_else(|| "The kit was not loaded.".to_string(), str::to_string);
            return refused(app, why);
        }
        Err(e) => return refused(app, e),
    };
    let after = super::patch_at(app.engine.session(), dev.key);
    let name = reply["kit"].as_str().unwrap_or_default().to_string();
    let notice = reply["notice"].as_str().unwrap_or_default().to_string();
    let missing = reply["missing"].as_array().is_some_and(|m| !m.is_empty());
    app.status = notice.clone();
    let v = view(app, uid);
    v.note = missing.then_some(notice);
    // Selection after a recall (UI state): the view, the bank and the selected pad stay.
    if let (Some(z), Some(p)) = (v.instrument.selected, after.as_ref())
        && p.zone(z).is_none()
    {
        v.instrument.selected = None;
    }
    v.slices.selected = 0;
    v.kit.baseline = after.map(|patch| Baseline { file: file.to_path_buf(), label: name.clone(), name, patch });
}

/// Save: over the kit file the field shows from this window's last recall or save; otherwise the
/// list opens for Save As.
fn save_current(app: &mut StudioApp, ui: &Ui, dev: Dev) {
    let file = current_baseline(app, dev).map(|b| b.file.clone()).filter(|f| f.is_file());
    match file {
        Some(f) => save_to(app, dev, &f),
        None => egui::Popup::open_id(ui.ctx(), popup_id(dev.uid())),
    }
}

/// Save As under `name` in the kit folder.
fn save_as(app: &mut StudioApp, dev: Dev, name: &str) {
    match app.services.config_dir.as_deref().and_then(|root| files::path(root, name)) {
        Some(file) => save_to(app, dev, &file),
        None => app.status = "Name the kit first.".to_string(),
    }
}

/// Saves the kit to `file` through the engine; the window's baseline becomes the saved kit.
fn save_to(app: &mut StudioApp, dev: Dev, file: &Path) {
    let reply = app.run(SAVE, json!({"track": dev.key.0.0, "path": file}));
    let uid = dev.uid();
    match reply {
        Ok(r) => {
            let path = PathBuf::from(r["path"].as_str().unwrap_or_default());
            let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let mut words = vec![format!("Kit \"{name}\" saved.")];
            words.extend(r["notes"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
            app.status = words.join(" ");
            let patch = super::patch_at(app.engine.session(), dev.key);
            let label = dev.ins.preset.clone();
            let v = view(app, uid);
            v.note = None;
            v.kit.baseline = patch.map(|patch| Baseline { file: path, name, patch, label });
        }
        Err(e) => {
            app.status = e.clone();
            view(app, uid).note = Some(e);
        }
    }
}

/// Deletes kit file `file`; a baseline at that file is forgotten (the kit then counts as changed).
fn delete(app: &mut StudioApp, dev: Dev, file: &Path) {
    match files::delete(file) {
        Ok(()) => {
            let kit = &mut view(app, dev.uid()).kit;
            if kit.baseline.as_ref().is_some_and(|b| b.file == file) {
                kit.baseline = None;
            }
        }
        Err(e) => app.status = format!("Kit: {e}"),
    }
}
