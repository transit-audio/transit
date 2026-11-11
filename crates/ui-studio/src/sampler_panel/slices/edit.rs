//! What the Slices view's gestures send (ADR 0041 and its amendment 2026-11-08; the commands in
//! docs/control-protocol.md), each one command and one undo step, with only the fields the
//! gesture changes. The engine decides: a refusal is its text in the panel's error banner, and the
//! UI never checks first what the engine checks. In Auto, an accepted marker edit is the engine's
//! own copy of Auto into Manual plus the edit, in one step; the view then shows the Notice with
//! Undo.

use super::SlicesView;
use super::state::Mode;
use crate::StudioApp;
use crate::device_editor::Dev;
use serde_json::{Value, json};

/// Most commands kept in the view's log (the tests read it; `ui.inspect` may too).
const SENT_KEPT: usize = 32;

/// Which panel a refusal or a last edit belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Left,
    Use,
}

/// Runs `id` with `params` (the track added) for the instance's view, logging it. A refusal is
/// the engine's text in `panel`'s banner; an accepted edit clears it.
pub(crate) fn send(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, panel: Panel, id: &str, mut params: Value) -> Option<Value> {
    if let Some(o) = params.as_object_mut() {
        o.insert("track".into(), json!(dev.key.0.0));
    }
    v.sent.push(format!("{id} {}", compact(&params)));
    if v.sent.len() > SENT_KEPT {
        v.sent.remove(0);
    }
    let (answer, error) = match app.execute_ui(id, &params) {
        Ok(a) => (Some(a), None),
        Err(e) => {
            let text = words(&e);
            app.status = text.clone();
            (None, Some(text))
        }
    };
    match panel {
        Panel::Left => v.feedback.left_error = error,
        Panel::Use => v.feedback.use_error = error,
    }
    // What the panels now say is about the document as this command left it.
    v.feedback.revision = Some(app.engine.revision);
    answer
}

/// The engine's own words for a refusal (a bad-parameters refusal without its wrapper naming the
/// command).
pub(crate) fn words(e: &transit_engine::EngineError) -> String {
    match e {
        transit_engine::EngineError::BadParams(_, m) => m.clone(),
        other => other.to_string(),
    }
}

/// The params without the track, for the log (`{"frame":1200,"op":"add"}`).
fn compact(p: &Value) -> String {
    let mut p = p.clone();
    if let Some(o) = p.as_object_mut() {
        o.remove("track");
    }
    p.to_string()
}

/// After an accepted marker edit: read the state again now (the next lines name the new set), set
/// the Success line and, when the engine copied Auto first, the Notice.
/// `what` gives the Success line and the Notice's short form (`added marker 4`).
fn landed(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, answer: &Value, auto_count: usize, what: impl FnOnce(&SlicesView) -> (String, String)) {
    refresh_view(app, dev, v);
    let (done, short) = what(v);
    if answer["copied_auto"] == true {
        let copied = format!("Copied Auto's {auto_count} markers to Manual");
        let text = if answer["fixed"] == true { format!("{copied}; the loop start stays.") } else { format!("{copied}, then {short}.") };
        v.feedback.notice = Some((text, app.engine.revision));
    }
    v.feedback.left_success = Some(done);
}

fn refresh_view(app: &mut StudioApp, dev: Dev, v: &mut SlicesView) {
    let at = v.at;
    v.snapshot.refresh(app, dev.key.0, at);
}

/// The 1-based position of the showing marker at `frame` and its time from S.
fn named(v: &SlicesView, frame: u64) -> (usize, String) {
    let Some(l) = v.snapshot.data.as_ref() else { return (0, String::new()) };
    let i = l.showing().iter().position(|s| s.frame == frame).unwrap_or(0);
    (i + 1, format!("{:.3} s", l.seconds(frame as f64)))
}

fn auto_count(v: &SlicesView) -> usize {
    v.snapshot.data.as_ref().map_or(0, |l| l.auto.len())
}

/// Click on the empty bench: one add at the frame under the pointer (the engine snaps it with the
/// loop's setting and clamps nothing: a refused add says why).
pub(crate) fn add(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, frame: u64) {
    let n = auto_count(v);
    let Some(a) = send(app, dev, v, Panel::Left, "mix.sampler_slice_marker", json!({"op": "add", "frame": frame})) else { return };
    let at = a["frame"].as_u64().unwrap_or(frame);
    landed(app, dev, v, &a, n, |v| {
        let (i, t) = named(v, at);
        (format!("Added marker {i} at {t}"), format!("added marker {i}"))
    });
    if let Some(i) = v.snapshot.data.as_ref().and_then(|l| l.showing().iter().position(|s| s.frame == at)) {
        v.selected = i;
    }
}

