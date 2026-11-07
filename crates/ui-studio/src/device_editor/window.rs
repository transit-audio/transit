//! The floating window of a device panel in the design language (326-M; the maintainer decided on
//! 2026-11-06 that devices stay floating windows): one window per insert, keyed by `Insert.uid`.
//! Every migrated panel (the Macro Oscillator, the Compressor) draws through [`show`] with its own
//! [`Panel`]; the state between frames ([`State`]) and the frame logic (Escape, follow selection,
//! Close All Editors, the remembered positions) are this one implementation.
//!
//! - **Frame:** the device window rule: the `bg.window` ground its panels sit on, a 1 px
//!   `line.strong` edge, radius 8, no shadow, no egui title bar: the device's own A bar, which
//!   moves the window when dragged. The body scrolls between the bar and the footer.
//! - **Content-sized, position remembered:** the shared device window policy
//!   ([`crate::device_window`], 326-X): the panel's design width and minimum, its content's height
//!   recomputed every frame (a section that opens or closes re-fits the window that frame), no
//!   resize handle, anchored at the top left. Positions are remembered per instance while the app
//!   runs ([`State::rects`], keyed by `Insert.uid`, unique within a launch and not saved) and per
//!   device across launches (`UiState::device_windows`, written when a window closes). An
//!   instance opens where it was left; one never opened in this launch takes the device's
//!   position, moved 24 px down and right while it would land on an open window.
//! - **Escape** closes the focused editor: the one last clicked or opened, until a click lands
//!   outside every editor window; a text field, a popup or a dialog keeps Escape.
//! - **Follow selection** (the bar's link toggle, `UiState::editor_follow`): the window shows the
//!   focused track's device in the same slot when it is the same device, and stays where it is
//!   (Logic's Link swaps the content, not the window): while it follows it writes no instance's
//!   position, and when follow is turned off its position becomes the shown instance's. Off, it
//!   stays on its insert.
//! - **Window > Close All Editors** (`window.close_editors`) closes every plug-in editor.

use super::Dev;
use super::title_bar::{self, TitleLayout};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_window::{self, Positions, Widths};
use crate::tokens::layout as space;
use egui::{Pos2, Rect, Sense, Vec2, pos2, vec2};
use serde_json::{Value, json};
use std::collections::HashMap;
use transit_model::{Insert, TrackId};

/// What a panel tells the shared window about itself.
pub(crate) trait Panel {
    /// The panel's record of what it drew (its `ui.inspect` struct).
    type Record: Default;
    /// The panel's design and minimum widths (the minimum after a test's override).
    fn widths(app: &StudioApp) -> Widths;
    /// The footer's height under the body.
    fn footer_h() -> f32;
    /// The body's height at `width` (the window's inside).
    fn content_height(app: &StudioApp, ctx: &egui::Context, key: (TrackId, usize), width: f32) -> f32;
    /// Draws the body (below the title bar, above the footer).
    fn body(app: &mut StudioApp, ui: &mut egui::Ui, dev: Dev, lay: &mut EditorLayout, rec: &mut Self::Record);
    /// Draws the footer into `rect`.
    fn footer(app: &StudioApp, ui: &mut egui::Ui, rect: Rect, lay: &mut EditorLayout, rec: &mut Self::Record);
    /// The record's title bar.
    fn title(rec: &mut Self::Record) -> &mut TitleLayout;
    /// Notes the window's rect in the record.
    fn set_window(rec: &mut Self::Record, rect: [f32; 4]);
    /// The id of the knob whose value is being typed, if one is (that field keeps Escape).
    fn typing(rec: &Self::Record, uid: u64) -> Option<egui::Id>;
    /// Files the record in the editor's layout.
    fn store(layout: &mut EditorLayout, rec: Self::Record);
}

/// Whether `plugin`'s editor is one of the migrated device panels.
pub fn handles(plugin: &str) -> bool {
    crate::macro_osc_panel::handles(plugin)
}

