//! Note output at the native boundary (ADR 0064, decision 3): the output event list's `try_push`
//! writes each note event the plug-in pushes into the block's [`HostedOutBuffer`] with the
//! plug-in's own signed addresses (and returns `false` when it is full), keeps parameter values
//! as before (editor edits), and the instance's note ports are read in both directions.

use super::*;
use clap_sys::events::{
    CLAP_EVENT_MIDI_SYSEX, CLAP_EVENT_MIDI2, CLAP_EVENT_NOTE_CHOKE, CLAP_EVENT_NOTE_END, clap_event_midi_sysex, clap_event_midi2,
};
use clap_sys::ext::note_ports::{CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI_MPE};
use transit_dsp::hosted_out::{HostedOut, HostedOutBuffer, OutDrop, OutForm, OutKind, OutValues};

/// What one `process` call's output list writes into: the plug-in's parameter values (editor
/// edits, as before), the block's note output, the port the stage reads, and the call's place in
/// the block.
pub(crate) struct OutSink<'a> {
    pub params: &'a mut Vec<(u32, f64)>,
    /// The block's note output; `None` without collection (note output answered as before).
    pub notes: Option<&'a mut HostedOutBuffer>,
    /// The first readable output note port (`None`: the plug-in has none).
    pub read: Option<u32>,
    pub start: u32,
    pub frames: u32,
}

/// The dialects a stage reads an output port in (ADR 0064, decision 2.1).
const READABLE: u32 = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_MIDI_MPE;

impl Instance {
    /// The output note ports: how many the plug-in declares and the first readable one (CLAP,
    /// MIDI or MIDI MPE dialect). Main thread (each activation).
    pub fn note_outputs(&self) -> (u32, Option<u32>) {
        // SAFETY: the extension pointer, when non-null, points to a static vtable in the plugin.
        let Some(p) = (unsafe { self.notes.as_ref() }) else { return (0, None) };
        let (Some(count), Some(get)) = (p.count, p.get) else { return (0, None) };
        // SAFETY: vtable call with its instance.
        let n = unsafe { count(self.plugin, false) };
        let read = (0..n.min(MAX_PORTS)).find(|&i| {
            // SAFETY: plain data; all-zero is valid.
            let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is writable.
            let got = unsafe { get(self.plugin, i, false, &mut info) };
            got && info.supported_dialects & READABLE != 0
        });
        (n, read)
    }
}

