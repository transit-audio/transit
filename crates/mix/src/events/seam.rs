//! Test seam (feature `testing`, issue #210; ADR 0029, amendment for #742, section 14): an
//! instrument slot's event state in the canonical pending form, for the strip prints of
//! [`crate::fingerprint`].
//!
//! **The canonical pending form.** A render worker's pool parks an away strip's events outside
//! its queue and admits them again when the strip is back (`StripPool::deliver`), so the raw
//! fields of a strip whose events were parked differ from those of a strip that queued them.
//! The form is one ordered sequence: the queue, then the pool's deferred ring, then an away
//! strip's live events. Parked events are admitted, in that order, into a copy of the queue and
//! its admission ledger through the admission the engine uses ([`EngineEvents::admission`], then
//! `InstrumentEvents::admit`: due-sorted insertion, coalescing, re-strike charges), at the strip's
//! next block start, after the checks readmission makes (epoch, revocation, binding); the copy is
//! then written. Due times are written relative to that start: an overdue one is normalized to
//! it (0), a future one is preserved. The note and pedal ledgers are written in table order and
//! a queued action names its entry by table index, because release paths walk the tables in
//! order and so the order reaches the output (ruling W-3 of 2026-11-17). A deferred
//! ring's safety release is its own entry with its silence mask (delivery calls
//! `release_instrument` for it); what is parked behind it is written as it stands, not admitted
//! (it is admitted only after that release, a block later).

use super::engine::{Admission, EngineEvents};
use super::{Action, InstrumentEvent, InstrumentEvents, NoteEntry, PedalEntry, Queued};
use crate::fingerprint::{message_record, source_record};
use crate::strip_pool::{Parked, ParkedEvent};
use crate::{Due, INSTRUMENT_SLOT, RenderFrame, SlotGen};
use transit_state_record::Writer;

/// A frame relative to the next block start: an overdue one is normalized to it. Asserts that a
/// raw timestamp the form omits (one normalized) was due at or before the start (section 14).
fn rel(due: u64, start: u64) -> u64 {
    let r = due.saturating_sub(start);
    assert!(r > 0 || due <= start, "a timestamp past the next block start was omitted");
    r
}

/// The admission a parked event gets when the strip is back: readmission's checks (epoch,
/// revocation, binding), then the engine's admission at the landing frame (the nominal due time,
/// normalized to the next block start when overdue). `None` when it would be refused.
fn readmission(ev: &InstrumentEvent, start: u64, engine: &EngineEvents, binding: SlotGen) -> Option<Admission> {
    if engine.precheck(ev).is_some() {
        return None;
    }
    let (_, slot, generation) = ev.target;
    if slot != INSTRUMENT_SLOT || generation == SlotGen::VACANT || generation != binding {
        return None;
    }
    let due = match ev.due {
        Due::Next => start,
        Due::Frame(RenderFrame(f)) => f.max(start),
    };
    engine.admission(&InstrumentEvent { due: Due::Frame(RenderFrame(due)), ..*ev }).ok()
}

impl InstrumentEvents {
    /// The event state in the canonical pending form (see the module docs), with `parked` (what
    /// the pool holds for the strip), admitted against `engine`'s epoch, revocations and
    /// transport at `start` (the next block's first frame: the engine's render clock), for the
    /// instrument instance `binding`; then the MIDI effect host.
    pub(crate) fn record_canonical(&self, w: &mut Writer<'_>, start: u64, parked: &Parked, engine: &EngineEvents, binding: SlotGen) {
        let mut rest = parked.events.iter().peekable();
        if parked.events.is_empty() && parked.silence.is_none() {
            self.record_ledger(w, start);
        } else {
            let mut copy = self.ledger_copy();
            let mut status = super::EventStatus::default();
            if parked.silence.is_none() {
                while let Some(p) = rest.next_if(|p| !matches!(p, ParkedEvent::Release)) {
                    let ParkedEvent::Event(ev) = p else { continue };
                    if let Some(Admission { due, transport, message, .. }) = readmission(ev, start, engine, binding) {
                        copy.admit(due, ev.source, transport, message, &mut status);
                    }
                }
            }
            copy.record_ledger(w, start);
        }
        // Not admitted: a safety release and what is parked behind it.
        let left: Vec<&ParkedEvent> = rest.collect();
        w.usize(usize::from(parked.silence.is_some()) + left.len());
        if let Some(mask) = parked.silence {
            w.u8(0);
            w.u16(mask);
        }
        for p in left {
            match p {
                ParkedEvent::Release => w.u8(1),
                ParkedEvent::Dropped(bytes) => {
                    w.u8(2);
                    w.bytes(bytes);
                }
                ParkedEvent::Event(ev) => {
                    w.u8(3);
                    let InstrumentEvent { engine: _, target: _, due, transport, source, message, arrival: _ } = ev;
                    w.u64(match due {
                        Due::Next => 0,
                        Due::Frame(RenderFrame(f)) => rel(*f, start),
                    });
                    source_record(w, source);
                    w.present(*transport, |w, t| w.u64(t.0));
                    message_record(w, message);
                }
            }
        }
        self.fx.record(w);
    }

