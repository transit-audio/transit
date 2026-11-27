//! A hosted adapter's event queue under ADR 0059: the [`Ledger`] for admission and ownership, the
//! ordinary allowance, the 48 reserved safety-controller slots, and the native list sized for the
//! whole ledger (Decision 1), delivered in time order with arrival order at one offset (3b), kept
//! through skipped calls at offset 0 (3a), and released at the panic, Stop and a processing error
//! (Decisions 2 and 4).
//!
//! The adapter's side of one block:
//! 1. [`Delivery::prepare`] with the block's frames;
//! 2. per native sub-call: [`Delivery::batch`] up to the sub-call's end, translate it, call the
//!    plug-in, then [`Delivery::finish`] with whether the call succeeded;
//! 3. [`Delivery::carry`] after the block (and in place of 1 and 2 when the call is skipped):
//!    what stays queued moves to offset 0 in arrival order.
//!
//! With `separate` (VST3, 3b), any two events of one channel at one frame whose order changes voice
//! state go on successive frames in arrival order, with bounded service: an event that must wait
//! is never overtaken by a later arrival of its channel, every frame serves the first waiting event
//! of each channel, and past a call's end the wait continues at offset 0 of the next call.
//!
//! Fixed-size and allocation-free after [`Delivery::new`]: real-time safe.

use super::ledger::{ENTRIES, Ledger, Owner, Settle, Verdict};
use crate::event::{CC_ALL_NOTES_OFF, CC_ALL_SOUND_OFF, CC_RESET_CONTROLLERS, Message, PluginEvent, is_pedal};

/// Native-event slots for events that own nothing (today's 1,024-event queues, kept): an ordinary
/// host event takes as many as it becomes in the format.
pub const ORDINARY: usize = 1024;
/// One reserved slot per channel and safety controller (CC 120, 121, 123).
pub const SAFETY_SLOTS: usize = 16 * 3;
/// The native list: the whole ledger at two events per entry, the ordinary allowance and the
/// safety slots (ADR 0059, Decision 1: 2,480 events).
pub const NATIVE_EVENTS: usize = 2 * ENTRIES + ORDINARY + SAFETY_SLOTS;

/// What a queued event is to the ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// The start of entry `.0`.
    Start(u16),
    /// The release of entry `.0`.
    Release(u16),
    /// A safety controller forwarded from reserved slot `.0`.
    Slot(u8),
    /// An event that owns nothing, counted against the ordinary allowance.
    Ordinary,
}

/// One queued event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    /// Sample offset in the next call (after [`Delivery::prepare`]: the frame it is delivered at).
    pub offset: u32,
    /// Arrival order.
    pub seq: u64,
    pub event: PluginEvent,
    /// The entry's native id, or [`PluginEvent::NO_ID`] for events that own nothing.
    pub native: i32,
    pub class: Class,
    /// Native events this host event becomes (an ordinary event reserves them all at admission).
    pub slots: u8,
}

impl Item {
    /// A note event (note-on, note-off, poly pressure).
    #[must_use]
    pub fn is_note(&self) -> bool {
        matches!(self.event.message, Message::NoteOn { .. } | Message::NoteOff { .. } | Message::PolyPressure { .. })
    }

    /// A controller-borne voice event (3b): a pedal edge (a press or a release, implied ones
    /// included) or CC 120, 121 or 123.
    #[must_use]
    pub fn is_controller_voice(&self) -> bool {
        match self.event.message {
            Message::Control { cc, .. } if is_pedal(cc) => matches!(self.class, Class::Start(_) | Class::Release(_)),
            Message::Control { cc, .. } => matches!(cc, CC_ALL_SOUND_OFF | CC_RESET_CONTROLLERS | CC_ALL_NOTES_OFF),
            _ => false,
        }
    }
}

/// Refusals, per instance; they only grow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refusals {
    /// Starts refused at a full ledger.
    pub starts: u64,
    /// Ordinary events refused at a full allowance.
    pub ordinary: u64,
    /// Safety controllers whose implied releases were stored but whose own forwarding was refused
    /// (a repeat of one kind and channel before a call, at a full allowance: partial acceptance).
    pub forwarding: u64,
}

