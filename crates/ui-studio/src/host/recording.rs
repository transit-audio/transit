use super::*;

impl StudioApp {
    /// Adapted from `SoundApp::start_recording`: without Cycle, recording starts at the cursor
    /// (with pre-roll when it is on) and runs until Stop; it does not punch into the selection,
    /// which in this UI is usually a clicked clip. With Cycle on, it loop-records
    /// the cycle range, one take per lap, until Stop.
    pub(crate) fn start_recording(&mut self) {
        let armed = self.engine.session().tracks.iter().any(|t| t.mixer.record_arm);
        if !armed {
            let why = if self.engine.session().tracks.is_empty() {
                "Nothing to record on yet: add a track, then click its R button."
            } else {
                "No track is record-enabled: click the R button on the track to record."
            };
            self.status = why.into();
            self.record_hint = Some((self.last_frame.unwrap_or_default(), why));
            return;
        }
        if self.audio_armed() {
            self.ensure_input();
        }
        let takes = match self.arm_recorder() {
            Ok(takes) => takes,
            Err(err) => {
                self.status = format!("Cannot record: {err}");
                return;
            }
        };
        self.lost_shown = 0;
        self.engine.transport.recording = self.player.is_none();
        self.begin_pass(takes);
    }

    /// Start the recording pass for `takes`: on the fly at the playhead when already playing, else
    /// from the count-in or pre-roll, or the cycle range.
    pub(crate) fn begin_pass(&mut self, takes: Vec<transit_engine::io::Take>) {
        let already_playing = self.is_playing();
        if already_playing {
            // Punch in on the fly at the current playhead.
            let at = self.position();
            self.record_start = at;
            self.punch = None;
            // Cycling already: the pass is the loop, so every lap is a take.
            // (The per-pass path cuts from the latch in whole laps, so it needs the latch at the
            // loop start; from inside the loop the wrapped-recording path cuts the laps instead.)
            let e = &self.engine.session().edit;
            let sel = e.cycle_range();
            let pass = (e.loop_playback && !sel.is_empty() && at == sel.start).then(|| sel.len());
            self.plan_recording(takes, pass, None, None);
            return;
        }
        let e = self.engine.session().edit.clone();
        let sel = e.cycle_range();
        let cycle = e.loop_playback && !sel.is_empty();
        if cycle {
            // Cycle on: Record loops the cycle range and keeps recording lap after lap until Stop,
            // each lap its own take (Cubase, Live). Punching out at the loop end, which Cycle used to
            // do, stopped the recording at the first wrap.
            self.record_start = sel.start;
            self.punch = None;
            let count_in = self.count_in_for(sel.start);
            self.plan_recording(takes, Some(sel.len()), Some((sel.start, None, Some(sel))), count_in);
        } else {
            let at = self.cursor();
            let from = (at - self.lead_in(at)).max(0);
            self.record_start = from;
            self.punch = None;
            // The count-in clicks with the transport parked at `from`, needing no room before it.
            let count_in = self.count_in_for(from);
            self.plan_recording(takes, None, Some((from, None, None)), count_in);
        }
    }

    /// How long Record plays before `at`: the pre-roll when it is on. (The count-in is separate:
    /// [`StudioApp::count_in_for`].) Never before the session start.
    pub(crate) fn lead_in(&self, at: Samples) -> Samples {
        let s = self.engine.session();
        let _ = at;
        if s.edit.pre_post_roll { s.edit.pre_roll } else { 0 }
    }

