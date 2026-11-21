use super::*;

/// The MIDI side of a Player being replaced, carried by the reopen to the replacement (#760): the
/// open MIDI inputs and the threads that own and read them. Dropping it closes those inputs.
pub struct MidiInputs(native::Native);

impl std::fmt::Debug for MidiInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiInputs").field("open", &self.0.status().open_inputs).finish()
    }
}

impl Player {
    /// Takes this Player's MIDI side for its replacement, on the control thread that replaces it,
    /// after the reopen can no longer be refused and before this Player drops. Its open inputs stay
    /// open; this Player keeps no MIDI. `None` when it has no MIDI side (or it is borrowed).
    pub fn take_midi_inputs(&self) -> Option<MidiInputs> {
        let mut midi = self.midi.try_borrow_mut().ok()?;
        let mut native = midi.take()?;
        native.hand_off();
        Some(MidiInputs(native))
    }

    /// Installs the MIDI side the Player this one replaced handed over
    /// ([`Self::take_midi_inputs`]), in place of this Player's own, before this Player is serviced.
    /// When it cannot be carried (see `Native::adopt`) it is dropped and this Player opens its
    /// inputs itself.
    pub fn adopt_midi_inputs(&self, carried: MidiInputs) {
        let Ok(mut midi) = self.midi.try_borrow_mut() else { return };
        let Some(fresh) = midi.take() else { return };
        *midi = Some(native::Native::adopt(fresh, carried.0));
    }
}
