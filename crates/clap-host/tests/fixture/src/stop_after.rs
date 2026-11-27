//! ADR 0059 R1 test hooks of the events plug-ins (one call site in the events loop):
//!
//! - **Record channel.** With the parameter log on (`CMD:log-start`), every event the plug-in
//!   applies is also logged as `REC <time> <code> <a> <b>` (code 1 note on, 2 note off with
//!   a = channel x 1000 + key and b = note id; 3 pressure; 4 parameter, a = id, b = value x 1e6;
//!   5 MIDI, a = the three bytes), apart from the audio output, so blocks of any size and a
//!   failed call (whose output the host silences) can be read through the saved state.
//! - **Stop after k.** A call that carries parameter 5 at a value in [0.9, 1.0) applies only the k
//!   events after it, k = round((value - 0.9) x 1000), and then reports `CLAP_PROCESS_ERROR`, as
//!   a plug-in that failed part-way would. The control value is neither applied, counted nor
//!   logged. State per instance, reset at each call's first event.

use clap_sys::events::*;
use std::sync::Mutex;

/// (instance, armed k, events applied since the control value).
static STATE: Mutex<Vec<(usize, Option<u32>, u32)>> = Mutex::new(Vec::new());

/// Whether the fixture skips event `index` of the current call at `key`; sets `fail` when the
/// stop mode is armed in this call; logs an applied event into `log` when the log is on.
pub(crate) fn skip(key: usize, index: u32, h: &clap_event_header, fail: &mut bool, log: &mut Option<Vec<String>>) -> bool {
    let skipped = stop(key, index, h, fail);
    if !skipped && let Some(log) = log.as_mut() {
        if let Some((code, a, b)) = record(h) {
            log.push(format!("REC {} {code} {a} {b}", h.time));
        }
    }
    skipped
}

fn stop(key: usize, index: u32, h: &clap_event_header, fail: &mut bool) -> bool {
    let mut all = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let i = match all.iter().position(|s| s.0 == key) {
        Some(i) => i,
        None => {
            all.push((key, None, 0));
            all.len() - 1
        }
    };
    let Some(s) = all.get_mut(i) else { return false };
    if index == 0 {
        s.1 = None;
        s.2 = 0;
    }
    if h.type_ == CLAP_EVENT_PARAM_VALUE {
        // SAFETY: The CLAP core PARAM_VALUE tag selects this aligned event payload.
        let e = unsafe { &*(std::ptr::from_ref(h).cast::<clap_event_param_value>()) };
        // From [0.9, 1.0), with room for the f32 rounding of the host's value.
        if e.param_id == 5 && (0.8999..1.0).contains(&e.value) {
            s.1 = Some(((e.value - 0.9) * 1000.0).round().max(0.0) as u32);
            s.2 = 0;
            *fail = true;
            return true;
        }
    }
    let Some(k) = s.1 else { return false };
    *fail = true;
    if s.2 >= k {
        return true;
    }
    s.2 += 1;
    false
}

/// An event as a record (code, a, b), as the events plug-ins write them.
fn record(h: &clap_event_header) -> Option<(u32, i64, i64)> {
    match h.type_ {
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF => {
            // SAFETY: The CLAP core NOTE_ON/OFF tag selects this aligned event payload.
            let e = unsafe { &*(std::ptr::from_ref(h).cast::<clap_event_note>()) };
            Some((if h.type_ == CLAP_EVENT_NOTE_ON { 1 } else { 2 }, i64::from(e.channel) * 1000 + i64::from(e.key), i64::from(e.note_id)))
        }
        CLAP_EVENT_NOTE_EXPRESSION => {
            // SAFETY: The CLAP core NOTE_EXPRESSION tag selects this aligned event payload.
            let e = unsafe { &*(std::ptr::from_ref(h).cast::<clap_event_note_expression>()) };
            Some((3, i64::from(e.channel) * 1000 + i64::from(e.key), (e.value * 1e6).round() as i64))
        }
        CLAP_EVENT_PARAM_VALUE => {
            // SAFETY: The CLAP core PARAM_VALUE tag selects this aligned event payload.
            let e = unsafe { &*(std::ptr::from_ref(h).cast::<clap_event_param_value>()) };
            Some((4, i64::from(e.param_id), (e.value * 1e6).round() as i64))
        }
        CLAP_EVENT_MIDI => {
            // SAFETY: The CLAP core MIDI tag selects this aligned three-byte event payload.
            let e = unsafe { &*(std::ptr::from_ref(h).cast::<clap_event_midi>()) };
            Some((5, i64::from(e.data[0]) << 16 | i64::from(e.data[1]) << 8 | i64::from(e.data[2]), 0))
        }
        _ => None,
    }
}
