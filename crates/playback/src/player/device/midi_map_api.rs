//! The Player's MIDI Learn surface (ADR 0028): hand the ingress worker the mappings, arm a
//! learn, keep the pickup cells current and take the events the worker published.

use super::Player;
use crate::player::{MapHeard, MapMsg, MapSpec};

impl Player {
    /// Replaces the MIDI Learn mappings the ingress worker matches. The ports they name are
    /// opened. An equal list changes nothing, so a frame-by-frame caller costs a comparison.
    pub fn set_midi_mappings(&self, specs: Vec<MapSpec>) {
        if let Ok(mut midi) = self.midi.try_borrow_mut()
            && let Some(m) = midi.as_mut()
        {
            m.set_mappings(specs);
        }
    }

    /// The epoch of the list last given to [`Player::set_midi_mappings`]; events carry it.
    pub fn midi_map_epoch(&self) -> u32 {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.map_epoch())).unwrap_or(0)
    }

    /// What the worker heard of each mapping of the list (how many messages matched, the last
    /// knob value, whether pickup has caught the control), in its order: why a knob moves nothing.
    pub fn midi_map_heard(&self) -> Vec<MapHeard> {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.map_heard())).unwrap_or_default()
    }

    /// Whether each mapping's input port is open for the worker (in the list's order); `None`
    /// until the worker has the list (or without MIDI).
    pub fn midi_map_ports_open(&self) -> Option<Vec<bool>> {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().and_then(|m| m.map_ports_open()))
    }

    /// Arms or disarms MIDI Learn. Armed, every present input port is opened and the worker
    /// forwards the next CC or note-on exactly once.
    pub fn set_midi_learn(&self, armed: bool) {
        if let Ok(mut midi) = self.midi.try_borrow_mut()
            && let Some(m) = midi.as_mut()
        {
            m.set_learn(armed);
        }
    }

    /// Once a frame: each mapped control's value as the session has it and how many mapped events
    /// of it have been applied, in the order of the list. Pickup compares against these.
    pub fn sync_midi_map(&self, values: &[(f32, u32)]) {
        if let Ok(midi) = self.midi.try_borrow()
            && let Some(m) = midi.as_ref()
        {
            m.sync_map_values(values);
        }
    }

    /// The MIDI Learn events published since the last call, oldest first.
    pub fn take_midi_map_events(&self) -> Vec<MapMsg> {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.take_map_events())).unwrap_or_default()
    }

    /// Tells this Player's MIDI side from the one an audio reconnect replaces (0 without MIDI).
    pub fn midi_map_generation(&self) -> u32 {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.map_generation())).unwrap_or(0)
    }

    /// A counter that grows whenever the MIDI Learn worker published something (a value, a press,
    /// a candidate). The UI compares it between frames to repaint only when there is news.
    pub fn midi_map_activity(&self) -> u64 {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.map_activity())).unwrap_or(0)
    }

    /// Whether the worker is armed to forward a learn candidate.
    pub fn midi_learn_armed(&self) -> bool {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.learn_armed())).unwrap_or(false)
    }

    /// Button presses and learn candidates the worker could not queue (the UI was not draining).
    pub fn midi_map_dropped(&self) -> u64 {
        self.midi.try_borrow().ok().and_then(|m| m.as_ref().map(|m| m.map_dropped())).unwrap_or(0)
    }
}
