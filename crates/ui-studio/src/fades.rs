//! Fade and crossfade handles on audio clips (#63, #182), Logic's look and feel.
//!
//! Conventions (Logic's region fades; Ableton's handles agree on all but the cursor):
//! - The fade zone is the whole top corner of the clip body, a triangle as tall as the body
//!   ([`corner`]); once a fade exists its drawn area is the handle ([`grab`]). Narrow clips keep
//!   a minimum target and give up the middle move zone before the fade zones.
//! - A fade is drawn as a shaded area over the waveform with its curve as a line; the waveform
//!   under it is dimmed, not hidden ([`overlay`] records the paint in the layout).
//! - Dragging the curve line bends it, no modifier; Alt only slows the drag ([`FINE`]).
//! - Dragging a fade handle past the clip's end into an overlapping neighbor makes an
//!   equal-power crossfade over the overlap, drawn as an X ([`draw_cross`]); its two ends are
//!   the two clips' handles and move independently once their lengths differ.
//! - The clip inspector ([`crate::clip_inspector`]) edits the same lengths and curves by number.
//!
//! The trim and loop hot zones live in [`crate::arrange::zone`], which asks [`corner`] for the
//! fade corners. This module holds what a fade gesture does: the pure mapping from the pointer to a fade length
//! ([`fade_len`]), from a vertical drag to a curvature ([`curvature_after`]), the commands one
//! gesture turns into ([`plan`]), the curve the clip draws ([`curve_points`], the same
//! `Fade::gain` the engine renders with), and the shape menu.
//!
//! Every gesture is one undo step: the commands run under one merge key (`composite`). Fades are
//! set through the engine's clip-targeted `edit.fades_create` (`clip`, `side`, `length`, `shape`,
//! `curvature`); a crossfade across an abutting boundary uses its range form (`ops::create_fades`).

use crate::StudioApp;
use crate::arrange::{DragKind, EDGE, Gesture, Layout, Map, R4};
use crate::theme::CLIP_NAME_H;
use egui::{Color32, Pos2, Stroke, Ui, pos2};
use serde_json::{Value, json};
use transit_model::{Clip, ClipContent, ClipId, Fade, FadeShape, Session, TrackId};
use transit_time::Samples;

/// Vertical drag that bends a fade's curvature across its whole range (points).
pub const CURVE_RANGE_PX: f32 = 120.0;
/// Alt scales a curve drag by this: fine adjust, half the step.
pub const FINE: f32 = 0.5;
/// The fade corner of a clip is this wide at most (points, from inside the trim edge) and as
/// tall as the body; narrower clips share what is between their trim edges.
pub const ZONE_W: f32 = 36.0;
/// A fade corner narrower than this is not a target (the clip is too narrow for anything else).
pub const ZONE_FLOOR: f32 = 4.0;
/// How near (points) a press is to a fade's curve line to bend it rather than resize the fade.
pub const CURVE_HIT: f32 = 5.0;
/// How much of the waveform shows through a fade's shaded area (the rest is the clip color).
pub const DIM: f32 = 0.55;

/// A fade at a clip's start (in) or end (out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Side {
    In,
    Out,
}

impl Side {
    fn id(self) -> &'static str {
        match self {
            Side::In => "in",
            Side::Out => "out",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Side::In => "Fade In",
            Side::Out => "Fade Out",
        }
    }

    /// The fade of `c` on this side.
    pub fn of(self, c: &Clip) -> Fade {
        match self {
            Side::In => c.fade_in,
            Side::Out => c.fade_out,
        }
    }

    fn other(self) -> Side {
        match self {
            Side::In => Side::Out,
            Side::Out => Side::In,
        }
    }
}

/// The fade length the pointer at `at` asks for on a clip spanning `start..end`: from the clip
/// edge to the pointer. Negative when the pointer is past the clip's edge (outward).
pub fn fade_len(side: Side, start: Samples, end: Samples, at: Samples) -> Samples {
    match side {
        Side::In => at.saturating_sub(start),
        Side::Out => end.saturating_sub(at),
    }
}

