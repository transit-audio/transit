//! The stage ledger at a hosted MIDI effect stage's output (ADR 0064, decision 5): host code, so
//! the plug-in is held to it without being trusted.
//!
//! - **Entries and host ids (5.1).** Every note-on the stage writes and its output accepts opens an
//!   entry (channel, key, the plug-in's note id or -1, a host id, a sequence number) and leaves the
//!   stage with the host id in `note_id`: a 31-bit counter (the chain's, so it runs on across
//!   barriers and instances) that skips any id still open in this ledger. Every
//!   note-off and poly pressure written for an entry carries its host id.
//! - **Matching (5.3).** An identified release (a plug-in id, channel and key given) closes the
//!   oldest open entry with that channel, key and id; an anonymous single release (no id) closes the
//!   oldest open entry of its channel and key, identified or not; a wildcard release (any of id,
//!   channel or key a wildcard) closes every open entry matching its other fields, oldest first,
//!   each with its own note-off. Poly pressure goes to the entries the same rules select and
//!   changes no ownership. A release or pressure that matches no open entry and no rejection
//!   record is consumed and counted (`unmatched`).
//! - **Rejected note-ons (5.6).** With [`MIDI_FX_NOTES`] entries open, or when the stage's output
//!   refuses it, a note-on is rejected: not written, and recorded (channel, key, id, sequence) in
//!   a table of [`MIDI_FX_NOTES`], so its release finds it in its place: an identified release
//!   matches records of its id, an anonymous one records without an id, by sequence among the
//!   entries; a wildcard clears every matching record. A release that closes a record writes
//!   nothing. With the table full a rejection is only counted (`unrecorded`).
//! - **A refused note-off (5.7).** A note-off the stage's output refuses leaves its entry open,
//!   owed: [`Ledger::owed`] writes it again at offset 0 ahead of the stage's next output, and an
//!   owed entry matches no later release or pressure.
//!
//! The ledger is cleared at every host barrier (the stage's `clear_ledger`). Nothing here
//! allocates.

use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::midi_fx::TimedEvent;

use super::MIDI_FX_NOTES;
use super::output::Output;

/// An open note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) used: bool,
    pub(super) channel: u8,
    pub(super) key: u8,
    /// The plug-in's note id, -1 for an anonymous note.
    pub(super) pid: i32,
    pub(super) host: i32,
    pub(super) seq: u64,
    /// The note-off the output refused, owed with its velocity (5.7).
    pub(super) owed: Option<u8>,
}

const NO_ENTRY: Entry = Entry { used: false, channel: 0, key: 0, pid: -1, host: -1, seq: 0, owed: None };

/// A rejected note-on, so its release finds it in its place (5.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub(super) used: bool,
    pub(super) channel: u8,
    pub(super) key: u8,
    pub(super) pid: i32,
    pub(super) seq: u64,
}

const NO_RECORD: Record = Record { used: false, channel: 0, key: 0, pid: -1, seq: 0 };

/// Which open notes a release or a pressure addresses (5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sel {
    Identified {
        channel: u8,
        key: u8,
        pid: i32,
    },
    Anonymous {
        channel: u8,
        key: u8,
    },
    /// `None`: that field is a wildcard.
    Wildcard {
        channel: Option<u8>,
        key: Option<u8>,
        pid: Option<i32>,
    },
}

impl Sel {
    fn entry(self, e: &Entry) -> bool {
        e.used
            && e.owed.is_none()
            && match self {
                Sel::Identified { channel, key, pid } => e.channel == channel && e.key == key && e.pid == pid,
                Sel::Anonymous { channel, key } => e.channel == channel && e.key == key,
                Sel::Wildcard { channel, key, pid } => {
                    channel.is_none_or(|c| e.channel == c) && key.is_none_or(|k| e.key == k) && pid.is_none_or(|p| e.pid == p)
                }
            }
    }

