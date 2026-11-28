//! Note output and the stage reset at the native boundary (ADR 0064, decisions 3 and 6): event
//! output bus 0 activated beside the input bus, each call's output events decoded into the
//! block's [`HostedOutBuffer`] with the plug-in's own signed addresses, and the
//! `setProcessing(false)`, `setProcessing(true)` cycle a hosted MIDI stage resets with.

use super::*;
use transit_dsp::hosted_out::{HostedOut, HostedOutBuffer, OutDrop, OutForm, OutKind, OutValues};

/// VST3 `ControllerNumbers` the record reads from a legacy controller output event.
const AFTER_TOUCH: u8 = 128;
const LEGACY_PITCH_BEND: u8 = 129;
const PROGRAM_CHANGE: u8 = 130;
const LEGACY_POLY_PRESSURE: u8 = 131;

/// The bus the stage reads: event output bus 0 (every VST3 event bus carries the same types).
const READ_BUS: i32 = 0;

/// One output event as the plug-in wrote it: its offset in the call (as written) and the event,
/// its kind by the record's table (ADR 0064, decision 3). Pointer fields (data, text) are never
/// read.
pub(crate) fn decode(e: &sv::Event) -> (i64, HostedOut) {
    use sv::Event_::EventTypes_ as T;
    let mut out = HostedOut {
        offset: 0,
        seq: 0,
        kind: OutKind::Dropped(OutDrop::Unknown),
        port: e.busIndex,
        channel: -1,
        key: -1,
        note_id: -1,
        form: OutForm::Native,
        values: OutValues::None,
    };
    let ty = e.r#type;
    if ty == T::kNoteOnEvent as u16 {
        // SAFETY: the plug-in tagged this event a note-on, and that member holds plain numbers
        // only (any bit pattern is a valid value); no pointer-bearing member is read.
        let n = unsafe { e.__field0.noteOn };
        (out.kind, out.channel, out.key, out.note_id) = (OutKind::NoteOn, n.channel, n.pitch, n.noteId);
        out.values = OutValues::Velocity(f64::from(n.velocity));
    } else if ty == T::kNoteOffEvent as u16 {
        // SAFETY: as above; tagged a note-off.
        let n = unsafe { e.__field0.noteOff };
        (out.kind, out.channel, out.key, out.note_id) = (OutKind::NoteOff, n.channel, n.pitch, n.noteId);
        out.values = OutValues::Velocity(f64::from(n.velocity));
    } else if ty == T::kPolyPressureEvent as u16 {
        // SAFETY: as above; tagged a poly pressure.
        let n = unsafe { e.__field0.polyPressure };
        (out.kind, out.channel, out.key, out.note_id) = (OutKind::PolyPressure, n.channel, n.pitch, n.noteId);
        out.values = OutValues::Pressure(f64::from(n.pressure));
    } else if ty == T::kLegacyMIDICCOutEvent as u16 {
        // SAFETY: as above; tagged a legacy controller output event.
        let c = unsafe { e.__field0.midiCCOut };
        out.channel = i16::from(c.channel);
        // `value` only: no public document states what `value2` holds (decision 3).
        out.values = OutValues::Controller { number: c.controlNumber, value: c.value };
        out.kind = match c.controlNumber {
            0..=127 => OutKind::Controller,
            AFTER_TOUCH => OutKind::ChannelPressure,
            PROGRAM_CHANGE => OutKind::Program,
            LEGACY_PITCH_BEND => OutKind::Dropped(OutDrop::LegacyPitchBend),
            LEGACY_POLY_PRESSURE => OutKind::Dropped(OutDrop::LegacyPolyPressure),
            _ => OutKind::Dropped(OutDrop::OtherController),
        };
    } else if ty == T::kDataEvent as u16 {
        out.kind = OutKind::Dropped(OutDrop::Data);
    } else if ty == T::kNoteExpressionValueEvent as u16 {
        // SAFETY: as above; tagged a note expression value.
        out.note_id = unsafe { e.__field0.noteExpressionValue }.noteId;
        out.kind = OutKind::Dropped(OutDrop::NoteExpression);
    } else if ty == T::kNoteExpressionIntValueEvent as u16 {
        // SAFETY: as above; tagged an integer note expression.
        out.note_id = unsafe { e.__field0.noteExpressionIntValue }.noteId;
        out.kind = OutKind::Dropped(OutDrop::NoteExpression);
    } else if ty == T::kNoteExpressionTextEvent as u16 {
        // SAFETY: as above; tagged a text note expression (its text pointer is not read).
        out.note_id = unsafe { e.__field0.noteExpressionText }.noteId;
        out.kind = OutKind::Dropped(OutDrop::NoteExpression);
    } else if ty == T::kChordEvent as u16 {
        out.kind = OutKind::Dropped(OutDrop::Chord);
    } else if ty == T::kScaleEvent as u16 {
        out.kind = OutKind::Dropped(OutDrop::Scale);
    }
    if e.busIndex != READ_BUS {
        out.kind = OutKind::Dropped(OutDrop::OtherOutput);
    }
    (i64::from(e.sampleOffset), out)
}

