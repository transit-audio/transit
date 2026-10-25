//! Windows only: embed the app icon and version info (VERSIONINFO) into `transit.exe`, so it shows
//! in Explorer, the taskbar, the Start menu and Alt-Tab.
//!
//! On every other target this does nothing. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `SOUNDCRAFT_REQUIRE_WINRES=1` turns it
//! into an error (for release builds).

/// `<short sha> <commit date>` of the checkout (`TRANSIT_BUILD` overrides it, for a build from a
/// source archive); `unknown` when neither is available. Shown by `--report` and Help > About.
fn build_id() -> String {
    println!("cargo:rerun-if-env-changed=TRANSIT_BUILD");
    if let Ok(id) = std::env::var("TRANSIT_BUILD") {
        return id;
    }
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git").args(args).output().ok().filter(|o| o.status.success())?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_owned()).filter(|s| !s.is_empty())
    };
    // Rebuild when the checked-out commit moves: HEAD itself, and the branch ref it names.
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &reference])
    {
        println!("cargo:rerun-if-changed={path}");
    }
    match (git(&["rev-parse", "--short=8", "HEAD"]), git(&["log", "-1", "--format=%cs"])) {
        (Some(sha), Some(date)) => format!("{sha} {date}"),
        (Some(sha), None) => sha,
        _ => "unknown".into(),
    }
}

fn main() {
    println!("cargo:rustc-env=TRANSIT_BUILD={}", build_id());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon/transit.ico");
    println!("cargo:rerun-if-env-changed=SOUNDCRAFT_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/transit.ico")
        .set("ProductName", "transit")
        .set("FileDescription", "transit digital audio workstation")
        .set("LegalCopyright", "Copyright (c) the transit authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "transit.exe")
        .set("InternalName", "transit");
    if let Err(e) = res.compile() {
        if std::env::var_os("SOUNDCRAFT_REQUIRE_WINRES").is_some() {
            eprintln!("embedding Windows resources failed: {e}");
            std::process::exit(1);
        }
        println!("cargo:warning=transit.exe built without icon/version resources: {e}");
    }
}
