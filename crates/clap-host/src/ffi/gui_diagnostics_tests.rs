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

/// Opened without raising the window: on a shared display, a top-level that a test maps and
/// closes can pass its id to the next test's window, which the window manager then moves (E74).
#[cfg(target_os = "linux")]
#[test]
fn timer_required_paints_only_after_the_host_delivers_its_registered_timer() {
    if !window_available() {
        return;
    }
    let inst = gain("timer_required");
    let before = transit_rt::run_loop::counts();
    inst.link().open_with("Fixture Gain", |_| {}).unwrap();
    assert_eq!(transit_rt::run_loop::counts(), (before.0, before.1 + 1));
    assert!(!inst.load_state(b"CMD:gui-painted"));
    let callbacks = inst.link().host().timer_callbacks();
    super::run_loop::deliver_registered_timers_for_test(&inst.link());
    assert!(inst.load_state(b"CMD:gui-painted"));
    assert_eq!(inst.link().host().timer_callbacks(), callbacks + 1, "count the delivered callback, not the advertised extension");
    inst.link().close();
    assert_eq!(transit_rt::run_loop::counts(), before);
}

/// Round-trips the host thread's X connection (the host window's surface readout queries the
/// server): the server has processed every request the host sent before this returns. It orders
/// requests per connection only, so a readout on the fixture's separate connection sees a host
/// request only after the host's own round trip (E64). The host maps the child with a round
/// trip but resizes it with a request it only flushes; a readout right after the map could
/// precede the resize and fail at the size (#346).
#[cfg(target_os = "linux")]
fn settle_host(inst: &Instance) {
    let link = inst.link();
    let g = link.lock();
    assert!(g.window.as_ref().and_then(|w| w.surface_readout()).is_some(), "the host window lives on this thread");
}

/// The host window's frame and content ids, for failure messages next to the fixture's report.
#[cfg(target_os = "linux")]
fn host_ids(inst: &Instance) -> String {
    let link = inst.link();
    let g = link.lock();
    g.window.as_ref().map_or_else(|| "no host window".to_owned(), |w| format!("host frame {:#x}, content {:#x}", w.raw().addr(), w.parent().addr()))
}

/// Host passes until `command` reads true on the fixture, at most 64; returns the last readout.
/// Each pass dispatches the host's events, runs its idle, round-trips its connection and then
/// reads the fixture's state on the fixture's own connection (each load_state round-trips that
/// connection and records the plug's events). The passes count host dispatches, not time: the
/// host maps the child on the dispatch after the one that detects it (a 100 ms fallback only
/// makes that sooner), and a stalled host or server delays a pass without using one up.
#[cfg(target_os = "linux")]
fn host_passes_until(q: &transit_rt::gui_thread::MessageQueue, inst: &Instance, command: &[u8]) -> bool {
    for _ in 0..64 {
        q.dispatch(256);
        inst.link().idle();
        settle_host(inst);
        if inst.load_state(command) {
            return true;
        }
    }
    false
}

/// The only test in this process that maps a top-level window. X gives a closed client's
/// resource ids to the next client, so the fixture's plug can get the id of a window another
/// test just closed, raised or not, and a window manager still handling that window then acts
/// on the plug: WSLg's Weston reparented it (E74, #346) or resized and moved it with a closed,
/// never-raised editor frame's late requests (E74 recurrence, #697). The host owns a plug it
/// mapped: it sizes it to the content at (0, 0) and puts it back when another client moves it,
/// so the embed is read as one state that ends the passes, and a third client then plays that
/// window manager deterministically. A reparent the host cannot undo; it fails `xembed-hosted`.
/// On any failure the fixture prints the plug's ids, parent, geometry and events (#349).
#[cfg(target_os = "linux")]
#[test]
fn xembed_plug_is_mapped_notified_resized_and_unmapped_by_the_host() {
    if !window_available() {
        return;
    }
    let q = transit_rt::gui_thread::MessageQueue::attach();
    let inst = gain("xembed_plug");
    inst.link().open("XEmbed Fixture").unwrap();
    let ids = host_ids(&inst);
    let explain = |what: &str| {
        inst.load_state(b"CMD:xembed-report");
        format!("{what}; the fixture printed the plug's state; {ids}")
    };
    // Mapped, not VIEWABLE: on the ambient display a window manager maps the top-level on its
    // own schedule (WSLg's XWM after an idle wake, #289), outside these bounded passes.
    if !host_passes_until(&q, &inst, b"CMD:xembed-embedded") {
        let report = explain("the embed did not complete");
        assert!(inst.load_state(b"CMD:xembed-hosted"), "the plug left the content window (another client, E74); {report}");
        assert!(inst.load_state(b"CMD:xembed-mapped"), "the fixture never maps itself; the host must honor XEMBED_MAPPED; {report}");
        assert!(inst.load_state(b"CMD:xembed-notified"), "the actual EMBEDDED_NOTIFY has the content ID and negotiated version 0; {report}");
        panic!("the child's initial 123x45 size follows the resized content; {report}");
    }
    // A window manager resizing and moving the mapped plug, as WSLg's did through a reused id:
    // the intruder's request is checked, and `settle_host` then puts the ConfigureNotify it
    // caused in front of the host's next dispatch. End to end only: on a display with a window
    // manager, a late frame configure can also make the host lay the plug out again, so this step
    // can pass without the restore; the deterministic check is rt's editor_x11 test
    // `xembed_a_plug_another_client_moved_is_put_back` on its private Xvfb. `xembed-restored`
    // also rejects the window following the plug's new size instead.
    assert!(inst.load_state(b"CMD:xembed-intrude"));
    settle_host(&inst);
    assert!(host_passes_until(&q, &inst, b"CMD:xembed-restored"), "{}", explain("the host puts back a plug another client moved and resized"));
    assert!(inst.load_state(b"CMD:xembed-unmap"));
    // The host unmaps with a request it only flushes, as it resizes.
    assert!(host_passes_until(&q, &inst, b"CMD:xembed-unmapped"), "{}", explain("the host unmaps the plug that cleared XEMBED_MAPPED"));
    inst.link().close();
}
