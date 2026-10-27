//! File-size gate (#149): a Rust file may not exceed `MAX_LINES` lines. Files over the budget today sit in
//! `OVER_BUDGET` with their line count; a listed file may only shrink, and the table only ever shrinks.

use std::path::Path;
use std::process::Command;

/// The line budget for any Rust file.
pub const MAX_LINES: usize = 1500;

/// Files over `MAX_LINES` today, as (repo-relative path, line budget). Remove a row when its file drops to `MAX_LINES` or fewer.
pub const OVER_BUDGET: &[(&str, usize)] = &[
    ("crates/clap-host/src/ffi.rs", 1928),
    ("crates/engine/src/io.rs", 1765),
    ("crates/mix/src/plan_tests.rs", 1727),
    ("crates/mix/tests/instrument_events.rs", 1653),
    ("crates/playback/src/player/audio/tests.rs", 1609),
    ("crates/playback/src/player/device.rs", 1849),
    ("crates/playback/src/player/device/tests.rs", 1583),
    ("crates/playback/src/record.rs", 2071),
    ("crates/ui-egui/src/edit_window.rs", 3896),
    ("crates/ui-egui/src/lib.rs", 2037),
    ("crates/ui-studio/src/audio_devices.rs", 1918),
    ("crates/ui-studio/src/automation.rs", 1581),
    ("crates/ui-studio/src/browser.rs", 1596),
    ("crates/ui-studio/src/host.rs", 1815),
    ("crates/video/src/isobmff/codec.rs", 1579),
    ("crates/vst3-host/src/ffi.rs", 3779),
    ("crates/vst3-host/tests/fixture/src/lib.rs", 1763),
];

/// Checks `files` (repo-relative path, line count) against `MAX_LINES` and `table`. Reports every problem.
pub fn check_file_size(files: &[(String, usize)], table: &[(&str, usize)]) -> Result<(), String> {
    let mut problems = Vec::new();
    for (path, lines) in files {
        let lines = *lines;
        match table.iter().find(|(p, _)| p == path) {
            Some((_, budget)) if lines > *budget => problems.push(format!(
                "{path}: {lines} lines, over its OVER_BUDGET budget of {budget}; a listed file may only shrink: split it by concern (see #149)"
            )),
            Some((_, budget)) if lines <= MAX_LINES => {
                problems.push(format!("{path}: {lines} lines, at or under {MAX_LINES} (budget {budget}): remove it from OVER_BUDGET"))
            }
            Some(_) => {}
            None if lines > MAX_LINES => problems
                .push(format!("{path}: {lines} lines, over the {MAX_LINES}-line budget (not listed in OVER_BUDGET): split it by concern (see #149)")),
            None => {}
        }
    }
    for (path, budget) in table {
        if !files.iter().any(|(p, _)| p == path) {
            problems.push(format!("{path}: listed in OVER_BUDGET (budget {budget}) but the file no longer exists: remove it from OVER_BUDGET"));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
}

pub fn count_lines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|b| **b == b'\n').count()
}

pub fn run(root: &Path) -> Result<(), String> {
    let out = Command::new("git").current_dir(root).args(["ls-files", "-z", "--", "*.rs"]).output().map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err(format!("git ls-files failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let mut files = Vec::new();
    for path in String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()) {
        if !["crates/", "apps/", "tools/", "xtask/"].iter().any(|dir| path.starts_with(dir)) {
            continue;
        }
        // A tracked file deleted in the working tree is reported by the "no longer exists" rule only if listed.
        if let Ok(bytes) = std::fs::read(root.join(path)) {
            files.push((path.to_string(), count_lines(&bytes)));
        }
    }
    check_file_size(&files, OVER_BUDGET)?;
    println!("file-size: ok ({} files, {} over budget)", files.len(), OVER_BUDGET.len());
    Ok(())
}
