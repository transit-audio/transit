use super::*;

impl SoundApp {
    pub fn is_playing(&self) -> bool {
        self.player.as_ref().map_or(self.engine.transport.playing, Player::is_playing)
    }

    /// Why the last Record press did nothing, for a few seconds and while it still holds.
    pub(crate) fn record_hint(&self) -> Option<&'static str> {
        let (at, why) = self.record_hint?;
        let fresh = self.last_frame.unwrap_or_default() - at < 4.0;
        (fresh && !self.engine.session().tracks.iter().any(|t| t.mixer.record_arm)).then_some(why)
    }

    /// Current transport position (playhead while playing, else the edit insertion).
    pub fn position(&self) -> Samples {
        if let Some(p) = &self.player
            && p.is_playing()
        {
            return p.position();
        }
        if self.engine.transport.playing { self.engine.transport.position } else { self.engine.session().edit.selection.start }
    }

    pub(super) fn start_play(&mut self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        if !self.sync_player_session() {
            return;
        }
        if let Some(p) = &self.player {
            let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.play(from, end, looped)) else { return };
            self.player_requests.play = Some(sequence);
            self.ui.status = "Play pending".into();
        } else {
            self.sim = Some((from, end, looped));
            self.engine.transport.playing = true;
            self.engine.transport.position = from;
        }
        self.last_write_at = i64::MIN;
        // A fresh start resumes the playhead follow: clear a manual hold and
        // re-sync the follow tracker so the current view is not mistaken for
        // an outside move on the first frame.
        self.edit_layout.follow_hold = false;
        self.edit_layout.last_scroll = self.engine.session().edit.zoom.scroll;
        self.edit_layout.last_follow_to = None;
    }

    pub(super) fn stop_play(&mut self) {
        if let Some(p) = &self.player {
            // Half-speed is a separate command. Stop must not leave that rate for the next Play.
            p.set_speed(1.0);
            let stopped = self.player_requests.result(&mut self.ui.status, p.stop());
            if let Some(sequence) = stopped {
                self.player_requests.stop = Some(sequence);
                self.player_requests.play = None;
                self.player_requests.recording = None;
                self.ui.status = "Stop pending".into();
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
        self.engine.transport.playing = false;
        self.engine.transport.recording = false;
        let stop_at = self.engine.transport.position;
        // Snapshot the session flags first: `session_mut` below needs a
        // mutable borrow, so no session borrow may span it.
        let (follow_insertion, after_playback) = {
            let s = self.engine.session();
            (s.edit.insertion_follows_playback, s.edit.scrolling == "after_playback")
        };
        if follow_insertion {
            let pos = self.engine.transport.position;
            self.engine.session_mut().edit.selection = Range::point(pos);
        }
        if after_playback {
            // After Playback never scrolls while playing, so bring the stopped
            // position into view now (only if it is off-screen); every other
            // mode keeps its view.
            let at = stop_at.max(0);
            let (scroll, spp) = {
                let z = &self.engine.session().edit.zoom;
                (z.scroll, z.samples_per_px.max(0.01))
            };
            let width = f64::from(self.edit_layout.timeline[2] - self.edit_layout.timeline[0]).max(0.0);
            let end = scroll.saturating_add((width * spp) as Samples);
            if at < scroll || at >= end {
                let _ = self.engine.execute("view.scroll", &json!({ "to": at }));
            }
        }
        self.edit_layout.follow_hold = false;
    }

    /// Play from the selection, honouring loop playback and selection end.
    pub fn play_from_selection(&mut self) {
        let s = self.engine.session();
        let sel = s.edit.selection;
        let preroll = if s.edit.pre_post_roll { s.edit.pre_roll } else { 0 };
        let postroll = if s.edit.pre_post_roll { s.edit.post_roll } else { 0 };
        if s.edit.loop_playback && !sel.is_empty() {
            self.start_play(sel.start, None, Some(sel));
        } else if !sel.is_empty() {
            self.start_play((sel.start - preroll).max(0), Some(sel.end + postroll), None);
        } else {
            self.start_play((sel.start - preroll).max(0), None, None);
        }
    }

    pub(super) fn handle_transport(&mut self, dt: f32) {
        let reqs: Vec<TransportRequest> = std::mem::take(&mut self.engine.transport_requests);
        for r in reqs {
            match r {
                TransportRequest::ResetSession => {
                    self.restore_pending = true;
                    if let Some(p) = &self.player {
                        if let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.stop()) {
                            self.player_requests.stop = Some(sequence);
                        }
                        if let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.set_recording(false)) {
                            self.player_requests.recording = Some(sequence);
                        }
                        p.set_speed(1.0);
                    }
                    self.sim = None;
                    // The old capture belongs to the replaced document, never to the new one.
                    self.close_input();
                    self.recorder_failed = false;
                    self.punch = None;
                    self.gesture = None;
                    self.touched.clear();
                    self.last_write_at = i64::MIN;
                    self.edit_layout.follow_hold = false;
                    self.meters.clear();
                    self.main_meter = MeterDisplay::default();
                    if self.sync_player_session()
                        && let Some(p) = &self.player
                        && let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.locate(self.engine.transport.position))
                    {
                        self.player_requests.locate = Some(sequence);
                    }
                }
                TransportRequest::Play => {
                    if !self.is_playing() && self.player_requests.play.is_none() {
                        self.play_from_selection();
                    }
                }
                TransportRequest::Stop | TransportRequest::Pause => self.stop_play(),
                TransportRequest::TogglePlay => {
                    if self.is_playing() || self.player_requests.play.is_some() {
                        self.stop_play();
                    } else {
                        self.play_from_selection();
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
                    if self.is_playing() {
                        if let Some(p) = &self.player
                            && let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.locate(at))
                        {
                            self.player_requests.locate = Some(sequence);
                        }
                        if let Some(sim) = &mut self.sim {
                            sim.0 = at;
                        }
                        self.last_write_at = i64::MIN;
                    }
                    self.engine.transport.position = at;
                }
                TransportRequest::PlaySelection => {
                    let sel = self.engine.session().edit.selection;
                    self.start_play(sel.start, Some(sel.end.max(sel.start + 1)), None);
                }
                TransportRequest::HalfSpeed => {
                    if let Some(p) = &self.player {
                        p.set_speed(0.5);
                    }
                    self.play_from_selection();
                }
                TransportRequest::Scrub(at) => self.engine.transport.position = at,
                TransportRequest::AllNotesOff => {
                    if let Some(p) = &self.player {
                        if let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.all_notes_off()) {
                            self.player_requests.release = Some(sequence);
                            self.ui.status = "All MIDI notes off pending".into();
                        }
                    } else {
                        self.ui.status = "All MIDI notes off".into();
                    }
                }
            }
        }
        // Read back the position.
        if let Some(p) = &self.player {
            let playing = p.is_playing();
            let ended = self.engine.transport.playing && !playing;
            if ended {
                p.set_speed(1.0);
            }
            self.engine.transport.playing = playing;
            self.engine.transport.position = p.position();
            let now = self.last_frame.unwrap_or_default();
            let meter_interval = if self.ui.window == MainWindow::Mix { 1.0 / 60.0 } else { 1.0 / 20.0 };
            if now - self.last_meter_poll >= meter_interval {
                self.player_requests.result(&mut self.ui.status, p.copy_meters(&mut self.meter_snapshot));
                self.last_meter_poll = now;
            }
            let snap = &self.meter_snapshot;
            for (id, m) in &snap.tracks {
                let d = self.meters.entry(*id).or_default();
                feed_meter(d, m.peak, m.gain_reduction_db, dt);
                if m.plugin_failures.count != d.plugin_failures {
                    d.plugin_failures = m.plugin_failures.count;
                    if let Some(msg) = failed_plugin_status(self.engine.session(), *id, m.plugin_failures.last) {
                        self.ui.status = msg;
                    }
                }
            }
            let main = snap.main.peak;
            feed_meter(&mut self.main_meter, main, 0.0, dt);
            if ended && self.player_requests.stop.is_none() {
                self.complete_stop();
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
mod tests;
