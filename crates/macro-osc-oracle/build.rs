//! Builds the vendored C++ (vendor/: the macro oscillator, the resonator and the granular texture
//! processor) and the C ABI wrappers (cpp/oracle.cc, cpp/resonator_oracle.cc, cpp/granular_oracle.cc).
//!
//! Floating point is kept strict so the oracle is a stable reference: `-DTEST` (the desktop branch
//! of stmlib, as the firmware's own test makefile), `-O2` whatever the cargo profile, no
//! fast-math, and no FP contraction (`-ffp-contract=off` on GCC and Clang; `/fp:strict` on MSVC,
//! where contraction cannot be turned off on its own on every supported version).
//!
//! vendor/ is never edited. `cpp/compat.h` is force-included into every file. The patches in
//! `patches/all/` (on every compiler) and, on MSVC only, `patches/msvc/` (unified diffs against
//! vendor/, `git apply -p0` format) are applied to copies in `OUT_DIR/overlay`: the overlay comes
//! first on the include path, so a patched header shadows the vendored one, and a patched source
//! file is compiled from the overlay. `patches/all/` restores a line an upstream merge lost and makes
//! the granular processor's past-the-end reads and one undefined shift defined (VENDOR.md); every
//! other file compiles as fetched.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

fn collect(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, ext, out);
        } else if path.extension().is_some_and(|x| x == ext) {
            out.push(path);
        }
    }
}

