//! The diagnostics card of the Audio Devices window: the open devices, the backend figures and the
//! notes, with the Copy diagnostics button in its top-right corner.

use super::*;

type Controls = Vec<(String, &'static str, egui::Rect)>;

/// Draws the card's contents top to bottom at `ui`'s cursor, inside its width. The button is drawn first, in the
/// top-right corner level with the first line (panel actions sit at the top right, docs/design/language); a row that
/// starts above the button's bottom edge is laid out left of it, so no line runs under it. Returns whether the
/// button was pressed. With `pinned` the button is not drawn here (the scrolling card keeps it outside its lines, so
/// it never scrolls away) and every row has the whole width.
fn contents(ui: &mut egui::Ui, app: &mut StudioApp, (opened, audio_on): (&[String], bool), controls: &mut Controls, pinned: bool) -> bool {
    let top = ui.cursor().min;
    let strip = egui::Rect::from_min_max(top, egui::pos2(ui.max_rect().right(), f32::INFINITY));
    let mut corner =
        ui.new_child(egui::UiBuilder::new().id_salt("copy_diagnostics_corner").max_rect(strip).layout(egui::Layout::top_down(egui::Align::Max)));
    let (mut clicked, mut covered_to, mut left_of) = (false, f32::NEG_INFINITY, 0.0);
    if !pinned {
        let (rect, pressed) = app.diagnostics_button_right(&mut corner, crate::help::Id::AudioDevicesCopyDiagnostics);
        controls.push(("Copy diagnostics".into(), "copy_diagnostics", rect));
        (clicked, covered_to, left_of) = (pressed, corner.min_rect().bottom(), corner.min_rect().left() - ui.spacing().item_spacing.x);
    }
    // Every row is a scope, whose width is the room left of the button while the row starts beside it.
    let beside = |ui: &mut egui::Ui, controls: &mut Controls, add: &mut dyn FnMut(&mut egui::Ui)| {
        let room = if ui.cursor().top() < covered_to { (left_of - ui.cursor().left()).max(0.0) } else { ui.available_width() };
        let shown = ui.scope(|ui| {
            ui.set_max_width(room);
            ui.set_width(room);
            add(ui)
        });
        // Only the part of a line that shows is recorded: a line scrolled out of the card is not a control there.
        let visible = shown.response.rect.intersect(ui.clip_rect());
        if visible.is_positive() {
            controls.push(("Diagnostics line".into(), "diag_text", visible));
        }
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

/// What sits under the card in the window's column and is not placed yet when the card is: the section list, the
/// separator above the body, and the recording refusal row when it is shown (`refusal` is its height from
/// [`refusal_height`], 0 when it is not). The sections are laid out after the card (above it in the bottom-up
/// column), so their measured size is not known in this frame; the estimate is from the spacing, and the tests check
/// it is at least what the section list and the refusal measure.
pub(crate) fn reserved_below(spacing: &egui::style::Spacing, refusal: f32) -> f32 {
    Section::ALL.len() as f32 * (crate::tokens::chrome::CONTROL_HEIGHT + spacing.item_spacing.y) + 2.0 * spacing.item_spacing.y + 1.0 + refusal
}

/// The height the refusal row takes in the column, with the gap above it: its text wrapped at `width` (the row does
/// not wrap today, so this is an upper bound that also holds if it ever does), never less than a control row.
pub(crate) fn refusal_height(ui: &egui::Ui, text: &str, width: f32) -> f32 {
    let size = 11.5;
    let text_width = (width - crate::chrome::glyph_size(size) - 3.0).max(1.0);
    let galley = egui::WidgetText::from(egui::RichText::new(text).size(size)).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        text_width,
        egui::TextStyle::Body,
    );
    galley.size().y.max(ui.spacing().interact_size.y) + ui.spacing().item_spacing.y
}

/// The card in the window's bottom-up column. Its height is not known until its lines are laid out, and the button
/// must sit at its top, so the contents are laid out once in an invisible ui to measure them, then for real in a
/// rect of that height. The card takes the room left above the button row, less [`reserved_below`]: when its lines
/// are taller than that, they scroll under a pinned Copy diagnostics button inside the card and nothing is drawn
/// outside it (#732).
pub(super) fn show(ui: &mut egui::Ui, app: &mut StudioApp, state: (&[String], bool), refusal: Option<&str>, controls: &mut Controls) -> bool {
    let frame =
        egui::Frame::NONE.fill(TOKENS.bg).corner_radius(egui::CornerRadius::same(crate::theme::RADIUS as u8)).inner_margin(egui::Margin::same(8));
    let below = reserved_below(ui.spacing(), refusal.map_or(0.0, |text| refusal_height(ui, text, ui.available_width())));
    let room = (ui.available_height() - below - frame.total_margin().sum().y).max(crate::tokens::chrome::CONTROL_HEIGHT);
    frame
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let kept = controls.len();
            // The probe must lay out top down, as the card does: a child ui inherits its parent's layout, and the
            // parent here is the window's bottom-up column, where the cursor starts at the bottom of the rect and runs
            // up (its top is -inf). No row then counts as beside the button, so the probe comes out shorter than the
            // card and the lines run out of it over the button row (#732).
            let probe = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(ui.max_rect().width(), 10_000.0));
            let mut scratch = ui.new_child(
                egui::UiBuilder::new().id_salt("diagnostics_card_probe").max_rect(probe).layout(egui::Layout::top_down(egui::Align::Min)).invisible(),
            );
            contents(&mut scratch, app, state, controls, false);
            controls.truncate(kept);
            let wanted = scratch.min_rect().height().max(crate::tokens::chrome::CONTROL_HEIGHT);
            let height = wanted.min(room);
            ui.allocate_ui_with_layout(egui::vec2(ui.max_rect().width(), height), egui::Layout::top_down(egui::Align::Min), |ui| {
                controls.push(("Diagnostics card".into(), "diag_card", ui.max_rect()));
                if wanted <= room {
                    return contents(ui, app, state, controls, false);
                }
                // The button row stays at the card's top; the lines scroll under it. A scroll area takes its parent's
                // layout: the contents are a column.
                let (rect, clicked) = app.diagnostics_button_right(ui, crate::help::Id::AudioDevicesCopyDiagnostics);
                controls.push(("Copy diagnostics".into(), "copy_diagnostics", rect));
                let left = (height - rect.height() - ui.spacing().item_spacing.y).max(0.0);
                egui::ScrollArea::vertical()
                    .id_salt("diagnostics_card_scroll")
                    .max_height(left)
                    .auto_shrink([false, false])
                    .show(ui, |ui| ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| contents(ui, app, state, controls, true)).inner);
                clicked
            })
            .inner
        })
        .inner
}
