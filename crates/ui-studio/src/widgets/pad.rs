//! The pad (`docs/design/language`, Components > Pad; the board "Pads with choke groups"): a
//! drum-machine trigger for the Sampler's Pads view and any pad grid. Square, radius 6,
//! `surface.raised` with a 1 px `line`; two sizes, 56 (a grid in a panel) and 80 (focused). The
//! numbers are `pad` in tokens.json ([`crate::tokens::pad`]); the insets of the id, the badge, the
//! name and the chip are the board's at each size.
//!
//! - **Top-left** the pad id, bank and slot (`A6`), Mono 400 9 (56) or 10 (80), `text.faint`.
//! - **Top-right** the choke badge: the group number in Mono 500 9 in a box at least 16 by 14,
//!   radius 3, 1 px `line.strong`, `text.muted`; absent when Off. Choked (for 200 ms after a cut by
//!   its group) it inverts: fill and edge `text.muted`, number `surface.panel`.
//! - **Top edge** the group's color as a 3 px tag, only when the user picked one.
//! - **Bottom** the sample name (Sans 500 10 or 11, cut with an ellipsis) and a 14 px M chip
//!   (`text`, letter `bg.window`) or S chip (`signal`, letter `surface.inset`); S shows when both.
//! - **Hit** a fill from the bottom in `signal` at 26 %, its height the velocity, with a 2 px
//!   solid `signal` top edge; it falls to nothing over 250 ms ([`hit_fill`]). A cut drops it at
//!   once.
//! - **Selected** a 2 px `signal` border; a **peer** of the selection (same choke group, this
//!   bank) a 1 px dashed `text.muted` outline inset 3. **Muted** or **solo-excluded** pads draw at
//!   40 %. **Empty** is a dashed `line.strong` border, transparent, "+ Drop" in `text.faint`
//!   (the board's empty pad keeps its dashed border when selected).
//!
//! The kit stores nothing: the caller passes the state and the ages of the last hit and cut, read
//! from its own clock (a test steps it at a fixed dt), and gets the response back.

use super::r4;
use crate::theme::TOKENS;
use crate::tokens::{Family, TypeRole};
use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// The pad's spec (`pad` in tokens.json).
pub const GRID: f32 = 56.0;
pub const FOCUS: f32 = 80.0;
pub const GAP: f32 = 4.0;
pub const RADIUS: f32 = 6.0;
pub const HIT_ALPHA: f32 = 0.26;
pub const HIT_EDGE: f32 = 2.0;
pub const HIT_DECAY_MS: f32 = 250.0;
pub const SELECTED_WIDTH: f32 = 2.0;
pub const DIM: f32 = 0.4;
pub const TAG_HEIGHT: f32 = 3.0;
pub const BADGE_MIN_W: f32 = 16.0;
pub const BADGE_H: f32 = 14.0;
pub const BADGE_RADIUS: f32 = 3.0;
pub const CHOKED_MS: f32 = 200.0;
pub const PEER_INSET: f32 = 3.0;
pub const PEER_RADIUS: f32 = 4.0;
pub const CHIP: f32 = 14.0;
pub const CHIP_RADIUS: f32 = 3.0;
/// The bank button's dot when another bank holds a peer of the selection.
pub const BANK_DOT: f32 = 5.0;
/// The empty pad's label.
pub const DROP: &str = "+ Drop";

/// The two pad sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Size {
    /// 56: a grid in a panel.
    Grid,
    /// 80: focused, an editor.
    Focus,
}

/// The insets and type of one size, from the board.
struct Metrics {
    side: f32,
    /// The id's left and top inset.
    id: [f32; 2],
    id_size: f32,
    /// The badge's right and top inset.
    badge: [f32; 2],
    /// The name's left, right and bottom inset.
    name: [f32; 3],
    name_size: f32,
    /// The chip's right and bottom inset, and its letter's size.
    chip: [f32; 2],
    chip_letter: f32,
    drop_size: f32,
}

