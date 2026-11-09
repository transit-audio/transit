//! The minimal menus (File, Edit, Track, Transport, Audio), keyboard shortcuts, the status bar,
//! the save-changes question and the recovery offer.
//!
//! Shortcuts come from one table, [`crate::shortcuts`] (Ableton where Ableton and Cubase differ).

#[path = "session_settings.rs"]
mod session_settings;

use crate::shortcuts::{Action as A, label, pressed};
use crate::theme::{STATUS_H, TOKENS};
use crate::{Dialog, StudioApp};
use serde_json::{Value, json};
use transit_model::TrackId;

impl StudioApp {
    /// Run a session-replacing command or Quit, asking to save unsaved changes first.
    pub fn run_or_ask(&mut self, id: &str, params: Value) {
        if self.engine.transport.recording && matches!(id, "session.new" | "session.open" | "app.quit") {
            self.stop_play();
        }
        if self.engine.is_dirty() && matches!(id, "session.new" | "session.open" | "app.quit") {
            self.dialog = Some(Dialog::SaveChanges { then: (id.into(), params) });
            return;
        }
        let _ = self.run(id, params);
    }

    /// File › Save: to the session's file, or Save As when it has none. False when cancelled or failed.
    pub fn save(&mut self) -> bool {
        if self.engine.path.is_some() {
            return self.run("session.save", json!({})).is_ok();
        }
        self.save_as()
    }

    /// File > Save As: the dialog starts in the session's folder (#385, `crate::session_folder`).
    pub fn save_as(&mut self) -> bool {
        let name = format!("{}.transit", self.engine.session().name);
        let start = self.session_start_dir().join(name);
        let Some(path) = self.services.pick_save.as_ref().and_then(|f| f(&start.to_string_lossy())) else { return false };
        self.run("session.save_as", json!({"path": path, "replace": true})).is_ok()
    }

    pub fn open_dialog(&mut self) {
        let start = self.session_start_dir();
        let Some(path) = self.services.pick_open.as_ref().and_then(|f| f("session.open", Some(&start))) else { return };
        self.run_or_ask("session.open", json!({"path": path}));
    }
}