/// The curvature after dragging `dy` points down from `start` (up bends the fade up, towards
/// full level sooner), clamped to the engine's -1..1.
pub fn curvature_after(start: f32, dy: f32) -> f32 {
    (start - dy / CURVE_RANGE_PX * 2.0).clamp(-1.0, 1.0)
}

/// Which fade's region (`in_w` / `out_w` points wide from the clip edges, below the name strip)
/// the point is in.
pub fn region_at(rect: R4, in_w: f32, out_w: f32, pos: Pos2) -> Option<Side> {
    if pos.y <= rect[1] + CLIP_NAME_H || pos.y > rect[3] || pos.x < rect[0] || pos.x > rect[2] {
        return None;
    }
    if in_w > 0.0 && pos.x - rect[0] <= in_w {
        Some(Side::In)
    } else if out_w > 0.0 && rect[2] - pos.x <= out_w {
        Some(Side::Out)
    } else {
        None
    }
}

/// The points of a fade's curve across `x0..x1` (the fade's extent on screen), between `bottom`
/// (silence) and `top` (unity): `n + 1` points, from the clip edge inward... for a fade-in that is
/// left to right with the gain rising; for a fade-out left to right with the gain falling.
pub fn curve_points(fade: Fade, side: Side, x0: f32, x1: f32, top: f32, bottom: f32, n: usize) -> Vec<Pos2> {
    let n = n.max(1);
    (0..=n)
        .map(|i| {
            let f = i as f32 / n as f32;
            let t = match side {
                Side::In => f,
                Side::Out => 1.0 - f,
            };
            pos2(x0 + (x1 - x0) * f, bottom - (bottom - top) * fade.gain(t))
        })
        .collect()
}

/// Two audio clips of one track that overlap: the left one (earlier start), the right one, and
/// the overlap `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overlap {
    pub left: ClipId,
    pub right: ClipId,
    pub start: Samples,
    pub end: Samples,
}

fn is_audio(c: &Clip) -> bool {
    matches!(c.content, ClipContent::Audio { .. })
}

/// The overlap `clip` has with a neighbor on its track (audio clips only).
pub fn overlap(s: &Session, clip: ClipId) -> Option<Overlap> {
    let (track, c) = s.find_clip(clip)?;
    if !is_audio(c) {
        return None;
    }
    let t = s.track(track)?;
    t.clips().iter().filter(|o| o.id != c.id && is_audio(o)).find_map(|o| {
        let (l, r) = if (o.start, o.id.0) < (c.start, c.id.0) { (o, c) } else { (c, o) };
        let (start, end) = (r.start, l.end().min(r.end()));
        (start < end).then_some(Overlap { left: l.id, right: r.id, start, end })
    })
}

/// What a fade-handle drag turns into.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// Nothing to do (the length is zero, or outward with no neighbor to cross into).
    None,
    /// Set the fades: (clip, side, fade) in order.
    Set(Vec<(ClipId, Side, Fade)>),
    /// Dragged to nothing: remove this fade (`edit.fades_delete` over the clip), keeping the
    /// clip's other fade.
    Remove { clip: ClipId, track: TrackId, keep: Option<(Side, Fade)>, start: Samples, end: Samples },
    /// Dragged outward across the boundary of an abutting neighbor: a crossfade over
    /// `start..end` on `track` (`edit.fades_create`'s range form).
    Cross { track: TrackId, start: Samples, end: Samples },
}

/// The neighbor `c` overlaps on `side`: the clip its fade crosses into.
fn partner<'a>(s: &'a Session, track: TrackId, c: &Clip, side: Side) -> Option<&'a Clip> {
    if !is_audio(c) {
        return None;
    }
    s.track(track)?.clips().iter().filter(|o| o.id != c.id && is_audio(o)).find(|o| match side {
        Side::In => o.start < c.start && o.end() > c.start,
        Side::Out => o.start > c.start && o.start < c.end(),
    })
}