    fn record(self, r: &Record) -> bool {
        r.used
            && match self {
                Sel::Identified { channel, key, pid } => r.channel == channel && r.key == key && r.pid == pid,
                // Records are not matched by the anonymous fallback to identified notes (5.6).
                Sel::Anonymous { channel, key } => r.channel == channel && r.key == key && r.pid == -1,
                Sel::Wildcard { channel, key, pid } => {
                    channel.is_none_or(|c| r.channel == c) && key.is_none_or(|k| r.key == k) && pid.is_none_or(|p| r.pid == p)
                }
            }
    }
}

/// What the ledger counted while matching one block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Tally {
    /// Releases and pressures that matched nothing (5.3).
    pub(super) unmatched: u64,
    /// Note-ons rejected because every entry was open (an output refusal is counted by the output).
    pub(super) full: u64,
    /// Rejections the full record table could not keep.
    pub(super) unrecorded: u64,
}

pub(crate) struct Ledger {
    open: [Entry; MIDI_FX_NOTES],
    records: [Record; MIDI_FX_NOTES],
    /// The last sequence number given to an entry or a record.
    seq: u64,
}

impl Ledger {
    pub(super) const fn new() -> Self {
        Ledger { open: [NO_ENTRY; MIDI_FX_NOTES], records: [NO_RECORD; MIDI_FX_NOTES], seq: 0 }
    }

    /// A barrier: every entry, record and owed release is gone.
    pub(super) fn clear(&mut self) {
        *self = Ledger::new();
    }

