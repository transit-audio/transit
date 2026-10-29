//! Test seam (feature `testing`, issue #210): an instrument slot's event state, for the
//! fingerprints of [`crate::fingerprint`].

use super::InstrumentEvents;
use std::hash::Hasher;

/// A value's debug text into `h` (test only: allocates).
fn text(h: &mut dyn Hasher, v: &impl std::fmt::Debug) {
    h.write(format!("{v:?}").as_bytes());
    h.write_u8(0);
}

impl InstrumentEvents {
    /// The delivered state (the sounding notes with their voice ids, the pedals held down, the
    /// values the plug-in last got, the bend and pressure writers, the next voice id), then the
    /// messages still queued, in order. Bookkeeping of entries not yet delivered (a queued
    /// note-on's entry, due frames) is left out: a render worker's pool parks such an event
    /// outside the queue and admits it only when it lands (see the module docs of
    /// [`crate::fingerprint`]).
    pub(crate) fn fingerprint(&self, h: &mut dyn Hasher) {
        let mut sounding: Vec<_> = self.notes.iter().filter(|e| e.used && e.voice >= 0).map(|e| (e.source, e.key, e.sender, e.voice)).collect();
        sounding.sort_by_key(|&(s, key, sender, voice)| (s.port, s.channel, s.generation, key, format!("{sender:?}"), voice));
        text(h, &sounding);
        let mut held: Vec<_> = self.pedals.iter().filter(|p| p.used && p.down).map(|p| (p.source, p.cc, p.value)).collect();
        held.sort_by_key(|&(s, cc, value)| (s.port, s.channel, s.generation, cc, value));
        text(h, &held);
        text(h, &self.holders);
        text(h, &self.sent);
        text(h, &self.writers);
        h.write_u32(self.next_voice);
        for q in &self.queue {
            text(h, &(q.source, q.transport, q.message));
        }
    }
}