/// The windows' memory between frames.
#[derive(Debug, Default)]
pub struct State {
    /// The title bars' output meters' displayed levels by insert uid.
    pub meters: HashMap<u64, f32>,
    /// Rows Add route revealed, by insert uid (view state for the session; the Macro Oscillator).
    pub pending: HashMap<u64, Vec<usize>>,
    /// The editor Escape closes.
    pub focused: Option<u64>,
    /// The editor whose model picker holds the arrow keys (from a click on a model until a press
    /// lands elsewhere; the Macro Oscillator).
    pub picker: Option<u64>,
    /// The Macro Oscillator's hero wells' rendered pictures and their worker (326-W).
    pub wave: crate::macro_osc_panel::wave::Preview,
    /// Windows that follow selection moved to this frame: they open without taking the Escape
    /// focus (the click that changed the track was not in them).
    no_focus: Vec<u64>,
    /// The editors with a value being typed, and the knob's id (that field keeps Escape).
    pub typing: Vec<(u64, egui::Id)>,
    /// The windows drawn last frame and this frame, with their rects and layers.
    shown: Vec<Shown>,
    shown_now: Vec<Shown>,
    /// Bumped each time a window opens, so it opens fresh at its remembered position.
    generation: HashMap<u64, u32>,
    /// Where each open window sits (its bar moves it).
    pos: HashMap<u64, Pos2>,
    /// Each instance's last position (the window's top left) by insert uid, for this launch (it
    /// survives the window closing).
    pub rects: Positions,
    /// A knob took a Ctrl+wheel nudge last frame: the frame logic keeps the merge open, so a wheel
    /// gesture is one undo step; a frame without one (the wheel stopped, or the pointer left the
    /// knob) lets the merge end.
    pub wheeling: bool,
    pub(crate) wheel_now: bool,
    /// Where a window that follow selection just moved to an instance opens: where it was.
    carry: HashMap<u64, Pos2>,
    /// A lower minimum width, so a test can lay a panel out narrower than its minimum allows (the
    /// minimum's negative control).
    #[cfg(test)]
    pub min_override: Option<f32>,
}

/// A window drawn this frame: its insert, rect, layer and device id.
#[derive(Debug, Clone, Copy)]
struct Shown(u64, Rect, egui::LayerId, &'static str);

/// The minimum width a panel whose own minimum is `min` keeps (a test may lower it).
pub fn min_width(app: &StudioApp, min: f32) -> f32 {
    #[cfg(test)]
    if let Some(w) = app.builtin_editor.devices.min_override {
        return w;
    }
    let _ = app;
    min
}

impl StudioApp {
    /// `window.close_editors`: closes every plug-in editor window (built-in and hosted).
    pub(crate) fn close_all_editors(&mut self) -> Value {
        let keys = self.ui.plugin_windows.clone();
        for key in &keys {
            self.close_editor(*key);
        }
        json!({"closed": keys.len()})
    }

    /// The open editor key of the insert `uid`, if a migrated panel's window shows it.
    fn editor_of(&self, uid: u64) -> Option<(TrackId, usize)> {
        self.ui.plugin_windows.iter().copied().find(|k| self.builtin_at(*k).is_some_and(|i| i.uid.get() == uid && handles(&i.plugin)))
    }
}

/// Escape closes the focused editor (before the views' own Escape: a click in an editor window
/// makes it the focused one, a click elsewhere hands the keyboard back).
pub(crate) fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(uid) = app.builtin_editor.devices.focused else { return };
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() || egui::Popup::is_any_open(ctx) {
        return;
    }
    if let Some((_, knob)) = app.builtin_editor.devices.typing.iter().find(|t| t.0 == uid) {
        // The value being typed keeps Escape: it gives the typing up (the views after this would
        // take the key otherwise, egui having already let go of the field's focus).
        if crate::shortcuts::pressed(ctx, crate::shortcuts::Action::CloseEditor) {
            let id = knob.with("editing");
            ctx.data_mut(|d| d.remove::<String>(id));
        }
        return;
    }
    let Some(key) = app.editor_of(uid) else { return };
    if crate::shortcuts::pressed(ctx, crate::shortcuts::Action::CloseEditor) {
        app.close_editor(key);
        app.builtin_editor.devices.focused = None;
    }
}

