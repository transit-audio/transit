//! The Macro Oscillator's floating window (326-M; the maintainer decided on 2026-11-06 that
//! devices stay floating windows): one window per insert, keyed by `Insert.uid` as before.
//!
//! - **Frame:** the device window rule: the `bg.window` ground its panels sit on, a 1 px
//!   `line.strong` edge, radius 8, no shadow, no egui title bar: the device's own A bar, which
//!   moves the window when dragged. The body scrolls between the bar and the footer.
//! - **Remembered rect:** per device id in `UiState::device_windows` (saved in `ui.json`): a device
//!   opens where it was last closed, moved and shrunk onto the screen when it no longer fits.

use super::{Dev, FOOTER_H, MIN_WIDTH, MacroLayout};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::tokens::layout as space;
use egui::{Pos2, Rect, Sense, Vec2, pos2, vec2};
use std::collections::HashMap;
use transit_model::{Insert, TrackId};

/// The shortest the window gets.
pub const MIN_HEIGHT: f32 = 360.0;
/// Room kept between a default-sized window and the screen's edges.
const SCREEN_MARGIN: f32 = 32.0;
/// Each further window opens this much down and to the right of the one before.
const CASCADE: f32 = 24.0;

/// The windows' memory between frames.
#[derive(Debug, Default)]
pub struct State {
    /// The output meters' displayed levels by insert uid.
    pub meters: HashMap<u64, f32>,
    /// Rows Add route revealed, by insert uid (view state for the session).
    pub pending: HashMap<u64, Vec<usize>>,
    /// The editor whose model picker holds the arrow keys (from a click on a model until a press
    /// lands elsewhere).
    pub picker: Option<u64>,
    /// The windows drawn last frame and this frame, with their rects and layers.
    shown: Vec<(u64, Rect, egui::LayerId)>,
    shown_now: Vec<(u64, Rect, egui::LayerId)>,
    /// Bumped each time a window opens, so it opens fresh at its remembered rect.
    generation: HashMap<u64, u32>,
    /// Where each open window sits (its bar moves it).
    pos: HashMap<u64, Pos2>,
}

/// `rect` moved and shrunk to lie on `screen`.
pub fn on_screen(rect: Rect, screen: Rect) -> Rect {
    let size = rect.size().min(screen.size());
    let x = rect.min.x.clamp(screen.min.x, screen.max.x - size.x);
    let y = rect.min.y.clamp(screen.min.y, screen.max.y - size.y);
    Rect::from_min_size(pos2(x, y), size)
}

/// Where a window opens: the device's remembered rect, else the board's width (less a margin on a
/// smaller screen), the content's height up to the screen, cascaded by `n`; on screen either way.
pub fn opening_rect(remembered: Option<[f32; 4]>, screen: Rect, content_h: f32, n: usize) -> Rect {
    let r = match remembered {
        Some([x0, y0, x1, y1]) => Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)),
        None => {
            let w = super::DESIGN_WIDTH.min(screen.width() - 2.0 * SCREEN_MARGIN).max(MIN_WIDTH);
            let h = (space::TITLE_BAR_HEIGHT + content_h + FOOTER_H).min(screen.height() - 2.0 * SCREEN_MARGIN).max(MIN_HEIGHT);
            let off = CASCADE * n as f32;
            Rect::from_min_size(pos2(screen.center().x - w / 2.0 + off, screen.min.y + SCREEN_MARGIN + off), vec2(w, h))
        }
    };
    on_screen(r, screen)
}

impl StudioApp {
}

/// Draws the window of `key`; returns whether it stays open.
pub(crate) fn show(
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
    let fresh = !app.builtin_editor.macro_osc.shown.iter().any(|s| s.0 == uid);
    let mut size = None;
    if fresh || !app.builtin_editor.macro_osc.pos.contains_key(&uid) {
        let content = super::content_height(app, key, super::DESIGN_WIDTH.min(screen.width()));
        let r = opening_rect(app.ui.device_windows.get(info.id).copied(), screen, content, n);
        let st = &mut app.builtin_editor.macro_osc;
        *st.generation.entry(uid).or_default() += 1;
        st.pos.insert(uid, r.min);
        size = Some(r.size());
    }
    let st = &app.builtin_editor.macro_osc;
    let pos = st.pos.get(&uid).copied().unwrap_or(screen.min);
    let generation = st.generation.get(&uid).copied().unwrap_or(0);
    let mut mlay = MacroLayout::default();
    let mut bar = super::title_bar::TitleOut::default();
    let mut win = crate::theme::device_panel_window(layout.title.clone())
        .id(egui::Id::new(("macro_osc_editor", uid, generation)))
        .current_pos(pos)
        .movable(false)
        .resizable(true)
        .min_size(vec2(MIN_WIDTH, MIN_HEIGHT));
    if let Some(s) = size {
        win = win.default_size(s);
    }
    let shown = win.show(ctx, |ui| {
        let avail = ui.available_rect_before_wrap();
        let top = Rect::from_min_size(avail.min, vec2(avail.width(), space::TITLE_BAR_HEIGHT));
        bar = super::title_bar::show(app, ui, dev, top, layout, &mut mlay);
        ui.allocate_rect(top, Sense::hover());
        let body_h = (avail.height() - space::TITLE_BAR_HEIGHT - FOOTER_H).max(0.0);
        egui::ScrollArea::vertical()
            .id_salt(("macro_osc_body", uid))
            .max_height(body_h)
            .min_scrolled_height(body_h)
            .auto_shrink([false, false])
            .show(ui, |ui| super::body(app, ui, dev, layout, &mut mlay));
        let foot = Rect::from_min_size(pos2(avail.min.x, top.max.y + body_h), vec2(avail.width(), FOOTER_H));
        super::footer(app, ui, foot, layout, &mut mlay);
        ui.allocate_rect(foot, Sense::hover());
    });
    let open = !bar.close;
    if let Some(w) = &shown {
        let r = w.response.rect;
        layout.rect = crate::widgets::r4(r);
        mlay.window = layout.rect;
        app.ui.device_windows.insert(info.id.to_string(), layout.rect);
        app.builtin_editor.macro_osc.shown_now.push((uid, r, w.response.layer_id));
    }
    if bar.drag != Vec2::ZERO
        && let Some(p) = app.builtin_editor.macro_osc.pos.get_mut(&uid)
    {
        *p += bar.drag;
    }
    layout.macro_osc = Some(Box::new(mlay));
    open
}

/// After every window is drawn: the windows no longer shown forget their place.
pub(crate) fn end_frame(app: &mut StudioApp, _ctx: &egui::Context) {
    let st = &mut app.builtin_editor.macro_osc;
    let now = std::mem::take(&mut st.shown_now);
    st.pos.retain(|u, _| now.iter().any(|s| s.0 == *u));
    st.meters.retain(|u, _| now.iter().any(|s| s.0 == *u));
    st.pending.retain(|u, _| now.iter().any(|s| s.0 == *u));
    if st.picker.is_some_and(|u| !now.iter().any(|s| s.0 == u)) {
        st.picker = None;
    }
    st.shown = now;
}
