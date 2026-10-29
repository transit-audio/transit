//! The unresponsive-plug-in marker (#213), shared by a stuck or given-up host thread (#205) and a
//! render worker hung inside a plug-in (#190, K89). One state per plug-in slot, one text
//! ([`UNRESPONSIVE`]) and one hover explanation, read by the mixer insert row, the device card and
//! the arrange track header. The state is polled from the engine each frame; nothing here
//! touches the audio thread.

use crate::StudioApp;
use crate::devices::{UNRESPONSIVE, hosted};
use transit_model::{Track, TrackId};

/// Why a plug-in does not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hang {
    /// Its plug-in host thread is stuck past the watchdog budget, or was given up on (#205).
    HostThread,
    /// A render worker holds its strip and does not return from `process` (#190).
    Worker,
}

/// What the marker says and explains, for one plug-in.
pub(crate) fn tip(hang: Hang, name: &str) -> String {
    match hang {
        Hang::HostThread => format!(
            "{name} is {}: its plug-in host thread is stuck inside the plug-in. Audio keeps playing, and saving keeps the state it had when it was last read. Choose Later or Give up in the message window, or save and restart transit if it does not recover.",
            UNRESPONSIVE.to_lowercase()
        ),
        Hang::Worker => format!(
            "{name} is {}: a render worker is stuck inside the plug-in, so this track is silent. Close the audio device or choose Reconnect to retire the worker.",
            UNRESPONSIVE.to_lowercase()
        ),
    }
}

#[cfg(test)]
thread_local! {
    /// A test's fake: this track's hosted plug-ins read as hung for this reason.
    pub(crate) static FAKE: std::cell::Cell<Option<(TrackId, Hang)>> = const { std::cell::Cell::new(None) };
}

impl StudioApp {
    /// Why the plug-in `plugin` in `slot` of `track` (at session index `index`) is unresponsive,
    /// if it is. Built-ins never are.
    pub(crate) fn slot_hang(&self, track: TrackId, index: Option<usize>, slot: usize, plugin: &str) -> Option<Hang> {
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
        index.filter(|&i| player.track_held(i)).map(|_| Hang::Worker)
    }

    /// The first unresponsive hosted plug-in on `t`: its id and why.
    pub(crate) fn track_hang(&self, t: &Track) -> Option<(String, Hang)> {
        let index = self.engine.session().tracks.iter().position(|x| x.id == t.id);
        let inserts = t.mixer.inserts.iter().enumerate().filter_map(|(s, i)| i.as_ref().map(|i| (s, i)));
        t.instrument
            .iter()
            .map(|i| (transit_mix::INSTRUMENT_SLOT, i))
            .chain(inserts)
            .find_map(|(slot, ins)| Some((ins.plugin.clone(), self.slot_hang(t.id, index, slot, &ins.plugin)?)))
    }
}