pub fn menu_bar(app: &mut StudioApp, ui: &mut egui::Ui) {
    app.arrange.bar_menu.clear();
    app.recent_menu.clear();
    // The menu bar (#326): 32 high on `bg.titlebar` with a 1 px `line` below; items 22 high.
    let item_pad = ((crate::tokens::chrome::MENU_BAR_HEIGHT - crate::tokens::chrome::MENU_BAR_ITEM_HEIGHT) / 2.0) as i8;
    egui::Panel::top("menu_bar")
        .exact_size(crate::tokens::chrome::MENU_BAR_HEIGHT)
        .frame(egui::Frame::NONE.fill(TOKENS.bg_titlebar).inner_margin(egui::Margin::symmetric(crate::theme::SPACE as i8, item_pad)))
        .show(ui, |ui| {
            let config = egui::containers::menu::MenuConfig::new().style(crate::chrome::menu_style);
            egui::MenuBar::new().style(crate::chrome::menu_bar_style).config(config).ui(ui, |ui| {
                let file_menu = crate::chrome::bar_menu(ui, "File", false, |ui| {
                    if ui.add(crate::chrome::MenuRow::new("New")).clicked() {
                        app.run_or_ask("session.new", json!({}));
                        ui.close();
                    }
                    if ui.add(crate::chrome::MenuRow::new("Open…")).clicked() {
                        ui.close();
                        app.open_dialog();
                    }
                    crate::recent::menu(app, ui);
                    session_settings::menu(ui);
                    if ui.add(crate::chrome::MenuRow::new("Save")).clicked() {
                        ui.close();
                        app.save();
                    }
                    if ui.add(crate::chrome::MenuRow::new("Save As…")).clicked() {
                        ui.close();
                        app.save_as();
                    }
                    ui.separator();
                    let b = ui.add(crate::chrome::MenuRow::new("Bounce…").shortcut(label(A::Bounce)));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Bounce…".into(), String::new(), b.rect));
                    if b.clicked() {
                        ui.close();
                        app.open_bounce();
                    }
                    ui.separator();
                    if ui.add(crate::chrome::MenuRow::new("Quit")).clicked() {
                        ui.close();
                        app.run_or_ask("app.quit", json!({}));
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "File".into(), String::new(), file_menu.response.rect));
                let edit_menu = crate::chrome::bar_menu(ui, "Edit", false, |ui| {
                    history_item(app, ui, false);
                    history_item(app, ui, true);
                    let items: [(&str, A, fn(&mut StudioApp)); 6] = [
                        ("Cut", A::Cut, |a| drop(a.copy_selected(true))),
                        ("Copy", A::Copy, |a| drop(a.copy_selected(false))),
                        ("Paste at Cursor", A::Paste, |a| drop(a.paste_at_cursor())),
                        ("Duplicate", A::Duplicate, |a| drop(a.duplicate_selected())),
                        ("Split at Cursor", A::Split, |a| drop(a.split_at_cursor())),
                        ("Delete", A::Delete, |a| drop(a.delete_selected())),
                    ];
                    for (text, action, f) in items {
                        if ui.add(crate::chrome::MenuRow::new(text).shortcut(label(action))).clicked() {
                            f(app);
                            ui.close();
                        }
                    }
                    let item = crate::bounce_in_place::clip_menu_item(app, ui);
                    app.arrange.bar_menu.push(item);
                    ui.separator();
                    crate::edit_tools::edit_menu_items(app, ui);
                    ui.separator();
                    let b = ui.add(crate::chrome::MenuRow::new("Undo History").shortcut(label(A::UndoHistory)));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Undo History".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.ui.show_history = true;
                        ui.close();
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Edit".into(), String::new(), edit_menu.response.rect));
                let track_menu = crate::chrome::bar_menu(ui, "Track", false, |ui| {
                    if ui.add(crate::chrome::MenuRow::new("Add Mono Audio Track").shortcut(label(A::NewAudioTrack))).clicked() {
                        let _ = app.add_audio_track("mono");
                        ui.close();
                    }
                    if ui.add(crate::chrome::MenuRow::new("Add Stereo Audio Track")).clicked() {
                        let _ = app.add_audio_track("stereo");
                        ui.close();
                    }
                    if ui.add(crate::chrome::MenuRow::new("Add Instrument Track").shortcut(label(A::NewInstrumentTrack))).clicked() {
                        let _ = app.add_track("instrument");
                        ui.close();
                    }
                    if ui.add(crate::chrome::MenuRow::new("Add MIDI Track")).clicked() {
                        let _ = app.add_track("midi");
                        ui.close();
                    }
                    if ui.add(crate::chrome::MenuRow::new("Add FX Channel")).clicked() {
                        let _ = app.add_fx_channel();
                        ui.close();
                    }
                    if ui.add(crate::chrome::MenuRow::new("New MIDI Clip at Cursor")).clicked() {
                        let _ = app.new_midi_clip(None);
                        ui.close();
                    }
                    let unused = crate::mixer::orphan_busses(&app.engine.session_arc()).len();
                    if unused > 0 && ui.add(crate::chrome::MenuRow::new(format!("Remove unused buses ({unused})"))).clicked() {
                        let _ = app.remove_unused_busses();
                        ui.close();
                    }
                    ui.separator();
                    let b = ui.add(crate::chrome::MenuRow::new("Rename").shortcut(label(A::Rename)));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Rename".into(), String::new(), b.rect));
                    if b.clicked() {
                        ui.close();
                        app.rename_selected();
                    }
                    let b = ui.add(crate::chrome::MenuRow::new("Bounce in Place").shortcut(label(A::BounceInPlace)));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Bounce in Place".into(), String::new(), b.rect));
                    if b.clicked() {
                        ui.close();
                        app.open_bounce_in_place();
                    }
                    ui.separator();
                    let activity = activity_item(app, ui, None);
                    app.arrange.bar_menu.push(activity);
                    let hidden = app.engine.session().tracks.iter().any(|t| t.hidden);
                    let b = ui.add_enabled(hidden, crate::chrome::MenuRow::new("Show Hidden Tracks…")).on_disabled_hover_text("No hidden tracks");
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Show Hidden Tracks…".into(), String::new(), b.rect));
                    if b.clicked() {
                        ui.close();
                        app.open_hidden_tracks();
                    }
                    ui.separator();
                    crate::groups::menu_items(app, ui, None);
                });
                // Where the Track menu sits, for `ui.inspect` and tests (slice 2c).
                app.arrange.bar_menu.retain(|m| m.label != "Track");
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Track".into(), String::new(), track_menu.response.rect));
                let mut view_items = Vec::new();
                let auto_menu = crate::chrome::bar_menu(ui, "Automation", true, |ui| {
                    // Follow-ups: a preference, not the document.
                    let on = app.ui.thin_after_write;
                    let b = ui.add(crate::chrome::MenuRow::new("Thin after write").checked(on));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Thin after write".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.ui.thin_after_write = !on;
                        ui.close();
                    }
                    let b = ui.add_enabled(app.focus_track().is_some_and(|t| !app.pass_on(t)), crate::chrome::MenuRow::new("Thin Focused Track"));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Thin Focused Track".into(), String::new(), b.rect));
                    if b.clicked() {
                        let _ = app.thin_focused();
                        ui.close();
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Automation".into(), String::new(), auto_menu.response.rect));
                let view_menu = crate::chrome::bar_menu(ui, "View", true, |ui| {
                    let u = &mut app.ui;
                    for (name, action, flag) in [
                        ("Browser", A::ToggleBrowser, u.browser.show),
                        ("Mixer", A::ToggleMixer, u.show_mixer),
                        ("Devices", A::ToggleDevices, u.show_devices),
                        ("Full Mixer Window", A::FullMixer, u.mixer_full),
                    ] {
                        let b = ui.add(crate::chrome::MenuRow::new(name).checked(flag).shortcut(label(action)));
                        view_items.push(crate::mixer::MenuItem::at(ui, name.into(), String::new(), b.rect));
                        if b.clicked() {
                            match action {
                                A::ToggleBrowser => u.browser.show = !flag,
                                A::ToggleMixer => toggle_mixer(u),
                                A::ToggleDevices => toggle_devices(u),
                                _ => u.mixer_full = !flag,
                            }
                            ui.close();
                        }
                    }
                    ui.separator();
                    crate::arrange::track_height::menu(app, ui, &mut view_items);
                    // Grid lines through clips (#302): Ableton draws them over clips, Logic behind regions.
                    let b = ui.add(crate::chrome::MenuRow::new("Grid Over Clips").checked(app.engine.session().edit.grid_over_clips));
                    crate::help::hover(ui, &b, crate::help::Id::ViewGridOverClips);
                    view_items.push(crate::mixer::MenuItem::at(ui, "Grid Over Clips".into(), "view.grid_over_clips".into(), b.rect));
                    if b.clicked() {
                        let _ = app.run("view.grid_over_clips", json!({}));
                        ui.close();
                    }
                    // Appearance: System follows the OS; kept in `UiState` (slice 5). v0.7.0 ships
                    // dark only: System and Light show unavailable (ADR 0045).
                    let sub = crate::chrome::submenu(ui, "Appearance", true, |ui| {
                        for a in crate::theme::Appearance::ALL {
                            let on = app.ui.appearance.effective() == a;
                            let available = a.available();
                            let b = ui.add_enabled_ui(available, |ui| ui.add(crate::chrome::MenuRow::new(a.label()).checked(on))).inner;
                            let b = b.on_disabled_hover_text(crate::theme::LIGHT_UNAVAILABLE_TIP);
                            let value = if available { "appearance" } else { "appearance.unavailable" };
                            view_items.push(crate::mixer::MenuItem::at(ui, a.label().into(), value.into(), b.rect));
                            if available && b.clicked() {
                                app.ui.appearance = a;
                                ui.close();
                            }
                        }
                        ui.separator();
                        let zoom_label = format!("Interface zoom: {:.0}%", app.ui.ui_zoom * 100.0);
                        let zoom = crate::chrome::submenu(ui, &zoom_label, true, |ui| {
                            for percent in [100, 125, 150, 175, 200] {
                                let factor = percent as f32 / 100.0;
                                let name = format!("{percent}%");
                                let b = ui.add(crate::chrome::MenuRow::new(&name).checked((app.ui.ui_zoom - factor).abs() < 0.001));
                                view_items.push(crate::mixer::MenuItem::at(ui, name, "interface_zoom".into(), b.rect));
                                if b.clicked() {
                                    app.set_interface_zoom(ui.ctx(), factor);
                                    ui.close();
                                }
                            }
                        });
                        view_items.push(crate::mixer::MenuItem::at(ui, zoom_label, "interface_zoom".into(), zoom.response.rect));
                        zoom.response.on_hover_text("Plug-in editor windows follow the display scale, not this zoom.");
                    });
                    view_items.push(crate::mixer::MenuItem::at(ui, "Appearance".into(), String::new(), sub.response.rect));
                });
                // Where the View menu and its entries sit, for `ui.inspect` and tests (slice 4).
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "View".into(), String::new(), view_menu.response.rect));
                app.arrange.bar_menu.extend(view_items);
                let options = crate::chrome::bar_menu(ui, "Options", true, |ui| {
                    let b = ui.add(crate::chrome::MenuRow::new("Low Latency").checked(app.ui.low_latency));
                    crate::help::hover(ui, &b, crate::help::Id::OptionsLowLatency);
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Low Latency".into(), "options.low_latency".into(), b.rect));
                    if b.clicked() {
                        let _ = app.run("options.low_latency", json!({}));
                        ui.close();
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Options".into(), String::new(), options.response.rect));
                let mut window_items = Vec::new();
                let window_menu = crate::chrome::bar_menu(ui, "Window", true, |ui| {
                    let on = app.ui.midi_monitor.open;
                    let b = ui.add(crate::chrome::MenuRow::new("MIDI Monitor").checked(on));
                    window_items.push(crate::mixer::MenuItem::at(ui, "MIDI Monitor".into(), String::new(), b.rect));
                    if b.clicked() {
                        let _ = app.run("window.midi_monitor", json!({"value": !on}));
                        ui.close();
                    }
                    let on = app.ui.midi_mappings.open;
                    let b = ui.add(crate::chrome::MenuRow::new("MIDI Mappings").checked(on));
                    window_items.push(crate::mixer::MenuItem::at(ui, "MIDI Mappings".into(), String::new(), b.rect));
                    if b.clicked() {
                        let _ = app.run("window.midi_mappings", json!({"value": !on}));
                        ui.close();
                    }
                    ui.separator();
                    // Issue #173: a preference; off keeps editors apart from the main window.
                    let on = app.ui.editors_follow;
                    let label = "Plug-in Windows Follow transit";
                    let b = ui
                        .add(crate::chrome::MenuRow::new(label).checked(on))
                        .on_hover_text("Plug-in windows hide, return and come to the front with transit's window, and have no taskbar button");
                    window_items.push(crate::mixer::MenuItem::at(ui, label.into(), String::new(), b.rect));
                    if b.clicked() {
                        let _ = app.run("window.editors_follow", json!({"value": !on}));
                        ui.close();
                    }
                    // 326-M: every plug-in editor window, built-in and hosted.
                    let b = ui.add_enabled(!app.ui.plugin_windows.is_empty(), crate::chrome::MenuRow::new("Close All Editors"));
                    window_items.push(crate::mixer::MenuItem::at(ui, "Close All Editors".into(), "window.close_editors".into(), b.rect));
                    if b.clicked() {
                        let _ = app.run("window.close_editors", json!({}));
                        ui.close();
                    }
                    // 326-V: fold every device window to its title bar, and back.
                    for (label, id, help) in [
                        ("Collapse All Editors", "window.collapse_editors", crate::help::Id::WindowCollapseEditors),
                        ("Expand All Editors", "window.expand_editors", crate::help::Id::WindowExpandEditors),
                    ] {
                        let b = ui.add_enabled(!app.ui.plugin_windows.is_empty(), crate::chrome::MenuRow::new(label));
                        crate::help::hover(ui, &b, help);
                        window_items.push(crate::mixer::MenuItem::at(ui, label.into(), id.into(), b.rect));
                        if b.clicked() {
                            let _ = app.run(id, json!({}));
                            ui.close();
                        }
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Window".into(), String::new(), window_menu.response.rect));
                app.arrange.bar_menu.extend(window_items);
                let transport_menu = crate::chrome::bar_menu(ui, "Transport", true, |ui| {
                    let cycle = app.engine.session().edit.loop_playback;
                    let items: [(&str, A, &str, Value); 5] = [
                        ("Play / Stop", A::PlayStop, "transport.toggle", json!({})),
                        ("Record", A::Record, "transport.record", json!({})),
                        ("Return to Zero", A::ReturnToZero, "transport.rtz", json!({})),
                        (if cycle { "Cycle Off" } else { "Cycle On" }, A::Cycle, "options.loop_playback", json!({"value": !cycle})),
                        ("Set Cycle to Selection", A::CycleToSelection, "transport.cycle_to_selection", json!({})),
                    ];
                    for (text, action, id, p) in items {
                        let b = ui.add(crate::chrome::MenuRow::new(text).shortcut(label(action)));
                        app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, text.into(), String::new(), b.rect));
                        if b.clicked() {
                            let _ = app.run(id, p);
                            ui.close();
                        }
                    }
                    let has_loop = !app.engine.session().edit.loop_range.is_empty();
                    let b = ui.add_enabled(has_loop, crate::chrome::MenuRow::new("Clear Loop"));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Clear Loop".into(), String::new(), b.rect));
                    if b.clicked() {
                        crate::loopbar::clear(app);
                        ui.close();
                    }
                    ui.separator();
                    // Slice 6: a preference, not the document.
                    let on = app.ui.count_in;
                    let b = ui.add(crate::chrome::MenuRow::new("Count-in").checked(on));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Count-in".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.ui.count_in = !on;
                        ui.close();
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Transport".into(), String::new(), transport_menu.response.rect));
                let mut audio_items = Vec::new();
                let audio_menu = crate::chrome::bar_menu(ui, "Audio", false, |ui| {
                    let name = app.player.as_ref().map_or("No audio device (--no-audio)".to_string(), |p| p.device_name.clone());
                    crate::chrome::menu_note(ui, egui::RichText::new(name).color(TOKENS.text_dim));
                    let b = ui.add(crate::chrome::MenuRow::new("Audio Devices…"));
                    audio_items.push(crate::mixer::MenuItem::at(ui, "Audio Devices…".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.open_audio_devices();
                        ui.close();
                    }
                    let b = ui.add_enabled(
                        !app.engine.transport.recording && (app.player.is_some() || app.audio_devices.fake.is_some()),
                        crate::chrome::MenuRow::new("Reconnect Device"),
                    );
                    audio_items.push(crate::mixer::MenuItem::at(ui, "Reconnect Device".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.reconnect_audio_devices();
                        ui.close();
                    }
                    if ui.add(crate::chrome::MenuRow::new("Scan Plug-ins")).clicked() {
                        crate::plugin_scan::rescan();
                        ui.close();
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Audio".into(), String::new(), audio_menu.response.rect));
                app.arrange.bar_menu.extend(audio_items);
                let help = crate::chrome::bar_menu(ui, "Help", true, |ui| {
                    let b = ui.add(crate::chrome::MenuRow::new("Keyboard Shortcuts"));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Keyboard Shortcuts".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.ui.show_shortcuts = true;
                        ui.close();
                    }
                    let b = ui.add(crate::chrome::MenuRow::new("Info Panel").checked(app.help.open).shortcut(label(A::ToggleInfo)));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Info Panel".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.help.toggle();
                        ui.close();
                    }
                    ui.separator();
                    let why = app.update_check_unavailable();
                    let b = ui
                        .add_enabled(why.is_none(), crate::chrome::MenuRow::new("Check for Updates…"))
                        .on_disabled_hover_text(why.unwrap_or_default());
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Check for Updates…".into(), String::new(), b.rect));
                    if b.clicked() {
                        let _ = app.run("help.check_updates", json!({}));
                        ui.close();
                    }
                    let on = app.ui.check_for_updates;
                    let b = ui
                        .add_enabled(why.is_none(), crate::chrome::MenuRow::new("Check for New Versions on Launch").checked(on))
                        .on_hover_text(crate::update::PRIVACY)
                        .on_disabled_hover_text(why.unwrap_or_default());
                    if b.clicked() {
                        app.ui.check_for_updates = !on;
                    }
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Check for New Versions on Launch".into(), String::new(), b.rect));
                    let logging = app.services.log.is_some();
                    let on = app.services.log.as_ref().is_some_and(|l| l.debug());
                    let b = ui
                        .add_enabled(logging, crate::chrome::MenuRow::new("Debug Logging").checked(on))
                        .on_hover_text("Log more detail to the log file for the rest of this session.")
                        .on_disabled_hover_text("This build has no log control.");
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Debug Logging".into(), String::new(), b.rect));
                    if b.clicked() {
                        let _ = app.run("help.debug_logging", json!({"on": !on}));
                        ui.close();
                    }
                    ui.separator();
                    let b = ui.add(crate::chrome::MenuRow::new("About transit…"));
                    app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "About transit…".into(), String::new(), b.rect));
                    if b.clicked() {
                        app.about.open = true;
                        ui.close();
                    }
                });
                app.arrange.bar_menu.push(crate::mixer::MenuItem::at(ui, "Help".into(), String::new(), help.response.rect));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let s = app.engine.session();
                    // Right to left: the unsaved-changes dot (6 px, painted in `text`) goes first so it sits after the name.
                    if app.engine.is_dirty() {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.0, TOKENS.text);
                    }
                    ui.label(egui::RichText::new(s.name.clone()).color(TOKENS.text_dim));
                });
            });
        });
}

/// The Track menu acts on the selection (or focus); a context menu acts on its own track.
pub(crate) fn activity_item(app: &mut StudioApp, ui: &mut egui::Ui, on: Option<TrackId>) -> crate::mixer::MenuItem {
    let tracks = on.map_or_else(
        || {
            let selected = &app.engine.session().edit.selected_tracks;
            if selected.is_empty() { app.focus_track().into_iter().collect() } else { selected.clone() }
        },
        |id| vec![id],
    );
    // Recover every inactive member of a mixed selection with one explicit action.
    let inactive = !tracks.iter().any(|id| app.engine.session().track(*id).is_some_and(|t| t.inactive));
    let (label, help) =
        if inactive { ("Make Inactive", crate::help::Id::TrackMenuMakeInactive) } else { ("Make Active", crate::help::Id::TrackMenuMakeActive) };
    let b = ui.add_enabled(!tracks.is_empty(), crate::chrome::MenuRow::new(label));
    crate::help::hover(ui, &b, help);
    let item = crate::mixer::MenuItem::at(ui, label.into(), String::new(), b.rect);
    if b.clicked() {
        let _ = app.run("track.make_inactive", json!({"tracks": tracks, "inactive": inactive}));
        ui.close();
    }
    item
}

pub fn status_bar(app: &mut StudioApp, ui: &mut egui::Ui) {
    let now = ui.ctx().input(|i| i.time);
    if let Some(left) = app.status_age.tick(&mut app.status, now, crate::diagnostics::now_secs()) {
        ui.ctx().request_repaint_after_secs(left as f32);
    }
    egui::Panel::bottom("status")
        .exact_size(STATUS_H)
        .frame(egui::Frame::NONE.fill(TOKENS.panel).inner_margin(egui::Margin::symmetric(crate::theme::SPACE as i8, 2)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new(&app.status).color(TOKENS.text_dim).size(11.5));
                if let Some(n) = app.update.notice.clone() {
                    ui.add_space(crate::theme::SPACE);
                    crate::update::notice_line(ui, &n);
                }
                // Issue #82: more than one slot reads "Not responding": one click retries them all.
                let stuck = app.player.as_ref().map_or(0, |p| p.stuck_slots().len());
                app.retry_all_rect = None;
                if stuck > 1 {
                    ui.add_space(crate::theme::SPACE);
                    let (r, glyph) = ui.allocate_exact_size(egui::vec2(crate::icons::ICON, crate::icons::ICON), egui::Sense::click());
                    let text = egui::RichText::new("Retry all").color(TOKENS.text).size(11.5);
                    let label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
                    let hot = glyph.hovered() || label.hovered();
                    ui.painter().extend(crate::devices::retry_glyph(r, if hot { TOKENS.text } else { TOKENS.text_dim }));
                    let tip = format!("Retry loading the {stuck} plug-ins that are not responding");
                    app.retry_all_rect = Some(r.union(label.rect));
                    let clicked = glyph.on_hover_text(tip.clone()).clicked() | label.on_hover_text(tip).clicked();
                    if clicked {
                        let _ = app.run("plugin.retry_all", json!({}));
                    }
                }
                if let Some(w) = app.io_warning() {
                    ui.add_space(crate::theme::SPACE);
                    let (r, _) = ui.allocate_exact_size(egui::vec2(crate::icons::ICON, crate::icons::ICON), egui::Sense::hover());
                    // An error (#326): the circle-alert in `meter.hot`, the message in `text`.
                    let sev = crate::chrome::Severity::Error;
                    crate::icons::paint(ui.painter(), r.center(), sev.icon(), 14.0, sev.glyph());
                    let text = egui::RichText::new(w).color(sev.ink()).size(11.5);
                    if ui.add(egui::Label::new(text).sense(egui::Sense::click())).on_hover_text("Open Audio Devices").clicked() {
                        app.open_audio_devices();
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let c = app.cursor();
                    let text = format!("Cursor {}  ·  {}", crate::transport::bars_beats(app, c).trim(), crate::transport::min_sec(app, c));
                    ui.label(egui::RichText::new(text).color(TOKENS.text_dim).size(11.5).monospace());
                });
            });
        });
}

/// Both history actions share the recording refusal used by shortcuts and control requests.
pub(crate) fn history_item(app: &mut StudioApp, ui: &mut egui::Ui, redo: bool) -> egui::Response {
    let (text, action, command) = if redo { ("Redo", A::Redo, "edit.redo") } else { ("Undo", A::Undo, "edit.undo") };
    let reason = app.history_disabled_reason();
    let response = ui
        .add_enabled(reason.is_none(), crate::chrome::MenuRow::new(text).shortcut(label(action)))
        .on_disabled_hover_text(reason.unwrap_or_default());
    if response.clicked() {
        let _ = app.run(command, json!({}));
        ui.close();
    }
    response
}

/// Keyboard shortcuts, unless a text field has the keyboard. Keys come from [`crate::shortcuts`].
pub fn shortcuts(app: &mut StudioApp, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() || app.dialog.is_some() {
        return;
    }
    if pressed(ctx, A::BounceInPlace) {
        app.open_bounce_in_place();
        return;
    }
    // Alt+C before C: clear every clip latch (`track.clear_clip_indicators`).
    if pressed(ctx, A::ClearClipIndicators) {
        let _ = app.run("track.clear_clip_indicators", json!({}));
    }
    // egui turns the platform's copy/cut/paste keys into these events.
    // Only with Ctrl alone: Ctrl+Alt+C is not Copy.
    let (copy, cut, paste) = ctx.input(|i| {
        if !i.modifiers.matches_exact(egui::Modifiers::COMMAND) {
            return (false, false, false);
        }
        let has = |f: fn(&egui::Event) -> bool| i.events.iter().any(f);
        (has(|e| matches!(e, egui::Event::Copy)), has(|e| matches!(e, egui::Event::Cut)), has(|e| matches!(e, egui::Event::Paste(_))))
    });
    if copy || pressed(ctx, A::Copy) {
        let _ = app.copy_selected(false);
    }
    if cut || pressed(ctx, A::Cut) {
        let _ = app.copy_selected(true);
    }
    if paste || pressed(ctx, A::Paste) {
        let _ = app.paste_at_cursor();
    }
    if pressed(ctx, A::Redo) {
        let _ = app.run("edit.redo", json!({}));
    }
    if pressed(ctx, A::Undo) {
        let _ = app.run("edit.undo", json!({}));
    }
    if pressed(ctx, A::Duplicate) {
        let _ = app.duplicate_selected();
    }
    if pressed(ctx, A::Split) {
        let _ = app.split_at_cursor();
    }
    crate::edit_tools::shortcuts(app, ctx);
    if pressed(ctx, A::FadeInToCursor) {
        crate::fades::fade_to_cursor(app, crate::fades::Side::In);
    }
    if pressed(ctx, A::FadeOutFromCursor) {
        crate::fades::fade_to_cursor(app, crate::fades::Side::Out);
    }
    if pressed(ctx, A::NewInstrumentTrack) {
        let _ = app.add_track("instrument");
    }
    if pressed(ctx, A::NewAudioTrack) {
        let _ = app.add_track("audio");
    }
    if pressed(ctx, A::SelectAll) {
        app.select_all_clips();
    }
    if pressed(ctx, A::SaveAs) {
        app.save_as();
    }
    if pressed(ctx, A::Save) {
        app.save();
    }
    if pressed(ctx, A::OpenRecent) {
        app.open_most_recent();
    }
    if pressed(ctx, A::Open) {
        app.open_dialog();
    }
    if pressed(ctx, A::New) {
        app.run_or_ask("session.new", json!({}));
    }
    if pressed(ctx, A::Record) {
        let _ = app.run("transport.record", json!({}));
    }
    if pressed(ctx, A::PlayStop) {
        let _ = app.run("transport.toggle", json!({}));
    }
    if pressed(ctx, A::ReturnToZero) {
        let _ = app.run("transport.rtz", json!({}));
    }
    if pressed(ctx, A::Cycle) {
        let on = app.engine.session().edit.loop_playback;
        let _ = app.run("options.loop_playback", json!({"value": !on}));
    }
    if pressed(ctx, A::CycleToSelection) {
        let _ = app.run("transport.cycle_to_selection", json!({}));
    }
    // Delete over a fader, knob or send level resets it (Ableton); anywhere else it deletes.
    // Backspace does the same. (A readout being edited has the keyboard, so both edit text.)
    match app.mixer.hover {
        Some(t) => {
            if pressed(ctx, A::ResetControl) {
                app.reset_control(t);
            }
        }
        None => {
            if pressed(ctx, A::Delete) {
                let _ = app.delete_selected();
            }
        }
    }
    // Views (Ableton where it has a key; Cubase's F3 for the full mixer).
    if pressed(ctx, A::ToggleMixer) {
        toggle_mixer(&mut app.ui);
    }
    if pressed(ctx, A::ToggleDevices) {
        toggle_devices(&mut app.ui);
    }
    if pressed(ctx, A::FullMixer) {
        app.ui.mixer_full = !app.ui.mixer_full;
    }
    if pressed(ctx, A::SelectNone) && !crate::arrange::marquee::cancel(app) {
        app.deselect_all();
    }
    if !(app.keys_shown() && app.keys.active) {
        for (action, direction) in [(A::TrackHeightIn, "in"), (A::TrackHeightOut, "out")] {
            if pressed(ctx, action) {
                let _ = app.run("track.height_zoom", json!({"direction": direction}));
            }
        }
    }
    if pressed(ctx, A::ZoomIn) {
        let _ = app.execute_ui("view.zoom_in", &json!({}));
    }
    if pressed(ctx, A::ZoomOut) {
        let _ = app.execute_ui("view.zoom_out", &json!({}));
    }
    if pressed(ctx, A::ZoomInterfaceIn) {
        app.set_interface_zoom(ctx, app.ui.ui_zoom * 1.1);
    }
    if pressed(ctx, A::ZoomInterfaceOut) {
        app.set_interface_zoom(ctx, app.ui.ui_zoom / 1.1);
    }
    if pressed(ctx, A::ResetInterfaceZoom) {
        app.set_interface_zoom(ctx, 1.0);
    }
    // The arrangement grid (view state): Ableton's Ctrl+1 narrow, Ctrl+2 widen, Ctrl+3 triplet, Ctrl+4 snap.
    let (finer, wider) = (pressed(ctx, A::GridNarrow), pressed(ctx, A::GridWiden));
    if finer || wider {
        let s = app.engine.session();
        let (ppt, bar) = (crate::grid::px_per_tick(s, app.arrange.layout.samples_per_px), crate::grid::bar_ticks(s));
        crate::grid::step(&mut app.arrange.grid, ppt, bar, finer);
    }
    if pressed(ctx, A::GridTriplet) {
        app.arrange.grid.triplet = !app.arrange.grid.triplet;
    }
    if pressed(ctx, A::GridSnap) {
        app.arrange.grid.snap_enabled = !app.arrange.grid.snap_enabled;
    }
}

/// The save-changes question.
pub fn dialogs(app: &mut StudioApp, ctx: &egui::Context) {
    session_settings::window(app, ctx);
    if let Some(Dialog::DeleteTracks { tracks, clips }) = app.dialog.clone() {
        return delete_tracks_dialog(app, ctx, &tracks, clips);
    }
    if let Some(Dialog::DeleteTake { track, take }) = app.dialog.clone() {
        return delete_take_dialog(app, ctx, track, take);
    }
    let Some(Dialog::SaveChanges { then }) = app.dialog.clone() else { return };
    // The chrome's modal (#326): the question is the title; Save is the default (Enter), Cancel
    // left of it (Escape), Don't Save alone at the far left.
    let ask = crate::chrome::Ask {
        question: format!("Save changes to “{}”?", app.engine.session().name),
        detail: Some("Your changes are lost if you don't save them.".into()),
        default: "Save",
        cancel: "Cancel",
        third: Some("Don't Save"),
    };
    let choice = match crate::chrome::modal(ctx, egui::Id::new("save_changes"), &ask) {
        Some(crate::chrome::Choice::Default) => Some(1),
        Some(crate::chrome::Choice::Third) => Some(2),
        Some(crate::chrome::Choice::Cancel) => Some(0),
        None => None,
    };
    match choice {
        Some(1) => {
            app.dialog = None;
            if app.save() {
                let _ = app.run(&then.0, then.1);
            }
        }
        Some(2) => {
            app.dialog = None;
            let _ = app.run(&then.0, then.1);
        }
        Some(_) => app.dialog = None,
        None => {}
    }
}

/// The delete questions' detail line (the artboard's): the deletion is one undo step.
pub(crate) fn undo_detail(many: bool) -> &'static str {
    if many { "Undo brings them back." } else { "Undo brings it back." }
}

/// What the delete-tracks question asks and what its default button says: the label names the
/// loss ("Delete 3 Tracks", never "OK"; #326).
pub(crate) fn delete_tracks_text(s: &transit_model::Session, tracks: &[TrackId], clips: bool) -> (String, String) {
    let group = tracks.first().filter(|_| tracks.len() == 1).and_then(|t| s.track(*t)).filter(|t| t.is_folder());
    if let Some(g) = group {
        let n = s.tracks.iter().filter(|t| crate::groups::parent_of(s, t) == Some(g.id)).count();
        let q = format!("Delete {}? Its {n} track{} stay{}.", g.name, if n == 1 { "" } else { "s" }, if n == 1 { "s" } else { "" });
        return (q, "Delete Group".into());
    }
    let q = match (tracks.len(), clips) {
        (1, true) => {
            let name = s.track(tracks[0]).map_or("the track".to_string(), |t| format!("“{}”", t.name));
            format!("Delete {name} and its clips?")
        }
        (1, false) => "Delete the track?".to_string(),
        (n, true) => format!("Delete {n} tracks and their clips?"),
        (n, false) => format!("Delete {n} tracks?"),
    };
    let label = if tracks.len() == 1 { "Delete Track".to_string() } else { format!("Delete {} Tracks", tracks.len()) };
    (q, label)
}

/// "Delete 3 tracks and their clips?" [Cancel] [Delete 3 Tracks]; Enter confirms, Escape cancels.
fn delete_tracks_dialog(app: &mut StudioApp, ctx: &egui::Context, tracks: &[TrackId], clips: bool) {
    let (question, label) = delete_tracks_text(app.engine.session(), tracks, clips);
    let detail = Some(undo_detail(tracks.len() > 1).to_string());
    let ask = crate::chrome::Ask { question, detail, default: &label, cancel: "Cancel", third: None };
    let clicked = crate::chrome::modal(ctx, egui::Id::new("delete_tracks"), &ask);
    let choice = clicked.map(|c| c == crate::chrome::Choice::Default);
    if let Some(yes) = choice {
        app.dialog = None;
        if yes {
            let _ = app.delete_tracks(tracks);
        }
    }
}

/// The delete-take question's default button: it names the loss (#326).
pub(crate) const DELETE_TAKE_LABEL: &str = "Delete Take";

/// "Delete take 2?" / "Undo brings it back." [Cancel] [Delete Take]; Enter confirms, Escape cancels.
fn delete_take_dialog(app: &mut StudioApp, ctx: &egui::Context, track: TrackId, take: u32) {
    let ask = crate::chrome::Ask {
        question: format!("Delete take {take}?"),
        detail: Some(undo_detail(false).to_string()),
        default: DELETE_TAKE_LABEL,
        cancel: "Cancel",
        third: None,
    };
    let clicked = crate::chrome::modal(ctx, egui::Id::new("delete_take"), &ask);
    let choice = clicked.map(|c| c == crate::chrome::Choice::Default);
    if let Some(yes) = choice {
        app.dialog = None;
        if yes {
            let _ = app.delete_take(track, take);
        }
    }
}

/// Unsaved work an earlier run left behind, offered once at launch.
pub fn recovery(app: &mut StudioApp, ctx: &egui::Context) {
    if app.leftovers.is_empty() {
        return;
    }
    let mut act: Option<(usize, bool)> = None;
    let mut later = false;
    crate::theme::modal_window(ctx, "Recover unsaved work").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| {
            ui.label("An earlier run left unsaved changes:");
            for (i, l) in app.leftovers.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&l.name);
                    if ui.button("Recover").clicked() {
                        act = Some((i, true));
                    }
                    if ui.button("Discard").clicked() {
                        act = Some((i, false));
                    }
                });
            }
            if ui.button("Later").clicked() {
                later = true;
            }
        },
    );
    if let Some((i, recover)) = act
        && i < app.leftovers.len()
    {
        let left = app.leftovers.remove(i);
        if recover {
            app.recover(&left);
        } else {
            transit_engine::autosave::discard(&left);
        }
    }
    if later {
        app.leftovers.clear();
    }
}

