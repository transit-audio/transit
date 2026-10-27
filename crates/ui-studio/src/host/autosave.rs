use super::*;

impl StudioApp {
    /// Every two minutes, copy a modified session to the autosave folder (not while recording).
    /// Timed by the wall clock: egui's stands still while the window is hidden.
    pub(crate) fn autosave(&mut self) {
        let Some(dir) = self.autosave_dir.clone() else { return };
        let last = *self.last_autosave.get_or_insert_with(std::time::Instant::now);
        if last.elapsed().as_secs() < 120 || !self.engine.is_dirty() || self.engine.transport.recording {
            return;
        }
        self.last_autosave = Some(std::time::Instant::now());
        if self.capture_plugin_states().is_ok() {
            self.autosave_to(&dir);
        }
    }

    /// Autosave a modified session within a second of every change, also while recording. The
    /// plugins' own state reaches the session every two minutes (`autosave`).
    pub(crate) fn autosave_changes(&mut self) {
        self.poll_journal();
        if let Some(e) = self.autosaver.as_ref().and_then(transit_engine::autosave::Autosaver::failure) {
            self.status = format!("Autosave failed: {e}");
        }
        if self.autosave_dir.is_none() || self.engine.revision == self.autosaved_rev {
            return;
        }
        self.autosaved_rev = self.engine.revision;
        if !self.engine.is_dirty() {
            self.release_autosave(false);
        } else if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    pub(super) fn queue_autosave(&mut self, root: std::path::PathBuf) {
        if let Some(point) = self.recorder.as_ref().and_then(transit_playback::record::Recorder::applied_start)
            && let Some(plan) = self.take_plan.as_mut().filter(|plan| !plan.takes.is_empty())
        {
            plan.start = point.position.sample;
            self.record_start = plan.start;
        }
        self.autosaved_rev = self.engine.revision;
        let placed = self.placed_journal.as_ref().filter(|p| !matches!(p.step, JournalStep::Release | JournalStep::Cleared(_)));
        let plan = self.take_plan.clone().or_else(|| placed.map(|p| p.plan.clone()));
        let carries = plan.as_ref().is_some_and(|p| p.midi_placed);
        let w = self.autosaver.get_or_insert_with(|| transit_engine::autosave::Autosaver::new(root));
        let ticket = w.write(self.engine.session_arc(), self.engine.path.clone(), plan);
        if let Some(p) = &mut self.placed_journal {
            p.step = match p.step {
                JournalStep::Queue | JournalStep::Placed(_) if carries => JournalStep::Placed(ticket),
                JournalStep::Queue | JournalStep::Placed(_) | JournalStep::Release => JournalStep::Cleared(ticket),
                JournalStep::Cleared(t) => JournalStep::Cleared(t),
            };
        }
    }

    /// Stop placed the pass's MIDI clips in memory. The journal is the only copy of those notes
    /// that a crash leaves, so it stays until the autosaver has written them: first a write whose
    /// plan says the clips are placed (a crash then discards the journal on recovery), then one
    /// whose plan no longer carries the recording, and only then is the journal deleted. Without
    /// an autosave folder there is no plan to pair it with, and it goes at once.
    pub(super) fn hold_journal(&mut self, journal: transit_engine::midi_journal::Writer, plan: &transit_engine::io::Recording) {
        if self.autosave_dir.is_none() {
            journal.discard();
            return;
        }
        let plan = transit_engine::io::Recording { midi_journal: Some(journal.path().to_path_buf()), midi_placed: true, ..plan.clone() };
        if let Some(old) = self.placed_journal.replace(PlacedJournal { journal, plan, step: JournalStep::Queue }) {
            old.journal.discard();
        }
    }

    /// Move a held journal along (see [`StudioApp::hold_journal`]); every frame.
    pub(crate) fn poll_journal(&mut self) {
        let Some(root) = self.autosave_dir.clone() else { return };
        let Some(step) = self.placed_journal.as_ref().map(|p| p.step) else { return };
        let landed = |app: &StudioApp, t| app.autosaver.as_ref().is_some_and(|w| w.landed(t));
        if !self.engine.is_dirty() {
            // Saved (or undone): the session file holds the clips, or nothing wants them.
            if let Some(p) = self.placed_journal.take() {
                p.journal.discard();
            }
            return;
        }
        match step {
            JournalStep::Queue => self.queue_autosave(root),
            JournalStep::Placed(t) if landed(self, t) => {
                if let Some(p) = &mut self.placed_journal {
                    p.step = JournalStep::Release;
                }
                self.queue_autosave(root);
            }
            JournalStep::Cleared(t) if landed(self, t) => {
                if let Some(p) = self.placed_journal.take() {
                    p.journal.discard();
                }
            }
            JournalStep::Placed(_) | JournalStep::Cleared(_) | JournalStep::Release => {}
        }
    }

    fn autosave_to(&mut self, root: &std::path::Path) -> bool {
        self.queue_autosave(root.to_path_buf());
        let Some(w) = &self.autosaver else { return false };
        let written = w.flush();
        if let Err(e) = &written {
            self.status = format!("Autosave failed: {e}");
        }
        written.is_ok()
    }

    pub(crate) fn write_autosave(&mut self) -> bool {
        self.autosave_dir.clone().is_some_and(|root| self.autosave_to(&root))
    }

    pub(crate) fn release_autosave(&mut self, keep: bool) {
        if let Some(w) = &self.autosaver {
            w.close(keep);
        }
    }

    /// Done with the open session. Unsaved changes stay in its autosave folder for the next
    /// launch, unless the user quit with Quit; a take still recording is finished and always stays.
    /// True when an autosave was written now.
    pub fn close_autosave(&mut self) -> bool {
        let recording = self.recording_active();
        if recording {
            self.stop_play();
        }
        let keep = self.engine.is_dirty() && (recording || !self.quit_requested);
        // A journal still held stays on disk with the autosave that names it, or goes with it.
        if let Some(mut held) = self.placed_journal.take() {
            if keep {
                held.step = JournalStep::Queue;
                self.placed_journal = Some(held);
            } else {
                held.journal.discard();
            }
        }
        let written = keep && self.capture_plugin_states().is_ok() && self.write_autosave();
        self.release_autosave(keep);
        if let Some(w) = &self.autosaver {
            let _ = w.flush();
        }
        self.placed_journal = None;
        written
    }

    /// Run part of a frame. If it panics, the session is autosaved (once per change) and the app
    /// carries on instead of quitting.
    pub fn guarded(&mut self, f: impl FnOnce(&mut StudioApp)) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))).is_ok() || self.panic_rev == self.engine.revision {
            return;
        }
        self.panic_rev = self.engine.revision;
        let saved = self.engine.is_dirty() && self.write_autosave();
        self.status = if saved {
            "Internal error (see the log). Your changes were autosaved: save them, then restart.".into()
        } else {
            "Internal error (see the log). Restart to be safe.".into()
        };
    }

    /// Replace the open session with unsaved work an earlier run left behind.
    pub fn recover(&mut self, left: &transit_engine::autosave::Leftover) {
        if self.recording_active() {
            self.status = "Stop recording before recovering other work.".into();
            return;
        }
        self.close_autosave();
        match transit_engine::autosave::recover(&mut self.engine, left) {
            Ok(missing) => {
                let midi = transit_engine::autosave::recover_midi(&mut self.engine, left);
                // A journal that can't be used (damaged, not ours) was deleted or is left alone: either
                // way it would only be offered again. One that may open next time keeps the leftover.
                let retry = midi.is_err() && transit_engine::autosave::midi_journal_usable(left);
                if !retry && self.write_autosave() {
                    transit_engine::autosave::discard(left);
                }
                self.status = match missing.len() {
                    0 => format!("Recovered “{}”. Save it to keep the changes.", left.name),
                    n => format!("Recovered “{}”, but {n} audio file(s) are missing. Save it to keep the changes.", left.name),
                };
                match midi {
                    Ok(clips) if !clips.is_empty() => {
                        self.status = format!("{} {} recovered MIDI clip(s) from the interrupted recording.", self.status, clips.len());
                    }
                    Ok(_) => {}
                    Err(e) => self.status = format!("{} The MIDI take couldn't be recovered: {e}", self.status),
                }
            }
            Err(e) => self.status = format!("Couldn't recover “{}”: {e}", left.name),
        }
    }
}
