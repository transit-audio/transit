//! The diagnostics card of the Audio Devices window: the open devices, the backend figures and the
//! notes, with the Copy diagnostics button in its top-right corner.

use super::*;

type Controls = Vec<(String, &'static str, egui::Rect)>;

/// Draws the card's contents top to bottom at `ui`'s cursor, inside its width. The button is drawn first, in the
/// top-right corner level with the first line (panel actions sit at the top right, docs/design/language); a row that
/// starts above the button's bottom edge is laid out left of it, so no line runs under it. Returns whether the
/// button was pressed.
fn contents(ui: &mut egui::Ui, app: &mut StudioApp, (opened, audio_on): (&[String], bool), controls: &mut Controls) -> bool {
    let top = ui.cursor().min;
    let strip = egui::Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), f32::INFINITY));
    let mut corner =
        ui.new_child(egui::UiBuilder::new().id_salt("copy_diagnostics_corner").max_rect(strip).layout(egui::Layout::top_down(egui::Align::Max)));
    let (rect, clicked) = app.diagnostics_button_right(&mut corner, crate::help::Id::AudioDevicesCopyDiagnostics);
    controls.push(("Copy diagnostics".into(), "copy_diagnostics", rect));
    let (covered_to, left_of) = (corner.min_rect().bottom(), corner.min_rect().left() - ui.spacing().item_spacing.x);
    // Every row is a scope, whose width is the room left of the button while the row starts beside it.
    let beside = |ui: &mut egui::Ui, controls: &mut Controls, add: &mut dyn FnMut(&mut egui::Ui)| {
        let room = if ui.cursor().top() < covered_to { (left_of - ui.cursor().left()).max(0.0) } else { ui.available_width() };
        let shown = ui.scope(|ui| {
            ui.set_max_width(room);
            ui.set_width(room);
            add(ui)
        });
        controls.push(("Diagnostics line".into(), "diag_text", shown.response.rect));
    };
    // The card has always read: notes, the open lines (the last first), then the hover hint.
    for n in app.audio_devices.notes.clone().iter().rev() {
        beside(ui, controls, &mut |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(icons::ICON, icons::ICON), egui::Sense::hover());
                icons::paint(ui.painter(), r.center(), Icon::Error, 14.0, TOKENS.record);
                ui.add(egui::Label::new(egui::RichText::new(n).color(TOKENS.text).size(11.5)).wrap());
            });
        });
    }
    if !audio_on {
        let note =
            if app.null_audio { NULL_AUDIO_NOTE } else { "Audio is off in this run (--no-audio): choices are saved and open at the next start." };
        beside(ui, controls, &mut |ui| {
            ui.label(egui::RichText::new(note).color(TOKENS.text_dim).size(11.5));
        });
    }
    for line in opened.iter().rev() {
        beside(ui, controls, &mut |ui| crate::help::status_line(ui, line, line.starts_with("Backend")));
    }
    beside(ui, controls, &mut |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Hover a figure for what it means.").color(TOKENS.text_dim).size(11.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                crate::help::header_button(app, ui, crate::help::Id::AudioDevicesWindow)
            });
        });
    });
    clicked
}

/// The card in the window's bottom-up column. Its height is not known until its lines are laid out, and the button
/// must sit at its top, so the contents are laid out once in an invisible ui to measure them, then for real in a
/// rect of that height. The card takes the room left above the button row, less what the section list below it needs:
/// when its lines are taller than that, they scroll inside the card and nothing is drawn outside it (#732).
pub(super) fn show(ui: &mut egui::Ui, app: &mut StudioApp, state: (&[String], bool), controls: &mut Controls) -> bool {
    let margin = 8;
    // The section list, the separator and one status row stay clear under the card.
    let spacing = ui.spacing();
    let below =
        Section::ALL.len() as f32 * (spacing.interact_size.y + spacing.item_spacing.y) + 2.0 * spacing.item_spacing.y + 1.0 + spacing.interact_size.y;
    let room = (ui.available_height() - below - 2.0 * margin as f32).max(crate::tokens::chrome::CONTROL_HEIGHT);
    egui::Frame::NONE
        .fill(TOKENS.bg)
        .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS as u8))
        .inner_margin(egui::Margin::same(margin))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let kept = controls.len();
            let probe = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), 10_000.0));
            let mut scratch = ui.new_child(egui::UiBuilder::new().id_salt("diagnostics_card_probe").max_rect(probe).invisible());
            contents(&mut scratch, app, state, controls);
            controls.truncate(kept);
            let wanted = scratch.min_rect().height().max(crate::tokens::chrome::CONTROL_HEIGHT);
            let height = wanted.min(room);
            ui.allocate_ui_with_layout(egui::vec2(ui.max_rect().width(), height), egui::Layout::top_down(egui::Align::Min), |ui| {
                controls.push(("Diagnostics card".into(), "diag_card", ui.max_rect()));
                if wanted <= room {
                    return contents(ui, app, state, controls);
                }
                // A scroll area takes its parent's layout: the contents are a column.
                egui::ScrollArea::vertical()
                    .id_salt("diagnostics_card_scroll")
                    .max_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| contents(ui, app, state, controls)).inner)
                    .inner
            })
            .inner
        })
        .inner
}
