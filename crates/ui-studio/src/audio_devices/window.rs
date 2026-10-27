use super::*;

/// One list of choices (`None` is "System default"). Returns the combo's rect.
fn choose<T: Clone + PartialEq>(
    ui: &mut egui::Ui,
    key: &str,
    enabled: bool,
    value: &mut Option<T>,
    options: &[(T, String)],
    items: &mut Vec<MenuItem>,
) -> (egui::Rect, String) {
    choose_or(ui, key, enabled, value, options, items, SYSTEM_DEFAULT)
}

/// [`choose`] with its own words for "no choice".
fn choose_or<T: Clone + PartialEq>(
    ui: &mut egui::Ui,
    key: &str,
    enabled: bool,
    value: &mut Option<T>,
    options: &[(T, String)],
    items: &mut Vec<MenuItem>,
    none: &str,
) -> (egui::Rect, String) {
    let shown = value.as_ref().and_then(|v| options.iter().find(|(o, _)| o == v).map(|(_, t)| t.clone()));
    // A saved choice not offered here (unplugged) still shows, marked.
    let text = match (value.as_ref(), shown) {
        (None, _) => none.to_string(),
        (Some(_), Some(t)) => t,
        (Some(_), None) => "Not available (falls back to the default)".to_string(),
    };
    let r = ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(("audio_devices", key)).selected_text(&text).width(360.0).show_ui(ui, |ui| {
            let b = crate::menus::check_button(ui, value.is_none(), none);
            items.push(MenuItem::at(ui, none.into(), key.into(), b.rect));
            if b.clicked() {
                *value = None;
            }
            for (v, t) in options {
                let b = crate::menus::check_button(ui, value.as_ref() == Some(v), t);
                items.push(MenuItem::at(ui, t.clone(), key.into(), b.rect));
                if b.clicked() {
                    *value = Some(v.clone());
                }
            }
        })
    });
    (r.inner.response.rect, text)
}

/// The Backend selector: Auto, cpal, and the native backend of the system (`options`). Returns the
/// combo's rect and text.
fn choose_backend(ui: &mut egui::Ui, enabled: bool, value: &mut Backend, options: &[Backend], items: &mut Vec<MenuItem>) -> (egui::Rect, String) {
    let text = value.label().to_string();
    let r = ui.add_enabled_ui(enabled, |ui| {
        egui::ComboBox::from_id_salt(("audio_devices", "backend")).selected_text(&text).width(360.0).show_ui(ui, |ui| {
            for option in options.iter().copied() {
                let b = crate::menus::check_button(ui, *value == option, option.label());
                items.push(MenuItem::at(ui, option.label().into(), "backend".into(), b.rect));
                if b.clicked() {
                    *value = option;
                }
            }
        })
    });
    (r.inner.response.rect, text)
}

/// The width of the sidebar listing the window's sections.
const SIDEBAR_WIDTH: f32 = 130.0;

/// The window's sections, one visible at a time (Ableton's Preferences shape: a category list on
/// the left). The status lines and the Apply row below stay visible in every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    /// The audio system, backend, devices, rate and buffer size.
    #[default]
    System,
    /// This machine's named paths over the open device's channels.
    IoPaths,
    /// Hardware inserts and their measured latency.
    Hardware,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::System, Section::IoPaths, Section::Hardware];

    pub const fn label(self) -> &'static str {
        match self {
            Section::System => "System",
            Section::IoPaths => "I/O paths",
            Section::Hardware => "Hardware inserts",
        }
    }

    /// The name saved in `ui.json` and the control key in `ui.inspect`.
    pub const fn key(self) -> &'static str {
        match self {
            Section::System => "section_system",
            Section::IoPaths => "section_io",
            Section::Hardware => "section_hardware",
        }
    }

    /// The section saved under `key` (the default for anything else).
    pub fn from_key(key: &str) -> Section {
        Self::ALL.into_iter().find(|s| s.key() == key).unwrap_or_default()
    }

    /// The section `by` steps along (wrapping): Ctrl+Tab is 1, Ctrl+Shift+Tab is -1.
    pub fn cycled(self, by: i32) -> Section {
        let n = Self::ALL.len() as i32;
        let at = Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as i32;
        Self::ALL[(at + by).rem_euclid(n) as usize]
    }
}

