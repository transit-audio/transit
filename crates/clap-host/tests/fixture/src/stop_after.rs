//! ADR 0059 R1: the events plug-ins' "apply the first k events, then fail" mode. A call that
//! carries parameter 5 at a value in [0.9, 1.0) applies (records) only the k events after it,
//! k = round((value - 0.9) x 1000), and then reports `CLAP_PROCESS_ERROR`, as a plug-in that
//! failed part-way through its call would. The control value itself is neither applied nor
//! counted. State per instance, reset at each call's first event.

use clap_sys::events::{CLAP_EVENT_PARAM_VALUE, clap_event_header, clap_event_param_value};
use std::sync::Mutex;

/// (instance, armed k, events applied since the control value).
static STATE: Mutex<Vec<(usize, Option<u32>, u32)>> = Mutex::new(Vec::new());

/// Whether the fixture skips event `index` of the current call at `key`; sets `fail` when the
/// mode is armed in this call.
pub(crate) fn skip(key: usize, index: u32, h: &clap_event_header, fail: &mut bool) -> bool {
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
