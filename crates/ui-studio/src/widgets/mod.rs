//! Small reusable controls drawn with the theme tokens.
//!
//! The design language's kit (ADR 0045, `docs/design/language`): each control takes its values,
//! draws them in the tokens of the theme being drawn, and returns its response plus a layout
//! struct (rects, the tokens it painted with, its text) for deterministic layout tests. None of
//! them stores anything: the caller owns the values, the view state (collapse) and the patch
//! state (enable).

pub mod bipolar_amount;
pub mod display_well;
pub mod fader;
pub mod hatch;
pub mod knob;
pub mod level_meter;
pub mod param_knob;
pub mod section_bar;
pub mod segmented;
pub mod switch;
pub mod toggle;

#[cfg(test)]
mod bipolar_amount_tests;
#[cfg(test)]
mod demo_tests;
#[cfg(test)]
mod display_well_tests;
#[cfg(test)]
mod fader_tests;
#[cfg(test)]
mod hatch_tests;
#[cfg(test)]
mod knob_tests;
#[cfg(test)]
mod level_meter_tests;
#[cfg(test)]
mod section_bar_tests;
#[cfg(test)]
mod segmented_tests;
#[cfg(test)]
mod switch_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod toggle_tests;

/// A rect as `[min x, min y, max x, max y]`, rounded to 0.01 (what the layout structs carry).
pub fn r4(r: egui::Rect) -> [f32; 4] {
    let q = |v: f32| (v * 100.0).round() / 100.0;
    [q(r.min.x), q(r.min.y), q(r.max.x), q(r.max.y)]
}

/// Paints `text` in a type role centered in `rect` (uppercased and tracked as the role says) and
/// returns the rect the text occupies.
pub fn role_text(
    painter: &egui::Painter,
    role: &crate::tokens::TypeRole,
    text: &str,
    color: egui::Color32,
    anchor: egui::Align2,
    at: egui::Pos2,
) -> egui::Rect {
    let galley = painter.layout_job(role.job(painter.ctx(), text, color));
    let rect = anchor.anchor_size(at, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// The size a role's text takes.
pub fn role_size(painter: &egui::Painter, role: &crate::tokens::TypeRole, text: &str) -> egui::Vec2 {
    painter.layout_job(role.job(painter.ctx(), text, egui::Color32::WHITE)).size()
}

/// Wheel notches this frame (up positive; one notch is a line, or 40 points of smooth scroll),
/// from the raw wheel events so Shift, which egui turns into horizontal scrolling, still counts.
pub fn wheel_notches(ui: &egui::Ui) -> f32 {
    ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => {
                    let d = delta.y + delta.x;
                    match unit {
                        egui::MouseWheelUnit::Point => d / 40.0,
                        egui::MouseWheelUnit::Line => d,
                        egui::MouseWheelUnit::Page => d * 4.0,
                    }
                }
                _ => 0.0,
            })
            .sum::<f32>()
            .clamp(-4.0, 4.0)
    })
}
