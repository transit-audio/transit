//! Unit tests of `plugin.rs`.

use super::*;

#[test]
fn only_values_the_host_set_are_re_sent() {
    assert_eq!(resend_indices(&[false, false, false], 8).count(), 0, "readings from the plugin stay put");
    assert_eq!(resend_indices(&[false, true, false, true], 8).collect::<Vec<_>>(), vec![1, 3]);
    assert_eq!(resend_indices(&[true, true, true], 2).collect::<Vec<_>>(), vec![0, 1], "bounded by the queue");
}

#[test]
fn a_table_published_while_it_is_read_is_not_taken() {
    let m = MidiMap::new();
    let a: Vec<Option<u32>> = (0..MAP_ENTRIES as u32).map(Some).collect();
    let b: Vec<Option<u32>> = vec![Some(7); MAP_ENTRIES];
    m.publish(&a);
    let mut out = vec![None; MAP_ENTRIES];
    let g = m.read(&mut out).expect("a published table");
    assert_eq!(out.to_vec(), a);
    // A publication that lands after the entries were copied: the copy may mix both tables,
    // so the sequence check must refuse it (review mutant V1).
    assert_eq!(m.read_with(&mut out, || m.publish(&b)), None);
    assert_eq!(m.read(&mut out), Some(g + 2));
    assert_eq!(out.to_vec(), b);
    // A write in progress (odd sequence) is refused before anything is copied.
    m.seq.fetch_add(1, Ordering::Relaxed);
    assert_eq!(m.read(&mut out), None);
}

#[test]
fn process_status_follows_the_tail() {
    assert_eq!(status_from_tail(u32::MAX), ProcessStatus::Continue);
    assert_eq!(status_from_tail(0), ProcessStatus::Tail);
    assert_eq!(status_from_tail(48_000), ProcessStatus::Tail);
}

/// ADR 0059 storage accounting at the VST3 adapter (review item 4): the shared ledger and queue,
/// the translated buffers and the native lists one instance preallocates, against the previous
/// implementation's. The COM header of each parameter queue object is not counted.
#[test]
fn storage_is_accounted_per_instance() {
    use std::mem::size_of;
    use transit_dsp::plugins::instrument_queue::{delivery, ledger};
    let shared = ledger::ENTRIES * size_of::<Option<ledger::Entry>>() + ledger::ENTRIES * 2 + delivery::NATIVE_EVENTS * size_of::<delivery::Item>();
    assert_eq!(shared, 117_504, "the shared ledger, free list and delivery items");
    assert_eq!((size_of::<NoteEvent>(), size_of::<ParamPoint>(), size_of::<vst3::Steinberg::Vst::Event>()), (16, 24, 48));
    let queue = |n: usize| n * (size_of::<usize>() + size_of::<crate::ffi::ParamQueue>()) + n * 16;
    let translated = NATIVE_EVENTS * size_of::<NoteEvent>() + PARAM_POINTS * size_of::<ParamPoint>() + PARAM_QUEUE_CAP * 16;
    let native = NATIVE_EVENTS * size_of::<vst3::Steinberg::Vst::Event>() + queue(PARAM_QUEUE_CAP);
    let total = shared + translated + native;
    // Before ADR 0059: the note queue and the controller points (EVENT_CAP each), the input list
    // (EVENT_CAP) and parameter storage for MAX_PARAMS + 2 x EVENT_CAP.
    let old_cap = crate::ffi::MAX_PARAMS as usize + 2 * EVENT_CAP;
    let before = EVENT_CAP * (size_of::<NoteEvent>() + size_of::<ParamPoint>())
        + old_cap * size_of::<ParamPoint>()
        + old_cap * 16
        + EVENT_CAP * size_of::<vst3::Steinberg::Vst::Event>()
        + queue(old_cap);
    assert_eq!((translated, native, total, before), (343_680, 605_440, 1_066_624, 729_088), "measured: translated {translated}, native {native}, total {total}, before {before}");
}
