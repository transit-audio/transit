use super::*;

impl StudioApp {
    pub(crate) fn start_play(&mut self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        self.clear_clip_latches();
        if self.refuse_for_reopen() {
            return;
        }
        if !self.sync_player_session() {
            return;
        }
        if let Some(p) = &self.player {
            // Disk-backed clips under the playhead are read before the transport moves (ADR 0035).
            prime_disk(self.engine.session(), from);
            let Some(sequence) = self.player_requests.result(&mut self.status, p.play(from, end, looped)) else { return };
            self.player_requests.play = Some(sequence);
            self.status = "Play pending".into();
        } else {
            self.sim = Some((from, end, looped));
            self.engine.transport.playing = true;
            self.engine.transport.position = from;
        }
    }

    pub(crate) fn stop_play(&mut self) {
        self.live_clear();
        if let Some(p) = &self.player {
            // Half-speed is a separate command. Stop must not leave that rate for the next Play.
            p.set_speed(1.0);
            let stopped = self.player_requests.result(&mut self.status, p.stop());
            if let Some(sequence) = stopped {
                self.player_requests.stop = Some(sequence);
                self.player_requests.play = None;
                self.player_requests.recording = None;
                self.status = "Stop pending".into();
            }
            if self.recording_active() {
                self.finish_recording();
            }
            return;
        }
        self.complete_stop();
    }

    pub(super) fn complete_stop(&mut self) {
        if self.recording_active() {
            self.finish_recording();
        }
        self.sim = None;
        self.sim_count_in = None;
        self.count_in_plan = None;
        if self.status == "Count-in" || self.status == "Record pending" {
            self.status.clear();
        }
        self.engine.transport.playing = false;
        self.engine.transport.recording = false;
        // Adapted: the cursor stays where it was (Ableton), so Stop then Play starts the same
        // place again; the stopped position is the cursor.
        self.engine.transport.position = self.cursor();
    }

    /// Adapted from `SoundApp::play_from_selection`: play from the cursor; with Cycle on and a
    /// selection, loop the selection (the engine's loop range is the selection, plan 06 L6).
    pub fn play_from_cursor(&mut self) {
        if let Some(sel) = self.wanted_loop() {
            self.start_play(sel.start, None, Some(sel));
        } else {
            let s = self.engine.session();
            let preroll = if s.edit.pre_post_roll { s.edit.pre_roll } else { 0 };
            let at = self.cursor();
            self.start_play((at - preroll).max(0), None, None);
        }
    }

    /// Move a running playhead (a click on the ruler or the timeline while playing).
    pub(crate) fn locate_playing(&mut self, at: Samples) {
        if !self.is_playing() {
            return;
        }
        prime_disk(self.engine.session(), at);
        if let Some(p) = &self.player
            && let Some(sequence) = self.player_requests.result(&mut self.status, p.locate(at))
        {
            self.player_requests.locate = Some(sequence);
        }
        if let Some(sim) = &mut self.sim {
            sim.0 = at;
        }
        if self.player.is_none() {
            self.engine.transport.position = at;
        }
    }

    /// The range Cycle loops, as Play starts it: the cycle range while Cycle is on.
    pub(crate) fn wanted_loop(&self) -> Option<Range> {
        let edit = &self.engine.session().edit;
        let cycle = edit.cycle_range();
        (edit.loop_playback && !cycle.is_empty()).then_some(cycle)
    }

    /// #81: Cycle going on or off, or the cycle range changing, reaches a running (or starting)
    /// player at once, like Ableton: the playhead is not moved. Inside the new range it wraps at
    /// the range end; before it, it plays on and wraps on reaching the end; off, it plays through.
    /// The same range Play loops (`play_from_cursor`), so Stop and Play changes nothing.
    ///
    /// Not while a take records: its laps are cut by the loop it started with (`take_cycle`), so
    /// the player keeps that loop until Stop, and a change made meanwhile lands when the take ends
    /// if the transport is still running.
    fn retarget_loop(&mut self) {
        let want = self.wanted_loop();
        if want == self.loop_seen || self.recording_active() {
            return;
        }
        self.loop_seen = want;
        if !self.is_playing() && self.player_requests.play.is_none() {
            return;
        }
        if let Some(p) = &self.player {
            let _ = self.player_requests.result(&mut self.status, p.set_loop(want));
        } else if let Some(sim) = &mut self.sim {
            sim.2 = want;
        }
    }