    /// Open entries (owed ones included).
    #[cfg(test)]
    pub(super) fn open(&self) -> impl Iterator<Item = &Entry> + '_ {
        self.open.iter().filter(|e| e.used)
    }

    /// Kept rejection records.
    #[cfg(test)]
    pub(super) fn records(&self) -> impl Iterator<Item = &Record> + '_ {
        self.records.iter().filter(|r| r.used)
    }

    fn next_seq(&mut self) -> u64 {
        if self.seq == u64::MAX {
            self.renumber();
        }
        self.seq += 1;
        self.seq
    }

    /// Renumbers the live entries and records to 1..n in their order (at most 512; once in 2^64
    /// numbers). As the host's delivery tables do (ADR 0057 P3b).
    fn renumber(&mut self) {
        let mut taken: Option<u64> = None;
        let mut n = 0u64;
        loop {
            let live = self.open.iter().filter(|e| e.used).map(|e| e.seq).chain(self.records.iter().filter(|r| r.used).map(|r| r.seq));
            let Some(old) = live.filter(|s| taken.is_none_or(|t| *s > t)).min() else { break };
            n += 1;
            for e in self.open.iter_mut().filter(|e| e.used && e.seq == old) {
                e.seq = n;
            }
            for r in self.records.iter_mut().filter(|r| r.used && r.seq == old) {
                r.seq = n;
            }
            taken = Some(old);
        }
        self.seq = n;
    }

    /// A host id from the chain's counter `ids` that no open entry holds (at most
    /// `MIDI_FX_NOTES + 1` tries).
    fn host_id(&self, ids: &mut u32) -> i32 {
        loop {
            let id = i32::try_from(*ids & 0x7FFF_FFFF).unwrap_or(0);
            *ids = (*ids + 1) & 0x7FFF_FFFF;
            if !self.open.iter().any(|e| e.used && e.host == id) {
                return id;
            }
        }
    }

    /// The owed note-offs, at offset 0, oldest first, ahead of the stage's output (5.7).
    pub(super) fn owed(&mut self, out: &mut Output) {
        loop {
            let next = self.open.iter().enumerate().filter(|(_, e)| e.used && e.owed.is_some()).min_by_key(|(_, e)| e.seq).map(|(i, _)| i);
            let Some(i) = next else { return };
            let Some(e) = self.open.get(i).copied() else { return };
            if !out.push(note_off(0, &e, e.owed.unwrap_or(0))) {
                return;
            }
            if let Some(e) = self.open.get_mut(i) {
                *e = NO_ENTRY;
            }
        }
    }

    /// A note-on the plug-in wrote (`pid` -1: anonymous), at `offset`; `ids` the chain's host id
    /// counter.
    pub(super) fn on(&mut self, channel: u8, key: u8, pid: i32, velocity: u8, offset: u32, ids: &mut u32, out: &mut Output, tally: &mut Tally) {
        let Some(free) = self.open.iter().position(|e| !e.used) else {
            tally.full += 1;
            self.reject(channel, key, pid, tally);
            return;
        };
        let host = self.host_id(ids);
        let ev = PluginEvent { channel, note_id: host, message: Message::NoteOn { key, velocity, id: NoteId::NONE } };
        if !out.push(TimedEvent { offset, event: ev }) {
            self.reject(channel, key, pid, tally);
            return;
        }
        let seq = self.next_seq();
        if let Some(e) = self.open.get_mut(free) {
            *e = Entry { used: true, channel, key, pid, host, seq, owed: None };
        }
    }

    fn reject(&mut self, channel: u8, key: u8, pid: i32, tally: &mut Tally) {
        let Some(free) = self.records.iter().position(|r| !r.used) else {
            tally.unrecorded += 1;
            return;
        };
        let seq = self.next_seq();
        if let Some(r) = self.records.get_mut(free) {
            *r = Record { used: true, channel, key, pid, seq };
        }
    }

    /// A release the plug-in wrote, matched as 5.3 and 5.6 say.
    pub(super) fn release(&mut self, sel: Sel, velocity: u8, offset: u32, out: &mut Output, tally: &mut Tally) {
        if let Sel::Wildcard { .. } = sel {
            let mut any = false;
            for r in self.records.iter_mut().filter(|r| sel.record(r)) {
                *r = NO_RECORD;
                any = true;
            }
            while let Some(i) = self.oldest_entry(sel) {
                any = true;
                self.close(i, velocity, offset, out);
            }
            if !any {
                tally.unmatched += 1;
            }
            return;
        }
        let entry = self.oldest_entry(sel);
        let record = self.records.iter().enumerate().filter(|(_, r)| sel.record(r)).min_by_key(|(_, r)| r.seq).map(|(i, r)| (i, r.seq));
        let entry_seq = entry.and_then(|i| self.open.get(i)).map(|e| e.seq);
        match (entry, entry_seq, record) {
            (Some(i), Some(s), r) if r.is_none_or(|(_, rs)| s < rs) => self.close(i, velocity, offset, out),
            (_, _, Some((j, _))) => {
                if let Some(r) = self.records.get_mut(j) {
                    *r = NO_RECORD;
                }
            }
            _ => tally.unmatched += 1,
        }
    }

    /// A poly pressure the plug-in wrote: to every entry the rules select (one for an identified or
    /// anonymous address).
    pub(super) fn pressure(&mut self, sel: Sel, value: u8, offset: u32, out: &mut Output, tally: &mut Tally) {
        let mut any = false;
        match sel {
            Sel::Wildcard { .. } => {
                for e in self.open.iter().filter(|e| sel.entry(e)) {
                    any = true;
                    let ev =
                        PluginEvent { channel: e.channel, note_id: e.host, message: Message::PolyPressure { key: e.key, value, id: NoteId::NONE } };
                    out.push(TimedEvent { offset, event: ev });
                }
            }
            _ => {
                if let Some(e) = self.oldest_entry(sel).and_then(|i| self.open.get(i)) {
                    any = true;
                    let ev =
                        PluginEvent { channel: e.channel, note_id: e.host, message: Message::PolyPressure { key: e.key, value, id: NoteId::NONE } };
                    out.push(TimedEvent { offset, event: ev });
                }
            }
        }
        if !any && !self.records.iter().any(|r| sel.record(r)) {
            tally.unmatched += 1;
        }
    }

    fn oldest_entry(&self, sel: Sel) -> Option<usize> {
        self.open.iter().enumerate().filter(|(_, e)| sel.entry(e)).min_by_key(|(_, e)| e.seq).map(|(i, _)| i)
    }

    /// Writes entry `i`'s note-off; a refused one leaves it owed.
    fn close(&mut self, i: usize, velocity: u8, offset: u32, out: &mut Output) {
        let Some(e) = self.open.get(i).copied() else { return };
        let accepted = out.push(note_off(offset, &e, velocity));
        if let Some(slot) = self.open.get_mut(i) {
            if accepted {
                *slot = NO_ENTRY;
            } else {
                slot.owed = Some(velocity);
            }
        }
    }

    /// Test seam (feature `testing`): the ledger's state record.
    #[cfg(feature = "testing")]
    pub(super) fn record_state(&self, w: &mut transit_state_record::Writer<'_>) {
        for e in &self.open {
            w.bool(e.used);
            if e.used {
                w.u8(e.channel);
                w.u8(e.key);
                w.i32(e.pid);
                w.i32(e.host);
                w.u64(e.seq);
                w.present(e.owed, |w, v| w.u8(v));
            }
        }
        for r in &self.records {
            w.bool(r.used);
            if r.used {
                w.u8(r.channel);
                w.u8(r.key);
                w.i32(r.pid);
                w.u64(r.seq);
            }
        }
        w.u64(self.seq);
    }
}