    /// Remember where the armed `takes` go (split into passes of `pass` when loop recording) and
    /// autosave that plan with the session.
    fn plan_recording(
        &mut self,
        takes: Vec<transit_engine::io::Take>,
        pass: Option<Samples>,
        play: Option<(Samples, Option<Samples>, Option<Range>)>,
        count_in: Option<CountIn>,
    ) {
        let rate = self.recorder.as_ref().map_or(0, |r| r.sample_rate);
        self.record_clock = None;
        // The loop this pass records under; Cycle edits made while it runs wait for its end (#81).
        self.take_cycle = self.wanted_loop();
        self.take_plan = Some(transit_engine::io::Recording {
            start: self.record_start,
            pass,
            punch: self.punch,
            rate,
            takes,
            midi_journal: self.midi_journal.as_ref().map(|j| j.path().to_path_buf()),
            midi_placed: false,
        });
        if self.refuse_for_reopen() || !self.sync_player_session() {
            self.finish_recording();
            return;
        }
        if let Some(p) = &self.player {
            let (from, end, looped) = play.map_or((None, None, None), |(from, end, looped)| (Some(from), end, looped));
            let _ = p.take_start_failure();
            let started = if self.take_plan.as_ref().is_some_and(|plan| plan.takes.is_empty()) && p.midi_recording_prepared() {
                // MIDI only: the latch is the capture pass's own.
                p.record_midi_with_count_in(from, end, looped, count_in)
            } else if let Some(recorder) = self.recorder.as_ref() {
                p.record_with_count_in(recorder, from, end, looped, count_in)
            } else {
                self.finish_recording();
                return;
            };
            let Some(sequence) = self.player_requests.result(&mut self.status, started) else {
                self.finish_recording();
                return;
            };
            self.player_requests.recording = Some(sequence);
            if play.is_some() {
                self.player_requests.play = Some(sequence);
            }
            self.status = if count_in.is_some() && from.is_some() { "Count-in".into() } else { "Record pending".into() };
            self.count_in_plan = count_in.filter(|_| from.is_some());
        } else if let Some((from, end, looped)) = play {
            match count_in {
                // No audio device: the same count-in, timed by the frame clock and silent.
                Some(plan) => {
                    self.sim_count_in = Some((plan.frames, (from, end, looped)));
                    self.count_in_plan = Some(plan);
                    self.status = "Count-in".into();
                }
                None => self.start_play(from, end, looped),
            }
        }
        if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Is an audio track record-enabled (so the input device must be opened and armed)?
    fn audio_armed(&self) -> bool {
        self.engine.session().tracks.iter().any(|t| t.mixer.record_arm && t.kind == transit_model::TrackKind::Audio && !t.inactive)
    }

    /// The record-enabled MIDI and instrument tracks: their live input is recorded as clips.
    fn midi_armed(&self) -> Vec<TrackId> {
        self.engine.session().tracks.iter().filter(|t| t.mixer.record_arm && t.kind.is_midi() && !t.inactive).map(|t| t.id).collect()
    }

    /// Arm the input for the record-enabled tracks; a session never saved records into its
    /// autosave folder. Returns the audio takes it records; armed MIDI tracks start a capture
    /// pass that shares the audio take's record-start latch.
    fn arm_recorder(&mut self) -> Result<Vec<transit_engine::io::Take>, String> {
        let midi = self.midi_armed();
        let mut takes = Vec::new();
        let audio = self.audio_armed();
        if audio || midi.is_empty() {
            takes = self.arm_audio_takes()?;
        }
        if !midi.is_empty() {
            match &self.player {
                Some(player) => {
                    let latch = if audio { self.recorder.as_ref().and_then(transit_playback::record::Recorder::start_reader) } else { None };
                    player.prepare_midi_recording(midi, latch)?;
                    self.midi_journal = Some(transit_engine::midi_journal::Writer::new(self.midi_journal_path()));
                }
                None if !audio => return Err("no audio device is open to record MIDI".into()),
                None => {}
            }
        }
        Ok(takes)
    }

    /// Where this pass's MIDI journal goes: beside the audio takes (the `Audio Files` of the
    /// session's folder, or its folder under the autosave root until it is saved).
    fn midi_journal_path(&self) -> std::path::PathBuf {
        let name = transit_engine::io::sanitize_name(&self.engine.session().name);
        let unsaved = self.autosave_dir.as_ref().map_or_else(std::env::temp_dir, |d| d.join(&name));
        let home = self.engine.path.as_deref().and_then(|p| std::path::Path::new(p).parent()).unwrap_or(&unsaved);
        transit_engine::midi_journal::journal_path(&home.join("Audio Files"), &self.engine.session().name)
    }

    /// Append the events and render-overrun marks (ADR 0029) of the pass the journal hasn't
    /// taken yet. Control thread; the audio side only pushes. A failure is reported once and the
    /// pass carries on.
    fn journal_midi(&mut self, take: &transit_playback::MidiTake, plan: &transit_engine::io::Recording, flush: bool) {
        let (Some(journal), Some(point)) = (self.midi_journal.as_mut(), take.start) else { return };
        let s = self.engine.session();
        let latch = transit_engine::midi_journal::Latch {
            output_frame: point.output_frame,
            position: point.position.sample,
            device_rate: take.device_rate,
            session_rate: s.sample_rate.hz(),
        };
        // The loop the take runs under (`take_cycle`), not the live Cycle state: recovery cuts laps by it (#85).
        let loop_range = self.take_cycle;
        let mut plan = plan.clone();
        plan.start = point.position.sample;
        let header = transit_engine::midi_journal::Header::new(latch, &plan, loop_range, take.tracks.clone());
        let from = journal.consumed().min(take.events.len());
        let records: Vec<_> = take.events[from..].iter().filter_map(journal_record).collect();
        // Render-worker overruns while the pass recorded (ADR 0029) go in as marks, never events.
        let marks = self.player.as_ref().map(transit_playback::Player::midi_recording_overruns).unwrap_or_default();
        let mut written = journal.append(&header, &records, take.events.len()).and_then(|()| journal.mark(&header, marks));
        if flush && written.is_ok() {
            written = journal.flush();
        }
        if let Err(e) = written {
            log::warn!("MIDI journal: {e}");
            self.status = format!("MIDI take journal failed ({e}); the pass is still recording");
        }
    }

    fn arm_audio_takes(&mut self) -> Result<Vec<transit_engine::io::Take>, String> {
        if self.recorder_failed {
            return Err(self.status.clone());
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

    pub(crate) fn recording_active(&self) -> bool {
        self.engine.transport.recording
            || self.take_plan.is_some()
            || self.recorder.as_ref().is_some_and(transit_playback::record::Recorder::has_pending_take)
    }

    /// Rebuild the provisional MIDI clip and notes of a pass under way from what the audio thread
    /// has captured so far. Control side only; with a long event list it runs every other frame.
    pub(crate) fn update_midi_live(&mut self) {
        self.observe_recording();
        let take = self.player.as_ref().and_then(Player::midi_recording_snapshot);
        #[cfg(test)]
        let take = take.or_else(|| self.finished_test_midi.clone());
        let (Some(take), Some(plan)) = (take, self.take_plan.clone()) else {
            self.midi_live = None;
            return;
        };
        self.journal_midi(&take, &plan, false);
        self.midi_live_tick = self.midi_live_tick.wrapping_add(1);
        if take.events.len() > 2_000 && self.midi_live_tick % 2 == 1 && self.midi_live.is_some() {
            return;
        }
        let Some(latch) = take.start else {
            self.midi_live = None;
            return;
        };
        let events: Vec<_> = take.timed(self.engine.session().sample_rate.hz()).iter().filter_map(|(offset, e)| rec_event(*offset, e)).collect();
        let start = latch.position.sample;
        let now = start + self.recording_elapsed(start);
        let cut = transit_engine::midi_take::Cut { start, pass: plan.pass, punch: plan.punch, stop: now - start };
        let s = self.engine.session();
        let cycle = self.take_cycle.unwrap_or_default();
        let wrap = self.take_cycle.filter(|_| plan.pass.is_none());
        let mut built = transit_engine::midi_take::build_wrapping(&s.tempo, s.sample_rate, &events, &cut, wrap);
        // After a wrap only the lap under way is drawn, from the loop start to the playhead.
        let (start, end) = match self.record_clock.filter(|c| c.laps > 0 && wrap.is_some()) {
            Some(c) => {
                let lap = usize::try_from(c.laps / cycle.len().max(1)).unwrap_or(0);
                built.retain(|b| b.pass == lap);
                (cycle.start, c.last)
            }
            None => (start, now),
        };
        self.midi_live = Some(MidiLive { tracks: take.tracks.clone(), start, end, built });
    }

    /// Follow the transport while a pass runs, counting each wrap of the cycle it records under
    /// (`take_cycle`) as a lap.
    pub(crate) fn observe_recording(&mut self) {
        if self.take_plan.is_none() || !self.is_playing() {
            return;
        }
        let at = self.position();
        let cycle = self.take_cycle;
        let clock = self.record_clock.get_or_insert(RecordClock { last: at, laps: 0 });
        if let Some(cycle) = cycle
            && at < clock.last
        {
            clock.laps += cycle.len();
        }
        clock.last = at;
    }

    /// Samples from the latch `start` to where the pass has got to, laps included. Never the
    /// position at Stop: that wrapped back into the loop, or is the edit cursor once stopped.
    fn recording_elapsed(&self, start: Samples) -> Samples {
        self.record_clock.map_or(0, |c| (c.last + c.laps - start).max(0))
    }

    pub(super) fn finish_recording(&mut self) {
        self.observe_recording();
        self.midi_live = None;
        if self.player.is_none() {
            self.engine.transport.recording = false;
        }
        if let Some(p) = &self.player
            && let Some(sequence) = self.player_requests.result(&mut self.status, p.set_recording(false))
        {
            self.player_requests.recording = Some(sequence);
        }
        let plan = self.take_plan.take();
        let approximate = self.pass_approximate();
        let finished = self.recorder.as_mut().filter(|r| r.has_pending_take()).map(transit_playback::record::Recorder::finish);
        // Tests substitute only the device's finished files; placement and stop ordering stay real.
        #[cfg(test)]
        let finished = finished.or_else(|| self.finished_test_takes.take());
        let midi = self.player.as_ref().and_then(Player::finish_midi_recording);
        #[cfg(test)]
        let midi = midi.or_else(|| self.finished_test_midi.take());
        let journal = self.midi_journal.take();
        if finished.is_none() && midi.is_none() {
            if let Some(journal) = journal {
                journal.discard();
            }
            return;
        }
        let audio = finished.is_some();
        let mut plan = plan.unwrap_or_else(|| transit_engine::io::Recording {
            start: self.record_start,
            pass: None,
            punch: self.punch,
            rate: 0,
            takes: Vec::new(),
            midi_journal: None,
            midi_placed: false,
        });
        // The audio take and the MIDI pass share one latch; either's applied start is the real one.
        let latch = finished
            .is_some()
            .then(|| self.recorder.as_ref().and_then(transit_playback::record::Recorder::applied_start))
            .flatten()
            .or_else(|| midi.as_ref().and_then(|m| m.start));
        if let Some(point) = latch {
            plan.start = point.position.sample;
            self.record_start = plan.start;
        }
        self.punch = None;
        self.autosaved_rev = 0;
        let tracked = self.record_clock.take();
        let end = tracked.map_or(self.engine.transport.position, |c| c.last + c.laps);
        let pushes = self.engine.undo_pushes();
        if let Some((takes, problem)) = finished {
            let tracks = takes.len().max(1);
            self.place_takes(&plan, takes, end, tracks);
            if let Some(p) = problem {
                log::warn!("recording: {p}");
                self.status = format!("{} ({p})", self.status);
            }
        }
        if let Some(midi) = midi {
            let end = tracked.map_or_else(|| self.position().max(plan.start), |c| (c.last + c.laps).max(plan.start));
            // Whatever the last frame hadn't journaled reaches the file before the clip is built,
            // which then deletes it: a crash while placing still leaves the whole pass.
            self.midi_journal = journal;
            self.journal_midi(&midi, &plan, true);
            let journal = self.midi_journal.take();
            self.place_midi_take(&plan, &midi, end);
            if let Some(journal) = journal {
                self.hold_journal(journal, &plan);
            }
        } else if let Some(journal) = journal {
            journal.discard();
        }
        // An audio take and a MIDI take of the same pass are one undo step.
        if let Ok(added) = usize::try_from(self.engine.undo_pushes() - pushes)
            && added > 1
        {
            let _ = self.engine.squash_undo(added, "Record");
        }
        if audio && approximate {
            self.telemetry.approximate_takes += 1;
            log::warn!("recording: the take was aligned without a validated audio device clock");
            self.status = format!("{}. {APPROXIMATE_TAKE}", self.status.trim_end_matches('.'));
        }
    }

    /// Did this pass start without a validated audio device clock (see
    /// `Recorder::start_approximate`)?
    fn pass_approximate(&self) -> bool {
        #[cfg(test)]
        if self.test_approximate_start {
            return true;
        }
        self.recorder.as_ref().is_some_and(transit_playback::record::Recorder::start_approximate)
    }

    /// Place a finished MIDI pass: one clip per armed track (per pass when loop recording), merged
    /// into a clip already under the take. Nothing played, nothing placed.
    fn place_midi_take(&mut self, plan: &transit_engine::io::Recording, midi: &transit_playback::MidiTake, end: Samples) {
        let events: Vec<_> = midi.timed(self.engine.session().sample_rate.hz()).iter().filter_map(|(offset, e)| rec_event(*offset, e)).collect();
        let ids = transit_engine::midi_take::place_midi_recording_wrapping(&mut self.engine, plan, &events, end - plan.start, self.take_cycle);
        if midi.dropped > 0 {
            self.status = format!("{} MIDI events were lost: the capture queue was full", midi.dropped);
        }
        if ids.is_empty() {
            if midi.dropped == 0 && midi.start.is_some() && !midi.tracks.is_empty() {
                self.status = "Recorded no MIDI notes".into();
            }
            return;
        }
        let s = self.engine.session_arc();
        // A take of an earlier lap lives on a playlist that is no longer the active one.
        let placed: Vec<(TrackId, &transit_model::Clip)> = ids
            .iter()
            .filter_map(|id| s.tracks.iter().find_map(|t| t.playlists.iter().flat_map(|p| p.clips.iter()).find(|c| c.id == *id).map(|c| (t.id, c))))
            .collect();
        let mut names: Vec<String> = Vec::new();
        let mut lanes: Vec<TrackId> = Vec::new();
        for (t, _) in &placed {
            if !lanes.contains(t) {
                lanes.push(*t);
                if let Some(tr) = s.track(*t) {
                    names.push(tr.name.clone());
                }
            }
        }
        let notes: usize =
            placed.iter().map(|(_, c)| if let transit_model::ClipContent::Midi { sequence } = &c.content { sequence.notes.len() } else { 0 }).sum();
        let passes = ids.len() / lanes.len().max(1);
        let status = if passes > 1 {
            format!("Recorded {passes} takes ({notes} notes) on {}", names.join(", "))
        } else {
            format!("Recorded {} MIDI clip{} ({notes} notes) on {}", ids.len(), if ids.len() == 1 { "" } else { "s" }, names.join(", "))
        };
        self.status = if midi.dropped > 0 { format!("{status}; {} events lost", midi.dropped) } else { status };
        let clips: Vec<u64> = ids.iter().map(|c| c.0).collect();
        let _ = self.run("edit.select", json!({"clips": clips}));
        if let Some((t, _)) = ids.first().and_then(|id| s.find_clip(*id)) {
            self.ui.focus_track = Some(t);
        }
    }

    /// Slice 6: place finished take files. A take recorded over clips on its track goes on a new
    /// playlist that copies the old one (the engine does it inside the one "Record" undo step), so the old clips stay as the
    /// previous take (plan 06, L8: takes are track playlists). The new clips are selected and the
    /// status line says what was recorded where. `end` is where the playhead stopped.
    pub(crate) fn place_takes(
        &mut self,
        plan: &transit_engine::io::Recording,
        takes: Vec<(TrackId, std::path::PathBuf)>,
        end: Samples,
        tracks: usize,
    ) {
        let from = plan.punch.map_or(plan.start, |p| p.start.max(plan.start));
        let to = plan.punch.map_or(end, |p| p.end.min(end));
        let mut new_take = Vec::new();
        if plan.pass.is_none() && to > from {
            for (t, _) in &takes {
                let over = self.engine.session().track(*t).is_some_and(|tr| tr.clips().iter().any(|c| c.start < to && c.end() > from));
                if over {
                    new_take.push(*t);
                }
            }
        }
        let ids = transit_engine::io::place_recording_take(&mut self.engine, plan, takes, true);
        if plan.pass.is_some() {
            let passes = ids.len() / tracks.max(1);
            self.status = if passes > 1 { format!("Recorded {passes} takes") } else { format!("Loop-recorded {passes} take(s)") };
            return;
        }
        let s = self.engine.session_arc();
        let placed: Vec<(TrackId, &transit_model::Clip)> = ids.iter().filter_map(|id| s.find_clip(*id)).collect();
        let Some(longest) = placed.iter().map(|(_, c)| c.length).max() else {
            self.status = "Recorded nothing".into();
            return;
        };
        let mut names: Vec<String> = Vec::new();
        for (t, _) in &placed {
            if let Some(tr) = s.track(*t)
                && !names.contains(&tr.name)
            {
                names.push(tr.name.clone());
            }
        }
        let secs = s.sample_rate.seconds(longest);
        let n = placed.len();
        self.status = format!("Recorded {n} clip{} ({secs:.1} s) on {}", if n == 1 { "" } else { "s" }, names.join(", "));
        if let [t] = new_take.as_slice()
            && let Some(tr) = s.track(*t)
        {
            self.status.push_str(&format!(" as take {}", tr.active_playlist + 1));
        }
        let clips: Vec<u64> = ids.iter().map(|c| c.0).collect();
        let _ = self.run("edit.select", json!({"clips": clips}));
        if let Some((t, _)) = placed.first() {
            self.ui.focus_track = Some(*t);
        }
    }
}

/// A captured event as the journal keeps it (device frame, not yet placed); None for what
/// [`rec_event`] cannot keep.
fn journal_record(e: &transit_playback::midi_capture::CapturedEvent) -> Option<transit_engine::midi_journal::Record> {
    let kept = rec_event(0, e)?;
    Some(transit_engine::midi_journal::Record { frame: e.frame, track: e.track, channel: e.channel, approximate: e.approximate, msg: kept.msg })
}

/// A captured live message as the engine's recorder takes it; controllers it cannot keep (14-bit
/// pairs, which ingress does not produce) are left out.
fn rec_event(offset: Samples, e: &transit_playback::midi_capture::CapturedEvent) -> Option<transit_engine::midi_take::RecEvent> {
    use transit_engine::midi_take::Msg;
    use transit_mix::Message;
    let msg = match e.message {
        Message::NoteOn { key, velocity, .. } => Msg::NoteOn { pitch: key, velocity },
        Message::NoteOff { key, velocity, .. } => Msg::NoteOff { pitch: key, velocity },
        Message::Control { cc, value } => Msg::Cc { cc, value },
        Message::PitchBend { value } => Msg::PitchBend { value },
        Message::ChannelPressure { value } => Msg::ChannelPressure { value },
        Message::PolyPressure { key, value, .. } => Msg::PolyPressure { pitch: key, value },
        Message::Program { program } => Msg::Program { program },
        Message::Control14 { .. } => return None,
    };
    Some(transit_engine::midi_take::RecEvent { track: e.track, channel: e.channel, offset, msg })
}
