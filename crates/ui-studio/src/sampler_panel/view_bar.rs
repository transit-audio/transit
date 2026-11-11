//! Shared view navigation and the Pads view's folder action. Navigation is session UI state.

use super::{SamplerLayout, view};
use crate::StudioApp;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{self, chrome, radius, sampler as tk};
use crate::widgets::{self, r4, segmented};
use egui::{Align2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    Instrument,
    #[default]
    Pads,
    Slices,
}

impl View {
    pub const ALL: [Self; 3] = [Self::Instrument, Self::Pads, Self::Slices];

    pub fn index(self) -> usize {
        match self {
            Self::Instrument => 0,
            Self::Pads => 1,
            Self::Slices => 2,
        }
    }
}

const FACE: tokens::TypeRole =
    tokens::TypeRole { size: tk::VIEW_SWITCH_FONT_SIZE, weight: tk::VIEW_SWITCH_FONT_WEIGHT as u16, ..tokens::LABEL_CONTROL };

/// The shared segmented track retains its existing padding until that design value has a token.
fn switch(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect) -> segmented::SegmentedLayout {
    let t = &TOKENS;
    let p = ui.painter().clone();
    let widths = tk::VIEW_BAR_VIEWS.map(|name| widgets::role_size(&p, &FACE, name).x + 2.0 * tk::VIEW_SWITCH_PADDING_X);
    let track = Rect::from_min_size(
        pos2(rect.min.x, rect.center().y - tk::VIEW_BAR_CONTROL_HEIGHT / 2.0),
        vec2(widths.iter().sum::<f32>() + 2.0 * segmented::PADDING, tk::VIEW_BAR_CONTROL_HEIGHT),
    );
    p.rect(track, radius::DISPLAY, t.surface_inset, Stroke::new(chrome::FIELD_EDGE_WIDTH, t.line), StrokeKind::Inside);
    let inner = track.shrink(segmented::PADDING);
    let mut x = inner.min.x;
    let mut segments = Vec::new();
    for (i, width) in widths.into_iter().enumerate() {
        let seg = Rect::from_min_size(pos2(x, inner.min.y), vec2(width, inner.height()));
        x += width;
        let response = ui.interact(seg, ui.id().with(("sampler_view", dev.uid(), i)), Sense::click());
        let selected = view(app, dev.uid()).view.index() == i;
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, tk::VIEW_BAR_VIEWS[i]));
        if response.clicked() {
            view(app, dev.uid()).view = View::ALL[i];
        }
        segments.push(seg);
    }
    let selected = view(app, dev.uid()).view.index();
    let mut inks = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let ink = if i == selected { t.text } else { t.text_faint };
        if i == selected {
            p.rect_filled(*seg, radius::SEGMENT, t.surface_selected);
        }
        widgets::role_text(&p, &FACE, tk::VIEW_BAR_VIEWS[i], ink, Align2::CENTER_CENTER, seg.center());
        inks.push(ink);
    }
    segmented::SegmentedLayout {
        track: r4(track),
        segments: segments.into_iter().map(r4).collect(),
        selected,
        inks,
        selected_fill: t.surface_selected,
    }
}

/// The Instrument bar's actions, unavailable until they are built.
pub(crate) const INSTRUMENT_ACTIONS: [&str; 2] = ["Add zone…", "Import SFZ…"];

/// The Pads bar's unavailable action and its tooltip.
pub(crate) const EXPAND_TO_RACK: &str = "Expand to rack…";
pub(crate) const EXPAND_TO_RACK_TIP: &str = "Not available yet";

/// A standard button at the dense height, centred in the bar, its right edge at `right`. An
/// unavailable one takes no input and is drawn at `sampler.unusedControlAlpha`.
/// Returns its response and the factor its own state applies to the window's opacity.
fn action_button(ui: &mut Ui, p: &egui::Painter, text: &str, right: f32, enabled: bool, bar: Rect) -> (egui::Response, f32) {
    let w = (widgets::role_size(p, &chrome::BUTTON_FONT, text).x + 2.0 * chrome::BUTTON_PADDING[1]).ceil();
    let r = Rect::from_min_size(pos2(right - w, bar.center().y - tk::VIEW_BAR_CONTROL_HEIGHT / 2.0), vec2(w, tk::VIEW_BAR_CONTROL_HEIGHT));
    ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
        if !enabled {
            ui.visuals_mut().disabled_alpha = tk::UNUSED_CONTROL_ALPHA;
            ui.disable();
        }
        ui.spacing_mut().button_padding = vec2(chrome::BUTTON_PADDING[1], chrome::BUTTON_PADDING[0]);
        // `Ui::disable` multiplies the painter's opacity by exactly this.
        let opacity = if ui.is_enabled() { 1.0 } else { ui.visuals().disabled_alpha() };
        (ui.put(r, egui::Button::new(egui::RichText::new(text).font(chrome::BUTTON_FONT.font_id(ui.ctx())))), opacity)
    })
    .inner
}

