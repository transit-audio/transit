#![allow(clippy::unwrap_used)]

use super::*;

fn frame(app: &mut StudioApp, ctx: &egui::Context, events: Vec<egui::Event>) -> Vec<String> {
    let raw =
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        // The same settings widget as the window, with keyboard focus for deterministic input.
        let response = automation(app, ui);
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("test_ramp_rect"), response.rect));
        response.request_focus();
    });
    out.textures_delta.clear();
    fn collect(shape: &egui::Shape, text: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => text.push(t.galley.text().to_owned()),
            egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, text)),
            _ => {}
        }
    }
    let mut text = Vec::new();
    for s in out.shapes {
        collect(&s.shape, &mut text);
    }
    text
}

#[test]
fn the_settings_control_shows_ms_and_edits_the_saved_session() {
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, crate::Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    let text = frame(&mut app, &ctx, vec![]);
    // DragValue paints its numeric value and suffix as adjacent, separate text shapes.
    let displayed = text.join("");
    for want in ["Automation", "Touch return ramp", "200 ms"] {
        assert!(displayed.contains(want), "missing {want}: {text:?}");
    }
    frame(
        &mut app,
        &ctx,
        vec![egui::Event::Key { key: egui::Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }],
    );
    let ms = app.engine.session().touch_return_ms;
    assert!(ms > 200 && ms <= 2000, "keyboard input changed the real settings control: {ms}");
    let loaded = Session::from_json(&app.engine.session().to_json().unwrap()).unwrap();
    assert_eq!(loaded.touch_return_ms, ms);
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().touch_return_ms, 200);
}

#[test]
fn session_setup_refuses_invalid_ramp_values_without_changing_the_document() {
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, crate::Services::default());
    for bad in [json!(-1), json!(2001), json!(1.5), json!("50")] {
        assert!(app.run("setup.session", json!({"touch_return_ms": bad, "name": "must not change"})).is_err());
        assert_eq!(app.engine.session().touch_return_ms, 200);
        assert_ne!(app.engine.session().name, "must not change");
    }
}

#[test]
fn dragging_the_ramp_is_one_undo_step() {
    let mut app = StudioApp::new(transit_engine::Engine::default(), None, crate::Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx, vec![]);
    frame(&mut app, &ctx, vec![]);
    let rect = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("test_ramp_rect")).unwrap());
    let at = |x| egui::pos2(rect.left() + x, rect.center().y);
    let pointer = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at(20.0)), pointer(at(20.0), true)]);
    frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at(50.0))]);
    frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at(80.0))]);
    frame(&mut app, &ctx, vec![pointer(at(80.0), false)]);
    assert!(app.engine.session().touch_return_ms > 200);
    assert_eq!(app.engine.undo_history().len(), 1);
    app.run("edit.undo", json!({})).unwrap();
    assert_eq!(app.engine.session().touch_return_ms, 200);
}
