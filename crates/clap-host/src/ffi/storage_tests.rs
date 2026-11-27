//! ADR 0059 storage accounting at the CLAP adapter, measured from the constructed buffers' real
//! capacities (review round 2). Scoped payload: the delivery queue and ledger, the adapter's event
//! queue (parameters plus one call's translated events) and the native list (`raw`). Total: every
//! byte the host requested while creating one events-fixture instance (the fixture's own
//! allocations are its own allocator's; transient allocations during creation are counted).

use super::events::RawEvent;
use super::{EVENT_CAP, Event, NATIVE_EVENTS, allocation_probe, fixture};
use std::mem::size_of;
use transit_dsp::plugins::instrument_queue::{delivery, ledger};

#[test]
fn storage_is_measured_from_the_constructed_buffers() {
    crate::set_scan_extra_dirs_only(true);
    let found = crate::scan_paths(&[fixture::events_fixture_dir().to_path_buf()]);
    let d = found.into_iter().find(|d| d.id == "clap:org.soundcraft.test.events").unwrap();
    let bundle = super::Bundle::load(&d.path).unwrap();
    // The native instance (bytes counted), then the adapter as the registry builds it.
    let (inst, made) = allocation_probe::measure(|| bundle.instantiate(&d.plugin_id).unwrap());
    let info = crate::registry::build_info(&d, &inst);
    let (plugin, adapted) = allocation_probe::measure(|| {
        let mut p = crate::plugin::ClapPlugin::new(inst, info);
        transit_dsp::Plugin::prepare(&mut p, 48_000.0, 1024, 2);
        p
    });
    drop(plugin);
    let [queue, items, raw, params] = crate::plugin::CONSTRUCTED.get();
    assert_eq!(
        (queue, items, raw),
        (EVENT_CAP + NATIVE_EVENTS + params, NATIVE_EVENTS, EVENT_CAP + NATIVE_EVENTS + params),
        "capacities as constructed"
    );
    let shared = ledger::ENTRIES * size_of::<Option<ledger::Entry>>() + ledger::ENTRIES * size_of::<u16>() + items * size_of::<delivery::Item>();
    let after = shared + queue * size_of::<Event>() + raw * size_of::<RawEvent>();
    // Before ADR 0059: the queue and the native list of EVENT_CAP + parameters (the native list's
    // amortized `reserve` then doubled it whenever the parameters were fewer than EVENT_CAP), the
    // held flags and the MIDI voice table.
    let old_raw = (2 * EVENT_CAP).max(EVENT_CAP + params);
    let before = (EVENT_CAP + params) * size_of::<Event>() + old_raw * size_of::<RawEvent>() + 16 * 128 + EVENT_CAP * 12;
    let total = usize::try_from(made.bytes + adapted.bytes).unwrap();
    assert_eq!((params, shared, after, before), (1, 117_504, 425_944, 170_008), "measured payload");
    // The whole creation's requested bytes vary with paths and platform (882,074 measured on
    // Windows on 2026-11-27); they always include the scoped payload.
    assert!(total >= after, "total {total}");
}
