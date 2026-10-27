use super::*;

impl StudioApp {
    /// Stop the transport, keeping a take in progress, when the audio output fails.
    pub(crate) fn watch_output(&mut self) {
        let Some(p) = &self.player else { return };
        let Some(trouble) = p.trouble() else { return };
        let name = p.device_name.clone();
        let (cause, then) = match trouble {
            OutputTrouble::Bug => ("the audio engine hit an internal error (see the log)".to_string(), "Use Audio › Reconnect Device, or restart."),
            OutputTrouble::Gone => (format!("“{name}” was disconnected"), "Plug it back in, then use Audio › Reconnect Device."),
            OutputTrouble::Stalled => (format!("“{name}” stopped asking for audio"), "Use Audio › Reconnect Device."),
            // The driver's settings moved under the stream (a buffer size or rate changed in its
            // panel): the output is silent until the device is reopened.
            OutputTrouble::ResetRequested => (transit_playback::RESET_REQUESTED_TEXT.to_string(), "Use Audio › Reconnect Device."),
        };
        let (recording, playing) = (self.recording_active(), self.engine.transport.playing);
        // A driver that asked to be reset is reopened by the app (`step_reopen`); a take in
        // progress ends here and is kept, never spliced onto the reopened stream.
        if let Some(cause) = p.reopen_cause() {
            self.stop_for_reopen();
            let now = self.last_frame.unwrap_or_default();
            if self.reopen.trouble(cause, now) || recording || playing {
                self.status = reopen_note(&self.reopen, recording, playing);
            }
            return;
        }
        if recording || playing {
            self.stop_play();
        }
        self.status = if recording {
            format!("Recording stopped: {cause}. The take is kept up to there. {then}")
        } else if playing {
            format!("Playback stopped: {cause}. {then}")
        } else {
            format!("Audio output: {cause}. {then}")
        };
    }

    /// Stop recording, keeping the take, when the input device or the disk fails.
    pub(crate) fn watch_input(&mut self) {
        let recording = self.recording_active();
        let Some(r) = &mut self.recorder else { return };
        let Some(trouble) = r.trouble() else {
            let lost = r.lost_ms();
            if recording && lost > self.lost_shown {
                self.lost_shown = lost;
                self.status = format!("The disk is too slow: this take has lost {lost} ms of input so far.");
            }
            return;
        };
        if recording {
            self.stop_play();
        }
        let (what, then) = match trouble {
            Trouble::Device(what) => {
                self.close_input();
                self.recorder_failed = true;
                // The input's half of a driver reset: the device reopens with both directions.
                if let Some(cause) = self.player.as_ref().and_then(Player::reopen_cause) {
                    self.stop_for_reopen();
                    let now = self.last_frame.unwrap_or_default();
                    if self.reopen.trouble(cause, now) || recording {
                        self.status = reopen_note(&self.reopen, recording, false);
                    }
                    return;
                }
                (what, "Reconnect it, then use Audio › Reconnect Device.")
            }
            Trouble::Disk(what) => (what, "Check the disk, then record again."),
        };
        self.status = if recording {
            format!("Recording stopped: {what}. The take is kept up to there. {then}")
        } else {
            format!("Audio input: {what}. {then}")
        };
    }

    /// Reopen audio with editors closed before teardown. Never interrupt a recording.
    pub fn reconnect_audio(&mut self) {
        match self.reconnect_audio_using(true, Player::with_config) {
            Ok(captured) => {
                self.reopen.reconnected();
                if !captured {
                    self.status = STATE_FROM_DOCUMENT.into();
                }
            }
            Err(error) => self.player_requests.fail(&mut self.status, error),
        }
    }

    /// Refuses a transport start, saying why, while the device waits to be reopened after a driver
    /// reset or stall, or after the automatic reopen gave up (until the user reconnects): the
    /// runtime can play nothing then, and Play must not just do nothing.
    pub(crate) fn refuse_for_reopen(&mut self) -> bool {
        let waiting = self.player.as_ref().is_some_and(|p| p.reopen_cause().is_some());
        if !waiting && !self.reopen.gave_up() {
            return false;
        }
        let what = self.reopen.cause().map_or("driver reset", ReopenCause::text);
        self.status = if self.reopen.gave_up() {
            format!("The audio device did not reopen after the {what}. Use Audio › Reconnect Device.")
        } else {
            format!("The audio device is reopening after the {what}; try again in a moment.")
        };
        true
    }

    /// Stops the transport of a Player whose driver asked to be reset, for good: a take in
    /// progress ends and is kept (`stop_play`), and since its faulted runtime acknowledges nothing
    /// the transport stops here, and the requests it refused are not shown as the user's errors.
    fn stop_for_reopen(&mut self) {
        if self.recording_active() || self.engine.transport.playing {
            self.stop_play();
        }
        self.complete_stop();
        self.player_requests.reset();
        self.player_requests.error = None;
    }

