//! Racks on the audio thread (ADR 0034): the rack's MIDI to its members, and its macros' lanes to
//! the members' parameters.
//!
//! A rack is a routing folder, so its sum is the folder's: members output to the folder's bus and
//! the folder's strip reads it (`strip.rs`), each member keeping its own inserts and fader and the
//! rack its own after the sum. This is Logic's Track Stack (summing stack) and the audio half of
//! Ableton's Instrument Rack. What this module adds is the MIDI half:
//!
//! - **Clips.** A member plays the rack's clips merged with its own (`midi::schedule_notes`), as a
//!   source of its own in its note table.
//! - **Live input.** Playback routes an armed or monitored rack's input to [`rack_binding`]; the
//!   engine fans each event out to the members here ([`MixEngine::queue_event`]), in the rack's
//!   order and at most [`MAX_RACK_MEMBERS`] of them, with a fixed array on the stack.
//!
//! Both follow one rule: a note-on reaches every member whose key zone and velocity zone contain
//! it, its velocity scaled by both fade factors (Ableton: "Key zone fade ranges attenuate the
//! velocities of notes entering a chain", and the same for velocity zones); a note-off reaches a
//! member only when that member's table holds the note, so a member that did not get the on does
//! not get the off, also when a zone was edited while the note sounded. Pitch bend, pressure,
//! controllers (the sustain pedal included) and program changes go to every member.
//!
//! Nothing here allocates, locks or logs.

use super::*;
use crate::events::{EventOutcome, InstrumentEvent, Message};
use transit_model::{MAX_RACK_MEMBERS, MACRO_SLOT, macro_index};

/// The slot a rack's live input names: not a plug-in slot (inserts are small, the instrument is
/// [`INSTRUMENT_SLOT`]), so an event bound to it can only mean the rack.
pub const RACK_SLOT: usize = usize::MAX - 1;

/// The binding playback gives the live input of `rack`: the engine fans its events out to the
/// members' instruments.
#[must_use]
pub fn rack_binding(rack: TrackId) -> SlotBinding {
    (rack, RACK_SLOT, plan::SlotGen::VACANT)
}

impl MixEngine {
    /// [`MixEngine::queue_event`] for an event bound to a rack ([`rack_binding`]): one event per
    /// member that takes it, each admitted as if playback had sent it to that member's
    /// instrument. The outcome is the first accepted member's; [`EventOutcome::Unmatched`] when no
    /// member takes it (every zone excludes the note, or no member holds the note being released).
    pub(crate) fn queue_rack_event(&mut self, ev: &InstrumentEvent) -> EventOutcome {
        let mut out = [None::<InstrumentEvent>; MAX_RACK_MEMBERS];
        {
            let Some(plan) = self.plan.as_deref() else { return EventOutcome::NoInstrument };
            let s: &Session = &plan.session;
            let Some(rack) = s.track(ev.target.0).filter(|t| t.is_rack()) else { return EventOutcome::NoInstrument };
            for (slot, (member, zones)) in out.iter_mut().zip(s.rack_members(rack)) {
                let Some(i) = plan.tables.strip_of(member.id) else { continue };
                let Some(st) = plan.tables.strips.get(i) else { continue };
                let Some(inst) = st.instrument.as_ref() else { continue };
                let parked = self.pool.as_ref().is_some_and(|p| p.parks(i));
                let message = match ev.message {
                    Message::NoteOn { key, velocity, id } if velocity > 0 => match zones.velocity(key, velocity) {
                        Some(velocity) => Message::NoteOn { key, velocity, id },
                        None => continue,
                    },
                    // A strip a render worker holds cannot be asked; its instrument drops an off it
                    // does not hold (`Unmatched`) when the event is admitted at home.
                    Message::NoteOn { key, id, .. } | Message::NoteOff { key, id, .. }
                        if !parked && !inst.events.as_deref().is_some_and(|e| e.holds_note(&ev.source, key, id)) =>
                    {
                        continue;
                    }
                    m => m,
                };
                *slot = Some(InstrumentEvent { target: (member.id, INSTRUMENT_SLOT, inst.generation), message, ..*ev });
            }
        }
        let mut result = None;
        for e in out.iter().flatten() {
            let o = self.queue_event(e);
            if result.is_none_or(|r: EventOutcome| !r.accepted() && o.accepted()) {
                result = Some(o);
            }
        }
        result.unwrap_or(EventOutcome::Unmatched)
    }
}

/// The value a rack's macro lane gives a member's parameter at `pos`, when the member is in a
/// rack that reads automation and a macro with points in its lane maps `(slot, param)`: the
/// newest such macro in macro order wins. `None` leaves the parameter to its own value and lane.
pub(crate) fn macro_param_value(s: &Session, t: &Track, slot: u8, param: &str, pos: Samples) -> Option<f32> {
    let (rack_track, _) = s.rack_of(t)?;
    let rack = rack_track.rack.as_ref()?;
    if !rack_track.mixer.automation_mode.reads() {
        return None;
    }
    let mut value = None;
    for (i, m) in rack.mappings_to(t.id, slot, param) {
        let lane = rack_track
            .automation
            .iter()
            .find(|l| matches!(&l.param, transit_model::AutoParam::Plugin { slot, param } if *slot == MACRO_SLOT && macro_index(param) == Some(i)))
            .filter(|l| !l.points.is_empty());
        if let Some(l) = lane {
            value = Some(m.plain(l.value_at(pos, 0.0)));
        }
    }
    value
}