/// Audio › Audio Devices.
pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    check(app);
    app.poll_diagnostics(ctx);
    app.audio_devices.layout = AudioDevicesLayout::default();
    if let Some((kind, note)) = app.audio_devices.measure_note.take() {
        match kind {
            crate::hardware_measure::MeasureKind::Insert => app.audio_devices.io_editor.hw_result = note,
            crate::hardware_measure::MeasureKind::Loop => app.audio_devices.calibration_result = note,
        }
    }
    if !app.audio_devices.open {
        app.cancel_measure("");
        return;
    }
    if app.audio_devices.base.as_ref() != Some(&app.ui.audio) {
        app.audio_devices.draft = app.ui.audio.clone();
        app.audio_devices.base = Some(app.ui.audio.clone());
    }
    if app.audio_devices.systems.is_none() && app.audio_devices.panel.is_none() {
        app.audio_devices.systems = Some(app.listing());
    }
    if app.audio_devices.systems.is_some() && app.audio_devices.panel.is_none() && app.asio_selection_changed() {
        // The driver that runs changed (after Apply): list the ASIO system again, probing it.
        app.relist_asio();
    }
    app.poll_asio_listing();
    let asio_listing =
        if app.audio_devices.asio_listing.is_some() { Some(ASIO_LISTING.to_owned()) } else { app.audio_devices.asio_listing_note.clone() };
    let systems = app.audio_devices.systems.clone().unwrap_or_default();
    let mut draft = app.audio_devices.draft.clone();
    let mut lay = AudioDevicesLayout { open: true, systems: systems.iter().map(|s| s.name.clone()).collect(), ..Default::default() };
    let recording = app.recording_active();
    let panel_open = app.audio_devices.panel.is_some();
    let audio_on = app.player.is_some() || app.audio_devices.fake.is_some();
    let (mut apply, mut reconnect, mut rescan, mut revert) = (false, false, false, false);
    let mut native_panel: Option<String> = None;
    let mut copy_diagnostics = false;
    // The path table is the engine's, read again every frame; one edit at most per frame.
    let io = app.engine.io_map().clone();
    let wasapi = crate::audio_devices_io::is_wasapi(&if io.system.is_empty() {
        pick_system(&systems, app.ui.audio.system.as_deref()).map(|s| s.name.clone()).unwrap_or_default()
    } else {
        io.system.clone()
    });
    // The open device's own channel names, for the table of the device that is open.
    let channel_names = app
        .player
        .as_ref()
        .filter(|p| !p.silent && p.device_name == io.device)
        .map(|p| crate::audio_devices_io::ChannelNames { outputs: p.output_channel_names.clone(), inputs: p.input_channel_names.clone() })
        .or_else(|| app.audio_devices.fake_channel_names.clone())
        .unwrap_or_default();
    let mut editor = std::mem::take(&mut app.audio_devices.io_editor);
    let mut io_edit = None;
    let mut io_items = Vec::new();
    let mut hw_action = None;
    let mut hw_layout = Default::default();
    // One measurement at a time: the running one's progress shows where it was started.
    let loop_running = app.measuring_kind() == Some(crate::hardware_measure::MeasureKind::Loop);
    let (progress, cal_progress) = if loop_running { (None, app.measure_progress()) } else { (app.measure_progress(), None) };
    let blocker = if loop_running { Some("The calibration measurement is running.") } else { app.measure_blocker() };
    let cal_blocker = app.calibration_blocker(&draft);
    let (mut cal_send, mut cal_ret) = (app.audio_devices.calibration_send.clone(), app.audio_devices.calibration_return.clone());
    let cal_result = app.audio_devices.calibration_result.clone();
    let mut cal_action = None;
    let rate = app.engine.session().sample_rate.hz();
    let mut open = true;
    // Ctrl+Tab and Ctrl+Shift+Tab (Cmd on macOS) cycle the sections.
    let mut section = Section::from_key(&app.ui.audio_devices_section);
    let cycle = ctx.input_mut(|i| {
        use egui::{Key, KeyboardShortcut, Modifiers};
        if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab)) {
            -1
        } else if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Tab)) {
            1
        } else {
            0
        }
    });
    if cycle != 0 {
        section = section.cycled(cycle);
    }
    let r = egui::Window::new("Audio Devices")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(ctx.content_rect().center() - egui::vec2(260.0, 320.0))
        .show(ctx, |ui| {
            let sys = pick_system(&systems, draft.system.as_deref());
            let asio = sys.is_some_and(|s| s.name == devices::ASIO_SYSTEM);
            let mut controls = Vec::new();
            let size = egui::vec2((ctx.content_rect().width() - 40.0).clamp(480.0, 780.0), (ctx.content_rect().height() - 110.0).clamp(360.0, 640.0));
            ui.allocate_ui_with_layout(size, egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.set_width(size.x);
                let pending = pending_text(&draft, &app.ui.audio);
                ui.horizontal(|ui| {
                    let mut apply_button = egui::Button::new("Apply");
                    if pending.is_some() {
                        apply_button = apply_button.fill(TOKENS.accent);
                    }
                    let b = ui.add_enabled(!recording && !panel_open, apply_button);
                    controls.push(("Apply".into(), "apply", b.rect));
                    apply = b
                        .on_hover_text("Reopen the devices with these choices. Playback stops first.")
                        .on_disabled_hover_text(if panel_open { PANEL_OPEN_FIRST } else { REFUSED_RECORDING })
                        .clicked();
                    let b = ui.add_enabled(pending.is_some(), egui::Button::new("Revert"));
                    controls.push(("Revert".into(), "revert", b.rect));
                    revert = b.on_hover_text("Discard these changes (Esc).").clicked();
                    let b = ui.add_enabled(!recording && audio_on && !panel_open, egui::Button::new("Reconnect"));
                    controls.push(("Reconnect".into(), "reconnect", b.rect));
                    reconnect = b
                        .on_hover_text("Reopen the saved devices (after plugging one back in).")
                        .on_disabled_hover_text(if panel_open { PANEL_OPEN_FIRST } else { REFUSED_RECORDING })
                        .clicked();
                    let b = ui.add_enabled(audio_on && !panel_open, egui::Button::new("Rescan Devices"));
                    controls.push(("Rescan Devices".into(), "rescan", b.rect));
                    rescan = b.on_disabled_hover_text(PANEL_OPEN_FIRST).clicked();
                    if let Some(driver) = app.running_native_asio() {
                        let b = ui.add_enabled(!panel_open, egui::Button::new(DRIVER_SETTINGS));
                        controls.push((DRIVER_SETTINGS.into(), "panel", b.rect));
                        if b.on_hover_text("The open ASIO driver's own settings (buffer size, clock). Reconnect afterwards.")
                            .on_disabled_hover_text(PANEL_OPEN_FIRST)
                            .clicked()
                        {
                            native_panel = Some(driver);
                        }
                    }
                });
                if let Some(text) = &pending {
                    let shown = ui.label(egui::RichText::new(text).color(TOKENS.text).size(11.5));
                    controls.push((text.clone(), "pending", shown.rect));
                    lay.pending = Some(text.clone());
                    lay.apply_highlighted = true;
                }
                lay.opened = opened_lines(app, ui.input(|i| i.time));
                // The diagnostics sit in an inset card: the open devices, the backend figures (monospace) and the notes.
                egui::Frame::NONE
                    .fill(TOKENS.bg)
                    .corner_radius(egui::CornerRadius::same(crate::theme::RADIUS as u8))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for line in &lay.opened {
                            let text = egui::RichText::new(line).color(TOKENS.text_dim).size(11.5);
                            ui.label(if line.starts_with("Backend") { text.monospace() } else { text });
                        }
                        if !audio_on {
                            ui.label(
                                egui::RichText::new("Audio is off in this run (--no-audio): choices are saved and open at the next start.")
                                    .color(TOKENS.text_dim)
                                    .size(11.5),
                            );
                        }
                        for n in &app.audio_devices.notes {
                            ui.horizontal(|ui| {
                                let (r, _) = ui.allocate_exact_size(egui::vec2(icons::ICON, icons::ICON), egui::Sense::hover());
                                icons::paint(ui.painter(), r.center(), Icon::Warning, 14.0, TOKENS.record);
                                ui.label(egui::RichText::new(n).color(TOKENS.text).size(11.5));
                            });
                        }
                        let (rect, clicked) = app.diagnostics_button(ui);
                        controls.push(("Copy diagnostics".into(), "copy_diagnostics", rect));
                        copy_diagnostics |= clicked;
                    });
                if recording {
                    ui.label(egui::RichText::new(REFUSED_RECORDING).color(TOKENS.record).size(11.5));
                }
                ui.separator();
                let body = egui::vec2(size.x, ui.available_height());
                ui.allocate_ui_with_layout(body, egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(SIDEBAR_WIDTH);
                            for s in Section::ALL {
                                let b = ui.selectable_label(section == s, s.label());
                                controls.push((s.label().into(), s.key(), b.rect));
                                if b.clicked() {
                                    section = s;
                                }
                            }
                        });
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .id_salt(("audio_devices_section", section.key()))
                            .max_height(body.y)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                // A scroll area takes its parent's layout (here a row): the section is a column.
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.set_width(size.x - SIDEBAR_WIDTH - 32.0);
                                    match section {
                                        Section::System => {
                                            egui::Grid::new("audio_devices_grid").num_columns(2).spacing([2.0 * SPACE, SPACE]).show(ui, |ui| {
                                                let label = |ui: &mut egui::Ui, t: &str| ui.label(egui::RichText::new(t).color(TOKENS.text_dim));
                                                label(ui, "Audio system");
                                                let names: Vec<(String, String)> = systems.iter().map(|s| (s.name.clone(), s.name.clone())).collect();
                                                let before = draft.system.clone();
                                                let (rect, text) = choose(ui, "system", audio_on, &mut draft.system, &names, &mut lay.items);
                                                if draft.system != before {
                                                    // Devices belong to their system, and so does a native backend.
                                                    (draft.output, draft.input, draft.input_pairing) = (None, None, None);
                                                    if draft.backend.is_native() && !draft.backend.native_on(draft.system.as_deref()) {
                                                        draft.backend = Backend::Auto;
                                                    }
                                                }
                                                controls.push((text, "system", rect));
                                                ui.end_row();

                                                // Offered only where it can apply: a build with the native backend of this system
                                                // (WASAPI or ASIO).
                                                if devices::native_offered(draft.system.as_deref()) {
                                                    label(ui, "Backend");
                                                    let options = Backend::offered(draft.system.as_deref());
                                                    let (rect, text) = choose_backend(ui, audio_on, &mut draft.backend, &options, &mut lay.items);
                                                    controls.push((text, "backend", rect));
                                                    ui.end_row();
                                                    if draft.backend == Backend::NativeWasapi
                                                        && devices::native_wasapi_offered(draft.system.as_deref())
                                                    {
                                                        ui.label("");
                                                        let mut exclusive = !draft.share_device;
                                                        let b = ui.add_enabled(audio_on, egui::Checkbox::new(&mut exclusive, SHARE_LABEL));
                                                        draft.share_device = !exclusive;
                                                        controls.push((SHARE_LABEL.into(), "share_device", b.rect));
                                                        ui.end_row();
                                                        ui.label("");
                                                        ui.label(
                                                            egui::RichText::new(share_hint(draft.share_device)).color(TOKENS.text_dim).size(11.0),
                                                        );
                                                        ui.end_row();
                                                    }
                                                }

                                                label(ui, "Output");
                                                let outs: Vec<(String, String)> = sys
                                                    .map(|s| s.outputs.iter().map(|d| (d.name.clone(), device_label(d))).collect())
                                                    .unwrap_or_default();
                                                let (rect, text) = choose(ui, "output", audio_on, &mut draft.output, &outs, &mut lay.items);
                                                controls.push((text, "output", rect));
                                                ui.end_row();
                                                // Only the selected driver's own diagnostic is said under the list (a driver that
                                                // failed to load or open, with why); the others show nothing.
                                                let selected_output = draft
                                                    .output
                                                    .clone()
                                                    .or_else(|| sys.and_then(|s| s.outputs.iter().find(|d| d.is_default)).map(|d| d.name.clone()));
                                                let selected_problem = selected_output.as_deref().and_then(|name| {
                                                    sys.and_then(|s| {
                                                        s.diagnostics.iter().find(|p| p.strip_prefix(name).is_some_and(|r| r.starts_with(": ")))
                                                    })
                                                });
                                                if asio && let Some(text) = &asio_listing {
                                                    ui.label("");
                                                    let shown = ui.label(egui::RichText::new(text).color(TOKENS.text_dim).size(11.0));
                                                    controls.push((text.clone(), "listing", shown.rect));
                                                    lay.listing = Some(text.clone());
                                                    ui.end_row();
                                                }
                                                if let Some(problem) = selected_problem {
                                                    ui.label("");
                                                    let line = short_problem(problem);
                                                    let shown =
                                                        ui.add(egui::Label::new(egui::RichText::new(&line).color(TOKENS.text_dim).size(11.0)).wrap());
                                                    let shown = shown.on_hover_text(problem);
                                                    controls.push((line.clone(), "diagnostic", shown.rect));
                                                    lay.diagnostics.push(line);
                                                    ui.end_row();
                                                }

                                                // The chosen output (the system's default output otherwise): its rates and buffers
                                                // are offered, and its name keys a calibration.
                                                let dev = sys.and_then(|s| {
                                                    let named = draft.output.as_deref().and_then(|w| s.outputs.iter().find(|d| d.name == w));
                                                    named.or_else(|| s.outputs.iter().find(|d| d.is_default))
                                                });
                                                let native =
                                                    devices::native_wasapi_offered(draft.system.as_deref()) && draft.backend == Backend::NativeWasapi;
                                                let native_asio =
                                                    devices::native_asio_offered(draft.system.as_deref()) && draft.backend == Backend::NativeAsio;
                                                label(ui, "Input");
                                                let ins: Vec<(String, String)> = sys
                                                    .map(|s| s.inputs.iter().map(|d| (d.name.clone(), device_label(d))).collect())
                                                    .unwrap_or_default();
                                                let (rect, text) = choose(ui, "input", audio_on && !asio, &mut draft.input, &ins, &mut lay.items);
                                                controls.push((text, "input", rect));
                                                ui.end_row();
                                                if asio {
                                                    ui.label("");
                                                    ui.label(
                                                        egui::RichText::new("On ASIO the input is the output's device.")
                                                            .color(TOKENS.text_dim)
                                                            .size(11.0),
                                                    );
                                                    ui.end_row();
                                                }

                                                // The native backend pairs the output with a capture endpoint for recording, and a
                                                // loopback calibration belongs to that pair.
                                                if native {
                                                    label(ui, "Paired input");
                                                    let (rect, text) = choose_or(
                                                        ui,
                                                        "paired_input",
                                                        audio_on,
                                                        &mut draft.input_pairing,
                                                        &ins,
                                                        &mut lay.items,
                                                        PAIRED_AUTOMATIC,
                                                    );
                                                    controls.push((text, "paired_input", rect));
                                                    ui.end_row();
                                                    label(ui, "Calibration");
                                                    let rate = draft.sample_rate.or(app.player.as_ref().map(|p| p.device_rate)).unwrap_or(48_000);
                                                    let device = dev
                                                        .map(|d| d.name.as_str())
                                                        .or(app.player.as_ref().map(|p| p.device_name.as_str()))
                                                        .unwrap_or_default();
                                                    let open_pair = app
                                                        .player
                                                        .as_ref()
                                                        .filter(|p| p.device_name.eq_ignore_ascii_case(device))
                                                        .and_then(|p| p.paired_input.as_deref());
                                                    let paired = open_pair.or_else(|| app.recorder.as_ref().map(|r| r.device_name.as_str()));
                                                    let line = calibration_line(&draft, device, paired, rate, &settings_path(app));
                                                    // The bench command is long: wrap it inside the panel, with a Copy button for it.
                                                    ui.vertical(|ui| {
                                                        ui.set_max_width(ui.available_width());
                                                        let text = egui::RichText::new(&line).color(TOKENS.text_dim).size(11.5);
                                                        let shown = ui.add(egui::Label::new(text).wrap());
                                                        controls.push((line.clone(), "calibration_text", shown.rect));
                                                        if let Some(command) = calibration_command(&line) {
                                                            let copy = ui.small_button("Copy").on_hover_text("Copy the command to the clipboard");
                                                            if copy.clicked() {
                                                                ui.ctx().copy_text(command.to_string());
                                                            }
                                                            controls.push(("Copy".into(), "calibration_copy", copy.rect));
                                                        }
                                                    });
                                                    lay.calibration = Some(line);
                                                    ui.end_row();
                                                }
                                                // Native ASIO: the input is the driver's own, so there is no pairing; a calibration
                                                // is the driver's (mode `asio`), measured here through a loop cable.
                                                if native_asio {
                                                    label(ui, "Calibration");
                                                    let driver = dev
                                                        .map(|d| d.name.as_str())
                                                        .or(app.player.as_ref().map(|p| p.device_name.as_str()))
                                                        .unwrap_or_default();
                                                    // The buffer and rate the open driver runs, when the window's choices are the open ones.
                                                    let running = app
                                                        .player
                                                        .as_ref()
                                                        .filter(|p| {
                                                            !p.silent && p.device_name.eq_ignore_ascii_case(driver) && app.draft_is_open(&draft)
                                                        })
                                                        .and_then(|p| p.calibration_key.as_ref())
                                                        .map(|k| (k.period, k.rate));
                                                    let line = asio_calibration_line(&draft, driver, running);
                                                    ui.vertical(|ui| {
                                                        ui.set_max_width(ui.available_width());
                                                        let shown = ui.add(
                                                            egui::Label::new(egui::RichText::new(&line).color(TOKENS.text_dim).size(11.5)).wrap(),
                                                        );
                                                        controls.push((line.clone(), "calibration_text", shown.rect));
                                                        let (measure, drawn) = calibration_measure(
                                                            ui,
                                                            &io,
                                                            (&draft, &mut cal_send, &mut cal_ret),
                                                            cal_blocker,
                                                            cal_progress,
                                                            &cal_result,
                                                            (&mut controls, &mut lay.items),
                                                        );
                                                        cal_action = measure;
                                                        lay.calibration_measure = drawn;
                                                    });
                                                    lay.calibration = Some(line);
                                                    ui.end_row();
                                                }

                                                // An ASIO driver is described only while it is open: any other shows the standard
                                                // choices, marked, until Apply opens it.
                                                let stale = sys
                                                    .zip(dev)
                                                    .and_then(|(s, d)| devices::asio_description(s, d))
                                                    .is_some_and(|a| a != devices::AsioDescription::Probed);
                                                label(ui, "Sample rate");
                                                let rates: Vec<(u32, String)> = dev
                                                    .map_or(devices::STANDARD_RATES.to_vec(), |d| d.sample_rates.clone())
                                                    .into_iter()
                                                    .map(|r| (r, format!("{r} Hz")))
                                                    .collect();
                                                let (rect, text) = choose(ui, "rate", audio_on, &mut draft.sample_rate, &rates, &mut lay.items);
                                                controls.push((text, "rate", rect));
                                                ui.end_row();
                                                if stale {
                                                    ui.label("");
                                                    let shown = ui.label(egui::RichText::new(STALE_NOTE).color(TOKENS.text_dim).size(11.0));
                                                    controls.push((STALE_NOTE.into(), "stale_rate", shown.rect));
                                                    ui.end_row();
                                                }

                                                label(ui, "Buffer size");
                                                let sizes = dev.map_or(devices::STANDARD_BUFFERS.to_vec(), DeviceInfo::buffer_sizes);
                                                // The size the open device runs is marked in its own list.
                                                let running_buffer = app
                                                    .player
                                                    .as_ref()
                                                    .filter(|p| !p.silent && dev.is_some_and(|d| d.name.eq_ignore_ascii_case(&p.device_name)))
                                                    .and_then(|p| p.device_buffer);
                                                let bufs: Vec<(u32, String)> = sizes
                                                    .into_iter()
                                                    .map(|b| {
                                                        (
                                                            b,
                                                            if running_buffer == Some(b) {
                                                                format!("{b} samples \u{b7} running")
                                                            } else {
                                                                format!("{b} samples")
                                                            },
                                                        )
                                                    })
                                                    .collect();
                                                let (rect, text) = choose(ui, "buffer", audio_on, &mut draft.buffer_size, &bufs, &mut lay.items);
                                                controls.push((text, "buffer", rect));
                                                ui.end_row();
                                                if stale {
                                                    ui.label("");
                                                    let shown = ui.label(egui::RichText::new(STALE_NOTE).color(TOKENS.text_dim).size(11.0));
                                                    controls.push((STALE_NOTE.into(), "stale_buffer", shown.rect));
                                                    ui.end_row();
                                                }
                                                if devices::is_jack_system(draft.system.as_deref()) {
                                                    ui.label("");
                                                    let shown = ui.label(egui::RichText::new(JACK_BUFFER_NOTE).color(TOKENS.text_dim).size(11.0));
                                                    controls.push((JACK_BUFFER_NOTE.into(), "jack_buffer_note", shown.rect));
                                                    ui.end_row();
                                                    // The system's own notes, the ones that name no device ("<device>: " ones show
                                                    // under that device): once, where the buffer is chosen (#146).
                                                    for note in sys.into_iter().flat_map(|s| s.system_notes()) {
                                                        ui.label("");
                                                        let shown = ui.add(
                                                            egui::Label::new(egui::RichText::new(note).color(TOKENS.text_dim).size(11.0)).wrap(),
                                                        );
                                                        controls.push((note.to_owned(), "system_note", shown.rect));
                                                        ui.end_row();
                                                    }
                                                }
                                            });
                                        }
                                        Section::IoPaths => {
                                            io_edit = crate::audio_devices_io::section(ui, &io, wasapi, &channel_names, &mut editor, &mut io_items);
                                        }
                                        Section::Hardware => {
                                            (hw_action, hw_layout) = crate::audio_devices_io::hardware_section(
                                                ui,
                                                &io,
                                                rate,
                                                blocker,
                                                progress,
                                                &mut editor,
                                                &mut io_items,
                                            );
                                        }
                                    }
                                });
                            });
                    });
                });
            });
            lay.controls = controls.into_iter().map(|(text, key, rect)| MenuItem::at(ui, text, key.into(), rect)).collect();
            lay.controls.append(&mut io_items);
        });
    if let Some(r) = r {
        lay.window = r4(r.response.rect);
    }
    lay.section = section.key().into();
    app.ui.audio_devices_section = section.key().into();
    lay.io = Some(io);
    lay.hardware = hw_layout;
    lay.draft = draft.clone();
    lay.notes = app.audio_devices.notes.clone();
    app.audio_devices.draft = draft.clone();
    app.audio_devices.layout = lay;
    if let Some(edit) = io_edit {
        match app.run("setup.io_map", edit.params()) {
            Ok(_) => editor.done(&edit),
            Err(error) => app.status = error,
        }
    }
    (app.audio_devices.calibration_send, app.audio_devices.calibration_return) = (cal_send, cal_ret);
    match cal_action {
        Some(CalibrationAction::Measure { send, ret }) => {
            app.audio_devices.calibration_result.clear();
            if let Err(why) = app.start_loop_measure(&send, &ret) {
                app.status.clone_from(&why);
                app.audio_devices.calibration_result = why;
            }
        }
        Some(CalibrationAction::Cancel) => {
            app.audio_devices.calibration_result.clear();
            app.cancel_measure("Measurement cancelled.");
        }
        None => {}
    }
    if let Some(action) = hw_action {
        use crate::audio_devices_io::HardwareAction;
        match action {
            HardwareAction::Measure { send, ret } => {
                editor.hw_result.clear();
                if let Err(why) = app.start_measure(&send, &ret) {
                    app.status.clone_from(&why);
                    editor.hw_result = why;
                }
            }
            HardwareAction::Cancel => {
                editor.hw_result.clear();
                app.cancel_measure("Measurement cancelled.");
            }
            HardwareAction::Clear { send, ret } => {
                editor.hw_result.clear();
                let _ = app.run("io.clear_hardware_latency", serde_json::json!({"send": send, "ret": ret}));
            }
        }
    }
    app.audio_devices.io_editor = editor;
    // Escape discards the pending choices, unless a list is open (it closes that first).
    let popup_open = egui::Popup::is_any_open(ctx);
    let escape = !popup_open && !ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    if revert || (escape && app.audio_devices.draft != app.ui.audio) {
        app.audio_devices.draft = app.ui.audio.clone();
    }
    if apply {
        let _ = app.apply_audio(draft);
    }
    if reconnect {
        app.reconnect_audio_devices();
    }
    if copy_diagnostics {
        app.request_diagnostics(ctx.input(|i| i.time));
    }
    if rescan {
        app.audio_devices.forget_listing();
    }
    if let Some(driver) = native_panel {
        app.open_native_asio_panel(&driver);
    }
    if !open {
        app.cancel_measure("");
        app.audio_devices.open = false;
        app.audio_devices.layout.open = false;
    }
}