/// The commands for dragging `clip`'s `side` handle to ask for `len` (see [`fade_len`]).
///
/// On a clip that overlaps a neighbor the drag moves a crossfade's end. The two ends move
/// together while their lengths are equal (a crossfade made by the menu or a drag), unless `alt`;
/// once they differ each end moves alone. Pulling the handle past the clip's end (`len < 0`)
/// into that neighbor makes the equal-power crossfade of the whole overlap, the same two fades
/// the context menu's Crossfade sets.
pub fn plan(s: &Session, clip: ClipId, side: Side, len: Samples, alt: bool) -> Plan {
    let Some((track, c)) = s.find_clip(clip) else { return Plan::None };
    let near = partner(s, track, c, side);
    if len > 0 {
        let fade = Fade { len: len.min(c.length), ..side.of(c) };
        let mut set = vec![(clip, side, fade)];
        if let Some(o) = near
            && !alt
            && side.of(c).len == side.other().of(o).len
        {
            set.push((o.id, side.other(), Fade { len: len.min(o.length), ..side.other().of(o) }));
        }
        return Plan::Set(set);
    }
    if len < 0 {
        if let Some(o) = near {
            let (left, right) = if side == Side::Out { (c, o) } else { (o, c) };
            let fade = Fade { len: left.end().min(right.end()) - right.start, shape: FadeShape::EqualPower, curvature: 0.0 };
            return Plan::Set(vec![(left.id, Side::Out, fade), (right.id, Side::In, fade)]);
        }
        let d = len.saturating_neg();
        let boundary = match side {
            Side::In => c.start,
            Side::Out => c.end(),
        };
        let abuts = s.track(track).is_some_and(|t| {
            t.clips().iter().any(|o| {
                o.id != c.id
                    && is_audio(o)
                    && match side {
                        Side::In => o.end() == boundary,
                        Side::Out => o.start == boundary,
                    }
            })
        });
        if abuts && is_audio(c) {
            return Plan::Cross { track, start: boundary.saturating_sub(d).max(0), end: boundary.saturating_add(d) };
        }
    }
    // To nothing: remove the fade, unless an overlapping neighbor would be trimmed by the delete.
    if side.of(c).len > 0 && overlap(s, clip).is_none() {
        let other = side.other();
        let keep = Some(other).filter(|o| o.of(c).len > 0).map(|o| (o, o.of(c)));
        return Plan::Remove { clip, track, keep, start: c.start, end: c.end() };
    }
    Plan::None
}

/// The `edit.fades_create` parameters that set `fade` on a clip's side.
fn set_params(clip: ClipId, side: Side, fade: Fade) -> Value {
    json!({"clip": clip.0, "side": side.id(), "length": fade.len, "shape": fade.shape.label(), "curvature": f64::from(fade.curvature.clamp(-1.0, 1.0))})
}

/// A fade gesture in progress, as it would land.
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    pub clip: ClipId,
    pub side: Side,
    /// The fade the dragged clip would have (what the clip draws while dragging).
    pub fade: Fade,
    /// What releasing the pointer does.
    pub plan: Plan,
    /// A curvature drag (the curve line) rather than a length drag.
    pub curve: bool,
}

/// The fade gesture from the pointer, if one is in progress.
pub(crate) fn drag(app: &StudioApp, ui: &Ui, map: Map) -> Option<Live> {
    let Some(Gesture::Clip { clip, kind, press, start, end, .. }) = app.arrange.gesture.clone() else { return None };
    let (pos, m) = ui.input(|i| (i.pointer.latest_pos(), i.modifiers));
    let pos = pos?;
    let s = app.engine.session();
    let c = s.find_clip(clip)?.1;
    let side = match kind {
        DragKind::FadeIn | DragKind::FadeCurveIn => Side::In,
        DragKind::FadeOut | DragKind::FadeCurveOut => Side::Out,
        _ => return None,
    };
    let base = side.of(c);
    if matches!(kind, DragKind::FadeCurveIn | DragKind::FadeCurveOut) {
        // Alt slows the drag to half: fine adjust.
        let dy = if m.alt { (pos.y - press.y) * FINE } else { pos.y - press.y };
        let fade = Fade { curvature: curvature_after(base.curvature, dy), ..base };
        return Some(Live { clip, side, fade, plan: Plan::Set(vec![(clip, side, fade)]), curve: true });
    }
    let at = map.at(pos.x);
    let at = if m.shift { at } else { crate::automation::snap_delta(s, map.spp, &map.grid, 0, at) };
    let len = fade_len(side, start, end, at);
    let plan = plan(s, clip, side, len, m.alt);
    let fade = match &plan {
        Plan::Set(v) => v.first().map_or(base, |x| x.2),
        _ => Fade { len: 0, ..base },
    };
    Some(Live { clip, side, fade, plan, curve: false })
}

