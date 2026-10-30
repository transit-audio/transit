//! Plug-in editor windows follow the main window (issue #173, ADR 0030).
//!
//! The editor windows live on the plug-in host thread with no owner window (ADR 0018 section 7:
//! an owner on another thread would attach the two threads' input queues, so a hung plug-in would
//! hang this UI). This module groups them by hand: every frame it reads the main window's focus
//! and minimized state, and on a change it posts a request to the editors' threads
//! (`Player::editor_windows`), which apply it to their own windows. Posting never waits for a host
//! thread, so a plug-in that stops answering leaves this frame loop running.
//!
//! | Main window | Editors |
//! |---|---|
//! | minimized | parked: the shown ones are hidden and remembered |
//! | restored | the parked ones come back where they were, unactivated |
//! | activated | the shown ones are brought above it, unactivated |
//! | deactivated | nothing: they go behind the other application with the main window |
//!
//! Deactivation lowers rather than hides, as owned windows do on Windows and as Ableton Live and
//! Logic Pro keep plug-in windows on screen when another application is in front. Selecting a
//! plug-in that has an editor open (a click on its mixer insert, the device card's title, the
//! instrument menu) raises that window instead of opening another ([`StudioApp::show_editor`]).
//! Editors that follow the main window take the tool-window style (no taskbar button, not in
//! Alt-Tab); `UiState::editors_follow` off keeps the behavior from before #173.

use crate::StudioApp;
use crate::audio_connection::Editors;
use serde_json::{Value, json};
use std::collections::VecDeque;
use transit_dsp::EditorWindowRequest as R;
use transit_model::TrackId;

/// How many posted requests `ui.inspect` lists.
const POSTED_SHOWN: usize = 16;

/// The main window as the grouping last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Main {
    pub focused: bool,
    pub minimized: bool,
}

impl Main {
    fn name(self) -> &'static str {
        match (self.minimized, self.focused) {
            (true, _) => "minimized",
            (false, true) => "active",
            (false, false) => "inactive",
        }
    }
}

/// The grouping's memory between frames.
#[derive(Debug, Default)]
pub struct EditorGroup {
    seen: Option<Main>,
    /// The follow setting the editors' threads were last told (`None`: not yet).
    follow_sent: Option<bool>,
    /// The requests posted, newest last (at most [`POSTED_SHOWN`]), for `ui.inspect`.
    posted: VecDeque<R>,
}

/// What a change of the main window from `before` to `now` asks of editors that follow it.
pub(crate) fn requests(before: Main, now: Main) -> Vec<R> {
    if now.minimized {
        return if before.minimized { Vec::new() } else { vec![R::Park] };
    }
    let mut out = Vec::new();
    if before.minimized {
        out.push(R::Unpark);
    }
    if now.focused && (!before.focused || before.minimized) {
        out.push(R::Front);
    }
    out
}

/// The editors' connection: the fake devices' when the tests (or `audio.fake_devices`) set one,
/// else the player's.
fn connection(app: &StudioApp) -> Option<&dyn Editors> {
    if let Some(fake) = app.audio_devices.fake.as_ref().and_then(|f| f.connection.as_ref()) {
        return Some(fake);
    }
    app.player.as_ref().map(|p| p as &dyn Editors)
}

/// Once per frame, before the editors are driven: the main window's change, posted.
pub(crate) fn drive(app: &mut StudioApp, ctx: &egui::Context) {
    let (focused, minimized) = ctx.input(|i| (i.viewport().focused, i.viewport().minimized));
    let before = app.editor_group.seen.unwrap_or_default();
    let now = Main { focused: focused.unwrap_or(before.focused), minimized: minimized.unwrap_or(before.minimized) };
    app.editor_group.seen = Some(now);
    let follow = app.ui.editors_follow;
    let mut out = Vec::new();
    if app.editor_group.follow_sent != Some(follow) {
        out.push(R::Follow(follow));
    }
    if follow {
        out.extend(requests(before, now));
    }
    if out.is_empty() {
        return;
    }
    // No player yet: no editor exists. The setting is sent once one is there.
    let Some(c) = connection(app) else { return };
    for r in &out {
        c.editor_windows(*r);
    }
    let group = &mut app.editor_group;
    group.follow_sent = Some(follow);
    for r in out {
        if group.posted.len() == POSTED_SHOWN {
            group.posted.pop_front();
        }
        group.posted.push_back(r);
    }
}