    /// The automatic reopen after a driver reset ([`transit_playback::reopen`]): once the
    /// attempt is due, and never while the driver's settings panel holds the driver or a take is
    /// still being finished, the device is reopened with the same choices through the one reopen
    /// path (`reconnect_audio_using`), which also takes the input again for monitoring and arming.
    pub(crate) fn step_reopen(&mut self) {
        let now = self.last_frame.unwrap_or_default();
        // The cause holds for the life of the Player: a report another check consumed first
        // still starts the reopen.
        if let Some(cause) = self.player.as_ref().and_then(Player::reopen_cause)
            && self.reopen.trouble(cause, now)
        {
            let (recording, playing) = (self.recording_active(), self.is_playing());
            self.stop_for_reopen();
            self.status = reopen_note(&self.reopen, recording, playing);
        }
        // The backend is asked only once an attempt is due, not every frame.
        if !self.reopen.due(now, false) {
            return;
        }
        let blocked = self.audio_devices.panel.is_some() || self.player.as_ref().is_some_and(|p| p.host_blocked(&self.ui.audio));
        if blocked {
            return;
        }
        if self.recording_active() {
            // A take started since (on a runtime that took no work): it ends before the device goes.
            self.stop_for_reopen();
        }
        let result = self.reconnect_with_player(true);
        let player = self.player.as_ref();
        let opened = result.is_ok() && player.is_some_and(|p| !p.silent && p.reopen_cause().is_none());
        self.reopen.attempted(opened, now);
        let name = player.map_or_else(String::new, |p| p.device_name.clone());
        let what = self.reopen.cause().map_or("driver reset", ReopenCause::text);
        let why = match &result {
            Err(error) => error.clone(),
            Ok(_) => player.and_then(Player::initial_error).unwrap_or("it did not open").to_owned(),
        };
        self.status = if opened {
            format!("“{name}” reopened after the {what}.")
        } else if self.reopen.gave_up() {
            format!(
                "The device did not reopen after the {what} ({why}), after {} attempts. Use Audio › Reconnect Device.",
                transit_playback::reopen::REOPEN_ATTEMPTS
            )
        } else {
            format!("The device did not reopen after the {what} yet ({why}); trying again.")
        };
        if matches!(result, Ok(false)) {
            self.status.push(' ');
            self.status.push_str(STATE_FROM_DOCUMENT);
        }
    }

    /// [`Self::reconnect_audio_using`] with the Player the app opens (in tests, the scripted one
    /// `reopen_with` holds, when set).
    pub(crate) fn reconnect_with_player(&mut self, preserve: bool) -> Result<bool, String> {
        #[cfg(test)]
        if let Some(open) = self.reopen_with.take() {
            let result = self.reconnect_audio_using(preserve, |session, cfg| open(session, cfg));
            self.reopen_with = Some(open);
            return result;
        }
        self.reconnect_audio_using(preserve, Player::with_config)
    }

    pub(crate) fn reconnect_audio_using(
        &mut self,
        preserve: bool,
        open: impl FnOnce(std::sync::Arc<transit_model::Session>, &transit_playback::AudioConfig) -> Player,
    ) -> Result<bool, String> {
        if self.recording_active() {
            return Err(crate::audio_devices::REFUSED_RECORDING.into());
        }
        if self.player.is_none() && self.audio_devices.fake.is_none() {
            return Ok(true);
        }
        let captured = !preserve || self.preserve_audio_state()?;
        // The one place the Player is replaced: a measurement on the old one can never finish.
        self.cancel_measure(crate::hardware_measure::CANCELLED_REOPEN);
        self.cancel_external_measure(crate::external_measure::CANCELLED_REOPEN);
        if self.is_playing() {
            self.stop_play();
        }
        // A device reopen closes native editors deliberately; they remain wanted. Give the new
        // instances a fresh loading deadline instead of treating their missing windows as user closes.
        self.editors.clear();
        for key in self.ui.plugin_windows.clone() {
            self.want_editor(key);
        }
        self.recorder_failed = false;
        let cfg = self.ui.audio.clone();
        if let Some(f) = &mut self.audio_devices.fake {
            let trace = std::rc::Rc::clone(&f.trace);
            crate::audio_connection::replace(&mut f.connection, &mut f.input, || crate::audio_connection::FakeConnection::new(cfg.clone(), trace))?;
            f.reopened.push(cfg);
        } else {
            let session = self.engine.session_arc();
            crate::audio_connection::replace(&mut self.player, &mut self.recorder, || open(session, &cfg))?;
            self.player_requests.reset();
        }
        if self.engine.session().needs_live_input() {
            self.ensure_input();
        }
        self.audio_devices.begin_verification();
        self.io_adopt = true;
        Ok(captured)
    }
}

/// Said when a reopen could not take the plug-ins' live state and used the document's.
const STATE_FROM_DOCUMENT: &str = "Audio recovered from the document state; live plug-in state was unavailable after the audio fault.";

/// The status line when a driver reset (or a driver that stopped calling back) stops the device
/// and the app reopens it.
fn reopen_note(reopen: &transit_playback::reopen::Reopener, recording: bool, playing: bool) -> String {
    let what = reopen.cause().map_or("driver reset", ReopenCause::text);
    let next = if reopen.gave_up() {
        format!("The device did not reopen after {} attempts; use Audio › Reconnect Device.", transit_playback::reopen::REOPEN_ATTEMPTS)
    } else {
        "Reopening the device.".to_owned()
    };
    if recording {
        format!("Recording ended: {what}. The take is kept up to there. {next}")
    } else if playing {
        format!("Playback stopped: {what}. {next}")
    } else {
        let mut what = what.to_owned();
        if let Some(first) = what.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        format!("{what}. {next}")
    }
}
