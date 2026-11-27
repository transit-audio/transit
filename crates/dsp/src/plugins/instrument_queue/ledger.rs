//! The ownership ledger of ADR 0059, Decision 1: accepting a note-on or a pedal press reserves
//! its eventual release until the release is delivered in a processing call that succeeds, or a
//! proven reset. One fixed table of [`ENTRIES`] reservations per instrument instance; a start is
//! admitted only while fewer than [`ENTRIES`] are outstanding, and a release that matches an owned
//! entry is always admitted into the entry its start reserved, so it can never be refused.
//!
//! Matching (ADR 0059, Decision 1, and ADR 0057's identity matching): an anonymous note-off (voice
//! id [`PluginEvent::NO_ID`]) matches the oldest anonymous owned entry of its channel and key that
//! is not already releasing; an identified note-off matches only the entry of its own voice id. A
//! release that matches nothing owned is accepted as nothing.
//!
//! Pedals (CC 64, 66, 67, 69): a value that crosses into down (64 or above) while no press of that
//! channel and pedal is open is a press, a start; one that crosses into up while a press is open
//! is that press's release; any other value is an ordinary value change and owns nothing.
//!
//! Fixed-size and allocation-free after [`Ledger::new`]: real-time safe.

use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_RESET_CONTROLLERS, Message, PluginEvent, is_pedal};

/// Reservations an instrument instance keeps (ADR 0059, Decision 1: 320 held at a block start and
/// 384 starts in one block, under ADR 0057's capacity bound).
pub const ENTRIES: usize = 704;

/// What an entry reserves the release of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// A note: its key and the host's voice id ([`PluginEvent::NO_ID`]: anonymous).
    Note { key: u8, voice: i32 },
    /// A pedal press: the pedal's controller.
    Pedal { cc: u8 },
}

/// Where an entry's start is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// Admitted, not yet handed to the plug-in.
    Queued,
    /// Handed to the plug-in in a processing call (which may have failed).
    Handed,
}

/// Where an entry's release is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// Not admitted yet: the voice or pedal is open.
    None,
    /// Admitted (explicit, implied by a safety controller, or owed after a panic or a fault) and
    /// waiting for delivery.
    Queued,
    /// Handed in the current processing call; freed when that call succeeds.
    Handed,
}

/// One reservation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub channel: u8,
    pub owner: Owner,
    /// The adapter's native id for this entry (CLAP and VST3 send the start and the release with
    /// it, so a plug-in cannot pair them differently).
    pub native: i32,
    /// Admission order (oldest first for anonymous matching).
    pub seq: u64,
    pub start: Start,
    pub release: Release,
}

impl Entry {
    /// The release event this entry is owed: a note-off with its identity, or the pedal up.
    #[must_use]
    pub fn release_event(&self) -> PluginEvent {
        let message = match self.owner {
            Owner::Note { key, .. } => Message::NoteOff { key, velocity: 0, id: crate::event::NoteId::NONE },
            Owner::Pedal { cc } => Message::Control { cc, value: 0 },
        };
        let note_id = match self.owner {
            Owner::Note { voice, .. } => voice,
            Owner::Pedal { .. } => PluginEvent::NO_ID,
        };
        PluginEvent { channel: self.channel, note_id, message }
    }

    /// Open: neither released nor owed a release.
    #[must_use]
    pub fn open(&self) -> bool {
        self.release == Release::None
    }
}

/// What [`Ledger::release_all`] does with an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    /// Its start was never handed: cancelled with its release.
    Cancel,
    /// Its release is owed: queue it now.
    Owe,
}

/// How the ledger classifies one event at admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A start admitted into entry `.0`: queue the start.
    Start(u16),
    /// A release admitted into entry `.0`: queue it (never refused).
    Release(u16),
    /// A release that matches nothing owned: accepted as nothing.
    Nothing,
    /// CC 120, 121 or 123: the caller lists its implied releases with [`Ledger::imply`] and
    /// forwards the controller from its reserved slot.
    Safety,
    /// Owns nothing: admitted while the ordinary allowance lasts.
    Ordinary,
    /// A start with no free entry: refused.
    Refused,
}