    pub(crate) fn handle_transport(&mut self, dt: f32) {
        self.push_metronome_level();
        // The count-in is over (or was cancelled): the status line says what happens next.
        if let Some(failure) = self.player.as_ref().and_then(Player::take_start_failure)
            && self.recording_active()
        {
            // The count-in ended but Record could not start: nothing is recording, so release the
            // take and the recorder arm, and say why.
            self.finish_recording();
            self.status = format!("Recording could not start: {}", failure.reason());
        }
        if self.status == "Count-in" && self.count_in_state().is_none() && self.sim_count_in.is_none() {
            self.status = if self.recording_active() { "Recording".into() } else { String::new() };
        }
        let mut reqs: std::collections::VecDeque<TransportRequest> = std::mem::take(&mut self.engine.transport_requests).into();
        while let Some(r) = reqs.pop_front() {
            match r {
                TransportRequest::ResetSession => {
                    self.restore_pending = true;
                    if let Some(p) = &self.player {
                        if let Some(sequence) = self.player_requests.result(&mut self.status, p.stop()) {
                            self.player_requests.stop = Some(sequence);
                        }
                        if let Some(sequence) = self.player_requests.result(&mut self.status, p.set_recording(false)) {
                            self.player_requests.recording = Some(sequence);
                        }
                        p.set_speed(1.0);
                    }
                    self.sim = None;
                    // The old capture belongs to the replaced document, never to the new one.
                    self.close_input();
                    self.recorder_failed = false;
                    self.punch = None;
                    self.arrange.gesture = None;
                    self.copied.clear();
                    self.meters.clear();
                    self.main_meter = MeterDisplay::default();
                    // The cursor is per-session view state.
                    self.ui.cursor = 0;
                    self.ui.cursor_track = None;
                    self.ui.focus_track = None;
                    self.mixer.gesture = None;
                    self.auto.gesture = None;
                    self.auto.selection.clear();
                    self.engine.transport.position = self.cursor();
                    prime_disk(self.engine.session(), self.engine.transport.position);
                    if self.sync_player_session()
                        && let Some(p) = &self.player
                        && let Some(sequence) = self.player_requests.result(&mut self.status, p.locate(self.engine.transport.position))
                    {
                        self.player_requests.locate = Some(sequence);
                    }
                }
                TransportRequest::Play => {
                    if self.count_in_state().is_some() {
                        // Play during a record count-in cancels it: no half-armed take.
                        self.stop_play();
                    } else if !self.is_playing() && self.player_requests.play.is_none() {
                        self.play_from_cursor();
                    }
                }
                TransportRequest::Stop => {
                    // Stop during a count-in only cancels it; the cursor stays where it was.
                    let was_stopped = !self.is_playing() && !self.engine.transport.recording && self.count_in_state().is_none();
                    self.stop_play();
                    if was_stopped {
                        // First Stop keeps the play-start cursor and finishes the take. A second
                        // Stop locates through the engine, without adding an undo step. Handle
                        // that locate before any later queued Play/Record request.
                        let _ = self.run("transport.rtz", json!({}));
                        for request in std::mem::take(&mut self.engine.transport_requests).into_iter().rev() {
                            reqs.push_front(request);
                        }
                    }
                }
                TransportRequest::Pause => self.stop_play(),
                TransportRequest::TogglePlay => {
                    if self.is_playing() || self.player_requests.play.is_some() || self.count_in_state().is_some() {
                        self.stop_play();
                    } else {
                        self.play_from_cursor();
                    }
                }
                TransportRequest::Record => {
                    if self.recording_active() {
                        self.stop_play();
                    } else {
                        self.start_recording();
                    }
                }
                TransportRequest::Locate(at) => {
                    // Adapted: a locate command (Return to Zero, `transport.locate`) also moves the cursor.
                    self.set_cursor(at, None);
                    self.locate_playing(at);
                    self.engine.transport.position = at;
                }
                TransportRequest::PlaySelection => {
                    if self.count_in_state().is_some() {
                        // Like Play: a count-in is cancelled, never turned into an unarmed play.
                        self.stop_play();
                    } else {
                        let sel = self.engine.session().edit.selection;
                        self.start_play(sel.start, Some(sel.end.max(sel.start + 1)), None);
                    }
                }
                TransportRequest::HalfSpeed => {
                    if self.count_in_state().is_some() {
                        self.stop_play();
                    } else {
                        if let Some(p) = &self.player {
                            p.set_speed(0.5);
                        }
                        self.play_from_cursor();
                    }
                }
                TransportRequest::Scrub(at) => self.engine.transport.position = at,
                TransportRequest::AllNotesOff => {
                    self.live_clear();
                    if let Some(p) = &self.player {
                        if let Some(sequence) = self.player_requests.result(&mut self.status, p.all_notes_off()) {
                            self.player_requests.release = Some(sequence);
                            self.status = "All MIDI notes off pending".into();
                        }
                    } else {
                        self.status = "All MIDI notes off".into();
                    }
                }
            }
        }
        self.retarget_loop();
        // Read back the position.
        if let Some(p) = &self.player {
            let playing = p.is_playing();
            let ended = self.engine.transport.playing && !playing;
            if ended {
                p.set_speed(1.0);
            }
            self.engine.transport.playing = playing;
            self.engine.transport.position = p.position();
            crate::debug_log::track_run_counts(&mut self.run_counts, p, playing);
            crate::debug_log::track_midi_outputs(&mut self.midi_out_connected, p);
            let now = self.last_frame.unwrap_or_default();
            if now - self.last_meter_poll >= 1.0 / 20.0 {
                self.player_requests.result(&mut self.status, p.copy_meters(&mut self.meter_snapshot));
                self.last_meter_poll = now;
            }
            let snap = &self.meter_snapshot;
            for (id, m) in &snap.tracks {
                let d = self.meters.entry(*id).or_default();
                feed_meter(d, m.peak, m.gain_reduction_db, dt);
                if m.plugin_failures.count != d.plugin_failures {
                    d.plugin_failures = m.plugin_failures.count;
                    if let Some(msg) = failed_plugin_status(self.engine.session(), *id, m.plugin_failures.last) {
                        self.status = msg;
                    }
                }
            }
            let main = snap.main.peak;
            feed_meter(&mut self.main_meter, main, 0.0, dt);
            if ended && self.player_requests.stop.is_none() {
                self.complete_stop();
            }
        } else if let Some((left, play)) = &mut self.sim_count_in {
            *left -= (f64::from(dt) * self.engine.session().sample_rate.as_f64()) as i64;
            if *left <= 0 {
                let (from, end, looped) = *play;
                self.sim_count_in = None;
                self.start_play(from, end, looped);
            }
        } else if let Some((pos, end, looped)) = &mut self.sim {
            let sr = self.engine.session().sample_rate.as_f64();
            *pos += (f64::from(dt) * sr) as i64;
            if let Some(l) = looped
                && *pos >= l.end
            {
                *pos = l.start;
            }
            let stop = end.is_some_and(|e| *pos >= e);
            self.engine.transport.position = *pos;
            if stop {
                self.stop_play();
            }
        }
        if !self.meters_live() {
            for d in self.meters.values_mut() {
                feed_meter(d, [0.0, 0.0], 0.0, dt);
            }
            feed_meter(&mut self.main_meter, [0.0, 0.0], 0.0, dt);
        }
    }
}

#[cfg(test)]
#[path = "live_cycle_tests.rs"]
mod live_cycle_tests;

/// Prime the disk-backed clips the playhead reaches from `at` (ADR 0035): the transport waits for
/// their rings as it waits for a plan. A clip still late plays silence, counted as a disk underrun.
fn prime_disk(s: &transit_model::Session, at: Samples) {
    let late = transit_model::disk::prime_session(s, at);
    if late > 0 {
        log::info!("{late} disk-backed clips were not ready to play from {at}");
    }
}