/// Draw navigation and actions; return the Start note field's slot on the Pads view.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, summary: &str, rect: Rect, out: &mut SamplerLayout) -> Option<Rect> {
    out.bar.rect = r4(rect);
    let switch = switch(app, ui, dev, rect);
    let summary_x = switch.track[2] + tk::VIEW_BAR_GAP;
    out.bar.switch = Some(switch);
    let p = ui.painter().clone();
    if view(app, dev.uid()).view == View::Slices {
        // The loop's summary; the view's action (Load loop…) is not built yet, so the slot runs to
        // the bar's end. A long file name is cut with an ellipsis at the slot's end (the names'
        // rule), never past the window.
        out.bar.summary = summary.to_string();
        let slot = Rect::from_min_max(pos2(summary_x, rect.min.y), rect.max);
        out.bar.summary_slot = r4(slot);
        let mut job = tokens::READOUT_DISPLAY.job(p.ctx(), summary, TOKENS.text_faint);
        job.wrap.max_width = slot.width();
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = p.layout_job(job);
        let at = pos2(slot.min.x, slot.center().y - galley.size().y / 2.0);
        out.bar.summary_rect = r4(Rect::from_min_size(at, galley.size()));
        p.galley(at, galley, TOKENS.text_faint);
        return None;
    }
    if view(app, dev.uid()).view == View::Instrument {
        // The map's summary, then the view's actions, shown unavailable until they are built
        // (Add zone… and Import SFZ…, #271).
        let mut right = rect.max.x;
        let mut actions = Vec::new();
        for text in INSTRUMENT_ACTIONS.iter().rev() {
            let (resp, opacity) = action_button(ui, &p, text, right, false, rect);
            let resp = resp.on_disabled_hover_text(EXPAND_TO_RACK_TIP);
            right = resp.rect.min.x - tk::VIEW_BAR_GAP;
            actions.insert(0, (r4(resp.rect), opacity));
        }
        out.bar.instrument_actions = actions;
        out.bar.summary = summary.to_string();
        let slot = Rect::from_min_max(pos2(summary_x, rect.min.y), pos2(right, rect.max.y));
        out.bar.summary_slot = r4(slot);
        out.bar.summary_rect =
            r4(widgets::role_text(&p, &tokens::READOUT_DISPLAY, summary, TOKENS.text_faint, Align2::LEFT_CENTER, slot.left_center()));
        return None;
    }
    if view(app, dev.uid()).view != View::Pads {
        return None;
    }
    // Expand to rack… is the last action, shown unavailable until its engine command exists
    // (ADR 0041 amendment 2026-11-08, Pads view bar); Load kit… sits one bar gap before it.
    let (expand, expand_opacity) = action_button(ui, &p, EXPAND_TO_RACK, rect.max.x, false, rect);
    let expand = expand.on_disabled_hover_text(EXPAND_TO_RACK_TIP);
    out.bar.expand = Some(r4(expand.rect));
    let (response, load_opacity) = action_button(ui, &p, "Load kit…", expand.rect.min.x - tk::VIEW_BAR_GAP, true, rect);
    out.bar.opacity = [load_opacity, expand_opacity];
    let action = response.rect;
    out.bar.load_kit = Some(r4(action));
    out.bar.load_enabled = true;
    if response.clicked() {
        super::kit_load::pick(app, dev);
    }
    let field = Rect::from_min_size(
        pos2(action.min.x - tk::VIEW_BAR_GAP - tk::START_NOTE_WIDTH, rect.center().y - tk::START_NOTE_HEIGHT / 2.0),
        vec2(tk::START_NOTE_WIDTH, tk::START_NOTE_HEIGHT),
    );
    let summary_right =
        field.min.x - tk::START_NOTE_LABEL_GAP - widgets::role_size(&p, &tokens::LABEL_CONTROL, tk::START_NOTE_LABEL).x - tk::VIEW_BAR_GAP;
    out.bar.summary = summary.to_string();
    let slot = Rect::from_min_max(pos2(summary_x, rect.min.y), pos2(summary_right, rect.max.y));
    out.bar.summary_slot = r4(slot);
    out.bar.summary_rect = r4(widgets::role_text(&p, &tokens::READOUT_DISPLAY, summary, TOKENS.text_faint, Align2::LEFT_CENTER, slot.left_center()));
    Some(field)
}

/// A view without its editor yet: plain centered text, using the existing control-label role.
pub(crate) fn placeholder(app: &mut StudioApp, ui: &Ui, uid: u64, rect: Rect, out: &mut SamplerLayout) {
    let name = tk::VIEW_BAR_VIEWS[view(app, uid).view.index()];
    let text = format!("{name} view is not built yet.");
    let drawn = widgets::role_text(ui.painter(), &tokens::LABEL_CONTROL, &text, TOKENS.text_muted, Align2::CENTER_CENTER, rect.center());
    out.placeholder = Some((text, r4(drawn)));
}

pub(crate) fn is_pads(app: &mut StudioApp, uid: u64) -> bool {
    view(app, uid).view == View::Pads
}
