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
use transit_model::{MACRO_SLOT, MAX_RACK_MEMBERS, macro_index};

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

/// The rack macros that rule the parameters of one device of a member this block: found once per
/// `apply_params` call ([`MacroLanes::of`]), then asked per parameter ([`MacroLanes::value`]).
pub(crate) struct MacroLanes<'a> {
    rack: &'a transit_model::Rack,
    lanes: &'a [transit_model::AutomationLane],
    member: TrackId,
    slot: u8,
}

/// The lane of macro `i` on the rack track, when it has points.
fn macro_lane(lanes: &[transit_model::AutomationLane], i: usize) -> Option<&transit_model::AutomationLane> {
    lanes
        .iter()
        .find(|l| matches!(&l.param, transit_model::AutoParam::Plugin { slot, param } if *slot == MACRO_SLOT && macro_index(param) == Some(i)))
        .filter(|l| !l.points.is_empty())
}

impl<'a> MacroLanes<'a> {
    /// `None` (the common case, decided once) unless `t` is a member of a rack that reads
    /// automation and a macro with points in its lane maps a parameter of `slot` on `t`.
    pub(crate) fn of(s: &'a Session, t: &Track, slot: u8) -> Option<MacroLanes<'a>> {
        t.folder?;
        let (rack_track, _) = s.rack_of(t)?;
        let rack = rack_track.rack.as_ref()?;
        if !rack_track.mixer.automation_mode.reads() {
            return None;
        }
        let lanes = rack_track.automation.as_slice();
        let ruled = rack
            .macros
            .iter()
            .enumerate()
            .any(|(i, m)| m.mappings.iter().any(|x| x.track == t.id && x.slot == slot) && macro_lane(lanes, i).is_some());
        ruled.then_some(MacroLanes { rack, lanes, member: t.id, slot })
    }

    /// The value the macro lanes give `param` at `pos`: the last mapping macro (in macro order)
    /// whose lane has points wins. `None` leaves the parameter to its own value and lane.
    pub(crate) fn value(&self, param: &str, pos: Samples) -> Option<f32> {
        let mut value = None;
        for (i, m) in self.rack.mappings_to(self.member, self.slot, param) {
            if let Some(l) = macro_lane(self.lanes, i) {
                value = Some(m.plain(l.value_at(pos, 0.0)));
            }
        }
        value
    }
}

/// The value a rack's macro lane gives a member's parameter at `pos` ([`MacroLanes`]).
pub(crate) fn macro_param_value(s: &Session, t: &Track, slot: u8, param: &str, pos: Samples) -> Option<f32> {
    MacroLanes::of(s, t, slot)?.value(param, pos)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use transit_model::{AutoParam, ChannelFormat, Insert, MacroMapping, Rack, RackMember, TrackKind, macro_param};

    /// `apply_params` asks [`MacroLanes::of`] once per device and block: it must say "nothing here"
    /// for a grouped track without macros, and for a slot no macro lane rules.
    #[test]
    fn macro_lanes_are_found_only_for_a_ruled_slot() {
        let mut s = Session::default();
        let folder = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, None);
        let a = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
        {
            let t = s.track_mut(a).unwrap();
            t.folder = Some(folder);
            t.mixer.inserts[0] = Some(Insert::new("gain"));
        }
        let t = s.track(a).unwrap().clone();
        assert!(MacroLanes::of(&s, &t, 0).is_none(), "a plain group");
        s.track_mut(folder).unwrap().rack = Some(Rack { members: vec![RackMember::new(a)], ..Rack::default() });
        assert!(MacroLanes::of(&s, &t, 0).is_none(), "a rack without mappings");
        let r = s.track_mut(folder).unwrap();
        r.rack.as_mut().unwrap().macros[0].mappings.push(MacroMapping { track: a, slot: 0, param: "gain".into(), max: 1.0, ..MacroMapping::default() });
        let t = s.track(a).unwrap().clone();
        assert!(MacroLanes::of(&s, &t, 0).is_none(), "a mapping whose macro lane has no points");
        s.track_mut(folder).unwrap().lane_mut(&AutoParam::Plugin { slot: MACRO_SLOT, param: macro_param(0) }).set_point(0, 1.0);
        let m = MacroLanes::of(&s, &t, 0).expect("ruled");
        assert_eq!(m.value("gain", 0), Some(1.0));
        assert_eq!(m.value("other", 0), None);
        assert!(MacroLanes::of(&s, &t, 1).is_none(), "another slot");
    }
}
