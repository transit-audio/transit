//! Editor capability and diagnostics against the real VST3 fixture.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::cell::RefCell;

thread_local! { static LOG: RefCell<Vec<(log::Level, String)>> = const { RefCell::new(Vec::new()) }; }
struct Capture;
impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, r: &log::Record<'_>) {
        LOG.with(|l| l.borrow_mut().push((r.level(), r.args().to_string())));
    }
    fn flush(&self) {}
}
fn capture() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Info);
    });
    LOG.with(|l| l.borrow_mut().clear());
}
#[test]
fn no_view_is_known_at_load_and_warns_once_on_direct_open() {
    let gain = tests::gain_instance();
    let descs = gain.bundle.classes();
    let synth = descs.iter().find(|d| d.name == "Fixture Synth").unwrap();
    let inst = gain.bundle.instantiate(&synth.cid).unwrap();
    assert!(!inst.editor().unwrap().has_view(), "a controller that returns no IPlugView has no GUI");
    capture();
    assert_eq!(inst.state_link().open("Fixture Synth"), Err("no gui extension".into()));
    LOG.with(|l| {
        let warnings = l.borrow().iter().filter(|(level, _)| *level == log::Level::Warn).map(|(_, s)| s.clone()).collect::<Vec<_>>();
        assert_eq!(warnings, ["plug-in editor not opened: Fixture Synth: no gui extension"]);
    });
}
#[test]
fn a_view_logs_one_probe_and_opens() {
    if cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none() {
        return;
    }
    let inst = tests::gain_instance();
    capture();
    let link = inst.editor().unwrap();
    link.open("Fixture Gain").unwrap();
    LOG.with(|l| {
        let log = l.borrow();
        assert_eq!(log.iter().filter(|(_, s)| s.starts_with("plug-in editor probe:")).count(), 1);
        assert_eq!(log.iter().filter(|(level, _)| *level == log::Level::Warn).count(), 0);
        assert!(log.iter().any(|(_, s)| s == "plug-in editor opened: Fixture Gain"));
    });
    link.close();
}

#[cfg(windows)]
fn size_after_attach_child(mode: &str) {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "ffi::editor_diagnostics_tests::size_after_attach", "--ignored", "--nocapture", "--test-threads=1"])
        .env("VST3_FIXTURE_SCALE", mode)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed"), "the child ran: {text}");
}

#[cfg(windows)]
#[test]
fn a_view_whose_size_fails_before_attach_opens_and_follows_its_size() {
    size_after_attach_child("size-error-before-attach");
}

#[cfg(windows)]
#[test]
fn a_view_whose_size_is_empty_before_attach_opens_and_follows_its_size() {
    size_after_attach_child("size-empty-before-attach");
}

/// No scale or resizeView callback supplies the size: the host must query after attached.
#[cfg(windows)]
#[test]
#[ignore = "run in a child process by the two size-before-attach tests"]
fn size_after_attach() {
    let mode = std::env::var("VST3_FIXTURE_SCALE").unwrap();
    assert!(matches!(mode.as_str(), "size-error-before-attach" | "size-empty-before-attach"));
    let inst = tests::gain_instance();
    let link = inst.editor().unwrap();
    capture();
    let mut initial_size = None;
    let opened = link.open_with("Fixture Gain", |w| initial_size = Some(w.content_size()));
    assert_eq!(opened, Ok(()), "open result: {opened:?}");
    assert_eq!(initial_size, Some((400, 300)), "default window before attached");
    assert_eq!(link.lock().window.as_ref().unwrap().content_size(), (640, 400), "window follows getSize after attached");
    LOG.with(|l| {
        let log = l.borrow();
        let probes = log.iter().filter(|(_, s)| s.starts_with("plug-in editor probe:")).collect::<Vec<_>>();
        assert_eq!(probes.len(), 1, "{log:?}");
        assert_eq!(probes[0].0, log::Level::Info);
        assert!(probes[0].1.contains("size=failed"), "the probe keeps its pre-attach fact: {log:?}");
        let fallback = "vst3 editor \"Fixture Gain\": size unknown before attached: default window 400×300, following the view's size after attached";
        assert_eq!(log.iter().filter(|(level, s)| *level == log::Level::Info && s == fallback).count(), 1, "{log:?}");
        assert_eq!(log.iter().filter(|(level, _)| *level == log::Level::Warn).count(), 0, "{log:?}");
        assert!(log.iter().any(|(_, s)| s == "plug-in editor opened: Fixture Gain"));
    });
    link.close();
    drop(link);
    drop(inst);
    crate::shutdown();
}
