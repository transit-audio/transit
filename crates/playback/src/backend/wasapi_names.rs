//! ADR 0046, decision 2: the native WASAPI list names the same endpoints as cpal's, so the device
//! names saved in `AudioConfig` stay valid when the native list replaces cpal's. Windows with the
//! native WASAPI backend only, while cpal is still in the build (slice 4 removes this test; its
//! recorded output stays as evidence). Not ignored: it skips, with the reason on stderr, where the
//! enumerator fails or no endpoint exists (a CI runner without audio).

use crate::devices::{self, DeviceInfo};
use std::collections::{BTreeMap, BTreeSet};

fn print_list(lister: &str, direction: &str, list: &[DeviceInfo]) {
    eprintln!("{lister} {direction}s ({}):", list.len());
    for d in list {
        eprintln!("  {}{}", d.name, if d.is_default { " (default)" } else { "" });
    }
}

/// The names that appear more than once in `list`, with their count.
fn duplicates(list: &[DeviceInfo]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for d in list {
        *counts.entry(d.name.as_str()).or_default() += 1;
    }
    counts.into_iter().filter(|(_, n)| *n > 1).map(|(name, n)| (name.to_owned(), n)).collect()
}

/// What the Audio Devices window offers for each endpoint both listers name, compared by name: the
/// buffer sizes (`buffer_sizes()`) and the sample-rate set. One line per difference.
fn row_differences(direction: &str, from_cpal: &[DeviceInfo], from_native: &[DeviceInfo]) -> Vec<String> {
    let mut differences = Vec::new();
    for c in from_cpal {
        let Some(n) = from_native.iter().find(|n| n.name == c.name) else { continue };
        if c.buffer_sizes() != n.buffer_sizes() {
            differences.push(format!("{direction} \"{}\" buffer sizes differ: cpal {:?}, native {:?}", c.name, c.buffer_sizes(), n.buffer_sizes()));
        }
        let rates = |d: &DeviceInfo| d.sample_rates.iter().copied().collect::<BTreeSet<u32>>();
        if rates(c) != rates(n) {
            differences.push(format!("{direction} \"{}\" sample rates differ: cpal {:?}, native {:?}", c.name, rates(c), rates(n)));
        }
    }
    differences
}

fn default_name(list: &[DeviceInfo]) -> Option<&str> {
    list.iter().find(|d| d.is_default).map(|d| d.name.as_str())
}

#[test]
fn cpal_and_native_wasapi_list_the_same_names() {
    let native = super::native::wasapi_system();
    if !native.diagnostics.is_empty() {
        eprintln!("skipped: the native WASAPI enumerator failed: {}", native.diagnostics.join("; "));
        return;
    }
    let Some(cpal) = devices::cpal_audio_systems().into_iter().find(|s| s.name == devices::WASAPI_SYSTEM) else {
        eprintln!("skipped: cpal lists no WASAPI host");
        return;
    };
    if [&native.outputs, &native.inputs, &cpal.outputs, &cpal.inputs].iter().all(|l| l.is_empty()) {
        eprintln!("skipped: neither listing has an endpoint (no active audio endpoint on this machine)");
        return;
    }
    let mut failures = Vec::new();
    for (direction, from_cpal, from_native) in [("output", &cpal.outputs, &native.outputs), ("input", &cpal.inputs, &native.inputs)] {
        print_list("cpal", direction, from_cpal);
        print_list("native", direction, from_native);
        for (lister, list) in [("cpal", from_cpal), ("native", from_native)] {
            let twins = duplicates(list);
            if twins.is_empty() {
                eprintln!("{lister} {direction}s: no duplicate names");
            }
            for (name, n) in twins {
                eprintln!("{lister} {direction}s: duplicate name \"{name}\" ({n} endpoints)");
            }
        }
        let names = |list: &[DeviceInfo]| list.iter().map(|d| d.name.clone()).collect::<BTreeSet<_>>();
        let (cpal_names, native_names) = (names(from_cpal), names(from_native));
        if cpal_names != native_names {
            failures.push(format!(
                "{direction} names differ: only cpal {:?}, only native {:?}",
                cpal_names.difference(&native_names).collect::<Vec<_>>(),
                native_names.difference(&cpal_names).collect::<Vec<_>>()
            ));
        }
        let rows = row_differences(direction, from_cpal, from_native);
        if rows.is_empty() {
            eprintln!("{direction}s: every endpoint offers the same buffer sizes and sample rates in both");
        }
        for difference in &rows {
            eprintln!("{difference}");
        }
        failures.extend(rows);
        for d in from_native {
            eprintln!("  native {direction} \"{}\": buffer sizes {:?}, rates {:?}", d.name, d.buffer_sizes(), d.sample_rates);
        }
        let (cpal_default, native_default) = (default_name(from_cpal), default_name(from_native));
        eprintln!("{direction} default: cpal {cpal_default:?}, native {native_default:?}");
        if cpal_default != native_default {
            failures.push(format!("{direction} default differs: cpal {cpal_default:?}, native {native_default:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The control of the row comparison: a native row whose buffer range is narrowed (the engine
/// period, 480-480) or whose rates differ is a difference; equal rows are none.
#[test]
fn a_narrowed_native_row_is_a_difference() {
    let row = |buffer_min, buffer_max, sample_rates: Vec<u32>| DeviceInfo {
        name: "Speakers".into(),
        is_default: true,
        max_channels: 2,
        sample_rates,
        buffer_min,
        buffer_max,
    };
    let cpal = [row(Some(0), Some(u32::MAX), vec![44_100, 48_000])];
    assert!(row_differences("output", &cpal, &[row(None, None, vec![48_000, 44_100])]).is_empty(), "same sizes and rates");
    assert_eq!(
        row_differences("output", &cpal, &[row(Some(480), Some(480), vec![44_100, 48_000])]),
        ["output \"Speakers\" buffer sizes differ: cpal [32, 64, 128, 256, 512, 1024, 2048, 4096, 8192], native []"]
    );
    assert_eq!(
        row_differences("output", &cpal, &[row(None, None, vec![48_000])]),
        ["output \"Speakers\" sample rates differ: cpal {44100, 48000}, native {48000}"]
    );
}