fn request_name(r: R) -> &'static str {
    match r {
        R::Park => "park",
        R::Unpark => "unpark",
        R::Front => "front",
        R::Follow(true) => "follow",
        R::Follow(false) => "no_follow",
    }
}

/// `ui.inspect`'s `editor_windows`: the setting, the main window as last seen, the requests posted
/// (newest last) and each wanted editor's state: `opening`, `not_responding`, `shown`, `parked`,
/// `minimized`, `hidden` (closed by the user, being torn down), `open` (no window state known, as
/// on macOS) or `closed`.
pub(crate) fn inspect(app: &StudioApp) -> Value {
    let c = connection(app);
    let editors: Vec<Value> = app
        .ui
        .plugin_windows
        .iter()
        .map(|&(track, slot)| json!({"track": track, "slot": slot, "state": editor_state(app, c, track, slot)}))
        .collect();
    json!({
        "follow": app.ui.editors_follow,
        "main": app.editor_group.seen.map(Main::name),
        "posted": app.editor_group.posted.iter().map(|r| request_name(*r)).collect::<Vec<_>>(),
        "editors": editors,
    })
}

fn editor_state(app: &StudioApp, c: Option<&dyn Editors>, track: TrackId, slot: usize) -> &'static str {
    if app.builtin_at((track, slot)).is_some() {
        return "builtin";
    }
    let Some(c) = c else { return "closed" };
    if let Some(p) = &app.player {
        if p.editor_opening(track, slot).is_some() {
            return "opening";
        }
        if p.editor_available(track, slot).is_err() {
            return "not_responding";
        }
    }
    match (c.editor_open(track, slot), c.editor_window(track, slot)) {
        (_, Some(state)) => match serde_json::to_value(state).ok().as_ref().and_then(Value::as_str) {
            Some("shown") => "shown",
            Some("parked") => "parked",
            Some("minimized") => "minimized",
            _ => "hidden",
        },
        (true, None) => "open",
        (false, None) => "closed",
    }
}

/// The open editors (track, slot): `ui.inspect`'s `editors_open`, from the fake devices too.
pub(crate) fn open(app: &StudioApp) -> Vec<(TrackId, usize)> {
    let Some(c) = connection(app) else { return Vec::new() };
    app.ui.plugin_windows.iter().copied().filter(|&(t, s)| c.editor_open(t, s)).collect()
}

impl StudioApp {
    /// Selecting a plug-in (its mixer insert, the device card's title, the instrument menu): its
    /// editor opens, or, when it is open already, its window comes to the front without taking the
    /// focus (issue #173). Never a second window.
    pub fn show_editor(&mut self, key: (TrackId, usize)) {
        let open = self.ui.plugin_windows.contains(&key) && connection(self).is_some_and(|c| c.editor_open(key.0, key.1));
        if open {
            if let Some(c) = connection(self) {
                c.raise_editor(key.0, key.1);
            }
            return;
        }
        self.want_editor(key);
    }

    /// `window.editors_follow`: plug-in editor windows follow the main window (`value`, or flip).
    pub(crate) fn set_editors_follow(&mut self, value: Option<bool>) -> Value {
        self.ui.editors_follow = value.unwrap_or(!self.ui.editors_follow);
        json!({"editors_follow": self.ui.editors_follow})
    }
}

/// `ui.window`: minimizes or restores the main window, or asks for it to be focused, so the
/// coordinator can drive the grouping's transitions through the control channel (issue #173).
pub(crate) fn window_command(ctx: &egui::Context, p: &Value) -> Value {
    if let Some(m) = p.get("minimized").and_then(Value::as_bool) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(m));
    }
    if p.get("focus").and_then(Value::as_bool) == Some(true) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
    ctx.request_repaint();
    json!({})
}

#[cfg(test)]
#[path = "editor_group_tests.rs"]
mod tests;
