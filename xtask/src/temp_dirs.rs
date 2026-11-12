//! `cargo xtask temp-dirs` (#497): code outside `transit-test-support` may not call the system temp
//! directory function directly. Tests take their scratch folders from `transit_test_support::TestDir`, which
//! removes the folder when the test ends (pass, fail or panic) and never reuses a path; a direct call is how
//! tests leaked thousands of `transit-*` folders into the system temp directory.
//!
//! The few product call sites (a fallback folder the app itself uses at run time) sit in `ALLOWED` with the
//! number of occurrences each file may hold; a file may not gain one, and a row whose file no longer has the
//! call must leave the table. Comment lines are ignored.

use std::path::Path;
use std::process::Command;

/// The call this gate looks for, split so this file does not match itself.
const NEEDLE: &str = concat!("env::temp", "_dir");

/// The helper crate that owns the one test-side call.
const HELPER: &str = "crates/test-support/";

/// Files allowed to name the temp directory function, with the occurrences each may hold. Product code only:
/// the no-preferences mode's own folder (removed on drop), and fallback locations the app picks at run time.
pub const ALLOWED: &[(&str, usize)] = &[
    ("apps/transit/src/config.rs", 1),
    ("crates/engine/src/cmd/file.rs", 1),
    ("crates/ui-studio/src/finish.rs", 2),
    ("crates/ui-studio/src/host/recording.rs", 2),
    ("crates/ui-studio/src/session_folder.rs", 2),
];

/// `files` is (repo-relative path, text). Reports every problem.
pub fn check_temp_dirs(files: &[(String, String)], allowed: &[(&str, usize)]) -> Result<(), String> {
    let mut problems = Vec::new();
    for (path, text) in files {
        if path.starts_with(HELPER) {
            continue;
        }
        let hits: Vec<usize> =
            text.lines().enumerate().filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(NEEDLE)).map(|(i, _)| i + 1).collect();
        let budget = allowed.iter().find(|(p, _)| p == path).map_or(0, |(_, n)| *n);
        if hits.len() > budget {
            let lines = hits.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
            problems.push(format!(
                "{path}: {} direct system temp directory call(s) (allowed {budget}) at line {lines}: take the folder from transit_test_support::TestDir (#497)",
                hits.len()
            ));
        } else if hits.len() < budget {
            problems
                .push(format!("{path}: has {} call(s) but ALLOWED lists {budget}: lower or remove the row in xtask/src/temp_dirs.rs", hits.len()));
        }
    }
    for (path, n) in allowed {
        if !files.iter().any(|(p, _)| p == path) {
            problems.push(format!("{path}: listed in ALLOWED ({n}) but the file no longer exists: remove the row"));
        }
    }
    if problems.is_empty() { Ok(()) } else { Err(problems.join("\n")) }
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
        if let Ok(text) = std::fs::read_to_string(root.join(path)) {
            files.push((path.to_string(), text));
        }
    }
    check_temp_dirs(&files, ALLOWED)?;
    println!("temp-dirs: ok ({} files, {} allowed product sites)", files.len(), ALLOWED.iter().map(|(_, n)| n).sum::<usize>());
    Ok(())
}
