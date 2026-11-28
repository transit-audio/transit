//! Test-only (feature `testing`): the input ledger of the host slices' owning fixtures (ADR 0057,
//! decision 11, "ledger.rs", and decision 13, "Host slices"). Not the kit of slice K1: a fixture's
//! own copy of decision 11's rules, written for the owning pass-through and the owning stacker.
//!
//! - **Capacity** 256 entries `{ channel, key, voice, seq, payload }`, each Open or Orphaned.
//!   **Insertion** in the first free entry with the next sequence number.
//! - **Closing.** A note-off with a voice id closes the Open or Orphaned entry of its channel with
//!   that id and nothing else. One without closes the oldest, by sequence number, of the anonymous
//!   entries and the anonymous rejection records of its channel and key.
//! - **Rejection.** With 256 entries in use a note-on is rejected (counted). An identified
//!   rejection keeps no record; an anonymous one is recorded `{ channel, key, seq }` in a table of
//!   256 with the next sequence number; with that table full it is only counted
//!   (`rejected_unrecorded`, the saturation rule).
//! - **Reset** clears the entries, the records and `rejected_unrecorded`. The other counters are
//!   the fixture's statistics for its tests and are kept.

/// Entries of the ledger, and anonymous rejection records.
pub const LEDGER_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, Default)]
struct Entry<P> {
    used: bool,
    orphaned: bool,
    channel: u8,
    key: u8,
    voice: i32,
    seq: u64,
    payload: P,
}

#[derive(Clone, Copy, Debug, Default)]
struct Record {
    used: bool,
    channel: u8,
    key: u8,
    seq: u64,
}

/// What a note-off closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Closed<P> {
    /// An Open entry, with its payload: its outputs are released.
    Open(P),
    /// An Orphaned entry: consumed.
    Orphaned,
    /// An anonymous rejection record: consumed.
    Record,
    /// Nothing: consumed and counted.
    Nothing,
}

#[derive(Clone, Debug)]
pub(super) struct Ledger<P: Copy + Default> {
    entries: [Entry<P>; LEDGER_ENTRIES],
    records: [Record; LEDGER_ENTRIES],
    seq: u64,
    /// Note-ons rejected (a full ledger, or the caller's own limit).
    pub(super) rejected: u64,
    /// Anonymous rejections a full record table could only count (cleared by `reset`).
    pub(super) rejected_unrecorded: u64,
    /// Note-offs that closed nothing.
    pub(super) unmatched_releases: u64,
    /// The most entries in use at once since the fixture was made.
    pub(super) peak: usize,
}

impl<P: Copy + Default> Default for Ledger<P> {
    fn default() -> Self {
        Ledger {
            entries: [Entry::default(); LEDGER_ENTRIES],
            records: [Record::default(); LEDGER_ENTRIES],
            seq: 0,
            rejected: 0,
            rejected_unrecorded: 0,
            unmatched_releases: 0,
            peak: 0,
        }
    }
}

impl<P: Copy + Default> Ledger<P> {
    /// Entries in use, Open or Orphaned.
    pub(super) fn open(&self) -> usize {
        self.entries.iter().filter(|e| e.used).count()
    }

    /// Anonymous rejection records in use.
    pub(super) fn records(&self) -> usize {
        self.records.iter().filter(|r| r.used).count()
    }

    /// A note-on: its entry's index, or `None` when it is rejected (`room`: the caller's own
    /// limits allow it, such as a schedule's reservation).
    pub(super) fn admit(&mut self, channel: u8, key: u8, voice: i32, payload: P, room: bool) -> Option<usize> {
        let free = self.entries.iter().position(|e| !e.used).filter(|_| room);
        self.seq += 1;
        let Some(i) = free else {
            self.rejected = self.rejected.saturating_add(1);
            if voice == crate::event::PluginEvent::NO_ID {
                match self.records.iter_mut().find(|r| !r.used) {
                    Some(r) => *r = Record { used: true, channel, key, seq: self.seq },
                    None => self.rejected_unrecorded = self.rejected_unrecorded.saturating_add(1),
                }
            }
            return None;
        };
        if let Some(e) = self.entries.get_mut(i) {
            *e = Entry { used: true, orphaned: false, channel, key, voice, seq: self.seq, payload };
        }
        self.peak = self.peak.max(self.open());
        Some(i)
    }

    /// The entry's payload, to be changed in place.
    pub(super) fn payload_mut(&mut self, i: usize) -> Option<&mut P> {
        self.entries.get_mut(i).filter(|e| e.used).map(|e| &mut e.payload)
    }

