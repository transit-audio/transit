//! The `TESTING_ONLY` check of `cargo xtask layers` (ADR 0029, amendment for #742, section 12): a
//! testing-only crate (the state record codec) may be named by any crate, standalone or not, only
//! as a dev-dependency, or as an optional normal dependency that no feature other than `testing`
//! enables, with `testing` absent from `default`.
//!
//! The check follows Cargo's feature rules: a dependency's key is its rename when it has one;
//! `dep:K` enables optional dependency `K`; a plain name enables a feature (or, for an optional
//! dependency nothing names with `dep:`, its implicit feature); `K/f` enables dependency `K` and
//! its feature `f`; `K?/f` enables `f` only if `K` is enabled otherwise. A package *activates* the
//! codec through a feature when the feature reaches, directly or through other features, the
//! codec's own dependency or a workspace dependency's feature that activates it (so `testing`
//! forwarding is followed across the workspace). Rejected: any activating feature other than
//! `testing` (which includes `default` reaching `testing`, however indirectly); a build or
//! non-optional normal dependency on the codec; and a normal or build dependency that requests an
//! activating feature of a workspace package (by name or through its default features), since
//! that enables the testing path outside `testing`-feature forwarding.

use super::{Dependency, Metadata, Package};
use std::collections::{BTreeMap, BTreeSet};

/// Testing-only crates (see the module docs).
pub(crate) const TESTING_ONLY: &[&str] = &["transit-state-record"];

/// A normal or build dependency (not a dev-dependency).
fn production(d: &Dependency) -> bool {
    d.kind.as_deref() != Some("dev")
}

fn key(d: &Dependency) -> &str {
    d.rename.as_deref().unwrap_or(&d.name)
}

/// The package's features, with the implicit feature of every optional dependency nothing names
/// with `dep:` (Cargo's rule; `cargo metadata` lists them too, but older outputs did not).
fn features(p: &Package) -> BTreeMap<String, Vec<String>> {
    let mut f = p.features.clone();
    let named: BTreeSet<&str> = p.features.values().flatten().filter_map(|v| v.strip_prefix("dep:")).collect();
    for d in p.dependencies.iter().filter(|d| d.optional && production(d)) {
        if !named.contains(key(d)) {
            f.entry(key(d).to_string()).or_insert_with(|| vec![format!("dep:{}", key(d))]);
        }
    }
    f
}

/// Whether production dependency `d` brings the codec in once it is enabled: it is the codec, or
/// it requests (by name or through its default features) a feature that activates the codec in
/// its workspace package.
fn carries(d: &Dependency, act: &BTreeMap<String, BTreeSet<String>>) -> bool {
    if TESTING_ONLY.contains(&d.name.as_str()) {
        return true;
    }
    let Some(a) = act.get(&d.name) else { return false };
    d.features.iter().any(|f| a.contains(f)) || (d.uses_default_features && a.contains("default"))
}

/// Every package's activating features (a fixed point over the workspace; see the module docs).
pub(crate) fn activating(metadata: &Metadata) -> BTreeMap<String, BTreeSet<String>> {
    let mut act: BTreeMap<String, BTreeSet<String>> = metadata.packages.iter().map(|p| (p.name.clone(), BTreeSet::new())).collect();
    loop {
        let mut changed = false;
        for p in &metadata.packages {
            let feats = features(p);
            let deps: Vec<&Dependency> = p.dependencies.iter().filter(|d| production(d)).collect();
            let by_key = |k: &str| deps.iter().filter(|d| key(d) == k).copied().collect::<Vec<&Dependency>>();
            // One value of a feature: does it reach the codec, given the features known so far?
            let value = |v: &str, mine: &BTreeSet<String>| -> bool {
                if let Some(k) = v.strip_prefix("dep:") {
                    return by_key(k).into_iter().any(|d| carries(d, &act));
                }
                if let Some((k, f)) = v.split_once('/') {
                    let (k, weak) = k.strip_suffix('?').map_or((k, false), |k| (k, true));
                    return by_key(k).into_iter().any(|d| (!weak && carries(d, &act)) || act.get(&d.name).is_some_and(|a| a.contains(f)));
                }
                mine.contains(v)
            };
            let mut mine = act.get(&p.name).cloned().unwrap_or_default();
            loop {
                let before = mine.len();
                for (f, values) in &feats {
                    if !mine.contains(f) && values.iter().any(|v| value(v, &mine)) {
                        mine.insert(f.clone());
                    }
                }
                if mine.len() == before {
                    break;
                }
            }
            if act.get(&p.name) != Some(&mine) {
                act.insert(p.name.clone(), mine);
                changed = true;
            }
        }
        if !changed {
            return act;
        }
    }
}

/// The problems of package `p` (see the module docs), with `act` from [`activating`].
pub(crate) fn check_package(p: &Package, act: &BTreeMap<String, BTreeSet<String>>, problems: &mut Vec<String>) {
    let name = &p.name;
    for f in act.get(name).into_iter().flatten().filter(|f| f.as_str() != "testing") {
        problems.push(format!("{name}: feature `{f}` enables a testing-only crate ({}); only `testing` may", TESTING_ONLY.join(", ")));
    }
    for d in p.dependencies.iter().filter(|d| production(d)) {
        if TESTING_ONLY.contains(&d.name.as_str()) {
            if d.kind.as_deref() == Some("build") {
                problems.push(format!("{name}: the testing-only crate {} as a build dependency", d.name));
            } else if !d.optional {
                problems.push(format!("{name}: the testing-only crate {} as a non-optional normal dependency", d.name));
            }
        } else if carries(d, act) {
            problems.push(format!(
                "{name}: its dependency on {} requests a feature that enables a testing-only crate (forward it through `testing` instead)",
                d.name
            ));
        }
    }
}

#[cfg(test)]
mod tests;