impl Size {
    pub fn side(self) -> f32 {
        self.metrics().side
    }

    fn metrics(self) -> Metrics {
        match self {
            Size::Grid => Metrics {
                side: GRID,
                id: [5.0, 5.0],
                id_size: 9.0,
                badge: [4.0, 4.0],
                name: [5.0, 18.0, 4.0],
                name_size: 10.0,
                chip: [4.0, 4.0],
                chip_letter: 8.0,
                drop_size: 10.0,
            },
            Size::Focus => Metrics {
                side: FOCUS,
                id: [6.0, 7.0],
                id_size: 10.0,
                badge: [5.0, 6.0],
                name: [6.0, 24.0, 6.0],
                name_size: 11.0,
                chip: [5.0, 5.0],
                chip_letter: 9.0,
                drop_size: 11.0,
            },
        }
    }
}

const fn face(family: Family, weight: u16, size: f32) -> TypeRole {
    TypeRole { family, weight, size, tracking: 0.0, uppercase: false, ink: None }
}

/// What one pad shows.
#[derive(Debug, Clone, Copy)]
pub struct Pad<'a> {
    pub size: Size,
    /// Bank and slot (`A6`).
    pub id: &'a str,
    /// The sample name (ignored when empty).
    pub name: &'a str,
    pub empty: bool,
    /// 0 Off, 1..=16.
    pub choke: u8,
    /// The group's palette bar color, once the user picked one.
    pub tag: Option<Color32>,
    pub mute: bool,
    pub solo: bool,
    /// Another pad is solo and this one is not.
    pub excluded: bool,
    pub selected: bool,
    /// A peer of the selected pad (its choke group, this bank).
    pub peer: bool,
    /// The hit fill's height, 0..1 ([`hit_fill`]).
    pub fill: f32,
    /// Cut by its group within the last 200 ms ([`choked`]).
    pub choked: bool,
}

/// The hit fill's height (0..1) `age_ms` after a trigger at `velocity`: the velocity's share of
/// the pad, falling linearly to nothing over [`HIT_DECAY_MS`].
pub fn hit_fill(velocity: u8, age_ms: f32) -> f32 {
    if !(age_ms >= 0.0) {
        return 0.0;
    }
    f32::from(velocity.min(127)) / 127.0 * (1.0 - age_ms / HIT_DECAY_MS).clamp(0.0, 1.0)
}

/// Whether a pad cut `age_ms` ago still shows its inverted badge.
pub fn choked(age_ms: f32) -> bool {
    (0.0..CHOKED_MS).contains(&age_ms)
}

/// The fill a pad shows from its last hit and its last cut (ages in ms; `None`: none yet): a cut
/// at or after the hit drops the fill at once.
pub fn live_fill(hit: Option<(u8, f32)>, cut_age: Option<f32>) -> f32 {
    let Some((velocity, age)) = hit else { return 0.0 };
    if cut_age.is_some_and(|c| c <= age) {
        return 0.0;
    }
    hit_fill(velocity, age)
}

