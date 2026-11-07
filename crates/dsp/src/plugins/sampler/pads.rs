//! What the core path publishes per pad for the Sampler's Pads view (#326, job 326-S): each pad's
//! [`PadWord`], its admitted triggers with the last velocity and its cuts by a choke group, read by
//! the mix after the block ([`crate::Plugin::pad_activity`]) and carried to the panel by the
//! instrument's scope tap.
//!
//! The words are worked out from the block's events before the core renders them, by ADR 0041's
//! admission rule as the core applies it (`SamplerCore`): a note-on (velocity above 0) triggers the
//! first pad whose trigger note it is and that has a zone; a pad press triggers its pad. A muted pad,
//! or a pad that is not solo while any pad is, admits nothing and cuts nothing. An admitted trigger
//! of a pad in a choke group cuts every other pad of the group that is sounding.
//!
//! The core does not expose which pad's voice sounds, so "sounding" is read from what it does
//! expose: a pad sounds when its zone's source is among the sources the core's voices were playing
//! at the block's start, or when it was triggered earlier in the same block and not cut since. Two
//! pads that share a source therefore count as sounding together; a stop, All Sound Off, All Notes
//! Off or a reset in the block ends every pad's sounding for the rest of it. The words are a
//! display: they never change what the core plays.
//!
//! Nothing here allocates or locks: a fixed array of words, a 256-bit set and a 64-bit mask on the
//! stack, and a scan of the core's voices only in a block that holds a trigger.

use transit_sampler_core::event::{Event, EventKind};
use transit_sampler_core::{PADS, SamplerCore};

use crate::{PAD_SLOTS, PadWord};

const _: () = assert!(PADS == PAD_SLOTS, "the published pads are the core's");

/// Each pad's word, updated from every block's events.
#[derive(Debug, Clone)]
pub(super) struct PadWords {
    words: [u32; PADS],
}

impl PadWords {
    pub(super) fn new() -> PadWords {
        PadWords { words: [0; PADS] }
    }

    pub(super) fn words(&self) -> &[u32; PADS] {
        &self.words
    }

    /// Counts the triggers and cuts of `events` (sorted, the block's own) against `core`'s pads as
    /// they stand before the block renders.
    pub(super) fn observe(&mut self, core: &SamplerCore, events: &[Event]) {
        let trigger = |e: &Event| matches!(e.kind, EventKind::NoteOn { velocity: 1.., .. } | EventKind::PadPress { velocity: 1.., .. });
        if !events.iter().any(trigger) {
            return;
        }
        let any_solo = (0..PADS).any(|p| core.pad(p).is_some_and(|p| p.solo));
        let mut playing = [0u64; 4];
        for s in core.playing_sources() {
            if let Some(w) = playing.get_mut(usize::from(s) / 64) {
                *w |= 1 << (s % 64);
            }
        }
        // Pads triggered earlier in this block and not cut since.
        let mut started = 0u64;
        for e in events {
            let (p, velocity) = match e.kind {
                EventKind::NoteOn { key, velocity, .. } if (1..=127).contains(&velocity) && key <= 127 => {
                    match (0..PADS).find(|&p| core.pad(p).is_some_and(|q| q.note == key && q.zone.is_some())) {
                        Some(p) => (p, velocity),
                        None => continue,
                    }
                }
                EventKind::PadPress { pad, velocity } if (1..=127).contains(&velocity) => (usize::from(pad), velocity),
                EventKind::Stop | EventKind::AllSoundOff | EventKind::AllNotesOff | EventKind::Reset => {
                    playing = [0; 4];
                    started = 0;
                    continue;
                }
                _ => continue,
            };
            let Some(pad) = core.pad(p).copied() else { continue };
            if pad.zone.is_none() || pad.mute || (any_solo && !pad.solo) {
                continue;
            }
            if pad.choke != 0 {
                for q in (0..PADS).filter(|&q| q != p) {
                    let Some(other) = core.pad(q) else { continue };
                    if other.choke != pad.choke {
                        continue;
                    }
                    let source_plays = other.zone.and_then(|z| core.zone(usize::from(z))).is_some_and(|z| {
                        let s = usize::from(z.region.source);
                        playing.get(s / 64).is_some_and(|w| w >> (s % 64) & 1 == 1)
                    });
                    if started >> q & 1 == 1 || source_plays {
                        if let Some(w) = self.words.get_mut(q) {
                            *w = PadWord(*w).choked().0;
                        }
                        started &= !(1 << q);
                    }
                }
            }
            if let Some(w) = self.words.get_mut(p) {
                *w = PadWord(*w).hit(velocity).0;
            }
            started |= 1 << p;
        }
    }
}