/// Commit a fade gesture: one undo step.
pub(crate) fn commit(app: &mut StudioApp, live: &Live) {
    let plan = live.plan.clone();
    let changes = match &plan {
        Plan::None => false,
        Plan::Set(v) => {
            let s = app.engine.session();
            v.iter().any(|(c, side, f)| s.find_clip(*c).is_some_and(|x| side.of(x.1) != *f))
        }
        Plan::Cross { .. } | Plan::Remove { .. } => true,
    };
    if !changes {
        return;
    }
    let _ = app.composite("Fade", |app, key| {
        match plan {
            Plan::None => {}
            Plan::Set(v) => {
                for (clip, side, fade) in v {
                    app.engine.execute_merged("edit.fades_create", &set_params(clip, side, fade), key)?;
                }
            }
            Plan::Remove { clip, track, keep, start, end } => {
                app.engine.execute_merged("edit.fades_delete", &json!({"tracks": [track.0], "start": start, "end": end}), key)?;
                if let Some((side, fade)) = keep {
                    app.engine.execute_merged("edit.fades_create", &set_params(clip, side, fade), key)?;
                }
            }
            Plan::Cross { track, start, end } => {
                app.engine.execute_merged(
                    "edit.fades_create",
                    &json!({"tracks": [track.0], "start": start, "end": end, "shape": FadeShape::EqualPower.label()}),
                    key,
                )?;
            }
        }
        Ok(())
    });
}

//// The fade corner under `pos` on a clip drawn in `rect`: the triangle in each top corner of the
/// body (under the header strip, inside the trim edge) as tall as the body and `ZONE_W` wide.
/// Clips too narrow for that share the room between the trim edges, so the two triangles meet at
/// the top and the middle move zone is left only below them; under `ZONE_FLOOR` there is none.
pub fn corner(rect: R4, pos: Pos2) -> Option<Side> {
    let w = rect[2] - rect[0];
    let edge = EDGE.min(w / 4.0);
    let zw = ZONE_W.min((w - 2.0 * edge) / 2.0);
    let top = rect[1] + CLIP_NAME_H;
    let h = rect[3] - top;
    if zw < ZONE_FLOOR || h <= 0.0 || pos.y <= top || pos.y > rect[3] {
        return None;
    }
    let down = (pos.y - top) / h;
    let left = (pos.x - rect[0] - edge) / zw;
    let right = (rect[2] - edge - pos.x) / zw;
    if left >= 0.0 && left + down <= 1.0 {
        Some(Side::In)
    } else if right >= 0.0 && right + down <= 1.0 {
        Some(Side::Out)
    } else {
        None
    }
}

/// What a press on a fade takes hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Part {
    /// The area (or the empty corner): drag to set the length.
    Length,
    /// The curve line: drag up or down to bend it.
    Curve,
}

/// A fade handle under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Grab {
    pub side: Side,
    pub part: Part,
}

impl Grab {
    /// The clip drag this press starts.
    pub fn kind(self) -> DragKind {
        match (self.part, self.side) {
            (Part::Length, Side::In) => DragKind::FadeIn,
            (Part::Length, Side::Out) => DragKind::FadeOut,
            (Part::Curve, Side::In) => DragKind::FadeCurveIn,
            (Part::Curve, Side::Out) => DragKind::FadeCurveOut,
        }
    }

    /// The cursor over the handle.
    pub fn cursor(self) -> egui::CursorIcon {
        match (self.part, self.side) {
            (Part::Curve, _) => egui::CursorIcon::ResizeVertical,
            (Part::Length, Side::In) => egui::CursorIcon::ResizeNwSe,
            (Part::Length, Side::Out) => egui::CursorIcon::ResizeNeSw,
        }
    }
}

