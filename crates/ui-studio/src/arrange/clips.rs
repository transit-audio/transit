use super::*;

/// Clips as solid regions in the track color, with a name strip and their content.
#[allow(clippy::too_many_arguments)]
pub(super) fn clips(app: &StudioApp, ui: &Ui, s: &Session, map: Map, tl: Rect, rows: &[&Track], geo: &RowGeo, layout: &mut Layout) {
    let p = ui.painter().with_clip_rect(tl);
    let sel = &s.edit.selected_clips;
    let preview = drag_preview(app, ui, map, rows, tl, geo);
    // Every clip the drag carries (#218): the whole selection when the dragged clip is in it.
    let carried = preview.map(|(id, dest, a, b, _)| drag_ghosts::members(app, (id, dest, a, b))).unwrap_or_default();
    let looping = loop_drag(app, ui, map);
    let fading = crate::fades::drag(app, ui, map);
    let rh = geo.rh;
    // The loop corner shows on the clip whose loop zone the pointer is over, and while it is being loop-dragged.
    let hover = if app.arrange.gesture.is_none() { ui.input(|i| i.pointer.hover_pos()) } else { None };
    let hot = |id: u64, rect: Rect| {
        looping.as_ref().is_some_and(|l| l.clip.0 == id) || hover.is_some_and(|pos| rect.contains(pos) && zone(r4(rect), pos, true) == DragKind::Loop)
    };
    let row_y = |t: TrackId| rows.iter().position(|r| r.id == t).map(|i| tl.min.y + geo.tops[i] - app.arrange.vscroll);
    for t in rows {
        let Some(y0) = row_y(t.id) else { continue };
        for c in t.clips() {
            let rect = Rect::from_min_max(pos2(map.x(c.start), y0 + 1.0), pos2(map.x(c.end()).max(map.x(c.start) + 2.0), y0 + rh - 1.0));
            layout.clips.push(ClipRect {
                id: c.id.0,
                audio: matches!(c.content, ClipContent::Audio { .. }),
                track: t.id.0,
                rect: r4(rect),
                start: c.start,
                end: c.end(),
            });
            if rect.max.x < tl.min.x || rect.min.x > tl.max.x || rect.max.y < tl.min.y || rect.min.y > tl.max.y {
                continue;
            }
            let moving = preview.is_some_and(|(_, _, _, _, copy)| !copy) && carried.iter().any(|g| g.clip == c.id.0);
            let alpha = if moving { 0.35 } else { 1.0 };
            // The fade handle dot: on the side being dragged, else where the pointer is over one.
            let hot_fade = match &fading {
                Some(f) if f.clip == c.id => Some(f.side),
                _ => hover.filter(|pos| rect.contains(*pos)).and_then(|pos| {
                    let kind = zone(r4(rect), pos, true);
                    crate::fades::grab_at(s, map, layout, pos, kind).filter(|t| t.clip == c.id).map(|t| t.grab.side)
                }),
            };
            match &fading {
                // The fade being dragged, drawn live over the waveform.
                Some(f) if f.clip == c.id => {
                    let mut live = c.clone();
                    match f.side {
                        crate::fades::Side::In => live.fade_in = f.fade,
                        crate::fades::Side::Out => live.fade_out = f.fade,
                    }
                    draw_clip(&p, s, map, tl, t, &live, rect, sel.contains(&c.id), alpha, hot(c.id.0, rect));
                    crate::fades::overlay(&p, layout, &live, map, rect, hot_fade);
                }
                _ => {
                    draw_clip(&p, s, map, tl, t, c, rect, sel.contains(&c.id), alpha, hot(c.id.0, rect));
                    crate::fades::overlay(&p, layout, c, map, rect, hot_fade);
                }
            }
        }
    }
    // Crossfades drawn as an X over their overlap, on top of both clips.
    for t in rows {
        let Some(y0) = row_y(t.id) else { continue };
        for o in crate::fades::crossfades(s, t.id) {
            crate::fades::draw_cross(&p, layout, s, map, o, y0, rh);
        }
    }
    // A crossfade drag across an abutting boundary: the band it would make.
    if let Some(crate::fades::Live { plan: crate::fades::Plan::Cross { track, start, end }, .. }) = &fading
        && let Some(y0) = row_y(*track)
    {
        let band = Rect::from_min_max(pos2(map.x(*start), y0 + 1.0), pos2(map.x(*end).max(map.x(*start) + 2.0), y0 + rh - 1.0));
        p.rect_filled(band, crate::theme::RADIUS, TOKENS.accent.gamma_multiply(0.3));
    }
    // A loop drag: the cycles (or copies) it would add as ghosts in the clip color at reduced alpha, and
    // the part it would cut off shaded.
    if let Some(l) = &looping
        && let Some((track, base)) = s.find_clip(l.clip).map(|(t, c)| (t, c.clone()))
        && let (Some(y0), Some(t)) = (row_y(track), s.track(track))
    {
        let span = |a: Samples, b: Samples| Rect::from_min_max(pos2(map.x(a), y0 + 1.0), pos2(map.x(b).max(map.x(a) + 2.0), y0 + rh - 1.0));
        // An overwrite stops at an edit-locked clip, and so do the ghosts.
        let stop = if l.overwrite && !l.independent { overwrite_stop(s, l) } else { Samples::MAX };
        for (a, b) in l.ghosts().into_iter().filter(|(a, _)| *a < stop).map(|(a, b)| (a, b.min(stop))) {
            let mut g = base.clone();
            // The ghost plays the cycle as it would: its audio starts at the loop phase of `a`.
            let mut virt = base.clone();
            virt.looping = Some(base.looping.unwrap_or(ClipLoop { start: 0, end: base.length }));
            if let ClipContent::Audio { source, offset } = base.content {
                g.content = ClipContent::Audio { source, offset: offset.saturating_add(virt.content_pos(a - base.start)) };
            }
            // A MIDI ghost's notes are drawn below from the cycle math; the body draws none.
            if let ClipContent::Midi { sequence } = &mut g.content {
                sequence.notes.clear();
            }
            g.start = a;
            g.length = b - a;
            g.looping = None;
            g.fade_in = Default::default();
            g.fade_out = Default::default();
            let rect = span(a, b);
            if rect.max.x >= tl.min.x && rect.min.x <= tl.max.x {
                draw_clip(&p, s, map, tl, t, &g, rect, false, GHOST_ALPHA, false);
                // MIDI notes of the cycle the ghost plays, dimmed like the ghost body.
                if let ClipContent::Midi { sequence } = &base.content
                    && let Some((lo, span)) = pitch_span(&sequence.notes)
                {
                    let body = Rect::from_min_max(pos2(rect.min.x, rect.min.y + CLIP_NAME_H), rect.max).shrink2(vec2(0.0, 2.0));
                    if body.height() > 4.0 {
                        let (body_col, ..) = theme::clip_colors(clip_base_color(&base, t), false);
                        let ink = theme::darken(body_col, 0.62).gamma_multiply(GHOST_ALPHA);
                        let wp = p.with_clip_rect(rect.intersect(tl));
                        for (from, to, pitch) in
                            loop_note_spans(s, &virt, virt.looping.unwrap_or(ClipLoop { start: 0, end: base.length }), true, a - base.start)
                        {
                            let (x0, x1) = (map.x(base.start + from), map.x(base.start + to));
                            let fy = 1.0 - f32::from(pitch.saturating_sub(lo)) / span;
                            let y = body.min.y + 2.0 + fy * (body.height() - 6.0);
                            wp.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1.max(x0 + 2.0), y + 2.5)), 0.0, ink);
                        }
                    }
                }
            }
        }
        for (a, b) in overwritten(s, l) {
            p.rect_filled(span(a, b), crate::theme::RADIUS, TOKENS.header.gamma_multiply(0.6));
        }
        if let Some((a, b)) = l.cut() {
            p.rect_filled(span(a, b), crate::theme::RADIUS, TOKENS.header.gamma_multiply(0.6));
        }
    }
    // The dragged clips where they would land: the whole selection, in the clip color at the drag alpha.
    for g in carried {
        let (Some((_, c)), Some(y0), Some(t)) = (s.find_clip(ClipId(g.clip)), row_y(TrackId(g.track)), s.track(TrackId(g.track))) else { continue };
        let rect = Rect::from_min_max(pos2(map.x(g.start), y0 + 1.0), pos2(map.x(g.end).max(map.x(g.start) + 2.0), y0 + rh - 1.0));
        draw_clip(&p, s, map, tl, t, c, rect, true, 0.85, false);
        layout.drag_ghosts.push(drag_ghosts::DragGhost { rect: r4(rect), ..g });
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_clip(p: &egui::Painter, s: &Session, map: Map, tl: Rect, t: &Track, c: &Clip, rect: Rect, selected: bool, alpha: f32, hot: bool) {
    let base = clip_base_color(c, t);
    let (body_col, strip_col, name_col) = theme::clip_colors(base, selected);
    let p = p.with_clip_rect(rect.intersect(tl));
    p.rect_filled(rect, crate::theme::RADIUS, body_col.gamma_multiply(alpha));
    let strip = Rect::from_min_max(rect.min, pos2(rect.max.x, (rect.min.y + CLIP_NAME_H).min(rect.max.y)));
    p.rect_filled(strip, crate::theme::top_corners(), strip_col.gamma_multiply(alpha));
    let name_x = rect.min.x.max(tl.min.x) + 4.0;
    let looped = c.looping.is_some_and(|l| l.is_valid());
    let corner = corner_alpha(looped, hot).and_then(|a| loop_corner_rect(rect, strip).map(|r| (r, a)));
    // The name stops short of the loop corner while it is drawn, so the two never overlap.
    let mut name_clip = strip.intersect(tl);
    if let Some((r, _)) = corner {
        name_clip.max.x = name_clip.max.x.min(r.min.x - 2.0);
    }
    p.with_clip_rect(name_clip).text(pos2(name_x, strip.center().y), Align2::LEFT_CENTER, &c.name, FontId::proportional(11.0), name_col);
    if let Some((r, a)) = corner {
        draw_loop_corner(&p, r, name_col.gamma_multiply(a * alpha));
    }
    let body = Rect::from_min_max(pos2(rect.min.x, strip.max.y), rect.max).shrink2(vec2(0.0, 2.0));
    let ink = theme::darken(body_col, 0.62).gamma_multiply(alpha);
    if let Some(l) = c.looping.filter(|l| l.is_valid()) {
        draw_cycles(&p, s, map, tl, t, c, l, rect, strip, body, ink);
    } else if body.height() > 4.0 {
        match &c.content {
            ClipContent::Audio { source, .. } => {
                if let Some(audio) = s.pool.get(*source) {
                    let wp = p.with_clip_rect(body.intersect(tl));
                    crate::waveform::draw(&wp, s, tl, c, audio, t.channels().min(2), body, ink);
                }
            }
            ClipContent::Midi { sequence } => {
                let notes = &sequence.notes;
                if let (Some(lo), Some(hi)) = (notes.iter().map(|n| n.pitch).min(), notes.iter().map(|n| n.pitch).max()) {
                    let span = f32::from(hi.saturating_sub(lo)).max(11.0);
                    let clip_tick = s.tempo.samples_to_ticks(c.start, s.sample_rate);
                    for n in notes.iter().take(5000) {
                        let a = s.tempo.tick_to_samples(clip_tick.saturating_add(n.start), s.sample_rate);
                        let b = s.tempo.tick_to_samples(clip_tick.saturating_add(n.end()), s.sample_rate);
                        let fy = 1.0 - f32::from(n.pitch.saturating_sub(lo)) / span;
                        let y = body.min.y + 2.0 + fy * (body.height() - 6.0);
                        let r = Rect::from_min_max(pos2(map.x(a), y), pos2(map.x(b).max(map.x(a) + 2.0), y + 2.5));
                        p.rect_filled(r, 0.0, ink);
                    }
                }
            }
        }
    }
    crate::fades::draw(&p, c, map, rect, body, ink, body_col.gamma_multiply(alpha), selected);
    let stroke = if selected { Stroke::new(2.0, TOKENS.selection_outline) } else { Stroke::new(1.0, theme::darken(base, 0.45)) };
    p.rect_stroke(rect, crate::theme::RADIUS, stroke, StrokeKind::Inside);
}

/// A looped clip's cycles (ADR 0019): the first pass `0..loop.end`, then one segment per cycle,
/// each (clip-relative start, end, content position at its start), cut at the clip's length.
pub(crate) fn cycle_segments(c: &Clip, l: ClipLoop) -> impl Iterator<Item = (Samples, Samples, Samples)> {
    let (length, end, cycle) = (c.length, l.end, l.cycle());
    std::iter::once((0, end.min(length), 0)).chain(
        std::iter::successors(Some(end), move |x| x.checked_add(cycle)).take_while(move |x| *x < length).map(move |a| {
            let b = a.saturating_add(cycle).min(length);
            (a, b, l.start)
        }),
    )
}

/// Draw a looped clip's content once per cycle, with a divider tick at every seam.
#[allow(clippy::too_many_arguments)]
fn draw_cycles(
    p: &egui::Painter,
    s: &Session,
    map: Map,
    tl: Rect,
    t: &Track,
    c: &Clip,
    l: ClipLoop,
    rect: Rect,
    strip: Rect,
    body: Rect,
    ink: Color32,
) {
    let sx = |rel: Samples| rect.min.x + (rel as f64 / map.spp) as f32;
    // A MIDI clip's seams sit where playback puts them: on the tick-rounded frame.
    let seams = if matches!(c.content, ClipContent::Midi { .. }) {
        let (origin, f) = transit_mix::loop_frame(s, c.start, l);
        let gap = origin - c.start;
        ClipLoop { start: f.start + gap, end: f.end + gap }
    } else {
        l
    };
    for (i, (a, b, phase)) in cycle_segments(c, seams).enumerate() {
        let (x0, x1) = (sx(a), sx(b).max(sx(a) + 1.0));
        if x1 < tl.min.x || x0 > tl.max.x {
            continue;
        }
        if i > 0 {
            // The seam: a tick through the header strip and a faint line down the body.
            p.line_segment([pos2(x0, strip.min.y), pos2(x0, strip.max.y)], Stroke::new(1.5, TOKENS.text_dim));
            p.line_segment([pos2(x0, strip.max.y), pos2(x0, rect.max.y)], Stroke::new(1.0, ink.gamma_multiply(0.6)));
        }
        if body.height() <= 4.0 {
            continue;
        }
        let seg = Rect::from_min_max(pos2(x0, body.min.y), pos2(x1, body.max.y));
        match &c.content {
            ClipContent::Audio { source, offset } => {
                if let Some(audio) = s.pool.get(*source) {
                    // The segment as a clip of its own that plays the file from its cycle phase.
                    let mut g = c.clone();
                    g.start = c.start.saturating_add(a);
                    g.length = (b - a).max(1);
                    g.looping = None;
                    g.content = ClipContent::Audio { source: *source, offset: offset.saturating_add(phase) };
                    if i > 0 {
                        g.fade_in = Default::default();
                    }
                    if b < c.length {
                        g.fade_out = Default::default();
                    }
                    let wp = p.with_clip_rect(seg.intersect(tl));
                    crate::waveform::draw(&wp, s, tl, &g, audio, t.channels().min(2), seg, ink);
                }
            }
            ClipContent::Midi { sequence } => {
                if let Some((lo, span)) = pitch_span(&sequence.notes) {
                    let wp = p.with_clip_rect(seg.intersect(tl));
                    for (from, to, pitch) in loop_note_spans(s, c, l, i > 0, a) {
                        let fy = 1.0 - f32::from(pitch.saturating_sub(lo)) / span;
                        let y = body.min.y + 2.0 + fy * (body.height() - 6.0);
                        let r = Rect::from_min_max(pos2(sx(from), y), pos2(sx(to.min(b)).max(sx(from) + 2.0), y + 2.5));
                        wp.rect_filled(r, 0.0, ink);
                    }
                }
            }
        }
    }
}

/// A clip's base color: its own or the track's, darkened when muted.
fn clip_base_color(c: &Clip, t: &Track) -> Color32 {
    let base = theme::rgb(c.color.unwrap_or(t.color));
    if c.muted { theme::darken(base, 0.5) } else { base }
}

/// The lowest pitch and the pitch span (at least an octave) a MIDI clip's mini notes are laid out in.
fn pitch_span(notes: &[transit_midi::Note]) -> Option<(u8, f32)> {
    let (lo, hi) = (notes.iter().map(|n| n.pitch).min()?, notes.iter().map(|n| n.pitch).max()?);
    Some((lo, f32::from(hi.saturating_sub(lo)).max(11.0)))
}

/// The notes one cycle of a looped MIDI clip plays, as (clip-relative start, end, pitch) spans of
/// the cycle that contains clip time `a` (ADR 0019): cycle 0 plays what starts before the loop end,
/// a `later` cycle what starts inside the range, and a note crossing the seam ends there. The cycles
/// come from [`transit_mix::loop_frame`], the tick-rounded frame playback uses, so an off-tick
/// loop edge draws where it is heard.
pub(crate) fn loop_note_spans(s: &Session, c: &Clip, l: ClipLoop, later: bool, a: Samples) -> Vec<(Samples, Samples, u8)> {
    let ClipContent::Midi { sequence } = &c.content else { return Vec::new() };
    let (origin, fl) = transit_mix::loop_frame(s, c.start, l);
    let clip_tick = s.tempo.samples_to_ticks(c.start, s.sample_rate);
    // Cycle `k >= 1` of the stepped frame that holds `a`.
    let k = (a - (origin - c.start) - fl.end).div_euclid(fl.cycle().max(1)) + 1;
    let mut out = Vec::new();
    for n in sequence.notes.iter().take(5000) {
        let on = s.tempo.tick_to_samples(clip_tick.saturating_add(n.start), s.sample_rate);
        let off = s.tempo.tick_to_samples(clip_tick.saturating_add(n.end()), s.sample_rate);
        let rel = on - origin;
        if rel >= fl.end || (later && rel < fl.start) || (!later && rel < 0) {
            continue;
        }
        let span = (off.min(origin + fl.end) - on).max(0);
        let at = if later { transit_mix::cycle_at(origin, fl, rel, k.max(1)) } else { on };
        out.push((at - c.start, at + span - c.start, n.pitch));
    }
    out
}

/// Smallest loop glyph (points) worth drawing; a clip too narrow for it shows no corner.
const LOOP_GLYPH_MIN: f32 = 6.0;
/// The loop corner glyph's full size (points).
const LOOP_GLYPH: f32 = 10.0;
/// The loop cursor badge's size (points).
const LOOP_BADGE: f32 = 11.0;
/// How far right and below the pointer tip the badge is centered (points).
const LOOP_BADGE_OFFSET: f32 = 12.0;

/// How strongly the loop corner shows: full while the pointer is in the zone or a loop drag is on,
/// dim on a clip that already loops, none otherwise.
fn corner_alpha(looped: bool, hot: bool) -> Option<f32> {
    if hot {
        Some(1.0)
    } else if looped {
        Some(0.5)
    } else {
        None
    }
}

/// Where the loop corner glyph goes: the loop hot zone's part of the name `strip`, between the
/// trim edge and the zone's inner end, so it never touches the trim edge or a neighboring clip.
/// `None` when the zone is too narrow for a legible glyph.
fn loop_corner_rect(rect: Rect, strip: Rect) -> Option<Rect> {
    let w = rect.width();
    let edge = EDGE.min(w / 4.0);
    let zone_w = LOOP_ZONE_W.min(w / 3.0);
    let size = (zone_w - edge - 2.0).min(LOOP_GLYPH).min(strip.height() - 2.0);
    if size < LOOP_GLYPH_MIN {
        return None;
    }
    let right = rect.max.x - edge - 1.0;
    Some(Rect::from_center_size(pos2(right - size / 2.0, strip.center().y), vec2(size, size)))
}

/// Points in the loop glyph's arc path (the shape signature the render tests look for).
const LOOP_ARC_POINTS: usize = 13;

/// The loop glyph: a three-quarter arc (gap at the upper right) ending in a small open arrowhead
/// (two short strokes), in `r`. The stroke is 1.25 px at 11 px and scales with the glyph; `extra`
/// widens it (the cursor badge's dark outline is the same shape drawn 2 px wider).
fn loop_glyph(r: Rect, color: Color32, extra: f32) -> Vec<Shape> {
    let size = r.width().min(r.height());
    let c = r.center();
    let rad = size * 0.34;
    let at = |a: f32| pos2(c.x + rad * a.cos(), c.y + rad * a.sin());
    // Screen angles run clockwise (y down): start at 3 o'clock, sweep 270 degrees, end at 12 o'clock.
    let (a0, a1) = (0.0_f32, std::f32::consts::PI * 1.5);
    let n = LOOP_ARC_POINTS - 1;
    let pts: Vec<Pos2> = (0..=n).map(|i| at(a0 + (a1 - a0) * i as f32 / n as f32)).collect();
    let end = at(a1);
    // Unit tangent (direction of travel) and outward normal at the end of the arc.
    let (tx, ty) = (-a1.sin(), a1.cos());
    let (nx, ny) = (a1.cos(), a1.sin());
    let h = size * 0.27;
    let tip = pos2(end.x + tx * h * 0.7, end.y + ty * h * 0.7);
    let barb = |s: f32| pos2(tip.x - tx * h + nx * h * s, tip.y - ty * h + ny * h * s);
    let stroke = Stroke::new(size * 1.25 / 11.0 + extra, color);
    vec![Shape::line(pts, stroke), Shape::line(vec![barb(-1.0), tip, barb(1.0)], stroke)]
}

/// Paint the loop corner glyph in `r`.
fn draw_loop_corner(p: &egui::Painter, r: Rect, color: Color32) {
    p.extend(loop_glyph(r, color, 0.0));
}

/// The loop cursor: the OS arrow stays, and a small loop badge with a 1 px dark outline is painted
/// at the arrow's lower right (Logic's pattern) on the topmost layer. Call every frame the pointer
/// is over the zone or a loop drag runs; on the first frame it is not called the badge is gone.
pub(super) fn loop_cursor(ctx: &egui::Context, pos: Pos2) {
    ctx.set_cursor_icon(CursorIcon::Default);
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("loop_cursor")));
    let r = Rect::from_center_size(pos + vec2(LOOP_BADGE_OFFSET, LOOP_BADGE_OFFSET), vec2(LOOP_BADGE, LOOP_BADGE));
    p.extend(loop_glyph(r, TOKENS.ink_dark, 2.0));
    p.extend(loop_glyph(r, TOKENS.ink_light, 0.0));
}

