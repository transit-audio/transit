//! Frame lifecycle and UI command dispatch.

use crate::*;

impl StudioApp {
    /// Plan 06, L7: this UI offers neither Shuffle nor Spot, so edit commands run as Slip while the
    /// document keeps the mode it was saved with. Only the commands see the override.
    fn slip_override(&self, id: &str) -> bool {
        use transit_model::EditMode;
        let edits = id.starts_with("edit.") || id.starts_with("clip.") || id.starts_with("midi.");
        edits
            && !matches!(id, "edit.mode" | "edit.undo" | "edit.redo")
            && matches!(self.engine.session().edit.edit_mode, EditMode::Shuffle | EditMode::Spot)
    }

    /// Run a command; errors go to the status line. Plug-in state is captured before saves.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.before_height_command(id);
        if matches!(id, "transport.stop" | "transport.pause") {
            if !self.midi_transport.dispatching
                && let Some(player) = &self.player
            {
                player.cancel_midi_buttons();
            }
            self.midi_transport.take_locate = None;
            self.midi_transport.deferred.clear();
            self.midi_transport.waiting.clear();
        }
        if id == "transport.record" && self.midi_transport.take_locate.take().is_some() {
            self.midi_transport.deferred.clear();
            self.midi_transport.waiting.clear();
            return Ok(json!({}));
        }
        if matches!(id, "edit.undo" | "edit.redo")
            && let Some(reason) = self.history_disabled_reason()
        {
            self.status = reason.into();
            return Err(reason.into());
        }
        if matches!(id, "edit.undo" | "edit.redo") {
            self.midi_transport.record_pass_pushes = None;
        }
        // A real-time render (ADR 0027) owns the transport while it plays, and runs on the Player.
        if let Some(r) = self.realtime_intercept(id, &params) {
            return r;
        }
        if (self.audio_devices.measuring.is_some() || self.external_measure.running.is_some())
            && matches!(
                id,
                "transport.play"
                    | "transport.toggle"
                    | "transport.record"
                    | "transport.half_speed"
                    | "transport.play_selection"
                    | "edit.play_timeline"
            )
        {
            // Playback would bleed into the return the measurement is listening on.
            self.status = if self.external_measure.running.is_some() {
                crate::external_measure::REFUSED_TRANSPORT
            } else {
                crate::hardware_measure::REFUSED_TRANSPORT
            }
            .into();
            return Err(self.status.clone());
        }
        // Cycle on makes a region where the playhead is, so the engine is told where that is.
        let mut params = params;
        if id == "options.loop_playback"
            && let Some(o) = params.as_object_mut()
        {
            o.entry("at").or_insert_with(|| self.position().into());
        }
        if id == "ui.set" {
            return self.set_ui(&params);
        }
        if id.starts_with("preview.") {
            return self.preview_command(id, &params);
        }
        if id == "help.check_updates" {
            self.check_for_updates_now()?;
            return Ok(json!({"checking": true}));
        }
        if id == "window.midi_mappings" {
            return Ok(self.toggle_midi_mappings(params.get("value").and_then(Value::as_bool)));
        }
        if id == "options.low_latency" {
            return self.set_low_latency(&params);
        }
        if id == "query.path_latency" {
            self.poll_low_latency();
        }
        if id == "window.editors_follow" {
            return Ok(self.set_editors_follow(params.get("value").and_then(Value::as_bool)));
        }
        if id == "window.close_editors" {
            return Ok(self.close_all_editors());
        }
        if matches!(id, "window.collapse_editors" | "window.expand_editors") {
            return Ok(self.collapse_all_editors(id == "window.collapse_editors"));
        }
        if id == "mix.monitor" {
            return self.monitor_command(&params);
        }
        if id == "window.midi_monitor" {
            return Ok(self.toggle_midi_monitor(params.get("value").and_then(Value::as_bool)));
        }
        if id == "help.debug_logging" {
            return self.set_debug_logging(params.get("on").and_then(Value::as_bool));
        }
        if id == "io.measure_hardware_latency" {
            return self.measure_hardware_latency(&params);
        }
        if id == "instrument.measure_external" {
            return self.measure_external_command(&params);
        }
        if id == "track.clear_clip_indicators" {
            self.clear_clip_latches();
            return Ok(json!({"cleared": true}));
        }
        if matches!(id, "plugin.retry" | "plugin.retry_all") {
            return self.retry_plugins(id, &params);
        }
        if finish::refuses(self, id) {
            return Err(self.status.clone());
        }
        if id.starts_with("session.save")
            || id.starts_with("file.bounce")
            || matches!(id, "track.bounce" | "clip.bounce" | "track.commit" | "track.freeze" | "track.flatten")
        {
            // Third-party plugins keep state the session can't see: store it first.
            self.capture_plugin_states()?;
        }
        // Any edit command (selection included) ends a header selection; `focus_header` sets it
        // again after its own `edit.select`.
        if id.starts_with("edit.") || id.starts_with("clip.") || id.starts_with("midi.") {
            self.tracks_selected_by_header = false;
        }
        let replacing = matches!(id, "session.new" | "session.open" | "session.close" | "session.revert");
        if replacing && self.recording_active() {
            self.stop_play();
            self.close_autosave();
        }
        let result = if id == "track.change_width" {
            self.change_track_width(&params)
        } else if self.slip_override(id) {
            self.engine.execute_with_mode(id, &params, transit_model::EditMode::Slip)
        } else {
            self.engine.execute(id, &params)
        };
        match result {
            Ok(v) => {
                if replacing {
                    // Collapsed groups and open take lanes belong to the session that was open.
                    self.ui.collapsed.clear();
                    self.reset_take_lanes_for_session();
                    self.ui.auto_shown.clear();
                    self.ui.auto_lanes.clear();
                    self.auto.selection.clear();
                    self.auto.highlight = None;
                    self.finish.rename = None;
                    self.bounce_place = bounce_in_place::State::default();
                    if matches!(self.dialog, Some(Dialog::BounceInPlace | Dialog::HiddenTracks)) {
                        self.dialog = None;
                    }
                }
                if replacing || (id.starts_with("session.save") && !self.engine.is_dirty()) {
                    self.release_autosave(false);
                }
                if matches!(id, "session.open" | "session.save" | "session.save_as") {
                    self.note_recent();
                }
                if id == "app.quit" {
                    self.quit_requested = true;
                }
                Ok(v)
            }
            Err(e) => {
                self.status = e.to_string();
                log::warn!("{id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// `ui.set`: patch [`UiState`] fields. Plug-in folders are not changed this way.
    pub(super) fn set_ui(&mut self, p: &Value) -> Result<Value, String> {
        let mut cur = serde_json::to_value(&self.ui).map_err(|e| e.to_string())?;
        if let (Some(obj), Some(patch)) = (cur.as_object_mut(), p.as_object()) {
            for (k, v) in patch {
                if matches!(k.as_str(), "audio" | "keys")
                    && let (Some(current), Some(fields)) = (obj.get_mut(k).and_then(Value::as_object_mut), v.as_object())
                {
                    current.extend(fields.iter().map(|(k, v)| (k.clone(), v.clone())));
                    continue;
                }
                obj.insert(k.clone(), v.clone());
            }
        }
        // A test seam, not state: `imports_pace` fakes a slow prepare (frames per file, at most
        // 600; null off). Applied only once the rest of the request is accepted.
        let pace = match p.get("imports_pace") {
            None => None,
            Some(v) if v.is_null() => Some(None),
            Some(v) => match v.as_u64().and_then(|n| u32::try_from(n).ok()).filter(|n| *n <= MAX_IMPORTS_PACE) {
                Some(n) => Some(Some(n)),
                None => return Err(format!("imports_pace must be null or 0-{MAX_IMPORTS_PACE} frames")),
            },
        };
        let ns: UiState = serde_json::from_value(cur).map_err(|e| format!("bad ui state: {e}"))?;
        audio_devices::validate(&ns.audio)?;
        if ns.plugin_dirs != self.ui.plugin_dirs {
            return Err("plugin folders can't be changed over the control channel".into());
        }
        let audio_changed = ns.audio != self.ui.audio;
        if audio_changed && self.recording_active() {
            return Err(audio_devices::REFUSED_RECORDING.into());
        }
        if let Some(pace) = pace {
            self.imports.pace = pace;
        }
        let mut ns = ns;
        (ns.cursor, ns.cursor_track, ns.focus_track) = (self.ui.cursor, self.ui.cursor_track, self.ui.focus_track);
        ns.collapsed = std::mem::take(&mut self.ui.collapsed);
        ns.takes_open = std::mem::take(&mut self.ui.takes_open);
        ns.midi_fx_sections = std::mem::take(&mut self.ui.midi_fx_sections);
        ns.auto_shown = std::mem::take(&mut self.ui.auto_shown);
        ns.auto_lanes = std::mem::take(&mut self.ui.auto_lanes);
        let before = std::mem::replace(&mut self.ui, ns);
        if before.midi_transport != self.ui.midi_transport {
            self.engine.set_midi_transport(self.ui.midi_transport.clone());
            if let Some(player) = &self.player {
                player.cancel_midi_buttons();
            }
        }
        // Editors no longer listed close; newly listed ones open (retried until loaded).
        for key in before.plugin_windows.iter().filter(|k| !self.ui.plugin_windows.contains(k)) {
            if let Some(p) = &self.player {
                self.player_requests.result(&mut self.status, p.close_editor(key.0, key.1));
            }
            self.editors.retain(|e| e.key != *key);
        }
        let wanted: Vec<(TrackId, usize)> = self.ui.plugin_windows.clone();
        for key in wanted {
            self.want_editor(key);
        }
        // A new device choice takes effect now, as the window's Apply does.
        if audio_changed && let Err(error) = self.reopen_audio() {
            self.ui.audio = before.audio;
            return Err(error);
        }
        Ok(json!({"ui": self.ui}))
    }

    /// Per-frame logic, once per frame even when egui requests another layout pass.
    ///
    /// Ordering contract:
    /// 1. Drain control requests before polling player replies and editor changes so commands
    ///    can make progress in this frame. Poll before reading meters, MIDI and editor state.
    /// 2. Apply wheel gestures before ending merges; automation owns its merge until Stop.
    /// 3. Finish background bounce/import work before reconciling transport and the document.
    /// 4. Handle transport before publishing a replacement session, and wait for outstanding
    ///    play/locate/record admissions before syncing I/O and session state.
    /// 5. Capture automation before autosaving, then open any newly needed input and schedule
    ///    the next frame. Collect screenshots last, after the frame's state changes.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if ctx.current_pass_index() > 0 {
            return;
        }
        if self.theme_applied != Some(self.ui.appearance) {
            theme::apply(ctx, self.ui.appearance);
            self.theme_applied = Some(self.ui.appearance);
        }
        theme::follow(ctx);
        if self.zoom_applied != Some(self.ui.ui_zoom) {
            self.ui.apply_interface_zoom(ctx);
            self.zoom_applied = Some(self.ui.ui_zoom);
        }
        let now = ctx.input(|i| i.time);
        let dt = self.last_frame.replace(now).map_or(1.0 / 60.0, |t| (now - t) as f32).clamp(0.0, 0.25);
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;
        control::drain(self, ctx);
        let plugin_requests = service_plugin_requests();
        // E7: plug-in editors only work while `Player::idle` runs every frame.
        self.apply_editor_edits();
        self.poll_player();
        self.step_measure();
        self.step_external_measure();
        if self.audio_devices.measuring.is_some() || self.external_measure.running.is_some() {
            ctx.request_repaint();
        }
        self.update_midi_live();
        self.keys.step.chromatic = ctx.input(|i| i.modifiers.shift);
        self.step_poll();
        editor_group::drive(self, ctx);
        self.drive_editors();
        self.drive_plugin_headers();
        theme::probe_fonts(ctx);
        let wheeling = self.drive_wheel(ctx, now) | self.drive_tempo_wheel(ctx, now);
        // A write pass (slice 7) holds its merge until Stop.
        if !wheeling
            && !automation::passing(self)
            && !self.midi_learn.sweeping()
            && !ctx.input(|i| i.pointer.any_down())
            && !ctx.egui_wants_keyboard_input()
            && !self.builtin_editor.devices.wheeling
        {
            self.engine.end_merge();
        }
        self.watch_output();
        self.watch_input();
        self.step_reopen();
        self.save_good_audio();
        finish::step_bounce(self, ctx);
        import::step(self, ctx);
        crate::drops::step(self, ctx);
        // Stop the old playback before sending a replacement session to the audio thread.
        self.handle_transport(dt);
        automation::write_pass(self, ctx);
        if self.player_requests.play.is_none() && self.player_requests.locate.is_none() && self.player_requests.recording.is_none() {
            self.sync_io();
            self.sync_player_session();
            self.poll_low_latency();
        }
        self.autosave_changes();
        self.autosave();
        if self.recorder.is_none() && !self.recorder_failed && self.engine.session().needs_live_input() {
            self.ensure_input();
        }
        // Autosave only runs on frames, and plugin editors are native windows egui never hears from.
        // egui takes its predicted frame time off every delay, so add it back.
        let editors = self.player.as_ref().is_some_and(Player::any_editor_open)
            || !self.editors.is_empty()
            || self.scope_open()
            || self.plugin_header.any_busy();
        // A plug-in being made is created one frame after it is announced; come back for it.
        let loading = self.player.as_ref().is_some_and(Player::any_loading);
        ctx.request_repaint_after_secs(
            if editors || plugin_requests || loading || self.player_requests.pending() { 0.03 } else { 1.0 } + ctx.input(|i| i.predicted_dt),
        );
        self.player_requests.show_error(&mut self.status);
        control::collect_screenshots(self, ctx);
    }

    /// Feed queued synthetic input (from the control channel) into egui: one pointer event per
    /// frame (so drags span frames like real ones), keys as press+release pairs, text at once.
    /// Copied from `transit-ui-egui` (`SoundApp::raw_input_hook`, transit 2af868e), plus the
    /// modifiers a synthetic gesture holds (`ui.drag {alt: true}`) applied to the frame's input.
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        arrange::track_height::bound_raw_wheel(raw);
        if !self.synthetic.is_empty() {
            self.synthetic_grace = 6;
        }
        if self.synthetic_grace > 0 {
            self.synthetic_grace -= 1;
            raw.events.retain(|e| {
                !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone | egui::Event::MouseMoved(_))
            });
        }
        let Some(first) = self.synthetic.first().cloned() else {
            if let Some(m) = self.synthetic_mods.take() {
                raw.events.insert(0, egui::Event::ModifiersChanged(m));
            }
            return;
        };
        let take = match first {
            egui::Event::Key { pressed: true, .. } => {
                if matches!(self.synthetic.get(1), Some(egui::Event::Key { pressed: false, .. })) {
                    2
                } else {
                    1
                }
            }
            _ => 1,
        };
        let n = take.min(self.synthetic.len());
        let evs: Vec<egui::Event> = self.synthetic.drain(..n).collect();
        let mut release = false;
        for e in &evs {
            match e {
                egui::Event::PointerButton { pos, pressed, modifiers, .. } => {
                    if *pressed {
                        self.synthetic_mods = Some(*modifiers);
                    } else {
                        release = true;
                    }
                    // Keep the hover position current for widgets that check it.
                    raw.events.push(egui::Event::PointerMoved(*pos));
                }
                egui::Event::PointerMoved(pos) => raw.events.push(egui::Event::PointerMoved(*pos)),
                // A key press holds its modifiers for the frame, as a real keyboard reports them.
                egui::Event::Key { pressed, modifiers, .. } => {
                    if *pressed {
                        self.synthetic_mods = Some(*modifiers);
                    } else {
                        release = true;
                    }
                }
                _ => {}
            }
        }
        if let Some(m) = self.synthetic_mods {
            raw.events.insert(0, egui::Event::ModifiersChanged(m));
        }
        if release {
            // Held through this frame's release, then let go on the next.
            self.synthetic_mods = Some(egui::Modifiers::NONE);
        }
        raw.events.extend(evs.into_iter().filter(|e| !matches!(e, egui::Event::PointerMoved(_))));
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        theme::follow(&ctx);
        ui.set_style(ctx.global_style());
        midi_learn::keys(self, &ctx); // an armed MIDI Learn takes Escape first
        if let Some(delay) = self.midi_learn_feed(ctx.input(|i| i.time)) {
            ctx.request_repaint_after(delay);
        }
        self.midi_transport_feed();
        if self.player.is_some() {
            ctx.request_repaint_after(midi_learn::IDLE_POLL);
        }
        finish::rename_keys(self, &ctx); // slice 6: a name being typed owns Escape and Enter
        tempo_edit::keys(self, &ctx); // the tempo and meter fields own Escape and Enter
        clip_inspector::keys(self, &ctx); // Escape in a fade field gives its draft up
        crate::device_editor::window::keys(self, &ctx); // 326-M: Escape closes the focused device editor
        keys::shortcuts(self, &ctx); // slice 3: before the arrangement's keys
        automation::shortcuts(self, &ctx); // slice 7: selected points take Delete before clips
        groups::shortcuts(self, &ctx);
        browser::shortcuts(self, &ctx);
        finish::shortcuts(self, &ctx);
        menus::shortcuts(self, &ctx);
        if !ctx.egui_wants_keyboard_input() && self.dialog.is_none() && shortcuts::pressed(&ctx, shortcuts::Action::ToggleInfo) {
            self.help.toggle();
        }
        self.help.layout.question_marks.clear();
        menus::menu_bar(self, ui);
        arrange::track_height::boundaries(self, &ctx);
        transport::bar(self, ui);
        self.update_tick(&ctx);
        menus::status_bar(self, ui);
        browser::panel(self, ui); // slice 4: left of the panes and the arrangement
        // Slice 3: the piano roll replaces the mixer and device chain panes while it is open.
        let roll = keys::pane(self, ui);
        if !roll {
            mixer::panes(self, ui);
        }
        clip_inspector::panel(self, ui, roll);
        splash::advance_open(self, &ctx);
        if splash::update(self, &ctx) {
            splash::show(self, ui);
        } else if self.ui.mixer_full {
            mixer::show_full(self, ui);
        } else {
            arrange::show(self, ui);
        }
        import::paint(self, &ctx);
        finish::rename_field(self, &ctx);
        menus::dialogs(self, &ctx);
        finish::bounce_dialog(self, &ctx);
        bounce_in_place::dialog(self, &ctx);
        finish::history_window(self, &ctx);
        menus::recovery(self, &ctx);
        host_hang::window(self, &ctx);
        menus::shortcuts_window(self, &ctx);
        about::window(self, &ctx);
        telemetry::window(self, &ctx);
        audio_devices::window(self, &ctx);
        midi_monitor::window(self, &ctx);
        midi_learn::window(self, &ctx);
        builtin_editor::windows(self, &ctx);
        arrange::track_height::boundaries(self, &ctx);
        help::panel(self, &ctx); // last, so it sees the control hovered in this frame
        crate::chrome::focus_ring(&ctx); // over everything, around the widget the keyboard focused
        if self.is_playing() || self.engine.transport.recording {
            ctx.request_repaint_after(std::time::Duration::from_millis(1000 / 30));
        } else if self.main_meter.level[0] > 0.0001 {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        } else if !self.pending_shots.is_empty() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }
}
