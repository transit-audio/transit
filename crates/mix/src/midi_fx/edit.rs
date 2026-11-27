//! Chain edits (ADR 0057, decision 2.13, slice P3a): the instrument's pedal table and the edit's
//! pedal-ups.
//!
//! The host keeps, per channel and pedal kind (CC 64 sustain, 66 sostenuto, 67 soft, 69 hold 2),
//! the last value the instrument accepted: every event to the instrument passes
//! [`MidiFxHost::give`], with or without a chain, so the table is updated there for every pedal
//! controller the instrument accepts, and an accepted CC 121 (Reset All Controllers) clears its
//! channel. At a chain edit, after the note-offs, the host gives a value 0 for every pedal the
//! table holds at 64 or above, at offset 0 ([`MidiFxHost::release_pedals`]). Nothing else is sent:
//! no CC 120, 121 or 123, and no expression. A pedal-up the instrument refuses goes to the retry
//! buffer, with the limits 2.13 states (ADR 0059 owns the guarantee).

use transit_dsp::Plugin;
use transit_dsp::event::{CC_RESET_CONTROLLERS, Message, PluginEvent};

use super::MidiFxHost;

/// The pedal kinds, in the order the edit releases them on each channel.
pub(super) const PEDAL_CCS: [u8; 4] = [64, 66, 67, 69];

fn kind(cc: u8) -> Option<usize> {
    PEDAL_CCS.iter().position(|&c| c == cc)
}

/// The instrument's pedal values, as it accepted them: 16 channels by 4 pedal kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PedalTable([[u8; 4]; 16]);

impl PedalTable {
    pub(super) const fn new() -> Self {
        PedalTable([[0; 4]; 16])
    }

    /// An event the instrument accepted.
    pub(super) fn accepted(&mut self, ev: &PluginEvent) {
        let Some(row) = self.0.get_mut(usize::from(ev.channel & 0x0F)) else { return };
        match ev.message {
            Message::Control { cc: CC_RESET_CONTROLLERS, .. } => *row = [0; 4],
            Message::Control { cc, value } => {
                if let Some(v) = kind(cc).and_then(|k| row.get_mut(k)) {
                    *v = value;
                }
            }
            _ => {}
        }
    }

    /// The value the instrument holds for `cc` on `channel` (0 for a controller that is no pedal).
    pub(super) fn value(&self, channel: u8, cc: u8) -> u8 {
        self.0.get(usize::from(channel & 0x0F)).and_then(|row| kind(cc).and_then(|k| row.get(k))).copied().unwrap_or(0)
    }

    /// Test seam (feature `testing`): the table in channel order.
    #[cfg(feature = "testing")]
    pub(super) fn record(&self, w: &mut transit_state_record::Writer<'_>) {
        w.uints(64, self.0.iter().flatten().map(|&v| u64::from(v)));
    }
}

impl MidiFxHost {
    /// A chain edit's second step (2.13): after the note-offs, a pedal-up (value 0) at offset 0 for
    /// every pedal the instrument holds at 64 or above, channel by channel. A refused one is held
    /// for the next block like any refused release.
    pub(crate) fn release_pedals(&mut self, plugin: &mut dyn Plugin) {
        for channel in 0..16u8 {
            for cc in PEDAL_CCS {
                if self.pedals.value(channel, cc) < 64 {
                    continue;
                }
                let up = PluginEvent { channel, note_id: PluginEvent::NO_ID, message: Message::Control { cc, value: 0 } };
                if !self.give(plugin, 0, &up) {
                    self.counts.refused = self.counts.refused.saturating_add(1);
                    self.hold_for_retry(&up);
                }
            }
        }
    }

    /// The value the instrument holds for pedal `cc` on `channel`, as the host saw it accept it.
    #[cfg(test)]
    pub(crate) fn pedal_value(&self, channel: u8, cc: u8) -> u8 {
        self.pedals.value(channel, cc)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn cc(channel: u8, cc: u8, value: u8) -> PluginEvent {
        PluginEvent { channel, note_id: PluginEvent::NO_ID, message: Message::Control { cc, value } }
    }

    #[test]
    fn the_table_keeps_each_pedal_and_an_accepted_cc_121_clears_its_channel() {
        let mut t = PedalTable::new();
        for (ch, c, v) in [(0, 64, 127), (0, 66, 90), (0, 67, 64), (0, 69, 100), (3, 64, 70), (0, 1, 99), (0, 7, 100)] {
            t.accepted(&cc(ch, c, v));
        }
        assert_eq!([64, 66, 67, 69].map(|c| t.value(0, c)), [127, 90, 64, 100]);
        assert_eq!(t.value(3, 64), 70);
        assert_eq!(t.value(0, 1), 0, "the mod wheel is no pedal");
        t.accepted(&cc(0, 64, 20));
        assert_eq!(t.value(0, 64), 20, "a pedal-up is kept as its value");
        t.accepted(&cc(0, CC_RESET_CONTROLLERS, 0));
        assert_eq!([64, 66, 67, 69].map(|c| t.value(0, c)), [0; 4], "CC 121 clears its channel");
        assert_eq!(t.value(3, 64), 70, "and only its channel");
    }
}
