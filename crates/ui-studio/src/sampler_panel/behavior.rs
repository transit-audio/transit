//! What the Pads view's gestures do, through the C2 commands (ADR 0041; `cmd/sampler_patch.rs`),
//! one undo step per gesture:
//!
//! - **Press a pad:** it becomes the selected pad, and an occupied pad auditions: a note-on of its
//!   trigger note at the piano roll's audition velocity through the player's live note path (the
//!   path MIDI takes, ADR 0041's "a press uses the same event path as MIDI"), its note-off when the
//!   pointer is let go. Selection is UI state: no undo step.
//! - **Drop an audio file on a pad:** one `mix.sampler_zone_source` with the file's path and the
//!   pad (the zone the pad is bound to, else the lowest free zone slot): the command decodes the
//!   file through the checked file access, admits it (ADR 0041's bounds), sets the zone and binds
//!   the pad in one atomic step. A file is referenced where it is (the command's default).
//! - **Drag from one pad to another:** the two pads swap their bindings (their zones, each with its
//!   playback and slot controls) and their names; trigger notes, choke groups, mute and solo stay
//!   with the pads. Two or three `mix.sampler_pad_bind`/`mix.sampler_pad_unbind` steps and the name
//!   steps run under one merge key: one undo step.
//! - **Choke picker, M, S:** one `mix.sampler_pad_set` each. **Rename:** the strip's name, in place
//!   (`mix.sampler_pad_set` name, through the app's rename field).
//! - **Start note** (the view bar, [`super::bar`]): `mix.sampler_pad_start`, merged while the
//!   pointer is held or the wheel turns; a typed value one step.
//! - **Zone knobs:** `mix.sampler_zone_edit` (a playback field) or `mix.sampler_control` (level,
//!   pan), merged while the pointer is held or the wheel turns; the trigger mode, Reverse and Key
//!   track one `mix.sampler_zone_edit` each.
//!
//! A refused command (an admission bound, a note in use) leaves the document as it was; its
//! message becomes the panel's note (and the status line) until the next edit that lands.

use super::{PadData, SamplerLayout, view};
use crate::StudioApp;
use crate::device_editor::Dev;
use serde_json::{Value, json};
use transit_model::sampler_patch::{PadEntry, PatchV1, ZONES, ZoneEntry};
use transit_model::{Session, TrackId};

/// A pad's name as the view shows it: its explicit name, else its zone's label, else the sample's
/// file stem, else nothing (ADR 0041's naming order, before the `Pad A1` fallback the view does not
/// need: the id is on the pad).
pub fn pad_name(s: &Session, patch: &PatchV1, entry: &PadEntry, zone: Option<&ZoneEntry>) -> String {
    if let Some(n) = entry.name.as_deref().filter(|n| !n.is_empty()) {
        return n.to_string();
    }
    let Some(z) = zone else { return String::new() };
    if let Some(l) = z.label.as_deref().filter(|l| !l.is_empty()) {
        return l.to_string();
    }
    let source = patch.sources.get(usize::from(z.source)).and_then(|r| s.source(r.source));
    source.map(|src| std::path::Path::new(&src.name).file_stem().and_then(|x| x.to_str()).unwrap_or(&src.name).to_string()).unwrap_or_default()
}

/// The outcome of a command: a refusal becomes the view's note.
fn landed(app: &mut StudioApp, uid: u64, r: Result<Value, String>) -> bool {
    match r {
        Ok(_) => {
            view(app, uid).note = None;
            true
        }
        Err(e) => {
            app.status = e.clone();
            view(app, uid).note = Some(e);
            false
        }
    }
}

fn track(dev: Dev) -> TrackId {
    dev.key.0
}

/// A one-off command (its own undo step).
pub(crate) fn run(app: &mut StudioApp, dev: Dev, id: &str, mut p: Value) -> bool {
    if let Some(o) = p.as_object_mut() {
        o.insert("track".into(), json!(track(dev).0));
    }
    let r = app.run(id, p);
    landed(app, dev.uid(), r)
}

/// A command merged into the gesture in progress under `key`.
pub(crate) fn merged(app: &mut StudioApp, dev: Dev, id: &str, mut p: Value, key: &str) -> bool {
    if let Some(o) = p.as_object_mut() {
        o.insert("track".into(), json!(track(dev).0));
    }
    app.builtin_editor.gesture = true;
    let r = app.execute_ui_merged(id, &p, key).map_err(|e| e.to_string());
    landed(app, dev.uid(), r)
}