/// One pad as last drawn.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PadLayout {
    pub rect: [f32; 4],
    pub size: Size,
    pub id: String,
    pub id_rect: [f32; 4],
    /// The name as given and the room it is drawn in (cut with an ellipsis to that width).
    pub name: String,
    pub name_rect: [f32; 4],
    /// The choke badge (`None` when Off) and its number.
    pub badge: Option<[f32; 4]>,
    pub badge_text: String,
    /// The top-edge tag (`None` without a color).
    pub tag: Option<[f32; 4]>,
    /// The chip and its letter (`M` or `S`).
    pub chip: Option<[f32; 4]>,
    pub chip_letter: Option<char>,
    /// The hit fill (`None` at rest).
    pub hit: Option<[f32; 4]>,
    /// The peer outline (`None` unless a peer).
    pub peer: Option<[f32; 4]>,
    /// The "+ Drop" label of an empty pad.
    pub drop_label: Option<[f32; 4]>,
    pub empty: bool,
    pub selected: bool,
    pub choked: bool,
    /// 1, or 0.4 muted or solo-excluded.
    pub opacity: f32,
    pub border_width: f32,
    pub dashed: bool,
    #[serde(skip)]
    pub border: Color32,
    #[serde(skip)]
    pub fill: Color32,
    #[serde(skip)]
    pub badge_fill: Color32,
    #[serde(skip)]
    pub badge_ink: Color32,
    #[serde(skip)]
    pub tag_color: Color32,
    #[serde(skip)]
    pub chip_fill: Color32,
    #[serde(skip)]
    pub chip_ink: Color32,
    #[serde(skip)]
    pub hit_color: Color32,
}

pub struct PadOut {
    /// Click and drag sense over the pad (the caller selects, auditions, starts a swap).
    pub response: egui::Response,
    pub layout: PadLayout,
}

/// Draws `pad` with its top left at `at`, its interaction under `id`.
pub fn show(ui: &mut Ui, id: egui::Id, at: egui::Pos2, pad: &Pad) -> PadOut {
    let rect = Rect::from_min_size(at, vec2(pad.size.side(), pad.size.side()));
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let layout = paint(ui.painter(), rect, pad);
    PadOut { response, layout }
}

/// A dashed 1 px outline around `r` in `color` (the kit's dashed edge: the mixer's add rows).
fn dashed(p: &egui::Painter, r: Rect, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let r = r.shrink(0.5);
    for seg in [[r.left_top(), r.right_top()], [r.right_top(), r.right_bottom()], [r.right_bottom(), r.left_bottom()], [r.left_bottom(), r.left_top()]] {
        p.extend(egui::Shape::dashed_line(&seg, stroke, 3.0, 2.0));
    }
}

