//! The loop strip: a thin band at the top of the arrangement ruler. Dragging empty strip makes a
//! loop region, dragging an edge resizes it, dragging the body moves it, double-clicking turns
//! looping on or off. The region lives in the session (`edit.loop_range`, `edit.loop_playback`);
//! a gesture is only held here while it runs and lands as one `transport.loop` command on release.

use crate::StudioApp;
use crate::arrange::Map;
use crate::theme::TOKENS;
use egui::{CursorIcon, Rect, Sense, Stroke, StrokeKind, Ui, pos2};
use serde_json::json;
use transit_model::Session;
use transit_time::{Range, Samples};

/// Height of the strip, at the top of the ruler (points).
pub const STRIP_H: f32 = crate::tokens::arrangement::LOOP_STRIP_HEIGHT;
/// How close (points) to a region edge a press grabs the edge instead of the body.
pub const HANDLE: f32 = 6.0;

/// What a press on the strip took hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grab {
    /// Empty strip: a new region from `anchor` to the pointer.
    Create { anchor: Samples },
    /// The left edge; `end` stays put.
    Left { end: Samples },
    /// The right edge; `start` stays put.
    Right { start: Samples },
    /// The body, pressed at `press`; the region keeps its length.
    Body { start: Samples, end: Samples, press: Samples },
}

/// A gesture in progress: the grab and the region it currently describes (empty: none yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopDrag {
    pub grab: Grab,
    pub range: Range,
}

/// The part of a region under a press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Left,
    Right,
    Body,
    Outside,
}

/// Which part of a region spanning `xl..xr` the point `x` is on. Edges win within `handle` points
/// (the nearer one when the region is narrow).
pub fn hit(xl: f32, xr: f32, x: f32, handle: f32) -> Part {
    let (dl, dr) = ((x - xl).abs(), (x - xr).abs());
    if dl <= handle && dl <= dr {
        Part::Left
    } else if dr <= handle {
        Part::Right
    } else if x > xl && x < xr {
        Part::Body
    } else {
        Part::Outside
    }
}

/// The region a gesture describes with the pointer at `at`; `snap` rounds a position to the grid.
/// Dragging an edge past the other one swaps them; a body never moves before zero.
pub fn drag_range(grab: Grab, at: Samples, snap: impl Fn(Samples) -> Samples) -> Range {
    let r = match grab {
        Grab::Create { anchor } => Range::new(snap(anchor), snap(at)),
        Grab::Left { end } => Range::new(snap(at), end),
        Grab::Right { start } => Range::new(start, snap(at)),
        Grab::Body { start, end, press } => {
            let s = snap(start.saturating_add(at.saturating_sub(press))).max(0);
            Range { start: s, end: s.saturating_add(end.saturating_sub(start)) }
        }
    };
    Range { start: r.start.max(0), end: r.end.max(0) }
}

/// Remove the loop region (an empty pair clears it); one undo step.
pub(crate) fn clear(app: &mut StudioApp) {
    let _ = app.run("transport.loop", json!({"start": 0, "end": 0}));
}

/// Take a press-drag-release on `resp` over the region: grab a part of it (empty strip makes a new
/// region when `create`), follow the pointer within `bounds`, land one `transport.loop` on release.
/// A double-click turns looping on or off. Shared by the strip and the ruler's loop block.
fn gesture(app: &mut StudioApp, ui: &Ui, resp: &egui::Response, s: &Session, map: Map, bounds: Rect, create: bool) {
    let region = s.edit.loop_range;
    let enabled = s.edit.loop_playback;
    // Edges land on the nearest grid line (the one automation uses); Shift drags freely.
    let free = ui.input(|i| i.modifiers.shift);
    let snap = |at: Samples| if free { at.max(0) } else { crate::automation::snap(s, map.spp, &map.grid, at) };
    let part_at = |x: f32| {
        if region.is_empty() { Part::Outside } else { hit(map.x(region.start), map.x(region.end), x, HANDLE) }
    };
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(press) = ui.input(|i| i.pointer.press_origin())
    {
        let at = map.at(press.x);
        let grab = match part_at(press.x) {
            Part::Left => Some(Grab::Left { end: region.end }),
            Part::Right => Some(Grab::Right { start: region.start }),
            Part::Body => Some(Grab::Body { start: region.start, end: region.end, press: at }),
            Part::Outside => create.then_some(Grab::Create { anchor: at }),
        };
        app.arrange.loop_drag = grab.map(|grab| LoopDrag { grab, range: region });
    }
    if let (Some(d), Some(pos)) = (app.arrange.loop_drag.as_mut(), resp.interact_pointer_pos())
        && resp.dragged()
    {
        d.range = drag_range(d.grab, map.at(pos.x.clamp(bounds.min.x, bounds.max.x)), snap);
    }
    if resp.drag_stopped()
        && let Some(d) = app.arrange.loop_drag.take()
        && !d.range.is_empty()
        && d.range != region
    {
        let mut p = json!({"start": d.range.start, "end": d.range.end});
        if matches!(d.grab, Grab::Create { .. }) {
            p["enabled"] = json!(true);
        }
        let _ = app.run("transport.loop", p);
    }
    if resp.double_clicked() {
        let _ = app.run("options.loop_playback", json!({"value": !enabled}));
    }
    if let Some(pos) = resp.hover_pos().filter(|_| app.arrange.loop_drag.is_none()) {
        match part_at(pos.x) {
            Part::Left | Part::Right => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
            Part::Body => ui.ctx().set_cursor_icon(CursorIcon::Grab),
            Part::Outside => {}
        }
    }
}