/// The body of a clip drawn in a rect from `y0` to `y1`: where the waveform and the fade curves
/// run, as `(top, bottom)`.
fn body_span(y0: f32, y1: f32) -> (f32, f32) {
    (y0 + CLIP_NAME_H + 2.0, y1 - 2.0)
}

/// The on-screen extent `(x0, x1)` of `c`'s fade on `side` for a clip drawn from `left` to
/// `right`, if it has one.
fn extent(c: &Clip, map: Map, left: f32, right: f32, side: Side) -> Option<(f32, f32)> {
    let f = side.of(c);
    if f.len <= 0 {
        return None;
    }
    let w = match side {
        Side::In => map.x(c.start.saturating_add(f.len)) - map.x(c.start),
        Side::Out => map.x(c.end()) - map.x(c.end().saturating_sub(f.len)),
    }
    .clamp(1.0, (right - left).max(1.0));
    Some(match side {
        Side::In => (left, left + w),
        Side::Out => (right - w, right),
    })
}

/// How far `p` is from the polyline `pts` (points).
fn distance_to(pts: &[Pos2], p: Pos2) -> f32 {
    pts.windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let len2 = ab.length_sq();
            let t = if len2 <= f32::EPSILON { 0.0 } else { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) };
            (a + ab * t - p).length()
        })
        .fold(f32::MAX, f32::min)
}

/// The fade handle `pos` is over on audio clip `c` drawn in `rect`: the curve line of an existing
/// fade (bend), then its whole drawn extent (resize), then, when `corner_ok`, the empty corner
/// ([`corner`]; resize from nothing). Trim edges and the header strip are the caller's: ask only
/// where [`crate::arrange::zone`] did not say trim or loop.
pub(crate) fn grab(c: &Clip, map: Map, rect: R4, pos: Pos2, corner_ok: bool) -> Option<Grab> {
    if !is_audio(c) || pos.y <= rect[1] + CLIP_NAME_H || pos.y > rect[3] || pos.x < rect[0] || pos.x > rect[2] {
        return None;
    }
    let (top, bottom) = body_span(rect[1], rect[3]);
    let sides: Vec<(Side, f32, f32)> =
        [Side::In, Side::Out].into_iter().filter_map(|s| extent(c, map, rect[0], rect[2], s).map(|(a, b)| (s, a, b))).collect();
    for (side, x0, x1) in &sides {
        if pos.x >= *x0 && pos.x <= *x1 && distance_to(&curve_points(side.of(c), *side, *x0, *x1, top, bottom, 24), pos) <= CURVE_HIT {
            return Some(Grab { side: *side, part: Part::Curve });
        }
    }
    if let Some((side, ..)) = sides.iter().find(|(_, x0, x1)| pos.x >= *x0 && pos.x <= *x1) {
        return Some(Grab { side: *side, part: Part::Length });
    }
    if corner_ok { corner(rect, pos).map(|side| Grab { side, part: Part::Length }) } else { None }
}

/// A fade handle found under the pointer, and the clip it belongs to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Target {
    pub clip: ClipId,
    pub track: TrackId,
    pub start: Samples,
    pub end: Samples,
    pub grab: Grab,
}

/// The fade handle under `pos`, given the zone `kind` the clip under it reported. Only a body
/// press (move or a fade corner) can turn into a fade handle; the trim edges and the loop zone
/// win. The topmost clip is asked first, the empty corner only of it; clips beneath it are asked
/// for their drawn fades, so both ends of a crossfade can be reached through the clip on top.
pub(crate) fn grab_at(s: &Session, map: Map, layout: &Layout, pos: Pos2, kind: DragKind) -> Option<Target> {
    if !matches!(kind, DragKind::Move | DragKind::FadeIn | DragKind::FadeOut) {
        return None;
    }
    let under = layout.clips.iter().rev().filter(|c| pos.x >= c.rect[0] && pos.x <= c.rect[2] && pos.y >= c.rect[1] && pos.y <= c.rect[3]);
    for (i, r) in under.enumerate() {
        let Some(c) = s.find_clip(ClipId(r.id)).map(|x| x.1) else { continue };
        if let Some(grab) = grab(c, map, r.rect, pos, i == 0 && kind != DragKind::Move) {
            return Some(Target { clip: c.id, track: TrackId(r.track), start: r.start, end: r.end, grab });
        }
    }
    None
}