fn note_off(offset: u32, e: &Entry, velocity: u8) -> TimedEvent {
    TimedEvent {
        offset,
        event: PluginEvent { channel: e.channel, note_id: e.host, message: Message::NoteOff { key: e.key, velocity, id: NoteId::NONE } },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn fill(out: &mut Output, n: usize) {
        for k in 0..n {
            let ev = PluginEvent { channel: 9, note_id: -1, message: Message::NoteOff { key: (k % 128) as u8, velocity: 0, id: NoteId::NONE } };
            assert!(out.push(TimedEvent { offset: 0, event: ev }));
        }
    }

    /// 5.7 at the ledger: a note-off the output refuses leaves its entry owed; the owed note-off is
    /// written at offset 0 ahead of the next output; an owed entry matches no later release; a
    /// later note-on of its key opens its own entry.
    #[test]
    fn a_refused_note_off_is_owed_and_written_first_next_time() {
        let (mut l, mut out, mut t, mut ids) = (Ledger::new(), Output::new(), Tally::default(), 0u32);
        l.on(0, 60, 7, 100, 3, &mut ids, &mut out, &mut t);
        out.clear();
        fill(&mut out, crate::midi_fx::MIDI_FX_BLOCK);
        l.release(Sel::Identified { channel: 0, key: 60, pid: 7 }, 0, 9, &mut out, &mut t);
        assert_eq!(l.open().map(|e| (e.host, e.owed)).collect::<Vec<_>>(), vec![(0, Some(0))], "owed");
        // An owed entry matches no later release, and a new note-on of its key opens its own entry.
        out.clear();
        l.release(Sel::Anonymous { channel: 0, key: 60 }, 0, 1, &mut out, &mut t);
        assert_eq!((out.events().count(), t.unmatched), (0, 1));
        l.on(0, 60, -1, 90, 2, &mut ids, &mut out, &mut t);
        assert_eq!(l.open().count(), 2);
        // The next block's output: the owed note-off first, at offset 0.
        out.clear();
        l.owed(&mut out);
        l.release(Sel::Anonymous { channel: 0, key: 60 }, 0, 5, &mut out, &mut t);
        let got: Vec<(u32, i32, bool)> =
            out.events().map(|e| (e.offset, e.event.note_id, matches!(e.event.message, Message::NoteOff { .. }))).collect();
        assert_eq!(got, vec![(0, 0, true), (5, 1, true)]);
        assert_eq!(l.open().count(), 0);
    }

    #[test]
    fn host_ids_skip_ids_still_open_and_the_counter_is_the_callers() {
        let (mut l, mut out, mut t) = (Ledger::new(), Output::new(), Tally::default());
        let mut ids = 0x7FFF_FFFF;
        l.on(0, 1, -1, 1, 0, &mut ids, &mut out, &mut t);
        l.on(0, 2, -1, 1, 0, &mut ids, &mut out, &mut t);
        assert_eq!(l.open().map(|e| e.host).collect::<Vec<_>>(), vec![0x7FFF_FFFF, 0], "31 bits, wrapping, never -1");
        let mut ids = 0;
        l.on(0, 3, -1, 1, 0, &mut ids, &mut out, &mut t);
        assert_eq!(l.open().map(|e| e.host).collect::<Vec<_>>(), vec![0x7FFF_FFFF, 0, 1], "0 is open: skipped");
        assert!(l.records().next().is_none());
    }
}