/// Draw the strip and take its input.
pub(crate) fn strip(app: &mut StudioApp, ui: &mut Ui, s: &Session, map: Map, strip: Rect) {
    let resp = ui.interact(strip, ui.id().with("loop_strip"), Sense::click_and_drag());
    let region = s.edit.loop_range;
    let enabled = s.edit.loop_playback;
    gesture(app, ui, &resp, s, map, strip, true);
    crate::chrome::context_menu(&resp).show(|ui| {
        let item = ui.add_enabled(!region.is_empty(), crate::chrome::MenuRow::new("Clear loop"));
        app.arrange.menu.push(crate::mixer::MenuItem::at(ui, "Clear loop".into(), String::new(), item.rect));
        if item.clicked() {
            clear(app);
            ui.close();
        }
    });

    // Paint from the session, except while a gesture runs: then its region.
    let shown = app.arrange.loop_drag.map_or(region, |d| d.range);
    let p = ui.painter().with_clip_rect(strip);
    p.rect_filled(strip, 0.0, TOKENS.lane);
    let mut view = LoopView::default();
    if !shown.is_empty() {
        let r =
            Rect::from_min_max(pos2(map.x(shown.start), strip.min.y + 1.0), pos2(map.x(shown.end).max(map.x(shown.start) + 1.0), strip.max.y - 1.0));
        let look = brace(enabled);
        p.rect_filled(r, crate::theme::RADIUS_SM, look.fill);
        if let Some(edge) = look.edge {
            p.rect_stroke(r, crate::theme::RADIUS_SM, Stroke::new(1.0, edge), StrokeKind::Inside);
        }
        let text = range_text(s, shown);
        let font = crate::theme::mono(ui.ctx(), BRACE_TEXT);
        let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), look.ink);
        if galley.size().x + 8.0 <= r.width() {
            p.with_clip_rect(r).text(r.center(), egui::Align2::CENTER_CENTER, &text, font, look.ink);
            view.text_rect = crate::arrange::r4(Rect::from_center_size(r.center(), galley.size()));
            view.range_text = text;
        }
        view.brace = crate::arrange::r4(r);
        view.brace_look = look;
    }
    app.arrange.layout.loop_view.brace = view.brace;
    app.arrange.layout.loop_view.brace_look = view.brace_look;
    app.arrange.layout.loop_view.range_text = view.range_text;
    app.arrange.layout.loop_view.text_rect = view.text_rect;
}

/// The range in the brace: Mono 9 points.
pub const BRACE_TEXT: f32 = 9.0;

/// How the brace is painted.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BraceLook {
    pub fill: egui::Color32,
    /// Its 1 px edge (Cycle off only).
    pub edge: Option<egui::Color32>,
    /// The range's ink.
    pub ink: egui::Color32,
}

/// The brace (#326): solid in the loop hue with the range in `ink.dark` while Cycle is on; at 30
/// percent with a 55 percent edge and the range in `text` while it is off.
pub fn brace(enabled: bool) -> BraceLook {
    use crate::tokens::alpha;
    let t = &TOKENS;
    if enabled {
        BraceLook { fill: t.loop_region, edge: None, ink: t.ink_dark }
    } else {
        BraceLook {
            fill: t.loop_region.gamma_multiply(alpha::LOOP_BRACE_OFF),
            edge: Some(t.loop_region.gamma_multiply(alpha::LOOP_BRACE_OFF_EDGE)),
            ink: t.text,
        }
    }
}