/// Help > Keyboard Shortcuts as last drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ShortcutsLayout {
    pub open: bool,
    pub window: crate::arrange::R4,
    pub filter: crate::arrange::R4,
    /// The scopes shown, in order.
    pub scopes: Vec<String>,
    /// The rows shown: `Scope · Action · Keys`.
    pub rows: Vec<String>,
}

/// Help > Keyboard Shortcuts: the shortcut table grouped by scope, with a filter (read-only).
pub fn shortcuts_window(app: &mut StudioApp, ctx: &egui::Context) {
    let mut layout = ShortcutsLayout { open: app.ui.show_shortcuts, ..ShortcutsLayout::default() };
    if !app.ui.show_shortcuts {
        app.shortcuts_window = layout;
        return;
    }
    let mut open = true;
    let r = crate::theme::modal_window(ctx, "Keyboard Shortcuts")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size([560.0, 520.0])
        .default_pos(ctx.content_rect().center() - egui::vec2(280.0, 260.0))
        .show(ctx, |ui| {
            let resp = ui
                .horizontal(|ui| {
                    crate::icons::icon(ui, crate::icons::Icon::Filter);
                    crate::chrome::text_field(
                        ui,
                        egui::TextEdit::singleline(&mut app.ui.shortcuts_filter).hint_text("Filter by action or key").desired_width(f32::INFINITY),
                    )
                })
                .inner;
            layout.filter = crate::arrange::r4(resp.rect);
            ui.add_space(crate::theme::SPACE);
            let groups = crate::shortcuts::filtered(&app.ui.shortcuts_filter);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if groups.is_empty() {
                    ui.label(egui::RichText::new("No shortcut matches.").color(TOKENS.text_dim));
                }
                for (scope, rows) in &groups {
                    layout.scopes.push(scope.label().into());
                    ui.label(egui::RichText::new(scope.label()).strong().color(TOKENS.text));
                    egui::Grid::new(("shortcut_grid", scope.label()))
                        .num_columns(3)
                        .spacing([2.0 * crate::theme::SPACE, crate::theme::SPACE / 2.0])
                        .show(ui, |ui| {
                            for s in rows {
                                let keys = if s.action == A::StepTieRest && app.ui.keys.step_tie_rest_space {
                                    "Space".into()
                                } else {
                                    crate::shortcuts::text(s)
                                };
                                ui.label(egui::RichText::new(s.label).color(TOKENS.text));
                                if s.action == A::StepTieRest {
                                    crate::shortcuts::step_tie_rest_selector(ui, &mut app.ui.keys.step_tie_rest_space);
                                } else {
                                    ui.label(egui::RichText::new(&keys).font(crate::theme::mono(ui.ctx(), 12.0)).color(TOKENS.accent));
                                }
                                ui.label(egui::RichText::new(s.note).color(TOKENS.text_dim).size(11.0));
                                ui.end_row();
                                layout.rows.push(format!("{} · {} · {keys}", scope.label(), s.label));
                            }
                        });
                    ui.add_space(crate::theme::SPACE);
                }
            });
        });
    if let Some(r) = r {
        layout.window = crate::arrange::r4(r.response.rect);
    }
    if !open {
        app.ui.show_shortcuts = false;
    }
    layout.open = app.ui.show_shortcuts;
    app.shortcuts_window = layout;
}

