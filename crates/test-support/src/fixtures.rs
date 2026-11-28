//! The VST3 and CLAP hosts' fixture plug-ins (cdylibs outside the workspace), built and installed
//! for the integration tests of any crate. Moved here from the hosts' `tests/common` so a test of
//! another crate builds the same fixtures the same way (ADR 0064 slice HM2).
//!
//! `tmp` is the calling test's scratch root (its `CARGO_TARGET_TMPDIR`): each fixture build gets
//! its own target folder `target_dir` there.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Builds the VST3 fixture with `features` in its own target folder `target_dir` and installs it as
/// a bundle `<dir>/vendor/<name>.vst3`, which it returns.
pub fn install_vst3_named(tmp: &Path, features: &[&str], target_dir: &str, dir: &Path, name: &str) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vst3-host/tests/fixture/Cargo.toml");
    let target = tmp.join(target_dir);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .args(features)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("run cargo");
    assert!(st.success(), "building the VST3 fixture failed");
    let (lib, folder, exe) = if cfg!(target_os = "macos") {
        ("libvst3_fixture.dylib", "MacOS", name.to_string())
    } else if cfg!(windows) {
        ("vst3_fixture.dll", "x86_64-win", format!("{name}.vst3"))
    } else {
        ("libvst3_fixture.so", if cfg!(target_arch = "aarch64") { "aarch64-linux" } else { "x86_64-linux" }, format!("{name}.so"))
    };
    let built = target.join("debug").join(lib);
    let bundle = dir.join(format!("vendor/{name}.vst3"));
    let bin = bundle.join("Contents").join(folder);
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(&built, bin.join(exe)).unwrap();
    bundle
}

/// Builds the CLAP fixture with `features` in its own target folder `target_dir` and installs it as
/// `<dir>/vendor/<name>.clap` (a bundle on macOS), which it returns.
pub fn install_clap_named(tmp: &Path, features: &[&str], target_dir: &str, dir: &Path, name: &str) -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../clap-host/tests/fixture/Cargo.toml");
    let target = tmp.join(target_dir);
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .args(features)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .expect("run cargo");
    assert!(st.success(), "building the CLAP fixture failed");
    let lib = if cfg!(target_os = "macos") {
        "libclap_fixture.dylib"
    } else if cfg!(windows) {
        "clap_fixture.dll"
    } else {
        "libclap_fixture.so"
    };
    let built = target.join("debug").join(lib);
    let bundle = dir.join(format!("vendor/{name}.clap"));
    if cfg!(target_os = "macos") {
        // Install as a real macOS bundle to exercise bundle resolution.
        let exe = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&exe).unwrap();
        std::fs::copy(&built, exe.join(name)).unwrap();
    } else {
        std::fs::create_dir_all(dir.join("vendor")).unwrap();
        std::fs::copy(&built, &bundle).unwrap();
    }
    bundle
}
