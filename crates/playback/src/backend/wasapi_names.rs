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
        let (cpal_default, native_default) = (default_name(from_cpal), default_name(from_native));
        eprintln!("{direction} default: cpal {cpal_default:?}, native {native_default:?}");
        if cpal_default != native_default {
            failures.push(format!("{direction} default differs: cpal {cpal_default:?}, native {native_default:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
