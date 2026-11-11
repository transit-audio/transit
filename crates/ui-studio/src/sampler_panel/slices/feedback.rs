//! What the Slices view says back (README, Warnings and errors: Notice and Success; Sampler round
//! 2): a refused edit's error banner in the panel where the gesture belongs (the engine's words,
//! never the UI's), the Notice after an Auto gesture copied Auto into Manual (one action, Undo),
//! and the Success pattern, the panel's last-edit line led by a check.
//!
//! The error banner is the Notice card with the error's `meter.hot` edge and circle-alert (an
//! error carries its color on its glyph and edge; the message stays `text`). The Notice omits its
//! `info` glyph until the bundled icon set has it, as the legacy notice does.

use crate::theme::TOKENS;
use crate::tokens::{self, chrome as c, sampler as tk};
use crate::widgets::{self, r4};
use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// The view's feedback, per panel. Each goes at the next accepted edit in its panel.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Feedback {
    /// The Notice after an Auto gesture (its text) and the engine revision it was made at: it goes
    /// when the document changes again (the next edit, Undo, a mode switch).
    pub notice: Option<(String, u64)>,
    /// The left panel's (Detect or Manual) refusal and last edit.
    pub left_error: Option<String>,
    pub left_success: Option<String>,
    /// The Use slices panel's refusal and last edit.
    pub use_error: Option<String>,
    pub use_success: Option<String>,
    /// The Playback section's loop row refusal (the engine's words, or the One-shot press's).
    pub play_error: Option<String>,
    /// A refused file (Load… or a drop on the empty bench): the note row under the bench.
    pub note: Option<String>,
    /// The engine revision the panels' banners and lines were made at: when the document changes
    /// underneath them (undo, redo, an edit from elsewhere) they go.
    pub revision: Option<u64>,
}

impl Feedback {
    /// The panels' banners and last-edit lines go when the document is no longer the one they
    /// spoke of. Returns whether it was.
    pub fn follow(&mut self, revision: u64) -> bool {
        if self.revision == Some(revision) {
            return false;
        }
        self.revision = Some(revision);
        (self.left_error, self.left_success, self.use_error, self.use_success, self.play_error) = (None, None, None, None, None);
        true
    }
}

/// A card as drawn: its rect, its text, its edge color and (a Notice) its action's rect.
#[derive(Debug, Clone, PartialEq)]
pub struct CardRecord {
    pub rect: [f32; 4],
    pub text: String,
    pub edge: Color32,
    pub action: Option<[f32; 4]>,
}

/// A last-edit line as drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct LineRecord {
    pub rect: [f32; 4],
    pub text: String,
    /// Led by the Success check.
    pub check: bool,
}

/// The banner's glyph (the Notice icon's size) and the gap after it and before the action (the
/// `space` step 8, the board's).
const GLYPH: f32 = c::NOTICE_ICON_SIZE;
const GLYPH_GAP: f32 = tokens::SPACE[1];
/// The gap after the Success check (the `space` step 4; the board draws 6, which has no step).
const CHECK_GAP: f32 = tokens::SPACE[0];

fn padding() -> (f32, f32, f32, f32) {
    let [top, right, bottom, left] = c::NOTICE_PADDING;
    (top, right, bottom, left)
}

fn message_job(ui: &Ui, text: &str, width: f32) -> egui::text::LayoutJob {
    let mut job = tokens::LABEL_CONTROL.job(ui.ctx(), text, TOKENS.text);
    job.wrap.max_width = width.max(40.0);
    job
}

/// The text width an action of `label` leaves in a card `width` wide.
fn action_w(ui: &Ui, label: Option<&str>) -> f32 {
    label.map_or(0.0, |l| (widgets::role_size(ui.painter(), &c::BUTTON_FONT, l).x + 2.0 * c::BUTTON_PADDING[1]).ceil() + GLYPH_GAP)
}

