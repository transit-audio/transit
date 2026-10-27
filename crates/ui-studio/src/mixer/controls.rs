use super::*;

/// Fader position 0..1 for a gain in dB: the fourth root of the amplitude, scaled so +12 dB is
/// the top. 0 dB sits at about 70 %, -60 dB at about 13 %; -144 dB (silence) is the bottom.
pub fn db_to_k(db: f32) -> f32 {
    if db <= -144.0 {
        return 0.0;
    }
    let top = 10f32.powf(FADER_MAX_DB / 80.0);
    (10f32.powf(db / 80.0) / top).clamp(0.0, 1.0)
}

/// The inverse of [`db_to_k`].
pub fn k_to_db(k: f32) -> f32 {
    if k <= 0.0005 {
        return -144.0;
    }
    let top = 10f32.powf(FADER_MAX_DB / 80.0);
    (80.0 * (k.min(1.0) * top).log10()).clamp(-144.0, FADER_MAX_DB)
}

/// The panners for balance `b` (-1..1) on a track with `n` of them (plan 06, L10). Mono: the one
/// panner is `b`. Stereo: center keeps L hard left and R hard right; turning toward a side moves
/// the other channel's panner across, so at the end both sit on that side.
pub fn pans_for(b: f32, n: usize) -> Vec<f32> {
    let b = b.clamp(-1.0, 1.0);
    match n {
        0 => Vec::new(),
        1 => vec![b],
        _ => vec![-1.0 + 2.0 * b.max(0.0), 1.0 + 2.0 * b.min(0.0)],
    }
}

/// The balance a set of panners stands for (the inverse of [`pans_for`]).
pub fn balance_of(pan: &[f32]) -> f32 {
    match pan {
        [] => 0.0,
        [p] => *p,
        [l, r, ..] => ((l + r) / 2.0).clamp(-1.0, 1.0),
    }
}

/// Mix the strip background toward its readable theme ink to a fixed contrast. Selected
/// and normal strips therefore retain the same visual hierarchy in either appearance.
fn knob_ink(background: Color32, ratio: f32) -> Color32 {
    let ink = theme::text_on(background);
    let mix = |k: f32| background.lerp_to_gamma(ink, k);
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..12 {
        let mid = (low + high) * 0.5;
        if theme::contrast(mix(mid), background) < ratio {
            low = mid;
        } else {
            high = mid;
        }
    }
    mix(high)
}

pub(super) fn draw_knob(p: &egui::Painter, r: Rect, b: f32, hot: bool, background: Color32) {
    // Flat: a thin ring for the travel and the accent from the center to the value.
    let c = r.center();
    let rad = r.width() / 2.0 - 4.0;
    let ang = |v: f32| (-90.0 + v * 135.0_f32).to_radians();
    let arc = |from: f32, to: f32| -> Vec<egui::Pos2> {
        let n = 24;
        (0..=n).map(|i| from + (to - from) * i as f32 / n as f32).map(|v| c + vec2(ang(v).cos(), ang(v).sin()) * rad).collect()
    };
    p.add(egui::Shape::line(arc(-1.0, 1.0), Stroke::new(3.0, knob_ink(background, 3.0))));
    // The center notch: a short line across the ring at the top.
    p.line_segment([c + vec2(0.0, -(rad - 3.5)), c + vec2(0.0, -(rad + 3.5))], Stroke::new(1.0, knob_ink(background, 4.5)));
    p.add(egui::Shape::line(arc(0.0, b), Stroke::new(3.0, if hot { theme::lighten(TOKENS.accent, 0.15) } else { TOKENS.accent })));
    p.circle_filled(c + vec2(ang(b).cos(), ang(b).sin()) * rad, 2.5, knob_ink(background, 7.0));
}

/// A one-point line at `y` as whole physical pixels: the y of its center, the rows it covers
/// ([top, bottom)) and its thickness in points, so the line is crisp at any scale.
fn snap_line(y: f32, ppp: f32) -> (f32, [i32; 2], f32) {
    let n = ppp.round().max(1.0);
    let top = (y * ppp - n / 2.0).round();
    ((top + n / 2.0) / ppp, [top as i32, (top + n) as i32], n / ppp)
}

