//! The collapsible section bar (`docs/design/language`, Components > Section bar), for optional
//! or secondary sections such as Shaper and Modulation. Left: a chevron and the title, one button
//! that only collapses or expands (the chevron turns -90 degrees when collapsed). Middle: an
//! optional `tag`, then a description while open or a one-line mono summary while collapsed.
//! Right: the `ON`/`OFF` readout and switch, which only enable or disable.
//!
//! Collapse and enable are independent and the kit stores neither: collapse is a per-user view
//! setting and enable a patch parameter saved with the preset, so both come in as inputs and go
//! out as clicks. A disabled but open section dims its body to [`body_opacity`] and stays
//! editable.

use super::{r4, switch};
use crate::theme::TOKENS;
use crate::tokens::{self, alpha, layout};
use egui::{Align2, Color32, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

/// The bar is a control row at the design's larger control height.
pub const HEIGHT: f32 = layout::CONTROL_HEIGHT[1];
pub const CHEVRON: f32 = 12.0;
const GAP: f32 = 8.0;
const TAG_PAD: f32 = 6.0;
const TAG_H: f32 = 18.0;

/// The inputs of one bar.
pub struct Bar<'a> {
    pub title: &'a str,
    pub tag: Option<&'a str>,
    /// Shown while open.
    pub description: Option<&'a str>,
    /// Shown while collapsed: the current settings on one mono line.
    pub summary: Option<&'a str>,
    pub collapsed: bool,
    /// The section's enable parameter; `None` for a section that is always on (no switch).
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SectionBarLayout {
    pub bar: [f32; 4],
    /// The collapse button: chevron and title.
    pub button: [f32; 4],
    pub chevron: [f32; 4],
    /// Degrees the chevron is turned: 0 open (pointing down), -90 collapsed (pointing right).
    pub chevron_deg: f32,
    pub title: [f32; 4],
    pub tag: Option<[f32; 4]>,
    /// The middle text and where it sits (description or summary).
    pub middle: Option<(String, [f32; 4])>,
    pub switch: Option<switch::SwitchLayout>,
    #[serde(skip)]
    pub title_color: Color32,
    #[serde(skip)]
    pub tag_border: Color32,
}

pub struct SectionBarOut {
    pub layout: SectionBarLayout,
    /// The user clicked the chevron or the title: the caller flips its view setting.
    pub toggle_collapse: bool,
    /// The user clicked the switch: the caller flips the enable parameter (one undo step).
    pub toggle_enable: bool,
}

/// How opaque a section's body draws: dimmed while disabled, whether open or not.
pub fn body_opacity(enabled: Option<bool>) -> f32 {
    if enabled == Some(false) { alpha::DISABLED_SECTION } else { 1.0 }
}

/// The chevron as a polyline in `r`: a downward V, turned by `deg` (clockwise) about its center.
fn chevron(r: Rect, deg: f32) -> Vec<egui::Pos2> {
    let c = r.center();
    let (s, co) = deg.to_radians().sin_cos();
    let h = r.width() * 0.3;
    [vec2(-h, -h / 2.0), vec2(0.0, h / 2.0), vec2(h, -h / 2.0)].iter().map(|v| c + vec2(v.x * co - v.y * s, v.x * s + v.y * co)).collect()
}

/// Draws a bar `width` wide.
pub fn show(ui: &mut Ui, id: egui::Id, width: f32, b: &Bar) -> SectionBarOut {
    let t = &TOKENS;
    let (bar, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
    let p = ui.painter().clone();
    let cy = bar.center().y;
    let chev = Rect::from_center_size(pos2(bar.min.x + CHEVRON / 2.0, cy), vec2(CHEVRON, CHEVRON));
    let chevron_deg = if b.collapsed { -90.0 } else { 0.0 };
    p.add(Shape::line(chevron(chev, chevron_deg), Stroke::new(1.5, t.text_muted)));
    let title = super::role_text(&p, &tokens::TITLE_SECTION_BAR, b.title, t.text, Align2::LEFT_CENTER, pos2(chev.max.x + GAP / 2.0, cy));
    let button = chev.union(title);
    let toggle_collapse = ui.interact(button, id.with("collapse"), Sense::click()).clicked();
    // The switch on the right.
    let mut toggle_enable = false;
    let mut right = bar.max.x;
    let switch_layout = b.enabled.map(|on| {
        let size = switch::size();
        let r = Rect::from_min_size(pos2(bar.max.x - size.x, cy - size.y / 2.0), size);
        right = r.min.x - GAP;
        let out = switch::paint(ui, id.with("enable"), r, on);
        toggle_enable = out.toggled;
        out.layout
    });
    // The middle: tag, then description or summary, clipped before the switch.
    let mut x = title.max.x + GAP;
    let tag = b.tag.map(|text| {
        let size = super::role_size(&p, &tokens::TAG, text);
        let r = Rect::from_min_size(pos2(x, cy - TAG_H / 2.0), vec2(size.x + 2.0 * TAG_PAD, TAG_H));
        p.rect_stroke(r, tokens::radius::SEGMENT, Stroke::new(1.0, t.line_strong), StrokeKind::Inside);
        super::role_text(&p, &tokens::TAG, text, t.text_muted, Align2::CENTER_CENTER, r.center());
        x = r.max.x + GAP;
        r4(r)
    });
    let middle = if b.collapsed { b.summary.map(|s| (s, tokens::READOUT_DISPLAY)) } else { b.description.map(|s| (s, tokens::LABEL_CONTROL)) };
    let middle = middle.filter(|_| right > x).map(|(text, role)| {
        let clip = p.with_clip_rect(Rect::from_x_y_ranges(x..=right, bar.y_range()));
        let r = super::role_text(&clip, &role, text, if b.collapsed { t.text_faint } else { t.text_muted }, Align2::LEFT_CENTER, pos2(x, cy));
        (text.to_string(), r4(r.intersect(Rect::from_x_y_ranges(x..=right, bar.y_range()))))
    });
    let layout = SectionBarLayout {
        bar: r4(bar),
        button: r4(button),
        chevron: r4(chev),
        chevron_deg,
        title: r4(title),
        tag,
        middle,
        switch: switch_layout,
        title_color: t.text,
        tag_border: t.line_strong,
    };
    SectionBarOut { layout, toggle_collapse, toggle_enable }
}