impl Instance {
    /// Activates event output bus 0 (configure: UI thread, inactive component, as the input bus).
    pub(super) fn activate_event_output(&self) {
        let (event, dout) = (sv::MediaTypes_::kEvent, sv::BusDirections_::kOutput);
        // SAFETY: valid component; event output bus 0 exists (the caller counted it); the
        // component is inactive here (`configure` shut it down and activates it after), as the
        // SDK requires for bus activation.
        unsafe { self.component.activateBus(event as sv::MediaType, dout as sv::BusDirection, READ_BUS, 1) };
    }

    /// Asks for note output collection from the next configuration on (control thread).
    pub(crate) fn collect_notes(&mut self, on: bool) {
        self.collect_notes = on;
    }

    /// Collection is in force for this configuration.
    pub(crate) fn collecting(&self) -> bool {
        self.notes_out
    }

    /// Limits the next call's output list to `room` events (the block buffer's room left); without
    /// collection in force the list takes up to its capacity in every call, as before HM1.
    pub(crate) fn limit_output(&self, room: usize) {
        self.out_events.set_room(if self.notes_out { room } else { EVENT_CAP });
    }

    /// Without collection: the last call's output is left unread (the next call clears it, as
    /// before HM1), and the refusal count is dropped.
    pub(crate) fn forget_output(&self) {
        self.out_events.take_refused();
    }

    /// Moves the last call's output events into `sink` (the call of `frames` frames starting at
    /// block frame `start`), in write order, and counts what the list refused.
    pub(crate) fn drain_output(&self, sink: &mut HostedOutBuffer, start: u32, frames: u32) {
        sink.refused(self.out_events.take_refused());
        self.out_events.with(|v| {
            for e in v.iter() {
                let (local, ev) = decode(e);
                sink.push(local, start, frames, ev);
            }
            v.clear();
        });
    }

    /// The stage reset (ADR 0064, decision 6): `setProcessing(false)` then `setProcessing(true)`,
    /// except that the `false` is left out when the processor is not in the processing state
    /// (the `true` is still sent before `process`). True when each result is `kResultOk` or
    /// `kNotImplemented`.
    pub(crate) fn cycle_processing(&mut self) -> bool {
        let Some(p) = self.processor.clone() else { return false };
        let fine = |r: sb::tresult| r == sb::kResultOk || r == sb::kNotImplemented;
        let mut ok = true;
        if self.processing {
            trace::record(self.host.trace_id, trace::Call::SetProcessing, 0, 0.0, 0);
            // SAFETY: valid, active processor in the processing state; VST3 allows
            // `setProcessing` on the processing thread.
            ok &= fine(unsafe { p.setProcessing(0) });
        }
        trace::record(self.host.trace_id, trace::Call::SetProcessing, 0, 0.0, 1);
        // SAFETY: valid, active processor; as above.
        ok &= fine(unsafe { p.setProcessing(1) });
        self.processing = true;
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(ty: u16, bus: i32, offset: i32) -> sv::Event {
        // SAFETY: `Event` is plain data (numbers and a union of plain structs); all-zero is valid.
        let mut e: sv::Event = unsafe { std::mem::zeroed() };
        (e.r#type, e.busIndex, e.sampleOffset) = (ty, bus, offset);
        e
    }

    #[test]
    fn a_legacy_controller_reads_value_and_never_value2() {
        let mut e = event(sv::Event_::EventTypes_::kLegacyMIDICCOutEvent as u16, 0, 3);
        e.__field0.midiCCOut = sv::LegacyMIDICCOutEvent { controlNumber: 7, channel: 2, value: 17, value2: 99 };
        let (local, out) = decode(&e);
        assert_eq!((local, out.kind, out.channel, out.values), (3, OutKind::Controller, 2, OutValues::Controller { number: 7, value: 17 }));
        e.busIndex = 1;
        assert_eq!(decode(&e).1.kind, OutKind::Dropped(OutDrop::OtherOutput));
    }
}
