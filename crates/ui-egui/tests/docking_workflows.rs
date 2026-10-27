//! Real Edit/Mix panel workflows, persistence, and command ownership without OS windows.
use egui::{Event, Modifiers, PointerButton, vec2};
use egui_kittest::{Harness, kittest::Queryable};
use serde_json::json;
use transit_ui_egui::{MainWindow, Services, SoundApp, UiState, docking::Panel};

fn harness(state: Option<UiState>, width: f32, scale: f32) -> Harness<'static, SoundApp> {
    let mut app = SoundApp::new(transit_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"name":"Voice"})).unwrap();
    if let Some(state) = state {
        app.ui = state;
    }
    let mut h = Harness::builder().with_size(vec2(width, 900.0)).with_pixels_per_point(scale).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn drag(h: &mut Harness<'_, SoundApp>, from: egui::Pos2, to: egui::Pos2) {
    h.event(Event::PointerMoved(from));
    h.run_steps(2);
    h.event(Event::PointerButton { pos: from, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run_steps(2);
    for step in 1..=8 {
        h.event(Event::PointerMoved(from.lerp(to, step as f32 / 8.0)));
        h.run_steps(1);
    }
    h.event(Event::PointerButton { pos: to, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(4);
}

#[test]
fn real_drag_float_close_reopen_reload_retains_transport_and_mixer_commands() {
    let mut h = harness(None, 1400.0, 1.0);
    let source = h.get_by_role_and_label(egui::accesskit::Role::Tab, "Clips").rect().center();
    let target = h.get_by_role_and_label(egui::accesskit::Role::Tab, "Tracks and Groups").rect().center();
    drag(&mut h, source, target);
    assert_eq!(h.state().ui.docking.layout.location(&Panel::Clips).unwrap().anchor, Some(Panel::Tracks));
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "Clips").click_button(PointerButton::Secondary);
    h.run_steps(3);
    h.get_by_label("Float panel").click();
    h.run_steps(5);
    let rect = h.state().ui.docking.layout.floating.iter().find(|g| g.panels.contains(&Panel::Clips)).unwrap().rect;
    h.get_by_role_and_label(egui::accesskit::Role::Tab, "Clips").click_button(PointerButton::Secondary);
    h.run_steps(3);
    h.get_by_label("Close panel").click();
    h.run_steps(4);
    assert!(!h.state().ui.show_clip_list);
    h.state_mut().run("ui.dock", json!({"operation":"open","panel":"clips"})).unwrap();
    h.run_steps(4);
    assert_eq!(h.state().ui.docking.layout.floating.iter().find(|g| g.panels.contains(&Panel::Clips)).unwrap().rect, rect);
    let saved = serde_json::to_string(&h.state().ui).unwrap();
    let expected = serde_json::to_value(&h.state().ui.docking).unwrap();
    let mut reloaded = harness(Some(serde_json::from_str(&saved).unwrap()), 1400.0, 1.0);
    assert_eq!(serde_json::to_value(&reloaded.state().ui.docking).unwrap(), expected);
    reloaded.state_mut().run("ui.dock", json!({"operation":"move","panel":"clips","target":"tracks","zone":"center"})).unwrap();
    reloaded.run_steps(4);
    reloaded.get_by_role_and_label(egui::accesskit::Role::Tab, "Tracks and Groups").click();
    reloaded.run_steps(3);
    reloaded.get_by_label("Trim (F6)").click();
    reloaded.run_steps(3);
    assert_eq!(reloaded.state().engine.session().edit.tool, transit_model::Tool::Trim);
    assert!(!reloaded.state().engine.transport.playing);
    let edit = serde_json::to_value(&reloaded.state().ui.docking).unwrap();
    reloaded.state_mut().ui.window = MainWindow::Mix;
    reloaded.run_steps(5);
    reloaded.state_mut().run("ui.dock", json!({"operation":"float","panel":"tracks","rect":[900,70,220,420]})).unwrap();
    reloaded.run_steps(4);
    assert_eq!(serde_json::to_value(&reloaded.state().ui.docking).unwrap(), edit);
    assert!(!reloaded.state().ui.mix_docking.layout.contains(&Panel::Clips));
    let record = reloaded.get_by_label("Record enable");
    // A real pointer click must remain outside the floating panel's occluding rectangle.
    assert!(!egui::Rect::from_min_size(egui::pos2(900.0, 70.0), vec2(220.0, 420.0)).contains(record.rect().center()));
    record.click();
    reloaded.run_steps(3);
    assert!(reloaded.state().engine.session().tracks[0].mixer.record_arm);
    assert!(!reloaded.state().engine.transport.recording);
}

#[test]
fn invalid_docking_requests_are_atomic_in_edit_and_mix() {
    let mut h = harness(None, 1400.0, 1.0);
    for window in [MainWindow::Edit, MainWindow::Mix] {
        h.state_mut().ui.window = window;
        h.run_steps(4);
        let before = serde_json::to_value(&h.state().ui).unwrap();
        for params in [
            json!({"operation":"close","panel":"canvas"}),
            json!({"operation":"move","panel":"tracks","target":"canvas","zone":"center"}),
            json!({"operation":"float","panel":"tracks","rect":[0,0,-1,100]}),
        ] {
            assert!(h.state_mut().run("ui.dock", params).is_err());
            assert_eq!(serde_json::to_value(&h.state().ui).unwrap(), before);
        }
        if window == MainWindow::Mix {
            assert!(h.state_mut().run("ui.dock", json!({"operation":"open","panel":"midi"})).is_err());
        }
        assert_eq!(h.state().engine.session().tracks.len(), 1);
    }
}

#[test]
fn docking_renders_edit_and_mix_default_floating_and_redocked() {
    for width in [1000.0, 1400.0] {
        for scale in [1.0, 2.0] {
            for window in [MainWindow::Edit, MainWindow::Mix] {
                let mut h = harness(Some(UiState { window, ..Default::default() }), width, scale);
                h.state_mut().engine = transit_engine::demo::demo_engine();
                for state in ["default", "floating", "redocked"] {
                    if state == "floating" {
                        h.state_mut().run("ui.dock", json!({"operation":"float","panel":"tracks","rect":[100,80,260,500]})).unwrap();
                    } else if state == "redocked" {
                        h.state_mut().run("ui.dock", json!({"operation":"move","panel":"tracks","target":"canvas","zone":"left"})).unwrap();
                    }
                    h.run_steps(5);
                    h.state().ui.docking.validate().unwrap();
                    h.state().ui.mix_docking.validate().unwrap();
                    if let Ok(directory) = std::env::var("CRAFT_UI_DOCKING_DIR") {
                        std::fs::create_dir_all(&directory).unwrap();
                        h.render().unwrap().save(format!("{directory}/sound-{window:?}-{width}-{scale}-{state}.png")).unwrap();
                    }
                }
            }
        }
    }
}

#[test]
fn reset_is_enabled_and_resolves_in_native_and_in_window_menu_trees() {
    use transit_ui_egui::menus;
    let mut app = SoundApp::new(transit_engine::Engine::default(), None, Services::default());
    let path = "Window > Reset Panel Layout";
    let extra: Vec<_> = menus::UI_COMMANDS.iter().map(|(id, _, path, _)| (*path, *id)).collect();
    assert_eq!(menus::command_for_path(path, &extra).as_deref(), Some("ui.dock.reset"));
    let tree = menus::tree();
    let window = tree.iter().find(|node| node.label == "Window").unwrap();
    assert!(window.children.iter().any(|node| node.path == path));
    assert!(menus::item_state(&app, path, Some("ui.dock.reset")).0);
    app.run("ui.dock", json!({"operation":"float","panel":"tracks","rect":[50,60,260,420]})).unwrap();
    menus::invoke_menu(&mut app, "ui.dock.reset", path);
    assert!(!app.ui.docking.layout.floating.iter().any(|group| group.panels.contains(&Panel::Tracks)));
}

#[test]
fn legacy_preferences_restore_the_theme_and_validate_default_docking() {
    let mut value = serde_json::to_value(UiState { theme: transit_ui_egui::theme::ThemeMode::Light, ..Default::default() }).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("docking");
    object.remove("mix_docking");
    let restored: UiState = serde_json::from_value(value).unwrap();
    assert_eq!(restored.theme, transit_ui_egui::theme::ThemeMode::Light);
    restored.docking.validate().unwrap();
    restored.mix_docking.validate_mix().unwrap();
}