/// Which existing fade of an audio clip the point is over (and the clip), for clicks: the shape
/// menu and the reset. The empty corner is not a fade.
pub(crate) fn hit(s: &Session, map: Map, layout: &Layout, pos: Pos2) -> Option<(ClipId, Side)> {
    grab_at(s, map, layout, pos, DragKind::Move).map(|t| (t.clip, t.grab.side))
}

/// One fade as laid out and painted: where, and what the paint is made of. Tests read these.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Paint {
    pub clip: u64,
    /// "in", "out", "handle" (the hover dot on a clip with no fade there) or "cross".
    pub kind: &'static str,
    /// The shaded area `[x0, y0, x1, y1]`, from the top of the body down to the unity line.
    pub area: R4,
    /// The curve line, left to right; for a crossfade, the outgoing clip's.
    pub curve: Vec<[f32; 2]>,
    /// A crossfade's other curve: the incoming clip's.
    pub curve2: Vec<[f32; 2]>,
    /// The share of the clip color laid over the waveform inside the area (0: none).
    pub dim: f32,
    /// The handle dot at the fade's end on the clip's top edge, while the pointer is over it.
    pub dot: Option<[f32; 2]>,
}

/// One fade's drawn geometry on a clip drawn in `rect`.
struct Geo {
    side: Side,
    x0: f32,
    x1: f32,
    pts: Vec<Pos2>,
}

fn geometry(c: &Clip, map: Map, rect: egui::Rect, body: egui::Rect) -> Vec<Geo> {
    if !is_audio(c) || body.height() < 6.0 {
        return Vec::new();
    }
    [Side::In, Side::Out]
        .into_iter()
        .filter_map(|side| {
            let (x0, x1) = extent(c, map, rect.min.x, rect.max.x, side)?;
            Some(Geo { side, x0, x1, pts: curve_points(side.of(c), side, x0, x1, body.min.y, body.max.y, 24) })
        })
        .collect()
}

fn pt(p: Pos2) -> [f32; 2] {
    [p.x, p.y]
}

/// Draw `c`'s fades over its body: the area they attenuate, with the waveform dimmed under it
/// (`fill` is the clip color), the curve (the engine's own `Fade::gain`), and small marks at
/// the corners of a selected clip.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(p: &egui::Painter, c: &Clip, map: Map, rect: egui::Rect, body: egui::Rect, ink: Color32, fill: Color32, selected: bool) {
    if !is_audio(c) || body.height() < 6.0 {
        return;
    }
    for g in geometry(c, map, rect, body) {
        let mut mesh = egui::Mesh::default();
        let (dim, tint) = (fill.gamma_multiply(DIM), ink.gamma_multiply(0.12));
        for pair in g.pts.windows(2) {
            for color in [dim, tint] {
                let i = mesh.vertices.len() as u32;
                for q in [pos2(pair[0].x, body.min.y), pos2(pair[1].x, body.min.y), pair[1], pair[0]] {
                    mesh.colored_vertex(q, color);
                }
                mesh.add_triangle(i, i + 1, i + 2);
                mesh.add_triangle(i, i + 2, i + 3);
            }
        }
        p.add(egui::Shape::mesh(mesh));
        p.add(egui::Shape::line(g.pts, Stroke::new(1.5, ink)));
    }
    if selected {
        // Corner marks: small right triangles at the top of each fade corner.
        let y = body.min.y + 1.0;
        let a = 9.0;
        p.add(egui::Shape::convex_polygon(
            vec![pos2(rect.min.x + 6.0, y), pos2(rect.min.x + 6.0 + a, y), pos2(rect.min.x + 6.0, y + a)],
            ink,
            Stroke::NONE,
        ));
        p.add(egui::Shape::convex_polygon(
            vec![pos2(rect.max.x - 6.0, y), pos2(rect.max.x - 6.0 - a, y), pos2(rect.max.x - 6.0, y + a)],
            ink,
            Stroke::NONE,
        ));
    }
}