/// Applies one unified diff to files under `vendor`, writing the results under `overlay`.
/// Each hunk's old text (context and `-` lines) must occur exactly once in the file.
fn apply_patch(patch: &Path, vendor: &Path, overlay: &Path) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read_to_string(patch).map_err(|e| format!("{}: {e}", patch.display()))?.replace("\r\n", "\n");
    let mut written = Vec::new();
    let mut hunks = 0usize;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(rel) = line.strip_prefix("--- ") else { continue };
        let rel = rel.trim();
        if lines.next().is_none_or(|l| l.trim() != format!("+++ {rel}")) {
            return Err(format!("{}: expected '+++ {rel}'", patch.display()));
        }
        let target = overlay.join(rel);
        let source = if target.exists() { target.clone() } else { vendor.join(rel) };
        let mut content = std::fs::read_to_string(&source).map_err(|e| format!("{}: {e}", source.display()))?.replace("\r\n", "\n");
        while lines.peek().is_some_and(|l| l.starts_with("@@")) {
            lines.next();
            let (mut old, mut new) = (String::new(), String::new());
            while let Some(l) = lines.peek().copied() {
                if l.starts_with("@@") || l.starts_with("--- ") {
                    break;
                }
                lines.next();
                match l.split_at_checked(1) {
                    Some((" ", rest)) => {
                        old.push_str(rest);
                        old.push('\n');
                        new.push_str(rest);
                        new.push('\n');
                    }
                    Some(("-", rest)) => {
                        old.push_str(rest);
                        old.push('\n');
                    }
                    Some(("+", rest)) => {
                        new.push_str(rest);
                        new.push('\n');
                    }
                    _ if l.is_empty() => {
                        old.push('\n');
                        new.push('\n');
                    }
                    _ => return Err(format!("{}: unexpected line {l:?}", patch.display())),
                }
            }
            if content.matches(&old).count() != 1 {
                return Err(format!("{}: a hunk does not match {rel} exactly once", patch.display()));
            }
            content = content.replacen(&old, &new, 1);
            hunks += 1;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, content).map_err(|e| e.to_string())?;
        written.push(target);
    }
    // A patch that lost its `--- ` header, or has no hunks, would otherwise apply nothing silently.
    if hunks == 0 {
        return Err(format!("{}: no hunk applied", patch.display()));
    }
    Ok(written)
}

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    let vendor = root.join("vendor");
    let cpp = root.join("cpp");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor");
    println!("cargo:rerun-if-changed=cpp");
    println!("cargo:rerun-if-changed=patches");

    let mut sources = Vec::new();
    collect(&vendor.join("plaits"), "cc", &mut sources);
    collect(&vendor.join("rings"), "cc", &mut sources);
    collect(&vendor.join("clouds"), "cc", &mut sources);
    collect(&vendor.join("beads/dsp"), "cc", &mut sources);
    sources.push(vendor.join("beads/resources.cc"));
    sources.push(vendor.join("beads/gain_control.cc"));
    sources.push(vendor.join("stmlib/dsp/atan.cc"));
    sources.push(vendor.join("stmlib/dsp/units.cc"));
    sources.push(vendor.join("stmlib/utils/random.cc"));
    sources.push(cpp.join("oracle.cc"));
    sources.push(cpp.join("resonator_oracle.cc"));
    sources.push(cpp.join("granular_oracle.cc"));
    sources.push(cpp.join("texture_oracle.cc"));
    sources.push(cpp.join("texture_extras_oracle.cc"));
    // The texture processor's second build, at the port's longest buffer (cpp/texture_oracle.cc):
    // the effect chain's sources only, in a renamed namespace.
    let mut long_sources: Vec<PathBuf> = ["processor.cc", "mu_law.cc", "delay_clock.cc", "delay_controller.cc", "trigger_generator.cc"]
        .iter()
        .map(|f| vendor.join("beads/dsp").join(f))
        .collect();
    long_sources.push(vendor.join("beads/resources.cc"));
    long_sources.push(cpp.join("texture_oracle.cc"));

    let compat = cpp.join("compat.h");
    let msvc = cc::Build::new().cpp(true).get_compiler().is_like_msvc();
    let overlay = out_dir.join("overlay");
    let _ = std::fs::remove_dir_all(&overlay);
    let mut patches = Vec::new();
    collect(&root.join("patches/all"), "patch", &mut patches);
    if msvc {
        collect(&root.join("patches/msvc"), "patch", &mut patches);
    }
    for patch in &patches {
        let written = apply_patch(patch, &vendor, &overlay).unwrap_or_else(|e| panic!("patch failed: {e}"));
        // A patched source file is compiled from the overlay instead of vendor/.
        for file in written {
            let Ok(rel) = file.strip_prefix(&overlay) else { continue };
            for source in sources.iter_mut().chain(long_sources.iter_mut()) {
                if source.strip_prefix(&vendor).is_ok_and(|r| r == rel) {
                    *source = file.clone();
                }
            }
        }
    }
    let configure = |build: &mut cc::Build| {
        build.cpp(true).define("TEST", None).opt_level(2).debug(false).warnings(false);
        // The overlay first (a patched header shadows the vendored one), then the stand-in for the
        // one hardware header the texture processor's input stage includes, then vendor/.
        build.include(&overlay).include(cpp.join("shim"));
        if msvc {
            build.flag("/std:c++14").flag("/fp:strict").flag("/EHsc").flag(format!("/FI{}", compat.display()));
        } else {
            build.flag("-std=c++11").flag("-ffp-contract=off").flag("-fno-fast-math").flag("-include").flag(compat.to_string_lossy().as_ref());
        }
        build.include(&vendor);
    };
    let mut build = cc::Build::new();
    configure(&mut build);
    build.files(&sources).compile("macro_osc_oracle");

    // 8 seconds of 16-bit stereo at 48 kHz (384000 frames of 32 bits): TEXTURE_LONG_BUFFER_BITS in
    // src/texture.rs. The banks must hold the longest layout (8-bit mono, 1536004 bytes) and the
    // 2048-byte freeze tail at the end of the last bank.
    let mut long = cc::Build::new();
    configure(&mut long);
    // Its own object directory: the same source paths would otherwise overwrite the first build's
    // objects.
    long.out_dir(out_dir.join("texture_long"))
        .define("beads", "transit_texture_long")
        .define("TRANSIT_TEXTURE_LONG", None)
        .define("TRANSIT_ORACLE_DELAY_BUFFER_SIZE_BITS", "12288000")
        .define("TRANSIT_ORACLE_DELAY_BANK_SIZE", "524288")
        .files(&long_sources)
        .compile("texture_oracle_long");
}
