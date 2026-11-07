use super::*;
use crate::tokens;

/// The strip's dropdown face (Dropdown): `surface.inset`, 1 px `line` (`line.strong` while hot or
/// open), radius 5, the value in Sans 500 11 cut with an ellipsis, the chevron in `text.faint`
/// (`text.muted` while hot). The output dropdown (`arrow`) leads with a routing arrow in
/// `ink.arrow`; a route that is not on this device also leads with the error glyph.
pub(super) fn paint_dropdown(p: &egui::Painter, r: Rect, current: &str, hot: bool, tip: Option<&str>, arrow: bool) {
    p.rect_filled(r, tokens::radius::CONTROL, TOKENS.surface_inset);
    p.rect_stroke(r, tokens::radius::CONTROL, Stroke::new(1.0, if hot { TOKENS.line_strong } else { TOKENS.line }), egui::StrokeKind::Inside);
    let mut text_r = Rect::from_min_max(pos2(r.min.x + 6.0, r.min.y), pos2(r.max.x - 6.0 - 9.0 - 4.0, r.max.y));
    if arrow {
        let a = arrow_rect(r);
        crate::icons::paint(p, a.center(), crate::icons::Icon::ArrowRight, ARROW_SIZE, TOKENS.ink_arrow);
        text_r.min.x = a.max.x + 4.0;
    }
    if tip.is_some() {
        crate::icons::paint(p, pos2(text_r.min.x + 5.0, r.center().y), crate::icons::Icon::Error, 10.0, TOKENS.meter_hot);
        text_r.min.x += 14.0;
    }
    super::channel::text_in(p, &super::channel::CONTROL, current, TOKENS.text, text_r, Align2::LEFT_CENTER);
    crate::icons::paint(
        p,
        pos2(r.max.x - 6.0 - 4.5, r.center().y),
        crate::icons::Icon::ChevronDown,
        9.0,
        if hot { TOKENS.text_muted } else { TOKENS.text_faint },
    );
}

/// The size of the output dropdown's routing arrow (the board gives none: the error glyph's).
pub(super) const ARROW_SIZE: f32 = 10.0;

/// Where the routing arrow sits in a dropdown face `r`: after the 6 px padding, centered.
pub(super) fn arrow_rect(r: Rect) -> Rect {
    Rect::from_center_size(pos2(r.min.x + 6.0 + ARROW_SIZE / 2.0, r.center().y), egui::vec2(ARROW_SIZE, ARROW_SIZE))
}

/// A dropdown field: its current value, and a menu of `choices` when clicked. Returns the value
/// picked. The open menu's entries go to `menu` for `ui.inspect`.
#[allow(clippy::too_many_arguments)]
pub(super) fn dropdown(
    ui: &mut Ui,
    clip: Rect,
    r: Rect,
    id: egui::Id,
    current: &str,
    tip: Option<&str>,
    choices: &[RoutingChoice],
    menu: &mut Vec<MenuItem>,
    arrow: bool,
) -> Option<String> {
    let mut resp = ui.interact(r.intersect(clip), id, Sense::click());
    if let Some(tip) = tip {
        resp = resp.on_hover_text(tip);
    }
    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
    paint_dropdown(&ui.painter().with_clip_rect(clip), r, current, resp.hovered() || open, tip, arrow);
    choice_menu(&resp, current, choices, menu)
}

/// The menu of a dropdown-like field: routing entries and separators, the `current` label marked.
/// A [`NEW_FX`] entry comes after a separator. Returns the value picked.
pub(super) fn choice_menu(resp: &egui::Response, current: &str, choices: &[RoutingChoice], menu: &mut Vec<MenuItem>) -> Option<String> {
    let mut picked = None;
    crate::chrome_pick::dropdown(resp).width(150.0).show(|ui| {
        if choices.is_empty() {
            ui.weak("Nothing to pick");
        }
        for choice in choices {
            let (label, value) = match choice {
                RoutingChoice::Separator => {
                    ui.separator();
                    continue;
                }
                RoutingChoice::Entry { label, value } => (label, value),
            };
            if value == NEW_FX || value == REMOVE_UNUSED {
                ui.separator();
            }
            // The current entry is marked by label, or by the path name the route holds.
            let b = crate::menus::check_button(ui, label == current || (!value.is_empty() && value == current), label.as_str());
            menu.push(MenuItem::at(ui, label.clone(), value.clone(), b.rect));
            if b.clicked() {
                picked = Some(value.clone());
                ui.close();
            }
        }
    });
    picked
}
