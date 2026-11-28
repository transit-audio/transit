//! Shared by the integration tests: builds the fixture plugins (a cdylib outside the workspace)
//! and installs them in temp folders.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Builds the fixture once and returns a folder containing `Fixture.clap` (gain and synth).
#[allow(dead_code)]
pub fn fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&[], "clap-fixture-target", "clap-fixture-install", "Fixture", 2))
}

/// Builds the fixture with its `requests` feature once and returns a folder containing
/// `Requests.clap` (only the requests plug-in).
#[allow(dead_code)]
pub fn requests_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "requests"], "clap-requests-fixture-target", "clap-requests-install", "Requests", 1))
}

/// Builds the fixture with its `events` feature once and returns a folder containing
/// `Events.clap` (the native-note and MIDI-only events plug-ins, F10).
#[allow(dead_code)]
pub fn events_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "events"], "clap-events-fixture-target", "clap-events-install", "Events", 2))
}

/// Builds the fixture with its `sidechain` feature once and returns a folder containing
/// `Sidechain.clap` (only the sidechain plug-in, ADR 0015 slice c).
#[allow(dead_code)]
pub fn sidechain_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "sidechain"], "clap-sidechain-fixture-target", "clap-sidechain-install", "Sidechain", 1))
}

/// Builds the fixture with its `notes` feature once and returns a folder containing `Notes.clap`
/// with the note generators (ADR 0064 slices HM0 and HM1).
#[allow(dead_code)]
pub fn notes_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "notes"], "clap-notes-fixture-target", "clap-notes-install", "Notes", 2))
}

/// Builds the fixture with its `note-effects` feature once and returns a folder containing
/// `NoteEffects.clap` with the three zero-audio note effects (ADR 0064 slices HM0 and HM1).
#[allow(dead_code)]
pub fn note_effects_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "note-effects"], "clap-note-effects-fixture-target", "clap-note-effects-install", "NoteEffects", 3))
}

fn install(features: &[&str], target_dir: &str, install_dir: &str, name: &str, plugins: usize) -> PathBuf {
    transit_clap_host::set_scan_extra_dirs_only(true);
    let dir = Path::new(option_env!("CARGO_TARGET_TMPDIR").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/tmp")))
        .join(format!("{install_dir}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    install_named(features, target_dir, &dir, name);
    if plugins > 0 {
        assert_eq!(transit_clap_host::add_search_dir(&dir), plugins);
    }
    dir
}

/// Builds the fixture with `features` in its own target folder `target_dir` and installs it as
/// `<dir>/vendor/<name>.clap` (a bundle on macOS), which it returns.
#[allow(dead_code)]
pub fn install_named(features: &[&str], target_dir: &str, dir: &Path, name: &str) -> PathBuf {
    let tmp = Path::new(option_env!("CARGO_TARGET_TMPDIR").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/tmp")));
    transit_test_support::fixtures::install_clap_named(tmp, features, target_dir, dir, name)
}

/// Private-FFI unit tests load through their own crate instance, avoiding a second module entry.
#[allow(dead_code)]
pub fn unregistered_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&[], "clap-fixture-target", "clap-unit-fixture-install", "Fixture", 0))
}
