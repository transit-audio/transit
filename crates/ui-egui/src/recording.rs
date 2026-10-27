use super::*;

impl SoundApp {
    pub(super) fn start_recording(&mut self) {
        let armed = self.engine.session().tracks.iter().any(|t| t.mixer.record_arm);
        if !armed {
            let why = if self.engine.session().tracks.is_empty() {
                "Nothing to record on yet: add a track with Track › New…, then click its R button."
            } else {
                "No track is record-enabled: click the R button on the track to record."
            };
            self.ui.status = why.into();
            self.record_hint = Some((self.last_frame.unwrap_or_default(), why));
            return;
        }
        self.ensure_input();
        let takes = match self.arm_recorder() {
            Ok(takes) => takes,
            Err(err) => {
                self.ui.status = format!("Cannot record: {err}");
                return;
            }
        };
        self.lost_shown = 0;
        let already_playing = self.is_playing();
        self.engine.transport.recording = self.player.is_none();
        if already_playing {
            // Punch in on the fly at the current playhead.
            self.record_start = self.position();
            self.punch = None;
            self.plan_recording(takes, None, None);
            return;
        }
        let e = self.engine.session().edit.clone();
        let cycle = e.cycle_range();
        let sel = e.selection;
        if e.loop_playback && !cycle.is_empty() {
            // Cycle on: Record loops the range, each lap a take, until Stop.
            self.record_start = cycle.start;
            self.punch = None;
            self.plan_recording(takes, Some(cycle.len()), Some((cycle.start, None, Some(cycle))));
        } else {
            // Record into the selection when there is one (punch), with pre/post-roll if enabled;
            // the take is trimmed back to the punch range afterwards.
            let (pre, post) = if e.pre_post_roll { (e.pre_roll, e.post_roll) } else { (0, 0) };
            let from = (sel.start - pre).max(0);
            self.record_start = from;
            self.punch = (!sel.is_empty()).then_some(sel);
            let end = (!sel.is_empty()).then_some(sel.end + post);
            self.plan_recording(takes, None, Some((from, end, None)));
        }
    }

    /// Remember where the armed `takes` go (split into passes of `pass` when loop recording) and
    /// autosave that plan with the session.
    fn plan_recording(
        &mut self,
        takes: Vec<transit_engine::io::Take>,
        pass: Option<Samples>,
        play: Option<(Samples, Option<Samples>, Option<Range>)>,
    ) {
        let rate = self.recorder.as_ref().map_or(0, |r| r.sample_rate);
        self.take_plan = Some(transit_engine::io::Recording {
            start: self.record_start,
            pass,
            punch: self.punch,
            rate,
            takes,
            midi_journal: None,
            midi_placed: false,
        });
        if !self.sync_player_session() {
            self.finish_recording();
            return;
        }
        if let Some(p) = &self.player {
            let Some(recorder) = self.recorder.as_ref() else {
                self.finish_recording();
                return;
            };
            let (from, end, looped) = play.map_or((None, None, None), |(from, end, looped)| (Some(from), end, looped));
            let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.record(recorder, from, end, looped)) else {
                self.finish_recording();
                return;
            };
            self.player_requests.recording = Some(sequence);
            if play.is_some() {
                self.player_requests.play = Some(sequence);
            }
            self.ui.status = "Record pending".into();
        } else if let Some((from, end, looped)) = play {
            self.start_play(from, end, looped);
        }
        if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Arm the input for the record-enabled tracks; a session never saved records into its
    /// autosave folder. Returns the takes it records.
    fn arm_recorder(&mut self) -> Result<Vec<transit_engine::io::Take>, String> {
        if self.recorder_failed {
            return Err(self.ui.status.clone());
        }
        let name = transit_engine::io::sanitize_name(&self.engine.session().name);
        let unsaved_dir = self.autosave_dir.as_ref().map_or_else(std::env::temp_dir, |d| d.join(name));
        let Some(r) = &mut self.recorder else { return Err("no audio input device".into()) };
        let plan = transit_engine::io::plan_takes(&self.engine, &unsaved_dir, r.channels);
        if plan.is_empty() {
            return Err("no audio track is record-enabled".into());
        }
        let takes =
            plan.iter().map(|(track, inputs, path)| transit_engine::io::Take { track: *track, path: path.clone(), channels: inputs.len() }).collect();
        if let Some(player) = &self.player {
            player.prepare_recording(r, plan)?;
        } else {
            r.arm(plan)?;
        }
        Ok(takes)
    }

    pub(super) fn finish_recording(&mut self) {
        if self.player.is_none() {
            self.engine.transport.recording = false;
        }
        if let Some(p) = &self.player
            && let Some(sequence) = self.player_requests.result(&mut self.ui.status, p.set_recording(false))
        {
            self.player_requests.recording = Some(sequence);
        }
        let plan = self.take_plan.take();
        let Some(r) = &mut self.recorder else { return };
        if !r.has_pending_take() {
            return;
        }
        let (takes, problem) = r.finish();
        let tracks = takes.len().max(1);
        let mut plan = plan.unwrap_or_else(|| transit_engine::io::Recording {
            start: self.record_start,
            pass: None,
            punch: self.punch,
            rate: 0,
            takes: Vec::new(),
            midi_journal: None,
            midi_placed: false,
        });
        if let Some(point) = self.recorder.as_ref().and_then(transit_playback::record::Recorder::applied_start) {
            plan.start = point.position.sample;
            self.record_start = plan.start;
        }
        self.punch = None;
        self.autosaved_rev = 0;
        let ids = transit_engine::io::place_recording(&mut self.engine, &plan, takes);
        self.ui.status =
            if plan.pass.is_some() { format!("Loop-recorded {} take(s)", ids.len() / tracks) } else { format!("Recorded {} clip(s)", ids.len()) };
        if let Some(p) = problem {
            log::warn!("recording: {p}");
            self.ui.status = format!("{} ({p})", self.ui.status);
        }
    }
}

#[cfg(test)]
mod tests;