/// A completed handle drag (or a nudge): one move of marker `id` toward `frame`. `snap: false`
/// for a nudge, which moves by exact frames.
pub(crate) fn move_to(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, id: u32, frame: u64, snap: Option<bool>) {
    let n = auto_count(v);
    let mut p = json!({"op": "move", "id": id, "frame": frame});
    if let Some(s) = snap {
        p["snap"] = json!(s);
    }
    let Some(a) = send(app, dev, v, Panel::Left, "mix.sampler_slice_marker", p) else { return };
    let at = a["frame"].as_u64().unwrap_or(frame);
    let clamped = a["clamped"]["text"].as_str().map(str::to_string);
    landed(app, dev, v, &a, n, |v| {
        let (i, t) = named(v, at);
        let done = match clamped {
            Some(c) => format!("Moved marker {i} to {t} · {c}"),
            None => format!("Moved marker {i} to {t}"),
        };
        (done, format!("moved marker {i}"))
    });
}

/// Click on a flag (or Delete): one remove of marker `id`; its slice merges into the one before.
pub(crate) fn remove(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, id: u32, index: usize) {
    let n = auto_count(v);
    let Some(a) = send(app, dev, v, Panel::Left, "mix.sampler_slice_marker", json!({"op": "remove", "id": id})) else { return };
    landed(app, dev, v, &a, n, |v| {
        let before = index.max(1);
        let end = v.snapshot.data.as_ref().and_then(|l| l.showing().get(before - 1).map(|s| format!("{:.3} s", l.seconds(s.end as f64))));
        let done = match end {
            Some(e) if a["fixed"] != true => format!("Removed marker {}; slice {before} now runs to {e}", index + 1),
            _ => "Kept the loop start".to_string(),
        };
        (done, format!("removed marker {}", index + 1))
    });
    v.selected = index.saturating_sub(1);
}

/// A slice renamed in place from its cell: one label edit (`null` clears it).
pub(crate) fn label(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, id: u32, text: &str) {
    let n = auto_count(v);
    let name = text.trim();
    let label = if name.is_empty() { Value::Null } else { json!(name) };
    let Some(a) = send(app, dev, v, Panel::Left, "mix.sampler_slice_marker", json!({"op": "label", "id": id, "label": label})) else { return };
    let index = v.snapshot.data.as_ref().and_then(|l| l.showing().iter().position(|s| s.id == Some(id))).map_or(0, |i| i + 1);
    landed(app, dev, v, &a, n, |v| {
        let i = v.snapshot.data.as_ref().and_then(|l| l.showing().iter().position(|s| s.label.as_deref() == Some(name))).map_or(index, |i| i + 1);
        let done = if name.is_empty() { format!("Cleared slice {i}'s name") } else { format!("Named slice {i} {name}") };
        (done, format!("named slice {i}"))
    });
}

/// The mode switch: `mix.sampler_slice_mode` (one step; neither set changes).
pub(crate) fn mode(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, m: Mode) {
    let mode = if m == Mode::Auto { "auto" } else { "manual" };
    if send(app, dev, v, Panel::Left, "mix.sampler_slice_mode", json!({"mode": mode})).is_some() {
        v.selected = 0;
    }
}

/// SNAP 0×: `mix.sampler_slice_set` with `snap` only.
pub(crate) fn snap(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, on: bool) {
    send(app, dev, v, Panel::Left, "mix.sampler_slice_set", json!({"snap": on}));
}

/// Start from Auto's markers.
pub(crate) fn from_auto(app: &mut StudioApp, dev: Dev, v: &mut SlicesView) {
    let n = auto_count(v);
    if send(app, dev, v, Panel::Left, "mix.sampler_slice_manual_from_auto", json!({})).is_some() {
        refresh_view(app, dev, v);
        v.feedback.left_success = Some(format!("Started Manual from Auto's {n} markers"));
    }
}

/// Clear: Manual keeps only the loop start.
pub(crate) fn clear(app: &mut StudioApp, dev: Dev, v: &mut SlicesView) {
    if send(app, dev, v, Panel::Left, "mix.sampler_slice_manual_clear", json!({})).is_some() {
        refresh_view(app, dev, v);
        v.feedback.left_success = Some("Cleared Manual to the loop start".to_string());
        v.selected = 0;
    }
}

/// The Notice's Undo: the ordinary single undo step (Ctrl+Z does the same).
pub(crate) fn undo(app: &mut StudioApp, v: &mut SlicesView) {
    v.sent.push("edit.undo {}".to_string());
    let _ = app.run("edit.undo", json!({}));
    v.feedback.notice = None;
    v.feedback.left_success = None;
}

/// The empty bench's Load… or a dropped file: `mix.sampler_slice_loop` with the file's path. A
/// refusal is the note row under the bench; there was no loop, so nothing else changes.
pub(crate) fn load(app: &mut StudioApp, dev: Dev, v: &mut SlicesView, path: &str) {
    if send(app, dev, v, Panel::Left, "mix.sampler_slice_loop", json!({"path": path})).is_some() {
        v.feedback.note = None;
    } else {
        v.feedback.note = v.feedback.left_error.take();
    }
}
