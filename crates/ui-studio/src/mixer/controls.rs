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

/// A fader position (0 at the bottom, 1 at the top) on the design's one taper
/// (`widgets::fader::taper`), shared by the strip's fader, its scale and its meter.
pub(crate) fn fader_k(db: f32) -> f32 {
    1.0 - crate::widgets::fader::taper(db)
}

/// The top of the design taper (+6 dB): the strip's fader, its gain field and its wheel stop
/// here, as Ableton's and Logic's do. A higher gain from elsewhere (an old session, an automation
/// lane, MIDI Learn) still reads truthfully, with the cap pinned at the top of the travel.
pub(crate) const FADER_TOP_DB: f32 = match crate::tokens::fader::TAPER[0].0 {
    Some(db) => db,
    None => 0.0,
};

/// The gain at a fader position (the inverse of [`fader_k`]); the bottom is silence.
pub(crate) fn fader_db(k: f32) -> f32 {
    let db = crate::widgets::fader::db_at(1.0 - k.clamp(0.0, 1.0));
    if db.is_finite() { db.max(fine::DB_MIN) } else { fine::DB_MIN }
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
    /// A plain click opens the field (the gain field, a knob's value); otherwise a double-click.
    pub(super) click_to_type: bool,
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
    let plain = ui.input(|i| !(i.modifiers.alt || i.modifiers.ctrl || i.modifiers.command));
    if resp.double_clicked() || (ro.click_to_type && resp.clicked() && plain) {
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
            let raw = app.shown_value(t).map_or(0.0, |v| match t {
                Target::Pan(_) | Target::SendPan(..) => v,
                Target::Fader(_) => fader_k(v),
                Target::Send(..) => db_to_k(v),
            });
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
                    let db = fader_db(g.raw);
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