/// `mix.sampler_pad_set` on pad `pad` (0-based) with `fields` (choke, mute, solo, name, note).
pub(crate) fn set_pad(app: &mut StudioApp, dev: Dev, pad: u8, fields: Value) -> bool {
    let mut p = fields;
    if let Some(o) = p.as_object_mut() {
        o.insert("pad".into(), json!(pad + 1));
    }
    run(app, dev, "mix.sampler_pad_set", p)
}

/// One `mix.sampler_zone_edit` (a toggle or a segment).
pub(crate) fn zone_once(app: &mut StudioApp, dev: Dev, zone: u16, settings: Value) -> bool {
    let mut p = settings;
    if let Some(o) = p.as_object_mut() {
        o.insert("zone".into(), json!(zone + 1));
    }
    run(app, dev, "mix.sampler_zone_edit", p)
}

/// A knob's `mix.sampler_zone_edit`, merged while its gesture lasts.
pub(crate) fn zone_merged(app: &mut StudioApp, dev: Dev, zone: u16, field: &str, settings: Value) -> bool {
    let mut p = settings;
    if let Some(o) = p.as_object_mut() {
        o.insert("zone".into(), json!(zone + 1));
    }
    let key = format!("sampler_zone:{}:{}:{field}", track(dev).0, zone);
    merged(app, dev, "mix.sampler_zone_edit", p, &key)
}

/// A slot control's `mix.sampler_control`, merged while its gesture lasts.
pub(crate) fn control(app: &mut StudioApp, dev: Dev, id: &str, value: f32) -> bool {
    let key = format!("sampler_control:{}:{id}", track(dev).0);
    merged(app, dev, "mix.sampler_control", json!({"id": id, "value": value}), &key)
}

/// New Kit: the instrument gets an empty patch and the window shows the pads.
pub(crate) fn new_kit(app: &mut StudioApp, dev: Dev) -> bool {
    let landed = run(app, dev, "mix.sampler_new_patch", json!({}));
    if landed {
        view(app, dev.uid()).view = super::View::Pads;
    }
    landed
}

/// The zone a sample dropped on pad `pad` goes to: the pad's own, else the lowest free slot.
pub(crate) fn zone_for(patch: &PatchV1, pad: u8) -> Option<u16> {
    patch.pad(pad).zone.or_else(|| (0..ZONES as u16).find(|z| patch.zone(*z).is_none()))
}

/// An audio file dropped on pad `pad`: its zone takes the file and the pad is bound, one step.
pub(crate) fn drop_file(app: &mut StudioApp, dev: Dev, pad: u8, path: &str) -> bool {
    let Some(patch) = super::patch_at(app.engine.session(), dev.key) else { return false };
    let Some(zone) = zone_for(&patch, pad) else {
        let msg = "Every zone slot is in use: clear a zone first".to_string();
        return landed(app, dev.uid(), Err(msg));
    };
    view(app, dev.uid()).selected = pad;
    run(app, dev, "mix.sampler_zone_source", json!({"zone": zone + 1, "pad": pad + 1, "path": path}))
}

/// The commands that swap pads `a` and `b`: their bindings and their explicit names.
pub(crate) fn swap_steps(patch: &PatchV1, a: u8, b: u8) -> Vec<(&'static str, Value)> {
    let (pa, pb) = (patch.pad(a), patch.pad(b));
    let mut steps = Vec::new();
    match (pa.zone, pb.zone) {
        (Some(za), Some(zb)) => {
            steps.push(("mix.sampler_pad_unbind", json!({"pad": b + 1})));
            steps.push(("mix.sampler_pad_bind", json!({"pad": a + 1, "zone": zb + 1})));
            steps.push(("mix.sampler_pad_bind", json!({"pad": b + 1, "zone": za + 1})));
        }
        (Some(za), None) => {
            steps.push(("mix.sampler_pad_unbind", json!({"pad": a + 1})));
            steps.push(("mix.sampler_pad_bind", json!({"pad": b + 1, "zone": za + 1})));
        }
        (None, Some(zb)) => {
            steps.push(("mix.sampler_pad_unbind", json!({"pad": b + 1})));
            steps.push(("mix.sampler_pad_bind", json!({"pad": a + 1, "zone": zb + 1})));
        }
        (None, None) => return steps,
    }
    if pa.name != pb.name {
        steps.push(("mix.sampler_pad_set", json!({"pad": a + 1, "name": pb.name})));
        steps.push(("mix.sampler_pad_set", json!({"pad": b + 1, "name": pa.name})));
    }
    steps
}

