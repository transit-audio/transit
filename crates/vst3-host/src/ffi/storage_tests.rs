//! ADR 0059 storage accounting at the VST3 adapter, measured from the constructed buffers' real
//! capacities (review round 2). Scoped payload: every buffer the record changed (the delivery
//! queue and ledger, the translated note and point buffers, the host-value batch, the input and
//! output parameter pools and the event lists). Total: every byte the host requested while
//! creating one "Fixture Events" instance (the fixture's own allocations are its own allocator's
//! and are not counted; transient allocations during creation are). Excluded from the payload: the
//! COM header of each parameter queue object and each event list, and the 64-byte mutex box macOS
//! takes once per event list.

use super::fixture;
use super::*;
use std::mem::size_of;
use transit_dsp::plugins::instrument_queue::ledger;

/// A parameter pool's payload: one queue object (pointer and body) per queue, and the slab.
fn pool(p: &ParamChanges) -> usize {
    p.queues.capacity() * (size_of::<usize>() + size_of::<ParamQueue>()) + p.slab_len() * 16
}

fn list(l: &EventList) -> usize {
    l.with(|v| v.capacity()).unwrap_or(0) * size_of::<sv::Event>()
}

fn events() -> crate::Vst3Descriptor {
    crate::set_scan_extra_dirs_only(true);
    let found = crate::scan_paths(&[fixture::unregistered_fixture_dir().to_path_buf()]);
    found.into_iter().find(|d| d.name == "Fixture Events").unwrap()
}

#[test]
fn storage_is_measured_from_the_constructed_buffers() {
    let d = events();
    let bundle = Bundle::load(&d.path).unwrap();
    // The native lists as `Bundle::instantiate` constructs them (bytes of the whole call counted).
    let (inst, made) = super::allocation_probe::measure(|| bundle.instantiate(&crate::parse_id(&d.id).unwrap()).unwrap());
    let native = pool(&inst.in_params) + pool(&inst.out_params) + list(&inst.in_events) + list(&inst.out_events);
    assert_eq!(
        (inst.in_params.queues.capacity(), inst.in_params.slab_len(), inst.out_params.queues.capacity(), inst.out_params.slab_len()),
        (PARAM_QUEUE_CAP, PARAM_POINTS, PARAM_QUEUE_CAP, 0)
    );
    assert_eq!((inst.in_events.with(|v| v.capacity()), inst.out_events.with(|v| v.capacity())), (Some(NATIVE_EVENTS), Some(EVENT_CAP)));
    // The adapter as the registry builds it: `Vst3Plugin::new`, then `prepare` (bytes counted).
    let info = crate::registry::build_info(&d, &inst);
    let (plugin, adapted) = super::allocation_probe::measure(|| {
        let mut p = crate::plugin::Vst3Plugin::new(inst, info);
        transit_dsp::Plugin::prepare(&mut p, 48_000.0, 1024, 2);
        p
    });
    drop(plugin);
    let [items, notes, timed, points, batch] = crate::plugin::CONSTRUCTED.get();
    assert_eq!([items, notes, timed, points, batch], [NATIVE_EVENTS, NATIVE_EVENTS, EVENT_CAP, PARAM_POINTS, PARAM_QUEUE_CAP]);
    let shared = ledger::ENTRIES * size_of::<Option<ledger::Entry>>()
        + ledger::ENTRIES * size_of::<u16>()
        + items * size_of::<transit_dsp::plugins::instrument_queue::delivery::Item>();
    let adapter = notes * size_of::<NoteEvent>() + timed * size_of::<ParamPoint>() + points * size_of::<ParamPoint>() + batch * 16;
    let after = shared + adapter + native;
    // Before ADR 0059, with the same constructors: notes, controls and timed points of EVENT_CAP,
    // points and batch of MAX_PARAMS + 2 x EVENT_CAP, the two pools at that queue count, and two
    // event lists of EVENT_CAP.
    let old = MAX_PARAMS as usize + 2 * EVENT_CAP;
    let before = EVENT_CAP * (size_of::<NoteEvent>() + 2 * size_of::<ParamPoint>())
        + old * size_of::<ParamPoint>()
        + old * 16
        + pool(&ParamChanges::new(old, old))
        + pool(&ParamChanges::new(old, 0))
        + 2 * list(&EventList::new(EVENT_CAP));
    let total = usize::try_from(made.bytes + adapted.bytes).unwrap();
    assert_eq!((shared, adapter, native, after, before), (117_504, 368_256, 1_019_392, 1_505_152, 1_097_728), "measured payload");
    // The whole creation's requested bytes vary with paths and platform (2,278,867 measured on
    // Windows on 2026-11-27); they always include the scoped payload.
    assert!(total >= after, "total {total}");
}
