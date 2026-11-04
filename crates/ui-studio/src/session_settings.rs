//! Project preferences, edited through the undoable session setup command.

use crate::{StudioApp, help};
use serde_json::json;
use transit_model::Session;

fn open_id() -> egui::Id {
    egui::Id::new("session_settings_open")
}

pub(super) fn menu(ui: &mut egui::Ui) {
    if ui.button("Session Settings…").clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(open_id(), true));
        ui.close();
    }
}

pub(super) fn window(app: &mut StudioApp, ctx: &egui::Context) {
    let mut open = ctx.data(|d| d.get_temp::<bool>(open_id()).unwrap_or(false));
    if !open {
        return;
    }
    crate::theme::window(ctx, "Session Settings").open(&mut open).resizable(false).show(ctx, |ui| {
        automation(app, ui);
    });
    ctx.data_mut(|d| d.insert_temp(open_id(), open));
}

fn automation(app: &mut StudioApp, ui: &mut egui::Ui) -> egui::Response {
    ui.heading("Automation");
    let mut ms = app.engine.session().touch_return_ms;
    let response = ui.add(egui::Slider::new(&mut ms, 0..=Session::MAX_TOUCH_RETURN_MS).text("Touch return ramp").suffix(" ms"));
    help::hover(ui, &response, help::Id::AutomationTouchReturn);
    if response.drag_started() {
        app.engine.end_merge();
    }
    if response.changed() {
        if let Err(e) = app.execute_ui_merged("setup.session", &json!({"touch_return_ms": ms}), "touch_return_ms") {
            app.status = e.to_string();
        }
        if !response.dragged() {
            app.engine.end_merge();
        }
    }
    if response.drag_stopped() {
        app.engine.end_merge();
    }
    response
}

#[cfg(test)]
#[path = "session_settings_tests.rs"]
mod tests;
