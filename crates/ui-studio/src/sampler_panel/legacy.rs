//! A Sampler on the legacy one-sample path (ADR 0041 decision 8: an old one-sample instrument
//! keeps its legacy renderer until a structural edit converts it). Its window keeps what it had:
//! the sample line, Load Sample and Clear (`builtin_editor::sample_picker`) and the 19 legacy
//! parameter rows, in a panel card in the device window, with a New Kit button that runs
//! `mix.sampler_new_patch` (the one-sample field is left unset, ADR 0041) and switches the window
//! to the pads. A dropped audio file loads as the sample, as before. The legacy controls have no
//! board of their own and are not restyled.

use super::{LegacyLayout, SamplerLayout, behavior, section_label};
use crate::StudioApp;
use crate::builtin_editor::EditorLayout;
use crate::device_editor::Dev;
use crate::theme::TOKENS;
use crate::tokens::{layout as space, radius};
use crate::widgets::r4;
use egui::{Rect, Stroke, StrokeKind, Ui, pos2, vec2};

/// The body's height before it is first measured.
pub const ESTIMATE: f32 = 640.0;

/// Draws the legacy body from `rect`'s top left at its width; returns its height.
pub(crate) fn show(app: &mut StudioApp, ui: &mut Ui, dev: Dev, rect: Rect, lay: &mut EditorLayout, out: &mut SamplerLayout) -> f32 {
    let t = &TOKENS;
    let card_w = rect.width() - 2.0 * space::WINDOW_PADDING;
    let bar_min = rect.min + vec2(space::WINDOW_PADDING, space::WINDOW_PADDING);
    super::view_bar::legacy_bar(app, ui, dev, Rect::from_min_size(bar_min, vec2(card_w, super::bar::HEIGHT)), out);
    let card_min = bar_min + vec2(0.0, super::view_bar::HEIGHT_WITH_GAP);
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner_min = card_min + vec2(space::PANEL_PADDING[1], space::PANEL_PADDING[0]);
    let label = section_label(ui.painter(), inner_min, "Sample");
    let content = Rect::from_min_size(pos2(inner_min.x, label.max.y + 10.0), vec2(card_w - 2.0 * space::PANEL_PADDING[1], 4000.0));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::Min)));
    let mut legacy = LegacyLayout::default();
    let pads = super::view_bar::is_pads(app, dev.uid());
    if pads {
        crate::builtin_editor::sample_picker(app, &mut c, dev.key.0, dev.ins, lay);
    } else {
        let placeholder = Rect::from_min_size(c.cursor().min, vec2(content.width(), super::grid::height(ui.painter())));
        super::view_bar::placeholder(app, &c, dev.uid(), placeholder, out);
        c.allocate_rect(placeholder, egui::Sense::hover());
    }
    let kit = new_kit_notice(&mut c);
    legacy.new_kit = r4(kit.rect);
    let kit = kit.on_hover_text("Make this Sampler a kit of pads (its one sample is not carried over)");
    if kit.clicked() {
        behavior::new_kit(app, dev);
    }
    c.add_space(8.0);
    if pads {
        egui::ScrollArea::vertical().id_salt(("sampler_legacy_rows", dev.uid())).max_height(480.0).show(&mut c, |ui| {
            ui.spacing_mut().slider_width = 170.0;
            egui::Grid::new(("builtin_params", dev.uid())).num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
                for p in dev.info.params {
                    crate::builtin_editor::param_row(app, ui, dev.key, dev.ins, p, lay);
                    ui.end_row();
                }
            });
        });
    }
    let bottom = c.min_rect().max.y + space::PANEL_PADDING[0];
    let card = Rect::from_min_max(card_min, pos2(card_min.x + card_w, bottom));
    ui.painter().set(bg, egui::epaint::RectShape::new(card, radius::PANEL, t.surface_panel, Stroke::new(1.0, t.line), StrokeKind::Inside));
    out.panels.push(("sample".to_string(), r4(card)));
    legacy.panel = r4(card);
    out.legacy = Some(legacy);
    // A file dropped anywhere on the window loads as the sample (the old window's behavior): the
    // window claimed the drop (#368).
    if let Some(offer) = crate::drops::take(app, dev.key)
        && let Some(path) = offer.paths.iter().find(|p| p.is_absolute() && crate::builtin_editor::is_audio_name(&p.to_string_lossy()))
    {
        crate::builtin_editor::load_sample(app, dev.key.0, &path.to_string_lossy());
    }
    bottom + space::WINDOW_PADDING - rect.min.y
}

fn new_kit_notice(ui: &mut Ui) -> egui::Response {
    use crate::tokens::chrome as c;
    let [top, right, bottom, left] = c::NOTICE_PADDING;
    egui::Frame::new()
        .fill(TOKENS.surface_raised)
        .stroke(Stroke::new(c::FIELD_EDGE_WIDTH, TOKENS.line_strong))
        .corner_radius(c::NOTICE_RADIUS)
        .inner_margin(egui::Margin { left: left as i8, right: right as i8, top: top as i8, bottom: bottom as i8 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = crate::tokens::sampler::VIEW_BAR_GAP;
                ui.label(
                    egui::RichText::new("This one-sample patch stays unchanged until New Kit.")
                        .font(crate::tokens::LABEL_CONTROL.font_id(ui.ctx()))
                        .color(TOKENS.text),
                );
                ui.spacing_mut().button_padding = vec2(c::BUTTON_PADDING[1], c::BUTTON_PADDING[0]);
                ui.add(
                    egui::Button::new(egui::RichText::new("New Kit").font(c::BUTTON_FONT.font_id(ui.ctx())))
                        .fill(TOKENS.button_hover)
                        .min_size(vec2(0.0, crate::tokens::sampler::VIEW_BAR_CONTROL_HEIGHT)),
                )
            })
            .inner
        })
        .inner
}