/// Paints `pad` in `rect` without taking input.
pub fn paint(painter: &egui::Painter, rect: Rect, pad: &Pad) -> PadLayout {
    let t = &TOKENS;
    let m = pad.size.metrics();
    let dim = pad.mute || (pad.excluded && !pad.empty);
    let opacity = if dim { DIM } else { 1.0 };
    let mut p = painter.clone();
    p.multiply_opacity(opacity);
    let (fill, border, border_width, is_dashed) = if pad.empty {
        (Color32::TRANSPARENT, t.line_strong, 1.0, true)
    } else if pad.selected {
        (t.surface_raised, t.signal, SELECTED_WIDTH, false)
    } else {
        (t.surface_raised, t.line, 1.0, false)
    };
    p.rect_filled(rect, RADIUS, fill);
    if is_dashed {
        dashed(&p, rect, border);
    }
    let inner = rect.shrink(border_width);
    let r = RADIUS as u8;
    // The hit fill: from the bottom, inside the edge.
    let fill_h = pad.fill.clamp(0.0, 1.0) * inner.height();
    let hit_color = t.signal.gamma_multiply(HIT_ALPHA);
    let hit = (!pad.empty && fill_h > 0.0).then(|| {
        let h = Rect::from_min_max(pos2(inner.min.x, inner.max.y - fill_h), inner.max);
        p.rect_filled(h, CornerRadius { nw: 0, ne: 0, sw: r, se: r }, hit_color);
        let edge = Rect::from_min_size(h.min, vec2(h.width(), HIT_EDGE.min(fill_h)));
        p.rect_filled(edge, 0.0, t.signal);
        r4(h)
    });
    let tag_color = pad.tag.unwrap_or(Color32::TRANSPARENT);
    let tag = (!pad.empty && pad.choke != 0).then_some(pad.tag).flatten().map(|c| {
        let g = Rect::from_min_size(inner.min, vec2(inner.width(), TAG_HEIGHT));
        p.rect_filled(g, CornerRadius { nw: r, ne: r, sw: 0, se: 0 }, c);
        r4(g)
    });
    let peer = (pad.peer && !pad.empty).then(|| {
        let o = rect.shrink(PEER_INSET);
        dashed(&p, o, t.text_muted);
        r4(o)
    });
    if !is_dashed {
        p.rect_stroke(rect, RADIUS, Stroke::new(border_width, border), StrokeKind::Inside);
    }
    let id_rect = super::role_text(&p, &face(Family::Mono, 400, m.id_size), pad.id, t.text_faint, Align2::LEFT_TOP, rect.min + vec2(m.id[0], m.id[1]));
    let (badge_fill, badge_line, badge_ink) =
        if pad.choked { (t.text_muted, t.text_muted, t.surface_panel) } else { (Color32::TRANSPARENT, t.line_strong, t.text_muted) };
    let badge_text = if pad.choke != 0 { pad.choke.to_string() } else { String::new() };
    let badge = (!pad.empty && pad.choke != 0).then(|| {
        let role = face(Family::Mono, 500, 9.0);
        let w = (super::role_size(&p, &role, &badge_text).x + 2.0 * 3.0).max(BADGE_MIN_W);
        let b = Rect::from_min_size(pos2(rect.max.x - m.badge[0] - w, rect.min.y + m.badge[1]), vec2(w, BADGE_H));
        p.rect(b, BADGE_RADIUS, badge_fill, Stroke::new(1.0, badge_line), StrokeKind::Inside);
        super::role_text(&p, &role, &badge_text, badge_ink, Align2::CENTER_CENTER, b.center());
        r4(b)
    });
    let drop_label = pad.empty.then(|| r4(super::role_text(&p, &face(Family::Sans, 400, m.drop_size), DROP, t.text_faint, Align2::CENTER_CENTER, rect.center())));
    let name_rect = Rect::from_min_max(pos2(rect.min.x + m.name[0], rect.max.y - m.name[2] - m.name_size * 1.4), pos2(rect.max.x - m.name[1], rect.max.y - m.name[2]));
    if !pad.empty && !pad.name.is_empty() {
        let role = face(Family::Sans, 500, m.name_size);
        let mut job = role.job(p.ctx(), pad.name, t.text);
        job.wrap.max_width = name_rect.width();
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = p.layout_job(job);
        p.galley(pos2(name_rect.min.x, name_rect.max.y - galley.size().y), galley, t.text);
    }
    let chip_letter = (!pad.empty && (pad.mute || pad.solo)).then_some(if pad.solo { 'S' } else { 'M' });
    let (chip_fill, chip_ink) = if pad.solo { (t.signal, t.surface_inset) } else { (t.text, t.bg_window) };
    let chip = chip_letter.map(|letter| {
        let c = Rect::from_min_size(pos2(rect.max.x - m.chip[0] - CHIP, rect.max.y - m.chip[1] - CHIP), vec2(CHIP, CHIP));
        p.rect_filled(c, CHIP_RADIUS, chip_fill);
        super::role_text(&p, &face(Family::Sans, 600, m.chip_letter), &letter.to_string(), chip_ink, Align2::CENTER_CENTER, c.center());
        r4(c)
    });
    PadLayout {
        rect: r4(rect),
        size: pad.size,
        id: pad.id.to_string(),
        id_rect: r4(id_rect),
        name: if pad.empty { String::new() } else { pad.name.to_string() },
        name_rect: r4(name_rect),
        badge,
        badge_text,
        tag,
        chip,
        chip_letter,
        hit,
        peer,
        drop_label,
        empty: pad.empty,
        selected: pad.selected,
        choked: pad.choked && badge.is_some(),
        opacity,
        border_width,
        dashed: is_dashed,
        border,
        fill,
        badge_fill,
        badge_ink,
        tag_color,
        chip_fill,
        chip_ink,
        hit_color,
    }
}
