//! The `TESTING_ONLY` check: the allowed uses pass, every forbidden activation path fails.

use super::super::tests::{dependency, package};
use super::super::{Dependency, Metadata, Package, check_layers};
use super::*;

const CODEC: &str = "transit-state-record";
const INHERIT: &str = "[lints]\nworkspace = true\n";

fn optional(name: &str) -> Dependency {
    Dependency { optional: true, ..dependency(name, None) }
}

fn with_features(mut p: Package, features: &[(&str, &[&str])]) -> Package {
    p.features = features.iter().map(|(k, v)| ((*k).to_string(), v.iter().map(|s| (*s).to_string()).collect())).collect();
    p
}

fn problems(packages: Vec<Package>) -> Vec<String> {
    let metadata = Metadata { packages };
    let act = activating(&metadata);
    let mut out = Vec::new();
    for p in &metadata.packages {
        check_package(p, &act, &mut out);
    }
    out
}

/// A codec user: an optional dependency enabled by `testing` alone.
fn user(name: &str) -> Package {
    let mut p = with_features(package(name, &[]), &[("testing", &["dep:transit-state-record"]), ("default", &["std"]), ("std", &[])]);
    p.dependencies.push(optional(CODEC));
    p
}

#[test]
fn a_dev_dependency_and_a_testing_only_optional_dependency_pass() {
    assert!(problems(vec![package(CODEC, &[]), package("transit-time", &[(CODEC, Some("dev"))])]).is_empty());
    assert!(problems(vec![package(CODEC, &[]), user("transit-instrument-core")]).is_empty());
}

#[test]
fn testing_forwarding_across_the_workspace_passes() {
    let mut mix =
        with_features(package("transit-mix", &[("transit-dsp", None)]), &[("testing", &["transit-dsp/testing", "dep:transit-state-record"])]);
    mix.dependencies.push(optional(CODEC));
    // A dev-dependency may request the testing feature.
    let mut engine = package("transit-engine", &[("transit-mix", None)]);
    engine.dependencies.push(Dependency { features: vec!["testing".into()], ..dependency("transit-mix", Some("dev")) });
    assert_eq!(problems(vec![package(CODEC, &[]), user("transit-dsp"), mix, engine]), Vec::<String>::new());
}

#[test]
fn a_non_optional_normal_or_any_build_dependency_on_the_codec_fails() {
    let normal = problems(vec![package(CODEC, &[]), package("transit-time", &[(CODEC, None)])]);
    assert!(normal.iter().any(|p| p.contains("non-optional normal dependency")), "{normal:?}");
    let mut build = package("transit-time", &[]);
    build.dependencies.push(Dependency { optional: true, ..dependency(CODEC, Some("build")) });
    let build = problems(vec![package(CODEC, &[]), build]);
    assert!(build.iter().any(|p| p.contains("build dependency")), "{build:?}");
}

#[test]
fn an_implicit_feature_or_another_feature_enabling_the_codec_fails() {
    // No `dep:` anywhere: Cargo gives the optional dependency an implicit feature of its name.
    let mut implicit = package("transit-time", &[]);
    implicit.dependencies.push(optional(CODEC));
    let implicit = problems(vec![package(CODEC, &[]), implicit]);
    assert!(implicit.iter().any(|p| p.contains("feature `transit-state-record`")), "{implicit:?}");
    let mut other =
        with_features(package("transit-time", &[]), &[("testing", &["dep:transit-state-record"]), ("probe", &["dep:transit-state-record"])]);
    other.dependencies.push(optional(CODEC));
    let other = problems(vec![package(CODEC, &[]), other]);
    assert!(other.iter().any(|p| p.contains("feature `probe`")), "{other:?}");
}

#[test]
fn testing_reached_from_default_fails_however_indirectly() {
    for default in [&["testing"][..], &["std"][..]] {
        let mut p = with_features(
            package("transit-time", &[]),
            &[("testing", &["dep:transit-state-record"]), ("std", &["extra"]), ("extra", &["testing"]), ("default", default)],
        );
        p.dependencies.push(optional(CODEC));
        let found = problems(vec![package(CODEC, &[]), p]);
        assert!(found.iter().any(|p| p.contains("feature `default`")), "default {default:?}: {found:?}");
    }
}

#[test]
fn a_renamed_dependency_is_followed_by_its_key() {
    let mut ok = with_features(package("transit-time", &[]), &[("testing", &["dep:record"])]);
    ok.dependencies.push(Dependency { rename: Some("record".into()), ..optional(CODEC) });
    assert!(problems(vec![package(CODEC, &[]), ok]).is_empty());
    // Named nowhere with `dep:`: the implicit feature takes the key, not the package name.
    let mut bad = package("transit-time", &[]);
    bad.dependencies.push(Dependency { rename: Some("record".into()), ..optional(CODEC) });
    let bad = problems(vec![package(CODEC, &[]), bad]);
    assert!(bad.iter().any(|p| p.contains("feature `record`")), "{bad:?}");
}

#[test]
fn forwarding_testing_from_another_feature_fails_weak_or_not() {
    for value in ["transit-dsp/testing", "transit-dsp?/testing"] {
        let mix = with_features(package("transit-mix", &[("transit-dsp", None)]), &[("testing", &["transit-dsp/testing"]), ("probe", &[value])]);
        let found = problems(vec![package(CODEC, &[]), user("transit-dsp"), mix]);
        assert!(found.iter().any(|p| p.contains("transit-mix: feature `probe`")), "{value}: {found:?}");
    }
}

#[test]
fn a_normal_dependency_requesting_the_testing_path_fails() {
    let mut mix = package("transit-mix", &[]);
    mix.dependencies.push(Dependency { features: vec!["testing".into()], ..dependency("transit-dsp", None) });
    let found = problems(vec![package(CODEC, &[]), user("transit-dsp"), mix]);
    assert!(found.iter().any(|p| p.contains("transit-mix: its dependency on transit-dsp")), "{found:?}");
    // Through the dependency's default features: its own default is flagged, and so is the request.
    let mut dsp = with_features(package("transit-dsp", &[]), &[("testing", &["dep:transit-state-record"]), ("default", &["testing"])]);
    dsp.dependencies.push(optional(CODEC));
    let found = problems(vec![package(CODEC, &[]), dsp, package("transit-mix", &[("transit-dsp", None)])]);
    assert!(found.iter().any(|p| p.contains("transit-dsp: feature `default`")), "{found:?}");
    assert!(found.iter().any(|p| p.contains("transit-mix: its dependency on transit-dsp")), "{found:?}");
}

/// The check binds exempt apps and standalone crates alike, before the exempt exit.
#[test]
fn the_check_runs_for_exempt_and_standalone_crates() {
    let metadata = Metadata { packages: vec![package(CODEC, &[]), package("transit", &[(CODEC, None)])] };
    let error = check_layers(&metadata, |_| Ok(INHERIT.into())).unwrap_err();
    assert!(error.contains("transit: the testing-only crate"), "{error}");
    // A standalone crate may take the codec through `testing`.
    let metadata = Metadata { packages: vec![package(CODEC, &[]), user("transit-instrument-core")] };
    assert!(check_layers(&metadata, |_| Ok(INHERIT.into())).is_ok());
    let metadata = Metadata { packages: vec![package(CODEC, &[]), package("transit-instrument-core", &[(CODEC, None)])] };
    assert!(check_layers(&metadata, |_| Ok(INHERIT.into())).is_err());
}