/// A plug-in event as note output: its offset in the call (the header's time read as signed, so
/// a time written as -1 is below 0) and the event; `None` for an event that is not note output
/// (parameter values and gestures, the transport, `NOTE_END`, which reports the plug-in's own
/// voices).
///
/// # Safety
/// `h` points to a correctly aligned, valid instance of the native event type its header
/// advertises (by space, type and a size of at least that type's), live for the call. The size
/// and type checks below select the type; they do not establish alignment, which the caller's
/// contract supplies (the CLAP ABI: plug-ins push their own typed event structs).
unsafe fn decode(h: *const clap_event_header, read: Option<u32>) -> Option<(i64, HostedOut)> {
    // SAFETY: caller contract.
    let hd = unsafe { &*h };
    let mut out = HostedOut {
        offset: 0,
        seq: 0,
        kind: OutKind::Dropped(OutDrop::Unknown),
        port: -1,
        channel: -1,
        key: -1,
        note_id: -1,
        form: OutForm::Native,
        values: OutValues::None,
    };
    let fits = |n: usize| hd.size as usize >= n;
    if hd.space_id != CLAP_CORE_EVENT_SPACE_ID {
        return Some((i64::from(hd.time as i32), out));
    }
    match hd.type_ {
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE if fits(size_of::<clap_event_note>()) => {
            // SAFETY: the header says this is a complete note event (type and size checked).
            let e = unsafe { &*h.cast::<clap_event_note>() };
            out.kind = match hd.type_ {
                CLAP_EVENT_NOTE_ON => OutKind::NoteOn,
                CLAP_EVENT_NOTE_OFF => OutKind::NoteOff,
                _ => OutKind::Choke,
            };
            (out.port, out.channel, out.key, out.note_id) = (i32::from(e.port_index), e.channel, e.key, e.note_id);
            out.values = OutValues::Velocity(e.velocity);
        }
        CLAP_EVENT_NOTE_END => return None,
        CLAP_EVENT_NOTE_EXPRESSION if fits(size_of::<clap_event_note_expression>()) => {
            // SAFETY: the header says this is a complete note expression (type and size checked).
            let e = unsafe { &*h.cast::<clap_event_note_expression>() };
            (out.port, out.channel, out.key, out.note_id) = (i32::from(e.port_index), e.channel, e.key, e.note_id);
            if e.expression_id == CLAP_NOTE_EXPRESSION_PRESSURE {
                out.kind = OutKind::PolyPressure;
                out.values = OutValues::Pressure(e.value);
            } else {
                out.kind = OutKind::Dropped(OutDrop::NoteExpression);
            }
        }
        CLAP_EVENT_MIDI if fits(size_of::<clap_event_midi>()) => {
            // SAFETY: the header says this is a complete MIDI event (type and size checked).
            let e = unsafe { &*h.cast::<clap_event_midi>() };
            let [status, d1, _] = e.data;
            out.form = OutForm::MidiBytes;
            out.port = i32::from(e.port_index);
            out.values = OutValues::Midi(e.data);
            out.channel = i16::from(status & 0x0F);
            out.kind = match status & 0xF0 {
                0x80 => OutKind::NoteOff,
                0x90 => OutKind::NoteOn,
                0xA0 => OutKind::PolyPressure,
                0xB0 => OutKind::Controller,
                0xC0 => OutKind::Program,
                0xD0 => OutKind::ChannelPressure,
                0xE0 => OutKind::PitchBend,
                _ => {
                    out.channel = -1;
                    OutKind::Dropped(OutDrop::Unknown)
                }
            };
            if matches!(status & 0xF0, 0x80 | 0x90 | 0xA0) {
                out.key = i16::from(d1 & 0x7F);
            }
        }
        CLAP_EVENT_MIDI_SYSEX if fits(size_of::<clap_event_midi_sysex>()) => {
            // SAFETY: the header says this is a complete system exclusive event; its buffer
            // pointer is not read.
            out.port = i32::from(unsafe { &*h.cast::<clap_event_midi_sysex>() }.port_index);
            out.kind = OutKind::Dropped(OutDrop::Sysex);
        }
        CLAP_EVENT_MIDI2 if fits(size_of::<clap_event_midi2>()) => {
            // SAFETY: the header says this is a complete MIDI 2 event (type and size checked).
            out.port = i32::from(unsafe { &*h.cast::<clap_event_midi2>() }.port_index);
            out.kind = OutKind::Dropped(OutDrop::Midi2);
        }
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE | CLAP_EVENT_NOTE_EXPRESSION | CLAP_EVENT_MIDI => {}
        _ => return None,
    }
    // A wildcard port (-1) applies to the read port; any other port is another output.
    if out.port != -1 && u32::try_from(out.port).ok() != read {
        out.kind = OutKind::Dropped(OutDrop::OtherOutput);
    }
    Some((i64::from(hd.time as i32), out))
}

/// The output list's `try_push`: parameter values (typically edits in the plug-in's GUI) are
/// kept while there is room, so the UI can write them into the session, as before; with
/// collection, note output goes into the block's buffer, `false` when it is full (the plug-in is
/// told). Everything else, and all note output without collection, is accepted and ignored.
///
/// # Safety
/// Called by the plug-in only during the `process` call whose output list `list` is, serialized
/// (never concurrently with itself) and never after that call returns: `list.ctx` then points to
/// that call's live `OutSink`. `event` is null or points to a correctly aligned, valid instance of
/// the native event type its header advertises, live for this call. A plug-in that breaks these
/// rules can cause undefined behavior; the host does not isolate it.
pub(super) unsafe extern "C" fn out_events_try_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
    if list.is_null() || event.is_null() {
        return true;
    }
    // SAFETY: `ctx` is null or the `OutSink` `process` built on its stack frame and borrows
    // exclusively for the duration of the plug-in call.
    let Some(sink) = (unsafe { ((*list).ctx as *mut OutSink<'_>).as_mut() }) else { return true };
    // SAFETY: CLAP passes a valid event header for the duration of the call.
    let h = unsafe { &*event };
    if h.space_id == CLAP_CORE_EVENT_SPACE_ID && h.type_ == CLAP_EVENT_PARAM_VALUE && (h.size as usize) >= size_of::<clap_event_param_value>() {
        // SAFETY: the header says this is a complete `clap_event_param_value` (type and size checked).
        let e = unsafe { &*event.cast::<clap_event_param_value>() };
        if sink.params.len() < sink.params.capacity() && e.value.is_finite() {
            sink.params.push((e.param_id, e.value));
        }
        return true;
    }
    let Some(notes) = sink.notes.as_deref_mut() else { return true };
    // SAFETY: as above: a correctly aligned, valid event of its advertised type for this call.
    match unsafe { decode(event, sink.read) } {
        Some((local, ev)) => notes.push(local, sink.start, sink.frames, ev),
        None => true,
    }
}