/// Follow selection: each following window moves to the focused track's device in its slot when
/// that is the same device.
pub(crate) fn follow(app: &mut StudioApp) {
    let Some(focus) = app.ui.focus_track else { return };
    for key in app.ui.editor_follow.clone() {
        let target = (focus, key.1);
        let plugin = app.builtin_at(key).map(|i| i.plugin);
        let shows = app.builtin_at(target).is_some_and(|i| handles(&i.plugin) && Some(&i.plugin) == plugin.as_ref());
        if target == key || !shows || app.ui.plugin_windows.contains(&target) || !app.ui.plugin_windows.contains(&key) {
            continue;
        }
        let was = app.builtin_at(key).map(|i| i.uid.get());
        // The window stays where it is on the instance it moves to (that instance's own position
        // is left alone).
        let new_uid = app.builtin_at(target).map(|i| i.uid.get());
        let st = &mut app.builtin_editor.devices;
        st.no_focus.extend(new_uid);
        if let (Some(w), Some(n)) = (was, new_uid)
            && let Some(s) = st.shown.iter().find(|s| s.0 == w)
        {
            st.carry.insert(n, s.1.min);
        }
        for k in app.ui.plugin_windows.iter_mut().chain(app.ui.editor_follow.iter_mut()) {
            if *k == key {
                *k = target;
            }
        }
        if was.is_some() && app.builtin_editor.devices.focused == was {
            app.builtin_editor.devices.focused = app.builtin_at(target).map(|i| i.uid.get());
        }
    }
    // A following window that closed stops following.
    let open = app.ui.plugin_windows.clone();
    app.ui.editor_follow.retain(|k| open.contains(k));
}

/// Draws the window of `key` with panel `P`; returns whether it stays open.
pub(crate) fn show<P: Panel>(
    app: &mut StudioApp,
    ctx: &egui::Context,
    key: (TrackId, usize),
    ins: &Insert,
    info: &'static transit_dsp::PluginInfo,
    n: usize,
    layout: &mut EditorLayout,
) -> bool {
    let uid = ins.uid.get();
    let dev = Dev { key, ins, info };
    let screen = ctx.content_rect();
    // The content's height at the width the window will have, this frame (a section that opened
    // or closed since the last frame counts).
    let widths = P::widths(app);
    let footer_h = P::footer_h();
    let width = device_window::width(widths, screen) - device_window::edge().x;
    let size = device_window::size(widths, screen, space::TITLE_BAR_HEIGHT, P::content_height(app, ctx, key, width), footer_h);
    let fresh = !app.builtin_editor.devices.shown.iter().any(|s| s.0 == uid);
    if fresh || !app.builtin_editor.devices.pos.contains_key(&uid) {
        let open: Vec<Rect> = app.builtin_editor.devices.shown.iter().filter(|s| s.0 != uid).map(|s| s.1).collect();
        let carried = app.builtin_editor.devices.carry.remove(&uid);
        let own = carried.or_else(|| app.builtin_editor.devices.rects.get(&uid).copied());
        let device = app.ui.device_windows.get(info.id).copied();
        let at = device_window::opening_pos(own, device, &open, screen, size, n);
        let st = &mut app.builtin_editor.devices;
        *st.generation.entry(uid).or_default() += 1;
        st.pos.insert(uid, at);
        if !st.no_focus.contains(&uid) {
            st.focused = Some(uid);
        }
        st.no_focus.retain(|u| *u != uid);
    }
    let st = &mut app.builtin_editor.devices;
    // Anchored at its top left: a bottom that would leave the screen moves the window up.
    let pos = device_window::anchored(st.pos.get(&uid).copied().unwrap_or(screen.min), size, screen);
    st.pos.insert(uid, pos);
    let generation = st.generation.get(&uid).copied().unwrap_or(0);
    let follow = app.ui.editor_follow.contains(&key);
    let mut rec = P::Record::default();
    let mut bar = title_bar::TitleOut::default();
    let win = device_window::configure(
        crate::theme::device_panel_window(layout.title.clone()).id(egui::Id::new(("device_editor", uid, generation))),
        pos,
        size,
    );
    let shown = win.show(ctx, |ui| {
        let avail = ui.available_rect_before_wrap();
        let top = Rect::from_min_size(avail.min, vec2(avail.width(), space::TITLE_BAR_HEIGHT));
        bar = title_bar::show(app, ui, dev, top, follow, layout, P::title(&mut rec));
        ui.allocate_rect(top, Sense::hover());
        let body_h = (avail.height() - space::TITLE_BAR_HEIGHT - footer_h).max(0.0);
        // The body takes exactly the room between the bar and the footer (no item spacing above
        // it: the window's height is the content's).
        let body_r = Rect::from_min_size(pos2(avail.min.x, top.max.y), vec2(avail.width(), body_h));
        let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body_r).layout(egui::Layout::top_down(egui::Align::Min)));
        egui::ScrollArea::vertical()
            .id_salt(("device_editor_body", uid))
            .max_height(body_h)
            .min_scrolled_height(body_h)
            .auto_shrink([false, false])
            .show(&mut body_ui, |ui| P::body(app, ui, dev, layout, &mut rec));
        ui.allocate_rect(body_r, Sense::hover());
        let foot = Rect::from_min_size(pos2(avail.min.x, top.max.y + body_h), vec2(avail.width(), footer_h));
        P::footer(app, ui, foot, layout, &mut rec);
        ui.allocate_rect(foot, Sense::hover());
    });
    let mut open = !bar.close;
    if let Some(w) = &shown {
        let r = w.response.rect;
        layout.rect = crate::widgets::r4(r);
        P::set_window(&mut rec, layout.rect);
        // A following window writes no instance's position (see `follow`).
        if !follow {
            app.builtin_editor.devices.rects.insert(uid, r.min);
        }
        app.builtin_editor.devices.shown_now.push(Shown(uid, r, w.response.layer_id, info.id));
    }
    if bar.drag != Vec2::ZERO
        && let Some(p) = app.builtin_editor.devices.pos.get_mut(&uid)
    {
        *p += bar.drag;
    }
    if bar.toggle_follow {
        if follow {
            // Follow off: where the window is becomes the shown instance's place.
            app.builtin_editor.devices.rects.insert(uid, pos2(layout.rect[0], layout.rect[1]));
            app.ui.editor_follow.retain(|k| *k != key);
        } else {
            app.ui.editor_follow.push(key);
        }
    }
    if bar.close_all {
        app.close_all_editors();
        open = false;
    }
    let typing = P::typing(&rec, uid);
    let st = &mut app.builtin_editor.devices;
    st.typing.retain(|u| u.0 != uid);
    if let Some(knob) = typing {
        st.typing.push((uid, knob));
    }
    P::store(layout, rec);
    open
}