/// Swaps pads `a` and `b` in one undo step (a fresh merge key; nothing merges into it after).
pub(crate) fn swap(app: &mut StudioApp, dev: Dev, a: u8, b: u8) -> bool {
    let Some(patch) = super::patch_at(app.engine.session(), dev.key) else { return false };
    let steps = swap_steps(&patch, a, b);
    if steps.is_empty() || a == b {
        return false;
    }
    app.engine.end_merge();
    let key = format!("sampler_swap:{}:{a}:{b}:{}", track(dev).0, app.engine.undo_history().len());
    let before = app.engine.session_arc();
    for (id, p) in steps {
        let mut p = p;
        if let Some(o) = p.as_object_mut() {
            o.insert("track".into(), json!(track(dev).0));
        }
        if let Err(e) = app.execute_ui_merged(id, &p, &key) {
            // Not expected (the steps are worked out from the patch they edit): put the document
            // back as it was before the first step.
            app.engine.end_merge();
            if !std::sync::Arc::ptr_eq(&before, &app.engine.session_arc()) {
                app.engine.undo();
            }
            return landed(app, dev.uid(), Err(e.to_string()));
        }
    }
    app.engine.end_merge();
    view(app, dev.uid()).selected = b;
    landed(app, dev.uid(), Ok(Value::Null))
}

/// The piano roll's audition velocity (the app's one velocity for notes played from the screen).
fn audition_velocity(app: &StudioApp) -> u8 {
    app.ui.keys.velocity.clamp(1, 127)
}

/// Starts an audition of `note` on the track (a note-on through the player's live path).
fn audition(app: &mut StudioApp, dev: Dev, note: u8) {
    let velocity = audition_velocity(app);
    release(app, dev);
    let v = view(app, dev.uid());
    v.auditions += 1;
    v.last_audition = Some((note, velocity));
    if let Some(p) = &app.player
        && app.player_requests.result(&mut app.status, p.note(track(dev), note, velocity)).is_none()
    {
        return;
    }
    view(app, dev.uid()).held = Some(note);
}

/// Ends a held audition.
fn release(app: &mut StudioApp, dev: Dev) {
    let Some(note) = view(app, dev.uid()).held.take() else { return };
    if let Some(p) = &app.player {
        let _ = app.player_requests.result(&mut app.status, p.note(track(dev), note, 0));
    }
}

/// A pad's input this frame: a press selects it (and auditions an occupied pad), a drag that
/// starts on it remembers it for a swap.
pub(crate) fn pad_input(app: &mut StudioApp, ui: &egui::Ui, dev: Dev, index: u8, d: &PadData, resp: &egui::Response) {
    let pressed = ui.input(|i| i.pointer.primary_pressed()) && resp.contains_pointer();
    if pressed {
        view(app, dev.uid()).selected = index;
        if d.zone.is_some() {
            audition(app, dev, d.note);
        }
    }
    if resp.drag_started() && d.zone.is_some() {
        view(app, dev.uid()).drag = Some(index);
    }
}

/// The pointer let go: a held audition ends; a drag from a pad released on another pad swaps them.
pub(crate) fn end_drag(app: &mut StudioApp, ctx: &egui::Context, dev: Dev, out: &SamplerLayout) {
    let (released, pos) = ctx.input(|i| (i.pointer.primary_released(), i.pointer.latest_pos()));
    if !released {
        return;
    }
    release(app, dev);
    let Some(from) = view(app, dev.uid()).drag.take() else { return };
    let target = pos.and_then(|p| out.pads.iter().find(|r| super::grid_rect(r.pad.rect).contains(p)).map(|r| r.index));
    if let Some(to) = target.filter(|t| *t != from) {
        swap(app, dev, from, to);
    }
}

/// The drop this window claimed (#368): the first audio file, if it landed on a pad, becomes its
/// sample; anywhere else in the window it is refused.
pub(crate) fn drops(app: &mut StudioApp, dev: Dev, out: &SamplerLayout) {
    let Some(offer) = crate::drops::take(app, dev.key) else { return };
    let Some(at) = offer.pos else { return };
    let Some(index) = out.pads.iter().find(|r| super::grid_rect(r.pad.rect).contains(at)).map(|r| r.index) else { return };
    if let Some(path) = offer.paths.iter().find(|p| p.is_absolute() && crate::builtin_editor::is_audio_name(&p.to_string_lossy())) {
        drop_file(app, dev, index, &path.to_string_lossy());
    }
}
