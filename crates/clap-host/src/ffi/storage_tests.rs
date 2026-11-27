//! ADR 0059 storage accounting at the CLAP adapter (review item 4): the shared ledger and queue,
//! the adapter's event queue (parameters plus one call's translated events) and the native list
//! (`raw`), per instance and per parameter, against the previous implementation's.

use super::events::RawEvent;
use super::{EVENT_CAP, Event, NATIVE_EVENTS};
use std::mem::size_of;
use transit_dsp::plugins::instrument_queue::{delivery, ledger};

#[test]
fn storage_is_accounted_per_instance() {
    let shared = ledger::ENTRIES * size_of::<Option<ledger::Entry>>() + ledger::ENTRIES * 2 + delivery::NATIVE_EVENTS * size_of::<delivery::Item>();
    assert_eq!(shared, 117_504, "the shared ledger, free list and delivery items");
    let per_event = size_of::<Event>() + size_of::<RawEvent>();
    // Both buffers hold EVENT_CAP timed points + NATIVE_EVENTS + one block-start value per parameter.
    let fixed = (EVENT_CAP + NATIVE_EVENTS) * per_event;
    // Before ADR 0059: one EVENT_CAP queue for MIDI and timed points (plus the parameters), the held
    // flags and the MIDI voice table.
    let before = EVENT_CAP * per_event + 16 * 128 + EVENT_CAP * 12;
    assert_eq!((size_of::<Event>(), size_of::<RawEvent>(), fixed + shared, per_event, before), (24, 64, 425_856, 88, 104_448), "measured");
}