/// Record `c`'s fade paint in the layout and draw the handle dot at the end of the fade on
/// `hot` (the side the pointer is over, or the one being dragged) on the clip's top edge.
pub(crate) fn overlay(p: &egui::Painter, layout: &mut Layout, c: &Clip, map: Map, rect: egui::Rect, hot: Option<Side>) {
    if !is_audio(c) {
        return;
    }
    let (top, bottom) = body_span(rect.min.y, rect.max.y);
    let body = egui::Rect::from_min_max(pos2(rect.min.x, top), pos2(rect.max.x, bottom));
    let mut dotted = false;
    for g in geometry(c, map, rect, body) {
        let dot = (hot == Some(g.side)).then(|| match g.side {
            Side::In => pos2(g.x1, top),
            Side::Out => pos2(g.x0, top),
        });
        dotted |= dot.is_some();
        layout.fades.push(Paint {
            clip: c.id.0,
            kind: g.side.id(),
            area: [g.x0, top, g.x1, bottom],
            curve: g.pts.iter().map(|q| pt(*q)).collect(),
            dim: DIM,
            dot: dot.map(pt),
            ..Paint::default()
        });
        if let Some(d) = dot {
            handle_dot(p, d);
        }
    }
    if let (Some(side), false) = (hot, dotted)
        && body.height() >= 6.0
    {
        let d = match side {
            Side::In => pos2(rect.min.x, top),
            Side::Out => pos2(rect.max.x, top),
        };
        layout.fades.push(Paint { clip: c.id.0, kind: "handle", dot: Some(pt(d)), ..Paint::default() });
        handle_dot(p, d);
    }
}

fn handle_dot(p: &egui::Painter, at: Pos2) {
    p.circle(at, 4.0, crate::theme::TOKENS.text, Stroke::new(1.5, crate::theme::TOKENS.bg));
}

/// The crossfades of a track: overlaps of two audio clips where the left one fades out and the
/// right one fades in.
pub fn crossfades(s: &Session, track: TrackId) -> Vec<Overlap> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out: Vec<Overlap> = Vec::new();
    for c in t.clips().iter().filter(|c| is_audio(c)) {
        if let Some(o) = overlap(s, c.id)
            && !out.contains(&o)
            && s.find_clip(o.left).is_some_and(|l| l.1.fade_out.len > 0)
            && s.find_clip(o.right).is_some_and(|r| r.1.fade_in.len > 0)
        {
            out.push(o);
        }
    }
    out
}

/// Draw one crossfade as an X over its overlap: a band, the outgoing clip's falling curve and
/// the incoming clip's rising one, each over its own fade's extent. `y0` is the lane's top and
/// `rh` its height. Recorded in the layout as kind "cross".
pub(crate) fn draw_cross(p: &egui::Painter, layout: &mut Layout, s: &Session, map: Map, o: Overlap, y0: f32, rh: f32) {
    let (Some(l), Some(r)) = (s.find_clip(o.left).map(|x| x.1), s.find_clip(o.right).map(|x| x.1)) else { return };
    let (top, bottom) = body_span(y0 + 1.0, y0 + rh - 1.0);
    if bottom - top < 6.0 {
        return;
    }
    let (a, b) = (map.x(o.start), map.x(o.end).max(map.x(o.start) + 2.0));
    let out = curve_points(l.fade_out, Side::Out, map.x(l.end().saturating_sub(l.fade_out.len)), map.x(l.end()), top, bottom, 24);
    let inc = curve_points(r.fade_in, Side::In, map.x(r.start), map.x(r.start.saturating_add(r.fade_in.len)), top, bottom, 24);
    let ink = crate::theme::TOKENS.text;
    p.rect_filled(egui::Rect::from_min_max(pos2(a, top), pos2(b, bottom)), 0.0, crate::theme::TOKENS.accent.gamma_multiply(0.25));
    p.add(egui::Shape::line(out.clone(), Stroke::new(1.5, ink)));
    p.add(egui::Shape::line(inc.clone(), Stroke::new(1.5, ink)));
    layout.fades.push(Paint {
        clip: o.left.0,
        kind: "cross",
        area: [a, top, b, bottom],
        curve: out.iter().map(|q| pt(*q)).collect(),
        curve2: inc.iter().map(|q| pt(*q)).collect(),
        ..Paint::default()
    });
}

