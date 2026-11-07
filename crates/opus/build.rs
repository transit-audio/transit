//! Builds the vendored libopus 1.6.1 (vendor/, VENDOR.md) with `cc`: no cmake, autotools, meson or
//! system library, so every CI runner (Windows MSVC, macOS arm64, Linux) builds it from source.
//!
//! The file set is upstream's own: the source lists are read from vendor/celt_sources.mk,
//! vendor/silk_sources.mk and vendor/opus_sources.mk (`CELT_SOURCES`, `SILK_SOURCES`,
//! `SILK_SOURCES_FLOAT`, `OPUS_SOURCES`, `OPUS_SOURCES_FLOAT`), the floating-point build that
//! upstream's configure produces by default. The optional parts are left out: no run-time CPU
//! detection or SIMD sources (portable C only: a bounce is offline), no deep-learning PLC, DRED,
//! OSCE or QEXT (`dnn/` is not vendored), no custom modes, no assertions. The defines are the ones
//! upstream's CMake and meson builds set for this configuration (VENDOR.md lists them). vendor/ is
//! never edited and no patch is applied.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// The `.c` files of the make variable `var` in the upstream source list `mk`.
fn sources(mk: &Path, var: &str) -> Vec<String> {
    let text = std::fs::read_to_string(mk).unwrap_or_else(|e| panic!("{}: {e}", mk.display()));
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if !inside {
            if let Some(rest) = line.strip_prefix(var) {
                inside = rest.trim_start().starts_with('=');
            }
            continue;
        }
        if line.is_empty() {
            break;
        }
        out.extend(line.split_whitespace().filter(|w| w.ends_with(".c")).map(str::to_string));
        if !line.ends_with('\\') {
            break;
        }
    }
    assert!(!out.is_empty(), "{}: no sources in {var}", mk.display());
    out
}

fn main() {
    let vendor = PathBuf::from("vendor");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor");
    let mut files = Vec::new();
    for (mk, var) in [
        ("celt_sources.mk", "CELT_SOURCES"),
        ("silk_sources.mk", "SILK_SOURCES"),
        ("silk_sources.mk", "SILK_SOURCES_FLOAT"),
        ("opus_sources.mk", "OPUS_SOURCES"),
        ("opus_sources.mk", "OPUS_SOURCES_FLOAT"),
    ] {
        files.extend(sources(&vendor.join(mk), var));
    }
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let msvc = target_env == "msvc";
    let mut build = cc::Build::new();
    build
        .include(vendor.join("include"))
        .include(vendor.join("celt"))
        .include(vendor.join("silk"))
        .include(vendor.join("silk/float"))
        .include(vendor.join("src"))
        .define("OPUS_BUILD", None)
        .define("ENABLE_HARDENING", None)
        .define("PACKAGE_VERSION", Some("\"1.6.1\""))
        // Every supported C library has both (upstream's CMake defines them when it finds them).
        .define("HAVE_LRINT", None)
        .define("HAVE_LRINTF", None)
        // Upstream's build optimizes whatever the cargo profile (a debug bounce is still fast).
        .opt_level(2)
        // Upstream's own warnings are not transit's to fix (vendor/ is never edited).
        .warnings(false);
    if msvc {
        // MSVC has no variable-length arrays: upstream's CMake uses alloca there.
        build.define("USE_ALLOCA", None).define("_CRT_SECURE_NO_WARNINGS", None);
    } else {
        build.define("VAR_ARRAYS", None);
    }
    for f in &files {
        build.file(vendor.join(f));
    }
    build.compile("transit_opus_libopus");
}