/// After every window is drawn: a press inside a window focuses it, a press outside all of them
/// hands the keyboard back; the windows no longer shown forget their place.
pub(crate) fn end_frame(app: &mut StudioApp, ctx: &egui::Context) {
    let st = &mut app.builtin_editor.devices;
    let now = std::mem::take(&mut st.shown_now);
    // A window that closed leaves its position as its device's, for instances never opened before.
    for gone in st.shown.iter().filter(|s| !now.iter().any(|n| n.0 == s.0)) {
        app.ui.device_windows.insert(gone.3.to_string(), [gone.1.min.x, gone.1.min.y]);
    }
    let st = &mut app.builtin_editor.devices;
    if let Some(at) = ctx.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()).flatten()) {
        let top = ctx.layer_id_at(at);
        st.focused = now.iter().find(|s| s.1.contains(at) && top == Some(s.2)).map(|s| s.0).or_else(|| {
            // A press on a popup the window opened (a list, a menu) keeps its focus.
            top.filter(|l| l.order == egui::Order::Foreground || l.order == egui::Order::Tooltip).and(st.focused)
        });
    }
    st.pos.retain(|u, _| now.iter().any(|s| s.0 == *u));
    st.meters.retain(|u, _| now.iter().any(|s| s.0 == *u));
    st.pending.retain(|u, _| now.iter().any(|s| s.0 == *u));
    st.typing.retain(|u| now.iter().any(|s| s.0 == u.0));
    if st.picker.is_some_and(|u| !now.iter().any(|s| s.0 == u)) {
        st.picker = None;
    }
    st.shown = now;
    st.wheeling = std::mem::take(&mut st.wheel_now);
}