/// "5.1 – 9.1 · 4 bars": the region's start and end as bar.beat, and its length when it runs from
/// a bar line to a bar line.
pub fn range_text(s: &Session, range: Range) -> String {
    let (a, b) = (s.tempo.bar_beat_at(range.start, s.sample_rate), s.tempo.bar_beat_at(range.end, s.sample_rate));
    let ends = format!("{}.{} – {}.{}", a.bar, a.beat, b.bar, b.beat);
    let on_bars = a.beat == 1 && a.tick == 0 && b.beat == 1 && b.tick == 0;
    match b.bar - a.bar {
        n if on_bars && n == 1 => format!("{ends} · 1 bar"),
        n if on_bars && n > 1 => format!("{ends} · {n} bars"),
        _ => ends,
    }
}

/// The loop as the arrangement last painted it, for tests and `ui.inspect`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LoopView {
    /// The brace in the strip (zero without a region).
    pub brace: crate::arrange::R4,
    #[serde(skip)]
    pub brace_look: BraceLook,
    /// The range printed in the brace (empty when it does not fit).
    pub range_text: String,
    /// The printed range's text rect (zero when none is printed).
    pub text_rect: crate::arrange::R4,
    /// The ruler's tint under the loop while Cycle is on.
    pub ruler_tint: Option<crate::arrange::R4>,
    /// The loop's edges (x) down the ruler and the lanes while Cycle is on.
    pub edges: Vec<f32>,
    /// What lies outside the loop over the lanes, dimmed by `overlay`, while Cycle is on.
    pub outside: Vec<crate::arrange::R4>,
}

/// While Cycle is on (#326): the lanes outside the loop take `overlay` (nothing tints inside, so
/// the selection stays the only color there) and the loop's edges run from under the brace down
/// the ruler and the lanes at 75 percent of the hue. Paint after the clips, under the playhead.
pub(crate) fn lanes(app: &mut StudioApp, ui: &Ui, s: &Session, map: Map, ruler: Rect, lanes: Rect) {
    let view = &mut app.arrange.layout.loop_view;
    view.edges.clear();
    view.outside.clear();
    let shown = app.arrange.loop_drag.map_or(s.edit.loop_range, |d| d.range);
    if !s.edit.loop_playback || shown.is_empty() {
        return;
    }
    let (x0, x1) = (map.x(shown.start), map.x(shown.end));
    let p = ui.painter().with_clip_rect(lanes);
    for r in
        [Rect::from_min_max(lanes.min, pos2(x0.min(lanes.max.x), lanes.max.y)), Rect::from_min_max(pos2(x1.max(lanes.min.x), lanes.min.y), lanes.max)]
    {
        if r.is_positive() {
            p.rect_filled(r, 0.0, TOKENS.overlay);
            view.outside.push(crate::arrange::r4(r));
        }
    }
    let edge = Stroke::new(1.0, TOKENS.loop_region.gamma_multiply(crate::tokens::alpha::LOOP_EDGE));
    let span = Rect::from_min_max(pos2(ruler.min.x, ruler.min.y + STRIP_H), pos2(ruler.max.x, lanes.max.y));
    let pe = ui.painter().with_clip_rect(span);
    for x in [x0 + 0.5, x1 - 0.5] {
        if x >= span.min.x && x <= span.max.x {
            pe.line_segment([pos2(x, span.min.y), pos2(x, span.max.y)], edge);
            view.edges.push(x);
        }
    }
}

