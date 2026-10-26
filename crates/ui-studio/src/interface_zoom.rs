//! Per-machine interface zoom. Native plug-in editors keep the OS display scale.

use crate::{StudioApp, UiState};

fn clamp(value: f32) -> f32 {
    if value.is_finite() { value.clamp(0.75, 3.0) } else { 1.0 }
}

pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    <f32 as serde::Deserialize>::deserialize(deserializer).map(clamp)
}

impl UiState {
    pub(crate) fn apply_interface_zoom(&mut self, ctx: &egui::Context) {
        self.ui_zoom = clamp(self.ui_zoom);
        // The keymap owns these keys; egui otherwise consumes them at the end of a pass.
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        // egui multiplies this by native_pixels_per_point, including after a monitor change.
        ctx.set_zoom_factor(self.ui_zoom);
    }
}

impl StudioApp {
    pub(crate) fn set_interface_zoom(&mut self, ctx: &egui::Context, value: f32) {
        let value = clamp(value);
        if self.ui.ui_zoom != value {
            self.ui.ui_zoom = value;
            // Apply once in the next logic frame. egui queues zoom changes; setting it
            // twice in one pass cannot cancel a pending change back to the current factor.
            ctx.request_repaint();
            self.status = format!("Interface zoom {:.0}%", value * 100.0);
        }
    }
}
