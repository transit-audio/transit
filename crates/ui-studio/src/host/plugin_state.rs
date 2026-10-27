use super::*;

impl StudioApp {
    /// Synchronize the immutable snapshot, including view-only COW changes.
    /// Revision alone cannot establish the lineage required by state capture.
    pub(crate) fn sync_player_session(&mut self) -> bool {
        let Some(p) = &self.player else {
            self.restore_pending = false;
            self.last_rev = self.engine.revision;
            return true;
        };
        // ResetSession must first stop/close old-document transport and input.
        if self.engine.transport_requests.contains(&TransportRequest::ResetSession) {
            return false;
        }
        // A Player whose driver asked to be reset is about to be replaced (`step_reopen`), with
        // the session as it is then; its faulted runtime takes no more work.
        if p.reopen_cause().is_some() {
            return false;
        }
        let session = self.engine.session_arc();
        if !self.restore_pending && self.last_rev == self.engine.revision && p.has_requested_session(&session) {
            return true;
        }
        let result = if self.restore_pending { p.replace_session(session) } else { p.update_session(session) };
        if matches!(result, Err(transit_playback::PlayerError::NeedsReopen { .. })) {
            if let Err(error) = self.reconnect_with_player(false) {
                self.player_requests.fail(&mut self.status, error);
                return false;
            }
            // A device that opens again here clears a reopen that gave up (or is pending), as
            // Reconnect and Apply do: the new Player runs.
            if self.player.as_ref().is_some_and(|p| !p.silent && p.reopen_cause().is_none()) {
                self.reopen.reconnected();
            }
            if self.player.as_ref().is_some_and(|p| p.has_requested_session(&self.engine.session_arc())) {
                self.restore_pending = false;
                self.last_rev = self.engine.revision;
                return true;
            }
            return false;
        }
        if self.player_requests.result(&mut self.status, result).is_none() {
            return false;
        }
        self.restore_pending = false;
        self.last_rev = self.engine.revision;
        true
    }

    /// Reads every live third-party plugin's state and stores it on its insert
    /// (`mix.insert_state`), so saves, bounces and autosaves include it.
    pub fn capture_plugin_states(&mut self) -> Result<(), String> {
        // Saving must not supersede the prerequisite of an admitted Play/Locate.
        let action_pending = self.player_requests.play.is_some() || self.player_requests.locate.is_some() || self.player_requests.recording.is_some();
        let changed = self.player.as_ref().is_some_and(|p| !p.has_requested_session(&self.engine.session_arc()));
        if (action_pending && changed) || !self.sync_player_session() {
            return Err("The audio session is still updating; try again after it applies".into());
        }
        let Some(p) = &self.player else { return Ok(()) };
        let states = p.capture_states(&self.engine.session_arc());
        let busy = p.take_unrefreshed_states();
        self.store_plugin_states(states)?;
        self.note_unrefreshed(busy);
        Ok(())
    }

    /// Plug-ins a capture left out (stuck, or held by a modal dialog): their stored state goes
    /// into the save, the render or the reopened device (issue #56). The status line says so;
    /// [`Self::state_notice`] keeps it for a status that replaces this one in the same action.
    fn note_unrefreshed(&mut self, busy: Vec<String>) {
        self.state_notice = (!busy.is_empty()).then(|| format!("State of {} not refreshed: plug-in busy", busy.join(", ")));
        if let Some(notice) = &self.state_notice {
            self.status = notice.clone();
        }
    }

    /// `status`, followed by the notice of plug-ins the last capture left out, if any (once).
    pub(crate) fn with_state_notice(&mut self, status: String) -> String {
        match self.state_notice.take() {
            Some(notice) => format!("{status} ({notice})"),
            None => status,
        }
    }

    pub(super) fn preserve_audio_state(&mut self) -> Result<bool, String> {
        let Some(player) = &self.player else { return Ok(true) };
        let states = player.capture_states_for_reopen(&self.engine.session_arc());
        let busy = player.take_unrefreshed_states();
        let states = self.player_requests.result(&mut self.status, states).ok_or_else(|| self.status.clone())?;
        let Some(states) = states else { return Ok(false) };
        self.store_plugin_states(Ok(states))?;
        self.note_unrefreshed(busy);
        Ok(true)
    }

    fn store_plugin_states(&mut self, states: Result<transit_playback::PluginStates, transit_playback::PlayerError>) -> Result<(), String> {
        let states = self.player_requests.result(&mut self.status, states).ok_or_else(|| self.status.clone())?;
        for (t, slot, data) in states {
            let slot = if slot == transit_mix::INSTRUMENT_SLOT { json!("instrument") } else { json!(slot) };
            let state = transit_model::b64::encode(&data);
            if let Err(e) = self.engine.execute("mix.insert_state", &json!({"track": t.0, "slot": slot, "state": state})) {
                self.status = format!("Cannot store plugin state: {e}");
                return Err(self.status.clone());
            }
        }
        Ok(())
    }

    /// Parameter edits made in plugin editors go into the session (one undo step per gesture).
    /// Calls `Player::idle`, which hosted editors need every frame (E7).
    pub(crate) fn apply_editor_edits(&mut self) {
        let discard = self.restore_pending || self.engine.transport_requests.contains(&TransportRequest::ResetSession);
        let Some(p) = &self.player else { return };
        let Some(edits) = self.player_requests.result(&mut self.status, p.idle()) else { return };
        if discard {
            return;
        }
        self.apply_edits(edits);
    }

    /// Records plug-in parameter edits: into the session, and as a touch of the parameter's
    /// automation lane. The hosts report values only (VST3 `performEdit` and CLAP parameter
    /// events arrive without their begin/end-edit gestures), so a run of edits less than
    /// `HOSTED_HOLD_S` apart is one gesture: Write, Touch and Latch record it like a fader move
    /// and Touch lets go that long after the last value.
    pub(crate) fn apply_edits(&mut self, edits: Vec<(transit_model::TrackId, usize, String, f32)>) {
        for (t, slot, param, value) in edits {
            // A hosted plug-in re-reports a value it already has (a keep-alive, a rounded echo):
            // only a changed value is a touch, or the held lane would fill with identical points.
            let unchanged = crate::automation::Lane::of_param(t, slot, &param).is_some_and(|(track, lane)| {
                self.engine.session().track(track).is_some_and(|tr| (lane.static_value(tr) - value).abs() <= 1e-6 * value.abs().max(1.0))
            });
            if !unchanged {
                // "Learn next touched parameter" takes the first changed parameter it sees.
                self.learn_touched(t, slot, &param);
            }
            if !unchanged {
                self.touch_param(t, slot, &param, true);
            }
            // A hosted instrument's edits go into the session like an insert's (K83), so a mapped
            // controller picks up from where the editor left the parameter.
            let r = if slot == transit_mix::INSTRUMENT_SLOT {
                let key = format!("instrument_param:{}:{param}", t.0);
                self.engine.execute_merged("mix.instrument_param", &json!({"track": t.0, "param": param, "value": value}), &key)
            } else {
                let key = format!("param:{}:{slot}:{param}", t.0);
                self.engine.execute_merged("mix.insert_param", &json!({"track": t.0, "slot": slot, "param": param, "value": value}), &key)
            };
            if let Err(e) = r
                && !unchanged
            {
                log::debug!("plug-in edit not stored: track {} slot {slot} param {param}: {e}", t.0);
            }
        }
    }
}

#[cfg(test)]
#[path = "plugin_state_tests.rs"]
mod plugin_state_tests;
