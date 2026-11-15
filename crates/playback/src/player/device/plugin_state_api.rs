use super::*;

impl Player {
    /// Whether a hosted plug-in said its own state changed since the last call (issue #574: CLAP
    /// `mark_dirty`, VST3 `setDirty`), as the editors' [`Self::idle`] gathered it: something its
    /// parameters do not show, so the UI marks the session modified. Notices from the document a
    /// replacement is taking over from are dropped.
    pub fn take_plugin_state_changed(&self) -> bool {
        self.owner().is_ok_and(|mut owner| owner.take_plugin_state_changed())
    }
}