/// The ledger: [`ENTRIES`] reservations.
#[derive(Debug, Clone)]
pub struct Ledger {
    entries: Box<[Option<Entry>]>,
    /// Free entry indices (a stack).
    free: Vec<u16>,
    seq: u64,
    native: i32,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

/// The class a pedal value belongs to.
const fn down(value: u8) -> bool {
    value >= 64
}

impl Ledger {
    /// An empty ledger, its storage allocated here (control thread).
    #[must_use]
    pub fn new() -> Self {
        let mut free = Vec::with_capacity(ENTRIES);
        free.extend((0..ENTRIES as u16).rev());
        Self { entries: vec![None; ENTRIES].into_boxed_slice(), free, seq: 0, native: 0 }
    }

    /// Entries outstanding.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        ENTRIES - self.free.len()
    }

    /// Entry `i`, if outstanding.
    #[must_use]
    pub fn get(&self, i: u16) -> Option<&Entry> {
        self.entries.get(usize::from(i)).and_then(Option::as_ref)
    }

    fn get_mut(&mut self, i: u16) -> Option<&mut Entry> {
        self.entries.get_mut(usize::from(i)).and_then(Option::as_mut)
    }

    /// Every outstanding entry with its index.
    pub fn iter(&self) -> impl Iterator<Item = (u16, &Entry)> + '_ {
        (0u16..).zip(self.entries.iter()).filter_map(|(i, e)| e.as_ref().map(|e| (i, e)))
    }

    /// The open press of `cc` on `channel`, if any.
    #[must_use]
    pub fn open_pedal(&self, channel: u8, cc: u8) -> Option<u16> {
        self.iter().find(|(_, e)| e.open() && e.channel == channel && e.owner == Owner::Pedal { cc }).map(|(i, _)| i)
    }

    /// Open notes of `key` on `channel` whose start was handed (the built-ins' "still held").
    #[must_use]
    pub fn sounding(&self, channel: u8, key: u8) -> usize {
        self.iter()
            .filter(|(_, e)| {
                e.open() && e.start == Start::Handed && e.channel == channel && matches!(e.owner, Owner::Note { key: k, .. } if k == key)
            })
            .count()
    }

    fn reserve(&mut self, channel: u8, owner: Owner) -> Option<u16> {
        let i = self.free.pop()?;
        self.seq = self.seq.wrapping_add(1);
        let native = self.native;
        self.native = if self.native == i32::MAX { 0 } else { self.native + 1 };
        let entry = Entry { channel, owner, native, seq: self.seq, start: Start::Queued, release: Release::None };
        if let Some(slot) = self.entries.get_mut(usize::from(i)) {
            *slot = Some(entry);
        }
        Some(i)
    }

    /// The owned note a note-off of `key` on `channel` with voice id `voice` releases.
    fn match_note(&self, channel: u8, key: u8, voice: i32) -> Option<u16> {
        self.iter()
            .filter(|(_, e)| e.open() && e.channel == channel && e.owner == Owner::Note { key, voice })
            .min_by_key(|(_, e)| e.seq)
            .map(|(i, _)| i)
    }

    /// Classifies `ev` and applies its effect on ownership: a start takes an entry (or is refused),
    /// a matched release is admitted into its entry. Everything else changes nothing here.
    pub fn admit(&mut self, ev: &PluginEvent) -> Verdict {
        let ch = ev.channel & 0x0F;
        match ev.message {
            Message::NoteOn { key, velocity: 0, .. } | Message::NoteOff { key, .. } => self.release(self.match_note(ch, key, ev.note_id)),
            Message::NoteOn { key, .. } => self.start(ch, Owner::Note { key, voice: ev.note_id }),
            Message::Control { cc, value } if is_pedal(cc) => match (down(value), self.open_pedal(ch, cc)) {
                (true, None) => self.start(ch, Owner::Pedal { cc }),
                (false, Some(i)) => self.release(Some(i)),
                _ => Verdict::Ordinary,
            },
            Message::Control { cc: CC_ALL_SOUND_OFF | CC_RESET_CONTROLLERS | CC_ALL_NOTES_OFF, .. } => Verdict::Safety,
            _ => Verdict::Ordinary,
        }
    }

    fn start(&mut self, channel: u8, owner: Owner) -> Verdict {
        self.reserve(channel, owner).map_or(Verdict::Refused, Verdict::Start)
    }

    fn release(&mut self, i: Option<u16>) -> Verdict {
        match i.and_then(|i| self.get_mut(i).map(|e| (i, e))) {
            Some((i, e)) => {
                e.release = Release::Queued;
                Verdict::Release(i)
            }
            None => Verdict::Nothing,
        }
    }

    /// The releases safety controller `cc` implies on `channel` (CC 120 and 123: every open note;
    /// CC 121: every open pedal): each is admitted into its entry and `f` gets its index.
    pub fn imply(&mut self, channel: u8, cc: u8, mut f: impl FnMut(u16, &Entry)) {
        let ch = channel & 0x0F;
        let notes = matches!(cc, CC_ALL_SOUND_OFF | CC_ALL_NOTES_OFF);
        let pedals = cc == CC_RESET_CONTROLLERS;
        // Oldest first, so implied releases keep their starts' order. Each pass closes one entry.
        loop {
            let next = self
                .iter()
                .filter(|(_, e)| {
                    e.open()
                        && e.channel == ch
                        && ((notes && matches!(e.owner, Owner::Note { .. })) || (pedals && matches!(e.owner, Owner::Pedal { .. })))
                })
                .min_by_key(|(_, e)| e.seq)
                .map(|(i, _)| i);
            let Some(i) = next else { break };
            if let Some(e) = self.get_mut(i) {
                e.release = Release::Queued;
                let e = *e;
                f(i, &e);
            }
        }
    }

    /// Entry `i`'s start was handed to the plug-in.
    pub fn start_handed(&mut self, i: u16) {
        if let Some(e) = self.get_mut(i) {
            e.start = Start::Handed;
        }
    }

    /// Entry `i`'s release was handed to the plug-in in the current call.
    pub fn release_handed(&mut self, i: u16) {
        if let Some(e) = self.get_mut(i)
            && e.release == Release::Queued
        {
            e.release = Release::Handed;
        }
    }

    /// The current call succeeded: every entry whose release it handed is freed.
    pub fn settle(&mut self) {
        for i in 0..ENTRIES as u16 {
            if self.get(i).is_some_and(|e| e.release == Release::Handed) {
                self.free_entry(i);
            }
        }
    }

    /// Frees entry `i` (its release delivered in a call that succeeded, a cancelled start, a proven
    /// reset).
    pub fn free_entry(&mut self, i: u16) {
        if let Some(slot) = self.entries.get_mut(usize::from(i))
            && slot.take().is_some()
            && self.free.len() < self.free.capacity()
        {
            self.free.push(i);
        }
    }

    /// The panic, Stop and a processing error (ADR 0059, Decisions 2 and 4): an entry whose start
    /// was never handed is cancelled together with its release (`f(i, Settle::Cancel, entry)`, then
    /// it is freed); every other entry owes its release (`f(i, Settle::Owe, entry)` for each whose
    /// release must be queued now, not already queued). With `refault`, a release handed in the
    /// current call is owed again (a failed call may not have applied it).
    pub fn release_all(&mut self, refault: bool, mut f: impl FnMut(u16, Settle, &Entry)) {
        for i in 0..ENTRIES as u16 {
            let Some(e) = self.get_mut(i) else { continue };
            if e.start == Start::Queued {
                let e = *e;
                f(i, Settle::Cancel, &e);
                self.free_entry(i);
                continue;
            }
            let again = refault && e.release == Release::Handed;
            if e.release == Release::None || again {
                e.release = Release::Queued;
                let e = *e;
                f(i, Settle::Owe, &e);
            }
        }
    }

    /// A proven reset (a built-in's power-on reset, an instance's retirement): nothing is owed.
    pub fn clear(&mut self) {
        for i in 0..ENTRIES as u16 {
            self.free_entry(i);
        }
    }
}
