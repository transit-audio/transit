//! Shared by the integration tests: builds the fixture plugin (a cdylib outside the workspace)
//! and installs it as a `.vst3` bundle in a temp folder.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Builds the fixture once and returns a folder containing `Fixture.vst3`.
#[allow(dead_code)]
pub fn fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&[], "vst3-fixture-target", "vst3-fixture-install", 3))
}

/// Builds the fixture with its `sidechain` feature once and returns a folder containing
/// `Fixture.vst3` with only the sidechain plug-ins (ADR 0015 slice c).
#[allow(dead_code)]
pub fn sidechain_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "sidechain"], "vst3-sidechain-fixture-target", "vst3-sidechain-fixture-install", 2))
}

/// Builds the fixture with its `slow-create` feature once (its synth takes 500 ms to create) and
/// returns a folder containing `Fixture.vst3` with all of the fixture's plug-ins.
#[allow(dead_code)]
pub fn slow_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "slow-create"], "vst3-slow-fixture-target", "vst3-slow-fixture-install", 3))
}

/// Builds the fixture with its `notes` feature once and returns a folder containing `Fixture.vst3`
/// with the note generator classes (ADR 0064 slices HM0 and HM1).
#[allow(dead_code)]
pub fn notes_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "notes"], "vst3-notes-fixture-target", "vst3-notes-install", 2))
}

/// Builds the fixture with its `note-effects` feature once and returns a folder containing
/// `Fixture.vst3` with the three zero-audio note effects (ADR 0064 slices HM0 and HM1).
#[allow(dead_code)]
pub fn note_effects_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&["--features", "note-effects"], "vst3-note-effects-fixture-target", "vst3-note-effects-install", 3))
}

fn install(features: &[&str], target_dir: &str, install_dir: &str, plugins: usize) -> PathBuf {
    {
        transit_vst3_host::set_scan_extra_dirs_only(true);
        let dir = Path::new(option_env!("CARGO_TARGET_TMPDIR").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/tmp")))
            .join(format!("{install_dir}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Install as a real bundle to exercise bundle resolution and the module entry.
        install_named(features, target_dir, &dir, "Fixture");
        if plugins > 0 {
            assert_eq!(transit_vst3_host::add_search_dir(&dir), plugins, "the fixture's plug-ins");
        }
        dir
    }
}

/// Builds the fixture with `features` in its own target folder `target_dir` and installs it as a
/// bundle `<dir>/vendor/<name>.vst3`, which it returns.
#[allow(dead_code)]
pub fn install_named(features: &[&str], target_dir: &str, dir: &Path, name: &str) -> PathBuf {
    let tmp = Path::new(option_env!("CARGO_TARGET_TMPDIR").unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/tmp")));
    transit_test_support::fixtures::install_vst3_named(tmp, features, target_dir, dir, name)
}

/// Private-FFI unit tests load through their own crate instance, avoiding a second module entry.
#[allow(dead_code)]
pub fn unregistered_fixture_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| install(&[], "vst3-fixture-target", "vst3-unit-fixture-install", 0))
}
