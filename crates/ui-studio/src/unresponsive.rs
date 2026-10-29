//! The unresponsive-plug-in marker (#213), shared by a stuck or given-up host thread (#205) and a
//! render worker hung inside a plug-in (#190, K89). One state per plug-in slot, one text
//! ([`UNRESPONSIVE`]) and one hover explanation, read by the mixer insert row, the device card and
//! the arrange track header. The state is polled from the engine each frame; nothing here
//! touches the audio thread.

use crate::StudioApp;
use crate::devices::{UNRESPONSIVE, hosted};
use transit_model::{Track, TrackId};
use transit_playback::StuckCall;

/// Why a plug-in does not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hang {
    /// Its plug-in host thread is stuck past the watchdog budget, or was given up on (#205).
    HostThread,
    /// A render worker holds its strip and does not return from `process` (#190).
    Worker,
}

/// What the marker explains, for the plug-in `name`. `calls` are the host's stuck calls
/// (`Player::plugin_host_stuck`): a live one is named with how long it has been stuck; none live
/// means the thread was given up on and its call has since returned, or never will.
pub(crate) fn tip(hang: Hang, name: &str, calls: &[StuckCall]) -> String {
    match hang {
        Hang::HostThread => match calls.iter().find(|s| !s.given_up) {
            Some(s) => format!(
                "{name} is {}: its plug-in host thread has not returned from {} in {} for {} s. Audio keeps playing, and saving keeps the state this plug-in had when it was last read. Choose Later or Give up in the message window, or save and restart transit if it does not recover.",
                UNRESPONSIVE.to_lowercase(),
                s.call,
                s.plugin,
                s.stuck_for.as_secs()
            ),
            None => format!(
                "{name} is {}: you gave up on its plug-in host thread, so it stays quarantined until transit restarts. Audio keeps playing, and saving keeps the state this plug-in had when it was last read. Save and restart transit to use it again.",
                UNRESPONSIVE.to_lowercase()
            ),
        },
        Hang::Worker => format!(
            "A render worker is stuck inside a plug-in on this track ({name} may be the one), so the track is silent. Close the audio device or choose Reconnect to retire the worker."
        ),
    }
}

#[cfg(test)]
thread_local! {
    /// A test's fake: this track's hosted plug-ins read as hung for this reason.
    pub(crate) static FAKE: std::cell::Cell<Option<(TrackId, Hang)>> = const { std::cell::Cell::new(None) };
}

impl StudioApp {
    /// Why the plug-in `plugin` in `slot` of `track` is unresponsive, if it is. Built-ins never are.
    pub(crate) fn slot_hang(&self, track: TrackId, slot: usize, plugin: &str) -> Option<Hang> {
        if !hosted(plugin) {
            return None;
        }
        #[cfg(test)]
        if let Some((t, hang)) = FAKE.with(std::cell::Cell::get) {
            return (t == track).then_some(hang);
        }
        let player = self.player.as_ref()?;
        if player.slot_unresponsive(track, slot) {
            return Some(Hang::HostThread);
        }
        player.track_held(track).then_some(Hang::Worker)
    }

    /// The hover text of the marker for `name`.
    pub(crate) fn hang_tip(&self, hang: Hang, name: &str) -> String {
        let calls = self.player.as_ref().map(|p| p.plugin_host_stuck()).unwrap_or_default();
        tip(hang, name, &calls)
    }

    /// The first unresponsive hosted plug-in on `t`: its id and why. Nothing is copied unless
    /// one is.
    pub(crate) fn track_hang(&self, t: &Track) -> Option<(String, Hang)> {
        let inserts = t.mixer.inserts.iter().enumerate().filter_map(|(s, i)| i.as_ref().map(|i| (s, i)));
        t.instrument.iter().map(|i| (transit_mix::INSTRUMENT_SLOT, i)).chain(inserts).find_map(|(slot, ins)| {
            let hang = self.slot_hang(t.id, slot, &ins.plugin)?;
            Some((ins.plugin.clone(), hang))
        })
    }
}