// A shape menu opened by a click on a fade.
#[derive(Debug, Clone, Copy)]
pub struct Menu {
    pub clip: ClipId,
    pub side: Side,
    pub at: Pos2,
}

/// The five shapes and "Reset Fade", for `clip`'s `side`. True when one was picked.
pub(crate) fn shape_items(app: &mut StudioApp, ui: &mut Ui, clip: ClipId, side: Side) -> bool {
    let Some(f) = app.engine.session().find_clip(clip).map(|(_, c)| side.of(c)) else { return false };
    let mut picked = false;
    ui.label(side.label());
    for shape in FadeShape::ALL {
        let b = ui.selectable_label(f.shape == shape, shape.label());
        app.arrange.menu.retain(|m| !(m.label == shape.label() && m.value == side.id()));
        app.arrange.menu.push(crate::mixer::MenuItem::at(ui, shape.label().into(), side.id().into(), b.rect));
        if b.clicked() {
            set_shape(app, clip, side, shape);
            picked = true;
        }
    }
    ui.separator();
    let b = ui.button("Reset Fade");
    app.arrange.menu.retain(|m| !(m.label == "Reset Fade" && m.value == side.id()));
    app.arrange.menu.push(crate::mixer::MenuItem::at(ui, "Reset Fade".into(), side.id().into(), b.rect));
    if b.clicked() {
        reset(app, clip, side);
        picked = true;
    }
    picked
}

/// Switch a fade to `shape`, keeping its length and curvature.
pub fn set_shape(app: &mut StudioApp, clip: ClipId, side: Side, shape: FadeShape) {
    let Some(f) = app.engine.session().find_clip(clip).map(|(_, c)| side.of(c)).filter(|f| f.len > 0) else { return };
    let _ = app.run("edit.fades_create", set_params(clip, side, Fade { shape, ..f }));
}

/// Back to the default shape (equal power) and no bend, keeping the length (double-click).
pub fn reset(app: &mut StudioApp, clip: ClipId, side: Side) {
    let Some(f) = app.engine.session().find_clip(clip).map(|(_, c)| side.of(c)).filter(|f| f.len > 0) else { return };
    let _ = app.run("edit.fades_create", set_params(clip, side, Fade { len: f.len, shape: FadeShape::EqualPower, curvature: 0.0 }));
}

/// Context menu "Crossfade": equal-power fades over the overlap, both sides, one undo step.
pub fn crossfade(app: &mut StudioApp, clip: ClipId) {
    let Some(o) = overlap(app.engine.session(), clip) else { return };
    let len = o.end - o.start;
    let fade = Fade { len, shape: FadeShape::EqualPower, curvature: 0.0 };
    let _ = app.composite("Crossfade", |app, key| {
        app.engine.execute_merged("edit.fades_create", &set_params(o.left, Side::Out, fade), key)?;
        app.engine.execute_merged("edit.fades_create", &set_params(o.right, Side::In, fade), key)?;
        Ok(())
    });
}

/// The shape menu a click on a fade opens, drawn at the click until a pick or a click elsewhere.
pub(crate) fn click_menu(app: &mut StudioApp, ui: &mut Ui) {
    let Some(m) = app.arrange.fade_menu else { return };
    let mut picked = false;
    let area = egui::Area::new(egui::Id::new("fade_shape_menu")).order(egui::Order::Foreground).fixed_pos(m.at).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(120.0);
            picked = shape_items(app, ui, m.clip, m.side);
        });
    });
    if picked || (ui.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer()) {
        app.arrange.fade_menu = None;
        app.arrange.menu.retain(|x| !FadeShape::ALL.iter().any(|s| s.label() == x.label) && x.label != "Reset Fade");
    }
}
