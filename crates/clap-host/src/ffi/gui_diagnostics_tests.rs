//! #289: real fixture branches and thread-local log capture, with no scheduling assertions.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::cell::RefCell;

thread_local! {
    static LOG: RefCell<Vec<(log::Level, String)>> = const { RefCell::new(Vec::new()) };
}
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
fn gain(mode: &str) -> Instance {
    let descriptors = crate::scan_paths(&[fixture::unregistered_fixture_dir().to_path_buf()]);
    let d = descriptors.iter().find(|d| !d.is_instrument).unwrap();
    let bundle = Bundle::load(&d.path).unwrap();
    let inst = bundle.instantiate(&d.plugin_id).unwrap();
    assert!(inst.load_state(format!("CMD:{mode}").as_bytes()));
    if mode == "no_gui" {
        drop(inst);
        bundle.instantiate(&d.plugin_id).unwrap()
    } else {
        inst
    }
}
fn failure(mode: &str, reason: &str) {
    let inst = gain(mode);
    capture();
    let result = inst.link().open("Fixture Gain");
    let warnings = LOG.with(|l| l.borrow().iter().filter(|(level, _)| *level == log::Level::Warn).map(|(_, s)| s.clone()).collect::<Vec<_>>());
    // First assertion is the negative control: the original host emitted no WARN.
    assert_eq!(warnings, vec![format!("plug-in editor not opened: Fixture Gain: {reason}")]);
    assert_eq!(result, Err(reason.to_owned()));
    assert!(!inst.link().is_open());
}
fn window_available() -> bool {
    cfg!(windows) || std::env::var_os("DISPLAY").is_some()
}
#[test]
fn no_gui_warns_once() {
    failure("no_gui", "no gui extension");
    let inst = gain("no_gui");
    assert!(inst.gui().is_none(), "absence is known at load");
}
#[test]
fn neither_api_warns_once() {
    failure("neither", if cfg!(windows) { "win32 embedding not supported (neither)" } else { "x11 embedding not supported (neither)" });
}
#[test]
fn create_false_warns_once() {
    failure("create_false", "create failed");
}
#[test]
fn set_parent_false_warns_once() {
    if window_available() {
        failure("set_parent_false", "set_parent failed");
    }
}
#[test]
fn show_false_warns_once() {
    if window_available() {
        failure("show_false", "show failed");
    }
}
#[test]
fn zero_size_warns_once() {
    if window_available() {
        failure("zero_size", "zero size");
    }
}
#[test]
fn floating_only_opens_with_one_probe_and_no_warning() {
    let inst = gain("no_x11");
    capture();
    inst.link().open("Fixture Gain").unwrap();
    assert!(inst.link().is_open());
    LOG.with(|l| {
        let log = l.borrow();
        assert_eq!(log.iter().filter(|(level, _)| *level == log::Level::Warn).count(), 0);
        let probes = log.iter().filter(|(_, s)| s.starts_with("plug-in editor probe:")).collect::<Vec<_>>();
        assert_eq!(probes.len(), 1);
        assert!(probes[0].1.contains("format=CLAP"));
        assert!(probes[0].1.contains("embedded=false floating=true"));
        assert!(log.iter().any(|(level, s)| *level == log::Level::Info && s == "plug-in editor opened: Fixture Gain"));
    });
    inst.link().close();
}

#[cfg(target_os = "linux")]
#[test]
fn timer_required_paints_only_after_the_host_delivers_its_registered_timer() {
    if !window_available() {
        return;
    }
    let inst = gain("timer_required");
    let before = transit_rt::run_loop::counts();
    inst.link().open("Fixture Gain").unwrap();
    assert_eq!(transit_rt::run_loop::counts(), (before.0, before.1 + 1));
    assert!(!inst.load_state(b"CMD:gui-painted"));
    let callbacks = inst.link().host().timer_callbacks();
    super::run_loop::deliver_registered_timers_for_test(&inst.link());
    assert!(inst.load_state(b"CMD:gui-painted"));
    assert_eq!(inst.link().host().timer_callbacks(), callbacks + 1, "count the delivered callback, not the advertised extension");
    inst.link().close();
    assert_eq!(transit_rt::run_loop::counts(), before);
}

#[cfg(target_os = "linux")]
#[test]
fn xembed_plug_is_mapped_notified_resized_and_unmapped_by_the_host() {
    if !window_available() {
        return;
    }
    let q = transit_rt::gui_thread::MessageQueue::attach();
    let inst = gain("xembed_plug");
    inst.link().open("XEmbed Fixture").unwrap();
    // Explicit readouts on the fixture's separate X connection. Each load_state round-trips
    // that connection and records ClientMessages. 64 bounded idle passes allow the finite
    // create/property/notify exchanges to drain, without asserting elapsed time or timers.
    for _ in 0..64 {
        q.dispatch(256);
        inst.link().idle();
        if inst.load_state(b"CMD:xembed-viewable") && inst.load_state(b"CMD:xembed-notified") {
            break;
        }
    }
    assert!(inst.load_state(b"CMD:xembed-viewable"), "the fixture never maps itself; the host must honor XEMBED_MAPPED");
    assert!(inst.load_state(b"CMD:xembed-notified"), "the actual EMBEDDED_NOTIFY has the content ID and negotiated version 0");
    assert!(inst.load_state(b"CMD:xembed-sized"), "the child's initial 123x45 size follows the resized content");
    assert!(inst.load_state(b"CMD:xembed-unmap"));
    for _ in 0..64 {
        q.dispatch(256);
        inst.link().idle();
        if inst.load_state(b"CMD:xembed-unmapped") {
            break;
        }
    }
    assert!(inst.load_state(b"CMD:xembed-unmapped"));
    inst.link().close();
}