/// The ruler's loop block, under the strip: the stored loop region, tinted in the loop hue while
/// Cycle is on (and then live: edges resize, the body moves, double-click turns Cycle off); while
/// Cycle is off the parked brace in the strip shows what Cycle will use (#326). Nothing when there
/// is no region. Outside the block the ruler keeps click-to-locate.
pub(crate) fn block(app: &mut StudioApp, ui: &mut Ui, s: &Session, map: Map, ruler: Rect) {
    let region = s.edit.loop_range;
    let enabled = s.edit.loop_playback;
    let (top, bottom) = (ruler.min.y + STRIP_H + 14.0, ruler.max.y - 1.0);
    if enabled && !region.is_empty() {
        let hit_rect = Rect::from_min_max(pos2(map.x(region.start) - HANDLE, top), pos2(map.x(region.end) + HANDLE, bottom)).intersect(ruler);
        if hit_rect.is_positive() {
            let resp = ui.interact(hit_rect, ui.id().with("loop_block"), Sense::click_and_drag());
            gesture(app, ui, &resp, s, map, ruler, false);
            if resp.clicked()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                app.click_ruler(map.at(pos.x.clamp(ruler.min.x, ruler.max.x)));
            }
        }
    }
    let shown = app.arrange.loop_drag.map_or(region, |d| d.range);
    app.arrange.layout.loop_view.ruler_tint = None;
    if shown.is_empty() || !enabled {
        // Cycle off: the brace stays, parked; the ruler and the lanes are left alone (#326).
        return;
    }
    // Cycle on: the ruler under the brace takes the loop hue at 20 percent.
    let r = Rect::from_min_max(pos2(map.x(shown.start), ruler.min.y + STRIP_H), pos2(map.x(shown.end), ruler.max.y)).intersect(ruler);
    if r.is_positive() {
        ui.painter().rect_filled(r, 0.0, TOKENS.loop_region.gamma_multiply(crate::tokens::alpha::LOOP_RULER_TINT));
        app.arrange.layout.loop_view.ruler_tint = Some(crate::arrange::r4(r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Snap to multiples of 100.
    fn s(at: Samples) -> Samples {
        ((at as f64 / 100.0).round() as Samples * 100).max(0)
    }

    #[test]
    fn create_snaps_both_ends_and_orders_them() {
        assert_eq!(drag_range(Grab::Create { anchor: 130 }, 480, s), Range::new(100, 500));
        assert_eq!(drag_range(Grab::Create { anchor: 480 }, 130, s), Range::new(100, 500), "dragging backwards");
        assert_eq!(drag_range(Grab::Create { anchor: 130 }, -900, s), Range::new(0, 100), "clamped at zero");
        assert!(drag_range(Grab::Create { anchor: 130 }, 149, s).is_empty(), "within one step: no region");
    }

    #[test]
    fn resizing_an_edge_keeps_the_other_and_swaps_on_crossing() {
        let left = Grab::Left { end: 800 };
        assert_eq!(drag_range(left, 290, s), Range::new(300, 800));
        assert_eq!(drag_range(left, 1010, s), Range::new(800, 1000), "left edge dragged past the right end");
        assert_eq!(drag_range(left, -50, s), Range::new(0, 800));
        let right = Grab::Right { start: 300 };
        assert_eq!(drag_range(right, 910, s), Range::new(300, 900));
        assert_eq!(drag_range(right, 90, s), Range::new(100, 300), "right edge dragged past the left start");
        assert!(drag_range(right, 300, s).is_empty());
        // Shift: no snapping (the caller passes a plain clamp).
        assert_eq!(drag_range(right, 913, |at| at.max(0)), Range::new(300, 913));
    }

    #[test]
    fn moving_the_body_keeps_its_length_and_stops_at_zero() {
        let body = Grab::Body { start: 300, end: 800, press: 500 };
        assert_eq!(drag_range(body, 740, s), Range::new(500, 1000));
        assert_eq!(drag_range(body, 500, s), Range::new(300, 800), "no movement");
        assert_eq!(drag_range(body, 100, s), Range::new(0, 500), "clamped at zero, length kept");
        assert_eq!(drag_range(body, -5000, s), Range::new(0, 500));
    }

    #[test]
    fn a_press_picks_the_edge_the_body_or_empty_strip() {
        assert_eq!(hit(100.0, 300.0, 103.0, HANDLE), Part::Left);
        assert_eq!(hit(100.0, 300.0, 95.0, HANDLE), Part::Left, "just outside the edge still grabs it");
        assert_eq!(hit(100.0, 300.0, 297.0, HANDLE), Part::Right);
        assert_eq!(hit(100.0, 300.0, 200.0, HANDLE), Part::Body);
        assert_eq!(hit(100.0, 300.0, 50.0, HANDLE), Part::Outside);
        assert_eq!(hit(100.0, 300.0, 400.0, HANDLE), Part::Outside);
        assert_eq!(hit(100.0, 108.0, 106.0, HANDLE), Part::Right, "a narrow region: the nearer edge");
    }

    #[test]
    fn the_grid_snap_rounds_to_the_visible_grid_and_never_below_zero() {
        let sess = Session::default();
        let a = crate::automation::snap(&sess, 100.0, &crate::grid::GridView::default(), 123_457);
        assert_eq!(crate::automation::snap(&sess, 100.0, &crate::grid::GridView::default(), a), a, "snapping is idempotent");
        assert_eq!(crate::automation::snap(&sess, 100.0, &crate::grid::GridView::default(), -400), 0);
    }
}