/// The height of an error banner (`glyph`) or a Notice holding `text` across `width`.
pub fn card_height(ui: &Ui, text: &str, width: f32, glyph: bool, action: Option<&str>) -> f32 {
    let (top, right, bottom, left) = padding();
    let g = if glyph { GLYPH + GLYPH_GAP } else { 0.0 };
    let w = width - left - right - g - action_w(ui, action);
    let h = ui.painter().layout_job(message_job(ui, text, w)).size().y;
    let h = if action.is_some() { h.max(tk::VIEW_BAR_CONTROL_HEIGHT) } else { h.max(if glyph { GLYPH } else { 0.0 }) };
    top + h + bottom
}

/// Draws an error banner (`error`: the circle-alert and a `meter.hot` edge) or a Notice (a
/// `line.strong` edge, one action) at `rect`'s top across its width. Returns the record and
/// whether the action was clicked.
pub fn card(ui: &mut Ui, id: egui::Id, rect: Rect, text: &str, error: bool, action: Option<&str>) -> (CardRecord, bool) {
    let t = &TOKENS;
    let (top, right, bottom, left) = padding();
    let h = card_height(ui, text, rect.width(), error, action);
    let r = Rect::from_min_size(rect.min, vec2(rect.width(), h));
    let edge = if error { t.meter_hot } else { t.line_strong };
    let p = ui.painter().clone();
    p.rect(r, c::NOTICE_RADIUS, t.surface_raised, Stroke::new(c::FIELD_EDGE_WIDTH, edge), StrokeKind::Inside);
    let mut x = r.min.x + left;
    if error {
        crate::icons::paint(&p, pos2(x + GLYPH / 2.0, r.min.y + top + GLYPH / 2.0), crate::chrome::Severity::Error.icon(), GLYPH, t.meter_hot);
        x += GLYPH + GLYPH_GAP;
    }
    let aw = action_w(ui, action);
    let galley = p.layout_job(message_job(ui, text, r.max.x - right - aw - x));
    let ty = if action.is_some() { r.center().y - galley.size().y / 2.0 } else { r.min.y + top };
    p.galley(pos2(x, ty), galley, t.text);
    let mut clicked = false;
    let mut action_rect = None;
    if let Some(label) = action {
        let w = aw - GLYPH_GAP;
        let b =
            Rect::from_min_size(pos2(r.max.x - right - w, r.center().y - tk::VIEW_BAR_CONTROL_HEIGHT / 2.0), vec2(w, tk::VIEW_BAR_CONTROL_HEIGHT));
        let resp = ui.interact(b, id.with("action"), Sense::click());
        let fill = if resp.is_pointer_button_down_on() { t.surface_selected } else { t.button_hover };
        p.rect_filled(b, c::RADIUS_CONTROL, fill);
        widgets::role_text(&p, &c::BUTTON_FONT, label, t.text, Align2::CENTER_CENTER, b.center());
        clicked = resp.clicked();
        action_rect = Some(r4(b));
    }
    let _ = bottom;
    (CardRecord { rect: r4(r), text: text.to_string(), edge, action: action_rect }, clicked)
}

/// The last-edit line's height.
pub fn line_height(ui: &Ui) -> f32 {
    widgets::role_size(ui.painter(), &tokens::READOUT_DISPLAY, "Ag").y.max(c::SUCCESS_ICON_SIZE)
}

/// The Success line (`check`: a 12 px check in `text`, the message in `text`) or the panel's
/// resting line (no check, `text.faint`), from `at` (its top left).
pub fn last_edit(ui: &Ui, at: egui::Pos2, text: &str, check: bool) -> LineRecord {
    let t = &TOKENS;
    let p = ui.painter();
    let h = line_height(ui);
    let cy = at.y + h / 2.0;
    let mut x = at.x;
    if check {
        let g = c::SUCCESS_ICON_SIZE;
        crate::icons::paint(p, pos2(x + g / 2.0, cy), crate::icons::Icon::Check, g, t.text);
        x += g + CHECK_GAP;
    }
    let ink = if check { t.text } else { t.text_faint };
    let r = widgets::role_text(p, &tokens::READOUT_DISPLAY, text, ink, Align2::LEFT_CENTER, pos2(x, cy));
    LineRecord { rect: r4(Rect::from_min_max(at, pos2(r.max.x, at.y + h))), text: text.to_string(), check }
}