type Mark = (Option<f32>, Option<[i32; 2]>);

/// Draws the fader; returns the (center y, pixel rows) of the cap's center line and of the 0 dB tick.
pub(super) fn fader_widget(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, id: TrackId, db: f32, r: Rect) -> (Mark, Mark) {
    let ppp = ui.ctx().pixels_per_point();
    let mut zero = (None, None);
    let me = Target::Fader(id);
    let resp = ui.interact(r.expand2(vec2(4.0, 6.0)).intersect(clip), ui.id().with(("fader", id.0)), Sense::click_and_drag());
    // Track and marks: a line across the track at 0 dB, fainter ticks at -6, -12 and -24.
    let groove = Rect::from_center_size(r.center(), vec2(4.0, r.height()));
    p.rect_filled(groove, crate::theme::RADIUS_SM, TOKENS.bg);
    let y_of = |k: f32| r.max.y - k * r.height();
    for mark in fine::DB_MARKS {
        let (yy, rows, w) = snap_line(y_of(db_to_k(mark)), ppp);
        if mark == 0.0 {
            zero = (Some(yy), Some(rows));
            p.line_segment([pos2(r.min.x, yy), pos2(r.max.x, yy)], Stroke::new(w, TOKENS.text_dim));
        } else {
            p.line_segment([pos2(r.center().x - 8.0, yy), pos2(r.center().x + 8.0, yy)], Stroke::new(w, TOKENS.text_dim.gamma_multiply(0.5)));
        }
    }
    let k = db_to_k(db);
    let (cap_y, cap_rows, cap_w) = snap_line(y_of(k), ppp);
    let cap = Rect::from_center_size(pos2(r.center().x, cap_y), vec2(r.width().min(26.0), 12.0));
    let hot = resp.hovered() || app.mixer.gesture.is_some_and(|g| g.target == me);
    p.rect_filled(cap, crate::theme::RADIUS_SM, if hot { TOKENS.accent } else { TOKENS.text_dim });
    // The line through the cap marks the exact point the value sits at, to line up with the 0 dB line.
    p.line_segment([pos2(cap.min.x, cap_y), pos2(cap.max.x, cap_y)], Stroke::new(cap_w, TOKENS.bg));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    control(app, ui, &resp, me, r.height().max(1.0), true, true);
    resp.context_menu(|ui| app.midi_learn_menu(ui, id, transit_model::Control::Volume));
    crate::midi_learn::mark(app, p, r.expand2(vec2(4.0, 0.0)), id, transit_model::Control::Volume);
    ((Some(cap_y), Some(cap_rows)), zero)
}

/// A number under a control that is also a way in: double-click types a value in its place,
/// the wheel nudges it, Alt-click and Ctrl-click reset it.
pub(super) struct Readout {
    pub(super) target: Target,
    /// Tells apart two readouts of one control (a strip's and the device chain's).
    pub(super) salt: u64,
    /// Where the text is laid out, and where the pointer hits it.
    pub(super) draw: Rect,
    pub(super) hit: Rect,
    pub(super) text: String,
    /// What the field starts as when editing opens.
    pub(super) edit_text: String,
    pub(super) font: FontId,
    pub(super) color: Color32,
    pub(super) align: Align2,
    /// Also drag it like the control (a send's number).
    pub(super) drag: bool,
}

pub(super) fn readout(app: &mut StudioApp, ui: &mut Ui, p: &egui::Painter, clip: Rect, ro: Readout) {
    let id = ui.id().with(("readout", ro.target.salt(), ro.salt));
    if app.mixer.edit.as_ref().is_some_and(|e| e.target == ro.target && e.salt == ro.salt) {
        edit_field(app, ui, ro.draw, id.with("edit"), ro.font);
        return;
    }
    let sense = if ro.drag { Sense::click_and_drag() } else { Sense::click() };
    let resp = ui.interact(ro.hit.intersect(clip), id, sense);
    let color = if resp.hovered() { TOKENS.accent } else { ro.color };
    let at = ro.align.pos_in_rect(&ro.draw.shrink2(vec2(2.0, 0.0)));
    p.with_clip_rect(ro.draw.intersect(clip)).text(
        at + if ro.align == Align2::RIGHT_CENTER { vec2(0.0, -1.0) } else { vec2(0.0, 0.0) },
        ro.align,
        &ro.text,
        ro.font,
        color,
    );
    control(app, ui, &resp, ro.target, 1.0, ro.drag, false);
    if resp.double_clicked() {
        app.mixer.edit = Some(fine::ReadoutEdit { target: ro.target, salt: ro.salt, text: ro.edit_text, fresh: true });
    }
}