/// A fixed point-sized state column, followed by egui's point-sized icon spacing.
/// Keep this slot even for unchecked items and ordinary commands in the same menu.
const MENU_CHECK_SIZE: f32 = crate::tokens::chrome::CHECK_GUTTER;
/// The check mark inside the gutter (`text`, never `signal`: a menu check is a view setting).
const MENU_CHECK_GLYPH: f32 = crate::tokens::chrome::CHECK_GLYPH;

pub(crate) fn menu_contents(text: impl Into<egui::WidgetText>) -> (egui::Atom<'static>, egui::WidgetText) {
    (egui::Atom::custom(egui::Id::new("menu_check"), egui::Vec2::new(MENU_CHECK_SIZE, crate::tokens::chrome::CHECK_GLYPH)), text.into())
}

fn menu_button(text: impl Into<egui::WidgetText>) -> egui::Button<'static> {
    egui::Button::new(menu_contents(text))
}

/// Menu and popover choices use an icon in the state column, never a text glyph.
pub(crate) fn check_button(ui: &mut egui::Ui, on: bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    check_button_with(ui, on, text, None)
}

fn check_button_with(ui: &mut egui::Ui, on: bool, text: impl Into<egui::WidgetText>, shortcut: Option<String>) -> egui::Response {
    let mut b = menu_button(text);
    if let Some(s) = shortcut {
        b = b.shortcut_text(crate::chrome::shortcut(ui.ctx(), s));
    }
    let resp = b.atom_ui(ui);
    if on && let Some(gutter) = resp.rect(egui::Id::new("menu_check")) {
        crate::icons::paint(ui.painter(), gutter.center(), crate::icons::Icon::Check, MENU_CHECK_GLYPH, TOKENS.text);
    }
    resp.response
}

/// View > Mixer, its shortcut and the Mixer tab: show or hide the Mixer pane. The Devices pane is
/// independent; both may be shown.
pub(crate) fn toggle_mixer(ui: &mut crate::UiState) {
    ui.show_mixer = !ui.show_mixer;
}

/// View > Devices, its shortcut and the Devices tab: show or hide the Devices pane, independent of
/// the Mixer pane.
pub(crate) fn toggle_devices(ui: &mut crate::UiState) {
    ui.show_devices = !ui.show_devices;
}