#[cfg(test)]
mod loop_corner_tests {
    use super::*;

    fn frame(ctx: &egui::Context, f: impl FnMut(&mut Ui)) -> egui::FullOutput {
        let mut out = ctx.run_ui(Default::default(), f);
        out.textures_delta.clear();
        out
    }

    fn clip_rect(w: f32) -> (Rect, Rect) {
        let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(w, 60.0));
        let strip = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + CLIP_NAME_H));
        (rect, strip)
    }

    fn painted(w: f32, looped: bool, hot: bool) -> usize {
        let ctx = egui::Context::default();
        let (rect, strip) = clip_rect(w);
        let mut n = 0;
        let _ = frame(&ctx, |ui| {
            let ctx = ui.ctx();
            let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("t")));
            if let Some((r, a)) = corner_alpha(looped, hot).and_then(|a| loop_corner_rect(rect, strip).map(|r| (r, a))) {
                draw_loop_corner(&p, r, TOKENS.ink_light.gamma_multiply(a));
                n += 1;
            }
        });
        n
    }

    #[test]
    fn looped_clip_shows_corner_without_hover() {
        assert_eq!(painted(200.0, true, false), 1);
        assert_eq!(painted(200.0, false, false), 0);
        assert_eq!(painted(200.0, false, true), 1);
        assert!(corner_alpha(true, false) < corner_alpha(true, true));
    }

    #[test]
    fn narrow_clip_has_no_corner() {
        assert_eq!(painted(30.0, true, true), 0);
        assert_eq!(painted(120.0, true, true), 1);
    }

    #[test]
    fn corner_stays_inside_the_loop_zone() {
        for w in [45.0_f32, 60.0, 200.0] {
            let (rect, strip) = clip_rect(w);
            let r = loop_corner_rect(rect, strip).expect("fits");
            assert!(rect.contains_rect(r) && strip.contains_rect(r));
            assert!(rect.max.x - r.max.x >= EDGE.min(w / 4.0), "clear of the trim edge");
            assert_eq!(zone(r4(rect), r.center(), true), DragKind::Loop, "inside the hot zone");
        }
    }

    /// Every path painted in a frame that holds only the cursor badge (Tooltip layer), as (point count, bounds).
    fn badge_paths(out: &egui::FullOutput) -> Vec<(usize, Rect)> {
        fn walk(shape: &Shape, acc: &mut Vec<(usize, Rect)>) {
            match shape {
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, acc)),
                Shape::Path(p) => acc.push((p.points.len(), Rect::from_points(&p.points))),
                _ => {}
            }
        }
        let mut acc = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut acc);
        }
        acc
    }

    #[test]
    fn cursor_is_the_os_arrow_with_a_badge_and_restored_after() {
        let ctx = egui::Context::default();
        let tip = pos2(10.0, 10.0);
        let out = frame(&ctx, |ui| loop_cursor(ui.ctx(), tip));
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::Default, "the OS arrow stays visible");
        let paths = badge_paths(&out);
        // Outline and fill, each an arc plus an arrowhead.
        assert_eq!(paths.iter().filter(|(n, _)| *n == LOOP_ARC_POINTS).count(), 2, "outline and fill arcs");
        assert_eq!(paths.iter().filter(|(n, _)| *n == 3).count(), 2, "outline and fill arrowheads");
        let want = Rect::from_center_size(tip + vec2(LOOP_BADGE_OFFSET, LOOP_BADGE_OFFSET), vec2(LOOP_BADGE, LOOP_BADGE)).expand(1.5);
        for (_, b) in &paths {
            assert!(want.contains_rect(*b), "badge {b:?} stays near its 11 px cell {want:?}");
            assert!(b.min.x > tip.x + 4.0 && b.min.y > tip.y + 4.0, "badge sits at the lower right of the tip");
        }
        let out = frame(&ctx, |_| {});
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::Default);
        assert!(badge_paths(&out).is_empty(), "nothing painted after leaving");
    }

    /// A real app frame loop with one looped MIDI clip named "LoopName" on a lane.
    struct Rig {
        app: crate::StudioApp,
        ctx: egui::Context,
        time: f64,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl Rig {
        fn new() -> Rig {
            let mut app = crate::StudioApp::new(transit_engine::Engine::default(), None, crate::Services::default());
            let r = app.run("track.new", json!({"kind": "instrument"})).unwrap();
            let t = r["tracks"][0].as_u64().unwrap();
            let c = app.run("midi.new_clip", json!({"tracks": [t], "start": 0, "end": 0})).unwrap();
            let (id, end) = (c["clip"].as_u64().unwrap(), c["end"].as_i64().unwrap());
            app.run("clip.rename", json!({"clips": [id], "name": "LoopName"})).unwrap();
            app.run("clip.loop_to", json!({"clip": id, "to": end * 3})).unwrap();
            let mut rig = Rig { app, ctx: egui::Context::default(), time: 0.0, shapes: Vec::new() };
            rig.frames(3, None);
            rig
        }

        fn frames(&mut self, n: usize, hover: Option<Pos2>) {
            for _ in 0..n {
                self.time += 1.0 / 60.0;
                let mut raw = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))),
                    time: Some(self.time),
                    ..Default::default()
                };
                if let Some(pos) = hover {
                    raw.events.push(egui::Event::PointerMoved(pos));
                }
                self.app.raw_input_hook(&mut raw);
                let app = &mut self.app;
                let mut out = self.ctx.run_ui(raw, |ui| {
                    app.logic(ui.ctx());
                    app.ui(ui);
                });
                out.textures_delta.clear();
                self.shapes = out.shapes;
            }
        }

        fn clip(&self) -> Rect {
            let r = self.app.arrange.layout.clips.last().expect("a clip is laid out").rect;
            Rect::from_min_max(pos2(r[0], r[1]), pos2(r[2], r[3]))
        }
    }

    /// The name text's clip rect, and the alpha of the loop glyph's arc (a path inside the clip's name strip).
    fn name_and_glyph(rig: &Rig) -> (Rect, Option<u8>) {
        fn walk(shape: &Shape, clip: Rect, strip: Rect, name: &mut Option<Rect>, glyph: &mut Option<u8>) {
            match shape {
                Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, strip, name, glyph)),
                Shape::Text(t) if t.galley.text() == "LoopName" => *name = Some(clip),
                Shape::Path(path)
                    if path.points.len() == LOOP_ARC_POINTS
                        && (path.stroke.width - 10.0 * 1.25 / 11.0).abs() < 0.01
                        && strip.contains_rect(Rect::from_points(&path.points)) =>
                {
                    if let egui::epaint::ColorMode::Solid(c) = path.stroke.color {
                        *glyph = Some(c.a());
                    }
                }
                _ => {}
            }
        }
        let rect = rig.clip();
        let strip = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + CLIP_NAME_H));
        let (mut name, mut glyph) = (None, None);
        for s in &rig.shapes {
            walk(&s.shape, s.clip_rect, strip, &mut name, &mut glyph);
        }
        (name.expect("the clip name is painted"), glyph)
    }

    #[test]
    fn rendered_name_stops_before_the_loop_glyph() {
        let rig = Rig::new();
        let rect = rig.clip();
        let strip = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + CLIP_NAME_H));
        let glyph = loop_corner_rect(rect, strip).expect("a wide clip has a corner");
        let (name_clip, drawn) = name_and_glyph(&rig);
        assert!(drawn.is_some(), "the loop glyph is painted on a looped clip");
        assert!(name_clip.max.x < glyph.min.x, "name clip {} must end before the glyph at {}", name_clip.max.x, glyph.min.x);
    }

    #[test]
    fn hovering_the_loop_zone_paints_the_glyph_at_full_strength() {
        let mut rig = Rig::new();
        let rect = rig.clip();
        let idle = name_and_glyph(&rig).1.expect("dim glyph without hover");
        // Inside the loop zone: the name strip, between the trim edge and the zone's inner end.
        let zone_pos = pos2(rect.max.x - EDGE - 4.0, rect.min.y + CLIP_NAME_H / 2.0);
        assert_eq!(zone(r4(rect), zone_pos, false), DragKind::Loop);
        rig.frames(3, Some(zone_pos));
        let hot = name_and_glyph(&rig).1.expect("glyph painted while hovered");
        assert!(hot > idle, "hover strengthens the glyph: {idle} -> {hot}");
        // Pointer elsewhere on the clip: back to dim.
        rig.frames(3, Some(pos2(rect.min.x + 40.0, rect.min.y + CLIP_NAME_H / 2.0)));
        assert_eq!(name_and_glyph(&rig).1, Some(idle));
    }

    #[test]
    fn the_overwrite_preview_does_not_shade_clips_the_commit_will_not_touch() {
        use transit_model::{ChannelFormat, Clip, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let mut ids = Vec::new();
        for (start, len, locked) in [(0, 1000, false), (1000, 500, false), (1500, 500, true), (2000, 500, false)] {
            let id = s.new_clip_id();
            let mut c = Clip::midi(id, "m", start, len, Default::default());
            c.edit_locked = locked;
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
            ids.push(id);
        }
        let drag = |to| LoopDrag {
            clip: ids[0],
            from: 1000,
            to,
            cycle: 1000,
            first_end: 1000,
            start: 0,
            looped: false,
            independent: false,
            overwrite: true,
        };
        // Dragging across the locked clip shades the clip before it only.
        assert_eq!(overwritten(&s, &drag(2400)), vec![(1000, 1500)]);
        assert_eq!(overwrite_stop(&s, &drag(2400)), 1500);
        // Short of it, as before.
        assert_eq!(overwritten(&s, &drag(1200)), vec![(1000, 1200)]);
        // A locked clip that abuts the clip stops the drag at once: nothing shaded.
        assert!(overwritten(&s, &LoopDrag { from: 1500, start: 1000, clip: ids[1], ..drag(2400) }).is_empty());
    }
}