    /// The queue and its admission ledger, then the delivered state, written canonically (see the
    /// module docs; the MIDI effect host is written by the caller). Also a test's view of a queue.
    pub(crate) fn record_ledger(&self, w: &mut Writer<'_>, start: u64) {
        let InstrumentEvents {
            queue,
            non_release,
            notes,
            pedals,
            holders,
            sent,
            writers,
            next_voice,
            // Telemetry (section 14): counters only the report reads.
            status: _,
            rack_ports,
            in_rack,
            // Written by `record_canonical` after the ledger (a ledger copy has a fresh host).
            fx: _,
        } = self;
        w.usize(queue.len());
        for q in queue {
            let Queued { due, source, transport, message, action, cost } = q;
            w.u64(rel(*due, start));
            source_record(w, source);
            w.present(*transport, |w, t| w.u64(t.0));
            message_record(w, message);
            match *action {
                Action::On(n) | Action::Off(n) => {
                    w.u8(if matches!(action, Action::On(_)) { 0 } else { 1 });
                    w.u8(n);
                }
                Action::Down(n) | Action::Up(n) => {
                    w.u8(if matches!(action, Action::Down(_)) { 2 } else { 3 });
                    w.u8(n);
                }
                Action::Reset => w.u8(4),
                Action::Plain => w.u8(5),
            }
            w.u8(*cost);
        }
        w.usize(*non_release);
        // The ledgers in table order: release paths (`release_instrument`, an all-notes-off)
        // walk the tables in order, so the order reaches the output (#742 ruling W-3). An unused
        // entry holds nothing else observable: it is matched only while `used`, and
        // `free_note`/`free_pedal` hand it out to be overwritten whole.
        for e in notes {
            let NoteEntry { used, source, key, sender, voice, ons, open, release_queued, retry_release, last_on } = e;
            w.bool(*used);
            if *used {
                source_record(w, source);
                w.u8(*key);
                w.u32(sender.0);
                w.i32(*voice);
                w.u16(*ons);
                w.u16(*open);
                w.bool(*release_queued);
                w.present(*retry_release, |w, v| w.u8(v));
                w.u64(rel(*last_on, start));
            }
        }
        for p in pedals {
            let PedalEntry { used, source, cc, down, value, downs, release_queued, retry_release, last_down } = p;
            w.bool(*used);
            if *used {
                source_record(w, source);
                w.u8(*cc);
                w.bool(*down);
                w.u8(*value);
                w.u16(*downs);
                w.bool(*release_queued);
                w.present(*retry_release, |w, v| w.u8(v));
                w.u64(rel(*last_down, start));
            }
        }
        w.uints(64, holders.iter().flatten().map(|&v| u64::from(v)));
        w.uints(64, sent.iter().flatten().map(|&v| u64::from(v)));
        for wr in writers.iter().flatten() {
            w.present(*wr, |w, super::Writer { port, generation, neutral }| {
                w.u16(port);
                w.u64(generation);
                w.bool(neutral);
            });
        }
        w.u32(*next_voice);
        w.u64(*rack_ports);
        w.bool(*in_rack);
    }

    /// Test seam (#742's omission controls): one field of the event state (`start`: the next block
    /// start); the MIDI effect host's own fields go to it. `false` without a target.
    pub(crate) fn nudge(&mut self, what: crate::Nudge, start: u64) -> bool {
        use crate::Nudge;
        match what {
            Nudge::RackExit => self.rack_ports ^= 1 << 5,
            Nudge::NoteOpen | Nudge::LastOn => {
                let Some(e) = self.notes.iter_mut().find(|e| e.used) else { return false };
                if what == Nudge::NoteOpen {
                    e.open = e.open.wrapping_add(1);
                } else {
                    e.last_on = start + 7;
                }
            }
            Nudge::RestrikeCost => {
                let Some(q) = self.queue.iter_mut().find(|q| matches!(q.action, Action::On(_))) else { return false };
                q.cost = if q.cost == 2 { 1 } else { 2 };
            }
            _ => return self.fx.nudge(what),
        }
        true
    }

    /// A copy of the queue and the ledger for canonical admission, with a fresh MIDI effect host
    /// (admission never reads it; the original's is written).
    fn ledger_copy(&self) -> Box<InstrumentEvents> {
        let mut copy = InstrumentEvents::new();
        copy.queue.clone_from(&self.queue);
        copy.non_release = self.non_release;
        copy.notes = self.notes;
        copy.pedals = self.pedals;
        copy.holders = self.holders;
        copy.sent = self.sent;
        copy.writers = self.writers;
        copy.next_voice = self.next_voice;
        copy.rack_ports = self.rack_ports;
        copy.in_rack = self.in_rack;
        copy
    }
}
