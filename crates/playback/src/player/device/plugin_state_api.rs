use super::*;

impl Player {
    /// Whether a hosted plug-in said its own state changed since the last call (issue #574: CLAP
    /// `mark_dirty`, VST3 `setDirty`), as the editors' [`Self::idle`] gathered it: something its
    /// parameters do not show, so the UI marks the session modified. Notices from the document a
    /// replacement is taking over from are dropped.
    pub fn take_plugin_state_changed(&self) -> bool {
        self.owner().is_ok_and(|mut owner| owner.take_plugin_state_changed())
    }

    /// The timed autosave's bounded read (issue #574; `Handles::capture_states_timed`): refused
    /// unless `expected` is the confirmed document, as [`Self::capture_states`]. Returns the
    /// states read and whether every plug-in was read.
    pub fn capture_states_timed(
        &self,
        expected: &Arc<Session>,
        budget: std::time::Duration,
        skip: &[(TrackId, usize)],
    ) -> Result<(PluginStates, bool), PlayerError> {
        let mut owner = self.owner()?;
        owner.capture_states_timed(expected, budget, self.render_callback_runs(), skip)
    }
}