/// What a fader, knob or send level does with the pointer, the wheel and the keyboard:
/// - drag moves it (Shift: ten times finer); a slow drag snaps to a mark;
/// - double-click (`dbl_reset`), Alt-click, Ctrl-click and Delete reset it (0 dB, or center);
/// - the wheel nudges it, and the nudges of one gesture are one undo step.
pub(super) fn control(app: &mut StudioApp, ui: &Ui, resp: &egui::Response, t: Target, height: f32, drag: bool, dbl_reset: bool) {
    let (mods, now) = ui.input(|i| (i.modifiers, i.time));
    if resp.hovered() || resp.dragged() {
        app.mixer.hover = Some(t);
        app.mixer.wheel_taken = true;
    }
    if (resp.clicked() && (mods.alt || mods.ctrl || mods.command)) || (dbl_reset && resp.double_clicked()) {
        app.reset_control(t);
        return;
    }
    if drag {
        if resp.drag_started() {
            // A write pass (slice 7) holds one undo step for the pass and the drags in it.
            if !crate::automation::passing(app) {
                app.engine.end_merge();
            }
            app.mixer.wheel = None;
            let raw = app.shown_value(t).map_or(0.0, |v| if t.is_pan() { v } else { db_to_k(v) });
            app.mixer.gesture = Some(MixGesture { target: t, raw, height });
        }
        if resp.dragged()
            && let Some(mut g) = app.mixer.gesture.filter(|g| g.target == t)
        {
            let (step, now_delta) = ui.input(|i| {
                let d = if resp.drag_started() {
                    i.pointer.press_origin().zip(i.pointer.latest_pos()).map_or(vec2(0.0, 0.0), |(a, b)| b - a)
                } else {
                    i.pointer.delta()
                };
                (d, i.pointer.delta().length())
            });
            let ratio = if mods.shift { fine::FINE } else { 1.0 };
            let per_px = match t {
                Target::Fader(_) => -step.y / g.height,
                Target::Send(..) => (step.x - step.y) / 150.0,
                Target::Pan(_) => -step.y / 100.0,
                Target::SendPan(..) => (step.x - step.y) / 100.0,
            };
            let (lo, hi) = if t.is_pan() { (-1.0, 1.0) } else { (0.0, 1.0) };
            g.raw = (g.raw + per_px * ratio).clamp(lo, hi);
            app.mixer.gesture = Some(g);
            let slow = now_delta < fine::SLOW_PT && !mods.shift;
            let v = match t {
                Target::Pan(_) | Target::SendPan(..) if slow => fine::snap_pan(g.raw),
                Target::Pan(_) | Target::SendPan(..) => g.raw,
                Target::Fader(_) => {
                    let db = k_to_db(g.raw);
                    if slow { fine::snap_db(db, &fine::DB_MARKS) } else { db }
                }
                Target::Send(..) => {
                    let db = k_to_db(g.raw);
                    if slow { fine::snap_db(db, &fine::SEND_MARKS) } else { db }
                }
            };
            let _ = app.set_value_merged(t, v, None);
        }
        if resp.drag_stopped() && app.mixer.gesture.is_some_and(|g| g.target == t) {
            app.mixer.gesture = None;
        }
    }
    if resp.hovered() && app.mixer.gesture.is_none() {
        let (notches, shift) = ui.input(|i| {
            let mut n = 0.0;
            let mut shift = false;
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e {
                    let d = if delta.x.abs() > delta.y.abs() { delta.x } else { delta.y };
                    n += match unit {
                        egui::MouseWheelUnit::Point => d / 50.0,
                        _ => d,
                    };
                    shift |= modifiers.shift;
                }
            }
            (n, shift)
        });
        if notches != 0.0 {
            app.wheel_nudge(t, notches, shift, resp.rect, now);
        }
    }
}
