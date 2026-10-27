use super::*;

/// A small text button inside a strip; returns its response.
pub(super) fn small_button(ui: &mut Ui, clip: Rect, r: Rect, id: egui::Id, label: &str, on: bool, color: Color32, enabled: bool) -> egui::Response {
    let resp = ui.interact(r.intersect(clip), id, Sense::click());
    let p = ui.painter().with_clip_rect(clip);
    let fill = if on {
        color
    } else if resp.hovered() && enabled {
        TOKENS.button_hover
    } else {
        TOKENS.button
    };
    p.rect_filled(r, crate::theme::RADIUS_SM, fill);
    let fg = if on {
        theme::text_on(color)
    } else if enabled {
        TOKENS.text
    } else {
        TOKENS.text_dim.gamma_multiply(0.5)
    };
    p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), fg);
    resp
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
) -> Option<String> {
    let mut resp = ui.interact(r.intersect(clip), id, Sense::click());
    if let Some(tip) = tip {
        resp = resp.on_hover_text(tip);
    }
    let p = ui.painter().with_clip_rect(clip);
    p.rect_filled(r, crate::theme::RADIUS_SM, if resp.hovered() { TOKENS.button_hover } else { TOKENS.bg });
    let mut text_r = r.shrink2(vec2(4.0, 0.0));
    if tip.is_some() {
        // A tip marks a route that is not on this device: the warning icon leads the text.
        crate::icons::paint(&p, pos2(text_r.min.x + 5.0, r.center().y), crate::icons::Icon::Warning, 10.0, TOKENS.record);
        text_r.min.x += 12.0;
    }
    p.with_clip_rect(text_r.intersect(clip)).text(text_r.left_center(), Align2::LEFT_CENTER, current, FontId::proportional(10.5), TOKENS.text);
    crate::icons::paint(&p, pos2(r.max.x - 7.0, r.center().y), crate::icons::Icon::ChevronDown, 10.0, TOKENS.text_dim);
    choice_menu(&resp, current, choices, menu)
}

/// The menu of a dropdown-like field: routing entries and separators, the `current` label marked.
/// A [`NEW_FX`] entry comes after a separator. Returns the value picked.
pub(super) fn choice_menu(resp: &egui::Response, current: &str, choices: &[RoutingChoice], menu: &mut Vec<MenuItem>) -> Option<String> {
    let mut picked = None;
    egui::Popup::menu(resp).width(150.0).show(|ui| {
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
