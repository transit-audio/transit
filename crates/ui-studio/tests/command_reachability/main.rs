//! Gate (#734): every command the engine registers is reachable from the interface, or is on the
//! explicit exceptions list with a reason.
//!
//! Both sides are derived, nothing here lists commands except the exceptions:
//! - the registry is `transit_engine::command_specs()`, the table the control channel and the
//!   engine run from;
//! - the interface is every string literal in the shipped (non-test) code of the UI crate and the
//!   application, walked through the real module tree (`scan.rs`). The interface runs a command by
//!   naming its id, from a menu row, a shortcut handler, a context menu or a button, so an id that
//!   appears in that code is an id the interface can run.
//!
//! A new command with no surface and no exception fails the first test. A command that gains a
//! surface while still listed fails the second, so the list only ever shrinks.

mod exceptions;
mod scan;

use std::collections::BTreeSet;
use std::path::Path;
use transit_engine::command_specs;

/// The literals of the interface's shipped code, and how many source files were walked.
fn interface() -> (BTreeSet<String>, usize) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (mut literals, mut files) = scan::shipped_literals(&manifest.join("src/lib.rs"));
    let (app_literals, app_files) = scan::shipped_literals(&manifest.join("../../apps/transit/src/main.rs"));
    literals.extend(app_literals);
    files += app_files;
    (literals, files)
}

fn unreachable_commands() -> Vec<&'static transit_engine::CommandSpec> {
    let (literals, _) = interface();
    command_specs().iter().filter(|c| !literals.contains(c.id)).collect()
}

fn describe(c: &transit_engine::CommandSpec) -> String {
    let menu = if c.menu.is_empty() { "no menu path".to_string() } else { format!("menu path {}", c.menu.join(" > ")) };
    format!("{} ({}; {})", c.id, c.label, menu)
}

#[test]
fn the_scan_sees_the_interface() {
    // Guards the gate itself: a scan that walked nothing would pass every command as an exception.
    let (literals, files) = interface();
    assert!(files > 100, "the module walk reached only {files} files");
    for id in ["edit.trim_clip", "session.save", "transport.play", "track.new", "mix.volume"] {
        assert!(literals.contains(id), "{id} is run by the interface but the scan did not find it");
    }
    assert!(!literals.contains("transit.test.sentinel"), "a literal that appears nowhere was found");
}

#[test]
fn every_command_is_reachable_or_listed() {
    let listed: BTreeSet<&str> = exceptions::all().map(|(id, _)| *id).collect();
    let missing: Vec<String> = unreachable_commands().into_iter().filter(|c| !listed.contains(c.id)).map(describe).collect();
    assert!(
        missing.is_empty(),
        "{} registered command(s) have no interface surface and are not on the exceptions list in \
         tests/command_reachability/exceptions.rs. Draw them in a menu or run them from a control, or list them \
         there with a one-line reason:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

#[test]
fn the_exceptions_list_holds_only_unreachable_commands() {
    let unreachable: BTreeSet<&str> = unreachable_commands().into_iter().map(|c| c.id).collect();
    let registered: BTreeSet<&str> = command_specs().iter().map(|c| c.id).collect();
    let mut seen = BTreeSet::new();
    for (id, reason) in exceptions::all() {
        assert!(registered.contains(id), "exception `{id}` is not a registered command: remove it");
        assert!(unreachable.contains(id), "exception `{id}` now has an interface surface: remove it from the list");
        assert!(seen.insert(*id), "exception `{id}` is listed twice");
        assert!(!reason.trim().is_empty() && !reason.contains('\n'), "exception `{id}` needs a one-line reason");
    }
}

#[test]
fn the_two_sections_are_what_they_say() {
    // The decided section gives a reason; only the undecided one may still say so (and must).
    for (id, reason) in exceptions::decided() {
        assert!(
            !reason.to_ascii_lowercase().contains("undecided"),
            "decided exception `{id}` still says undecided: move it to undecided.rs or give it its reason"
        );
    }
    for (id, reason) in exceptions::undecided() {
        assert!(
            reason.starts_with("undecided"),
            "exception `{id}` is in undecided.rs but its reason does not say undecided: move it to control_only.rs"
        );
    }
}