/// The queue.
#[derive(Debug, Clone)]
pub struct Delivery {
    ledger: Ledger,
    /// Sorted by offset, then arrival.
    items: Vec<Item>,
    ordinary: usize,
    slots: [bool; SAFETY_SLOTS],
    seq: u64,
    /// Items of the current sub-call (a prefix of `items`).
    batch: usize,
    separate: bool,
    /// Native events an ordinary host event becomes in this format (a CLAP `Control14`: two).
    expand: fn(&Message) -> u8,
    pub refusals: Refusals,
}

const fn slot_of(channel: u8, cc: u8) -> Option<usize> {
    let k = match cc {
        CC_ALL_SOUND_OFF => 0,
        CC_RESET_CONTROLLERS => 1,
        CC_ALL_NOTES_OFF => 2,
        _ => return None,
    };
    Some((channel as usize & 0x0F) * 3 + k)
}

impl Delivery {
    /// An empty queue, its storage allocated here (control thread). `separate`: VST3's rule of 3b.
    /// Every host event becomes one native event.
    #[must_use]
    pub fn new(separate: bool) -> Self {
        Self::with_expansion(separate, |_| 1)
    }

    /// [`Delivery::new`] for a format where an ordinary host event can become several native
    /// events (`expand`, at least one): the ordinary allowance counts native slots, and an ordinary
    /// event reserves its whole expansion at admission or is refused whole.
    #[must_use]
    pub fn with_expansion(separate: bool, expand: fn(&Message) -> u8) -> Self {
        Self {
            ledger: Ledger::new(),
            items: Vec::with_capacity(NATIVE_EVENTS),
            ordinary: 0,
            slots: [false; SAFETY_SLOTS],
            seq: 0,
            batch: 0,
            separate,
            expand,
            refusals: Refusals::default(),
        }
    }

