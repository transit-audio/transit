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