    /// The device ended or cancelled everything it owed entry `i`: its note-off is consumed.
    pub(super) fn orphan(&mut self, i: usize) {
        if let Some(e) = self.entries.get_mut(i).filter(|e| e.used) {
            e.orphaned = true;
        }
    }

    /// A note-off of `channel` and `key` with voice id `voice`.
    pub(super) fn close(&mut self, channel: u8, key: u8, voice: i32) -> Closed<P> {
        if voice != crate::event::PluginEvent::NO_ID {
            let Some(e) = self.entries.iter_mut().find(|e| e.used && e.channel == channel && e.voice == voice) else {
                self.unmatched_releases = self.unmatched_releases.saturating_add(1);
                return Closed::Nothing;
            };
            let closed = if e.orphaned { Closed::Orphaned } else { Closed::Open(e.payload) };
            *e = Entry::default();
            return closed;
        }
        let anonymous = |c: u8, k: u8, v: i32| c == channel && k == key && v == crate::event::PluginEvent::NO_ID;
        let entry = self.entries.iter().enumerate().filter(|(_, e)| e.used && anonymous(e.channel, e.key, e.voice)).min_by_key(|(_, e)| e.seq);
        let record = self.records.iter().enumerate().filter(|(_, r)| r.used && r.channel == channel && r.key == key).min_by_key(|(_, r)| r.seq);
        match (entry.map(|(i, e)| (i, e.seq)), record.map(|(i, r)| (i, r.seq))) {
            (Some((i, s)), r) if r.is_none_or(|(_, rs)| s < rs) => {
                let Some(e) = self.entries.get_mut(i) else { return Closed::Nothing };
                let closed = if e.orphaned { Closed::Orphaned } else { Closed::Open(e.payload) };
                *e = Entry::default();
                closed
            }
            (_, Some((i, _))) => {
                if let Some(r) = self.records.get_mut(i) {
                    *r = Record::default();
                }
                Closed::Record
            }
            _ => {
                self.unmatched_releases = self.unmatched_releases.saturating_add(1);
                Closed::Nothing
            }
        }
    }

    /// Every host reset: the entries, the records and the saturation count.
    pub(super) fn reset(&mut self) {
        self.entries = [Entry::default(); LEDGER_ENTRIES];
        self.records = [Record::default(); LEDGER_ENTRIES];
        self.rejected_unrecorded = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::PluginEvent;

    const NO: i32 = PluginEvent::NO_ID;

    #[test]
    fn an_anonymous_rejection_closes_in_its_oldest_first_place() {
        // Decision 13, "The ledger (K1)": 256 anonymous entries of key 60, an anonymous rejection
        // (recorded), one note-off (closes the first), an admitted note, then the rest in order.
        let mut l: Ledger<()> = Ledger::default();
        for _ in 0..LEDGER_ENTRIES {
            assert!(l.admit(0, 60, NO, (), true).is_some());
        }
        assert!(l.admit(0, 60, NO, (), true).is_none());
        assert_eq!((l.rejected, l.records()), (1, 1));
        assert_eq!(l.close(0, 60, NO), Closed::Open(()));
        assert!(l.admit(0, 60, NO, (), true).is_some());
        for _ in 1..LEDGER_ENTRIES {
            assert_eq!(l.close(0, 60, NO), Closed::Open(()));
        }
        assert_eq!(l.close(0, 60, NO), Closed::Record, "the rejected note is older than the admitted one");
        assert_eq!(l.close(0, 60, NO), Closed::Open(()));
        assert_eq!(l.close(0, 60, NO), Closed::Nothing);
        assert_eq!((l.open(), l.records(), l.unmatched_releases, l.peak), (0, 0, 1, LEDGER_ENTRIES));
    }

    #[test]
    fn an_identified_release_closes_its_own_entry_only() {
        let mut l: Ledger<u8> = Ledger::default();
        let a = l.admit(0, 60, NO, 1, true).unwrap();
        l.admit(0, 60, 7, 2, true).unwrap();
        l.orphan(a);
        assert_eq!(l.close(0, 60, 8), Closed::Nothing, "no fallback to an anonymous entry");
        assert_eq!(l.close(1, 60, 7), Closed::Nothing, "another channel");
        assert_eq!(l.close(0, 61, 7), Closed::Open(2), "by its id, whatever the key");
        assert_eq!(l.close(0, 60, NO), Closed::Orphaned);
        assert!(l.admit(0, 60, 9, 3, false).is_none(), "no room by the caller's limit");
        assert_eq!(l.records(), 0, "an identified rejection keeps no record");
    }
}