    /// The ledger (read only).
    #[must_use]
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// The native list's allocated capacity, in items (storage accounting).
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.items.capacity()
    }

    /// Everything queued, in delivery order.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// Ordinary events queued.
    #[must_use]
    pub fn ordinary(&self) -> usize {
        self.ordinary
    }

    fn insert(items: &mut Vec<Item>, seq: &mut u64, offset: u32, event: PluginEvent, native: i32, class: Class) -> bool {
        Self::insert_slots(items, seq, offset, event, native, class, 1)
    }

    fn insert_slots(items: &mut Vec<Item>, seq: &mut u64, offset: u32, event: PluginEvent, native: i32, class: Class, slots: u8) -> bool {
        if items.len() >= items.capacity() {
            return false;
        }
        *seq = seq.wrapping_add(1);
        let item = Item { offset, seq: *seq, event, native, class, slots };
        let pos = items.iter().rposition(|q| q.offset <= offset).map_or(0, |i| i + 1);
        items.insert(pos, item);
        true
    }

    /// Admits `ev` at `offset`. `true` means stored: a start with an entry for its release, a
    /// release in its entry (never refused), a release of nothing (accepted as nothing), a safety
    /// controller's implied releases (its own forwarding may be refused, counted), an ordinary event
    /// within the allowance. `false` at once for a start at a full ledger or an ordinary event at a
    /// full allowance (counted).
    pub fn admit(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        match self.ledger.admit(ev) {
            Verdict::Start(i) => {
                let native = self.ledger.get(i).map_or(PluginEvent::NO_ID, |e| e.native);
                if Self::insert(&mut self.items, &mut self.seq, offset, *ev, native, Class::Start(i)) {
                    true
                } else {
                    self.ledger.free_entry(i);
                    self.refusals.starts = self.refusals.starts.saturating_add(1);
                    false
                }
            }
            Verdict::Release(i) => {
                let native = self.ledger.get(i).map_or(PluginEvent::NO_ID, |e| e.native);
                // Never ahead of its own start, whatever offsets the host gave them.
                let at = self.items.iter().find(|q| q.class == Class::Start(i)).map_or(offset, |s| s.offset.max(offset));
                Self::insert(&mut self.items, &mut self.seq, at, *ev, native, Class::Release(i));
                true
            }
            Verdict::Nothing => true,
            Verdict::Safety => {
                self.safety(offset, ev);
                true
            }
            Verdict::Ordinary => {
                let native = match ev.message {
                    Message::PolyPressure { key, .. } => self.ledger.native_of(ev.channel, key, ev.note_id),
                    _ => PluginEvent::NO_ID,
                };
                let n = (self.expand)(&ev.message).max(1);
                if self.ordinary + usize::from(n) <= ORDINARY
                    && Self::insert_slots(&mut self.items, &mut self.seq, offset, *ev, native, Class::Ordinary, n)
                {
                    self.ordinary += usize::from(n);
                    true
                } else {
                    self.refusals.ordinary = self.refusals.ordinary.saturating_add(1);
                    false
                }
            }
            Verdict::Refused => {
                self.refusals.starts = self.refusals.starts.saturating_add(1);
                false
            }
        }
    }

    /// A safety controller: its implied releases go into their entries at its offset, then the
    /// controller itself from its reserved slot (or the ordinary allowance for a repeat).
    fn safety(&mut self, offset: u32, ev: &PluginEvent) {
        let Message::Control { cc, .. } = ev.message else { return };
        let ch = ev.channel & 0x0F;
        let (items, seq) = (&mut self.items, &mut self.seq);
        self.ledger.imply(ch, cc, |i, e| {
            let at = items.iter().find(|q| q.class == Class::Start(i)).map_or(offset, |s| s.offset.max(offset));
            Self::insert(items, seq, at, e.release_event(), e.native, Class::Release(i));
        });
        let slot = slot_of(ch, cc);
        match slot.and_then(|s| self.slots.get_mut(s).map(|used| (s, used))) {
            Some((s, used)) if !*used => {
                *used = true;
                Self::insert(&mut self.items, &mut self.seq, offset, *ev, PluginEvent::NO_ID, Class::Slot(s as u8));
            }
            _ => {
                let n = (self.expand)(&ev.message).max(1);
                if self.ordinary + usize::from(n) <= ORDINARY
                    && Self::insert_slots(&mut self.items, &mut self.seq, offset, *ev, PluginEvent::NO_ID, Class::Ordinary, n)
                {
                    self.ordinary += usize::from(n);
                } else {
                    self.refusals.forwarding = self.refusals.forwarding.saturating_add(1);
                }
            }
        }
    }

    /// The start of a block of `frames` (more than zero): offsets past its end come to its last
    /// frame, and with `separate` every event gets the frame 3b gives it (events that wait past the
    /// block keep an offset of `frames` until [`Delivery::carry`]).
    pub fn prepare(&mut self, frames: u32) {
        let last = frames.saturating_sub(1);
        for it in &mut self.items {
            it.offset = it.offset.min(last);
        }
        if self.separate {
            self.schedule();
            for it in &mut self.items {
                it.offset = it.offset.min(frames);
            }
        }
        self.batch = 0;
    }

    /// 3b's rule: per channel, a controller-borne voice event never shares a frame with another
    /// voice-affecting event (a note event, another edge, a safety controller, an implied release),
    /// and waits in arrival order; no event of a channel goes ahead of an earlier arrival of that
    /// channel that waits (so a pedal value never precedes the edge or note event before it).
    /// Sorts in place (no allocation).
    fn schedule(&mut self) {
        let mut last: [Option<(u32, bool)>; 16] = [None; 16];
        for it in &mut self.items {
            let ch = usize::from(it.event.channel & 0x0F);
            let cb = it.is_controller_voice();
            let prev = last.get(ch).copied().flatten();
            let mut f = prev.map_or(it.offset, |(lf, _)| it.offset.max(lf));
            if cb || it.is_note() {
                if let Some((lf, lcb)) = prev
                    && f == lf
                    && (cb || lcb)
                {
                    f = lf.saturating_add(1);
                }
                if let Some(l) = last.get_mut(ch) {
                    *l = Some((f, cb));
                }
            }
            it.offset = f;
        }
        self.items.sort_unstable_by_key(|it| (it.offset, it.seq));
    }

    /// The events of the next sub-call: every queued event before `end` (offsets within the block).
    pub fn batch(&mut self, end: u32) -> &[Item] {
        let k = self.items.iter().position(|it| it.offset >= end).unwrap_or(self.items.len());
        self.batch = k;
        self.items.get(..k).unwrap_or(&[])
    }

    /// The sub-call that handed [`Delivery::batch`] returned: `ok` frees every entry whose release
    /// it delivered; a failed call is a processing error (ADR 0059, Decision 2): every entry whose
    /// start was handed, in this call or before, owes its release (queued at offset 0, including
    /// one this call handed), and every entry whose start was never handed is cancelled with its
    /// release.
    pub fn finish(&mut self, ok: bool) {
        let k = self.batch.min(self.items.len());
        for it in self.items.get(..k).unwrap_or(&[]) {
            match it.class {
                Class::Start(i) => self.ledger.start_handed(i),
                Class::Release(i) => self.ledger.release_handed(i),
                Class::Slot(s) => {
                    if let Some(used) = self.slots.get_mut(usize::from(s)) {
                        *used = false;
                    }
                }
                Class::Ordinary => self.ordinary = self.ordinary.saturating_sub(usize::from(it.slots)),
            }
        }
        self.items.drain(..k);
        self.batch = 0;
        if ok {
            self.ledger.settle();
        } else {
            self.release_all(true);
        }
    }

    /// What stays queued moves to offset 0 in arrival order: a skipped call (3a) and the events
    /// that wait past a block (3b).
    pub fn carry(&mut self) {
        for it in &mut self.items {
            it.offset = 0;
        }
        self.items.sort_unstable_by_key(|it| it.seq);
        self.batch = 0;
    }

    fn release_all(&mut self, refault: bool) {
        let (items, seq) = (&mut self.items, &mut self.seq);
        self.ledger.release_all(refault, |i, what, e| match what {
            Settle::Cancel => items.retain(|it| it.class != Class::Start(i) && it.class != Class::Release(i)),
            Settle::Owe => {
                Self::insert(items, seq, 0, e.release_event(), e.native, Class::Release(i));
            }
        });
    }

    /// The All MIDI Notes Off action (ADR 0059, Decision 4): every owned sounding voice gets a
    /// note-off and every owned pedal its release, at offset 0; starts still queued are cancelled
    /// with their releases. Nothing is reset.
    pub fn all_notes_off(&mut self) {
        self.release_all(false);
    }

    /// Stop's share (Decision 4): [`Delivery::all_notes_off`], then every queued event that owns
    /// nothing and need not survive a reset ([`Message::survives_reset`]) is dropped. Owned
    /// releases stay queued for the next actual call; their entries are freed when it succeeds.
    pub fn reset(&mut self) {
        self.release_all(false);
        let (ordinary, slots) = (&mut self.ordinary, &mut self.slots);
        self.items.retain(|it| match it.class {
            Class::Ordinary if !it.event.message.survives_reset() => {
                *ordinary = ordinary.saturating_sub(usize::from(it.slots));
                false
            }
            Class::Slot(s) if !it.event.message.survives_reset() => {
                if let Some(used) = slots.get_mut(usize::from(s)) {
                    *used = false;
                }
                false
            }
            _ => true,
        });
        self.batch = 0;
    }

    /// Owned releases waiting for delivery (a skipped call, a containment until Stop).
    #[must_use]
    pub fn owed(&self) -> usize {
        self.items.iter().filter(|it| matches!(it.class, Class::Release(_))).count()
    }

    /// Whether an entry for a note of `key` on `channel` is open (sounding or queued to start).
    #[must_use]
    pub fn holds(&self, channel: u8, key: u8) -> bool {
        self.ledger.iter().any(|(_, e)| e.open() && e.channel == channel & 0x0F && matches!(e.owner, Owner::Note { key: k, .. } if k == key))
    }
}
