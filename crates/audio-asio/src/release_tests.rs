//! The host's release audit (#363): every path that instantiates a driver ends with the driver's
//! release on the host thread, the thread that created and initialized it. ASIO has no exit method
//! of its own: the SDK's `ASIOExit` is the session's `stop` (when started) and `disposeBuffers` (when
//! buffers were created) followed by the COM `Release`, so each test counts, per instantiation, one
//! `Release`, on the host thread, with every buffer disposed and every start stopped. Fakes only:
//! no test here touches a real driver.

use super::*;
use crate::abi;
use crate::com::fake::{FakeLoader, FakeSpec, Gate, Observed, entry};
use crate::{AsioBackend, Loaded as LoadedHandle};
use std::time::Instant;

fn spec() -> FakeSpec {
    FakeSpec::new("Driver", 2, 2, abi::ASIOST_INT32_LSB)
}

/// One `Release` per instantiation, each on the host thread (never the test's), and the exit steps
/// done: no buffer left undisposed, every start stopped.
fn assert_released_once_each(observed: &Observed, loads: usize) {
    assert_eq!(observed.created.load(Ordering::Acquire), loads, "instantiations");
    assert_eq!(observed.released.load(Ordering::Acquire), loads, "one Release per instantiation");
    assert_eq!(observed.live.load(Ordering::Acquire), 0, "nothing left loaded");
    let releases = observed.releases.lock().unwrap().clone();
    let host = observed.threads.lock().unwrap().first().copied();
    assert!(releases.iter().all(|t| Some(*t) == host), "every Release on the thread that created the driver: {releases:?} vs {host:?}");
    assert!(releases.iter().all(|t| *t != std::thread::current().id()), "never on the caller's thread");
    assert!(observed.buffers.lock().unwrap().is_empty(), "every created buffer disposed");
    let calls = observed.call_names();
    let count = |name: &str| calls.iter().filter(|c| **c == name).count();
    assert_eq!(count("start"), count("stop"), "every start stopped: {calls:?}");
}

/// Waits (bounded) until `observed` has no live driver: the host releases between requests.
fn wait_released(observed: &Observed) {
    let deadline = Instant::now() + Duration::from_secs(10);
    // timing: stall-bound ten seconds for the host's between-requests sweep
    while observed.live.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Holds the host thread in a request until the returned sender is dropped (or sends).
fn hold<'scope>(scope: &'scope std::thread::Scope<'scope, '_>, host: &'scope Host) -> mpsc::Sender<()> {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    scope.spawn(move || {
        host.call("hold", move |_| {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
            Ok(())
        })
        .expect("the hold is answered");
    });
    entered_rx.recv_timeout(Duration::from_secs(10)).expect("the host thread entered the hold");
    release_tx
}

#[test]
fn every_probe_releases_its_driver_once_on_the_host_thread() {
    let mut broken = spec();
    broken.channels_error = Some(abi::ASE_HW_MALFUNCTION);
    let mut absent = spec();
    absent.init_ok = false;
    let drivers = vec![(entry("Driver", 1), spec()), (entry("Broken", 2), broken), (entry("Absent", 3), absent), (entry("Gone", 4), spec())];
    let (mut loader, observed) = FakeLoader::new(drivers);
    loader.create_fails.push("Gone".into());
    let backend = AsioBackend::with_loader(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("backend");
    for _ in 0..10 {
        backend.probe("Driver").expect("probe");
    }
    assert_released_once_each(&observed, 10);
    assert!(backend.probe("Broken").is_err(), "a query that fails after init");
    assert_released_once_each(&observed, 11);
    assert!(backend.probe("Absent").is_err(), "an init that fails");
    assert_released_once_each(&observed, 12);
    assert!(backend.probe("Gone").is_err(), "a CoCreateInstance that fails instantiates nothing");
    assert_released_once_each(&observed, 12);
}

#[test]
fn a_loaded_handle_releases_its_driver_on_close_and_on_drop() {
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), spec())]);
    let backend = AsioBackend::with_loader(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("backend");
    backend.load("Driver").expect("load").close().expect("close");
    assert_released_once_each(&observed, 1);
    drop(backend.load("Driver").expect("load"));
    assert_released_once_each(&observed, 2);
    let held = backend.load("Driver").expect("load");
    drop(backend);
    assert_eq!(observed.live.load(Ordering::Acquire), 1, "the handle keeps the host and its driver");
    drop::<LoadedHandle>(held);
    assert_released_once_each(&observed, 3);
}

/// A handle dropped while the host thread is busy (its unload request is refused) used to leave the
/// driver loaded for the life of the process; the host now releases it between requests.
#[test]
fn a_handle_dropped_while_the_host_is_stuck_is_released_between_requests() {
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), spec())]);
    let backend = AsioBackend::with_loader(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("backend");
    let loaded = backend.load("Driver").expect("load");
    let host = &*backend.host;
    std::thread::scope(|scope| {
        let release = hold(scope, host);
        host.set_stuck(true);
        drop(loaded);
        assert_eq!(observed.live.load(Ordering::Acquire), 1, "the unload was not sent");
        host.set_stuck(false);
        drop(release);
    });
    wait_released(&observed);
    assert_released_once_each(&observed, 1);
    backend.load("Driver").expect("the slot is free again").close().expect("close");
    assert_released_once_each(&observed, 2);
}

/// A call timeout: the caller of a load gives up while the driver blocks in `init`; when `init`
/// returns the host releases the driver on its own thread.
#[test]
fn a_load_whose_caller_gave_up_is_released_when_init_returns() {
    let gate = Arc::new(Gate::default());
    let mut slow = spec();
    slow.init_gate = Some(Arc::clone(&gate));
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), slow)]);
    let mut host = Host::spawn(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("host");
    host.timeout = Duration::from_millis(200);
    let e = entry("Driver", 1);
    assert!(host.call("load", move |s| s.load(&e)).is_err(), "the caller gave up");
    let e = entry("Driver", 1);
    // The probe path too: its request is refused while the host is stuck, and sends nothing.
    assert!(host.call("probe", move |s| s.probe(&e)).is_err());
    gate.open();
    wait_released(&observed);
    host.timeout = Duration::from_secs(60);
    assert_released_once_each(&observed, 1);
    drop(host);
    assert_released_once_each(&observed, 1);
}

/// Dropping the host while its thread is blocked in a driver's `init` waits (bounded) for `init`
/// to return, so the driver is released on its own thread before the drop returns, not abandoned.
#[test]
fn dropping_the_host_while_init_blocks_waits_and_releases_on_the_host_thread() {
    let gate = Arc::new(Gate::default());
    let mut slow = spec();
    slow.init_gate = Some(Arc::clone(&gate));
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), slow)]);
    let mut host = Host::spawn(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("host");
    host.timeout = Duration::from_millis(200);
    let e = entry("Driver", 1);
    assert!(host.call("load", move |s| s.load(&e)).is_err(), "the caller gave up");
    let opener = Arc::clone(&gate);
    let late = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        opener.open();
    });
    drop(host);
    assert_released_once_each(&observed, 1);
    late.join().expect("opener");
}

/// The thread-start timeout: a host thread that starts after its caller gave up is joined before
/// the spawn returns (it can hold no driver: no request reached it).
#[test]
fn a_host_thread_that_starts_late_is_joined_before_the_spawn_returns() {
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), spec())]);
    let err = Host::spawn_after(
        Box::new(loader),
        Slot::default(),
        crate::HOST_TIMEOUT,
        Duration::from_millis(100),
        crate::watchdog::STALL_FLOOR,
        Duration::from_millis(400),
    )
    .err();
    assert!(matches!(&err, Some(ab::OpenError::Other(t)) if t.contains("did not start")), "{err:?}");
    assert_eq!(Arc::strong_count(&observed), 1, "the late thread exited (dropping its loader) before the spawn returned");
    assert_released_once_each(&observed, 0);
}

/// The streaming paths: a closed session stops, disposes and releases; an open that fails after
/// the load releases; an open whose caller gave up (the driver blocked in `init`) is torn down and
/// released when the open finishes.
#[test]
fn every_stream_session_ends_with_its_exit_steps_and_one_release() {
    let _guard = crate::stream_tests::lock();
    let gate = Arc::new(Gate::default());
    gate.open();
    let mut gated = spec();
    gated.init_gate = Some(Arc::clone(&gate));
    let mut refusing = spec();
    refusing.create_error = Some(abi::ASE_NO_MEMORY);
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), gated), (entry("Refusing", 2), refusing)]);
    let backend =
        AsioBackend::with_loader_watching(Box::new(loader), Slot::default(), Duration::from_secs(60), Duration::from_secs(3600)).expect("backend");
    let wish = |device: &str| ab::OutputWish { device: Some(device.into()), rates: vec![48_000], channels: 2, period: None };
    let plan = ab::Backend::plan_render(&backend, &wish("Driver")).expect("plan (one probe)");
    let request = plan.attempts[0];
    let render = || -> ab::RenderCallback { Box::new(|_| {}) };
    let error = || -> ab::ErrorCallback { Box::new(|_| {}) };
    let (stream, _) = ab::Backend::open_render(&backend, &plan.endpoint, &request, render(), error()).expect("open");
    stream.start().expect("start");
    drop(stream);
    assert_released_once_each(&observed, 2);
    let refused = ab::Backend::plan_render(&backend, &wish("Refusing")).expect("plan (one probe)");
    assert!(ab::Backend::open_render(&backend, &refused.endpoint, &refused.attempts[0], render(), error()).is_err(), "createBuffers fails");
    assert_released_once_each(&observed, 4);
    // The open's load blocks in init past the caller's timeout; the open completes afterwards and
    // its answer is dropped unread.
    gate.close();
    let host = Arc::clone(&backend.host);
    let e = plan.endpoint.handle::<crate::AsioEndpoint>().expect("handle").entry.clone();
    let abandoned = std::thread::spawn(move || {
        host.call_within("open render", Duration::from_millis(200), move |s| {
            s.open_stream(&e, &request, crate::session::Callback::Render(Box::new(|_| {})), Box::new(|_| {}))
        })
        .map(|_| ())
    });
    assert!(abandoned.join().expect("caller").is_err(), "the caller gave up");
    gate.open();
    wait_released(&observed);
    assert_released_once_each(&observed, 5);
    assert!(observed.call_names().iter().rev().take(2).any(|c| *c == "disposeBuffers"), "the abandoned session was torn down");
}

/// The listing's query of the driver that runs instantiates nothing: a driver that is not open is
/// named from the registry alone, the open one is queried in place. Control: a probe of the same
/// unloaded driver instantiates it once.
#[test]
fn the_listing_query_of_the_open_driver_instantiates_nothing() {
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), spec()), (entry("Other", 2), spec())]);
    let backend = AsioBackend::with_loader(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("backend");
    let created = || observed.created.load(Ordering::Acquire);
    assert_eq!(backend.driver_names().expect("registry"), ["Driver", "Other"]);
    assert!(backend.probe_open("Driver").expect("installed").is_none(), "not open: nothing to query");
    assert!(ab::Backend::endpoints(&backend, ab::Direction::Output).expect("listing").is_empty(), "nothing kept for the listing");
    assert!(backend.probe_open("Missing").is_err(), "not installed");
    assert_eq!(created(), 0, "the registry names alone");
    let loaded = backend.load("Driver").expect("load");
    let info = backend.probe_open("Driver").expect("query").expect("open: queried in place");
    assert_eq!((info.inputs, info.outputs), (2, 2));
    assert_eq!(ab::Backend::endpoints(&backend, ab::Direction::Output).expect("listing").len(), 1, "kept for the listing");
    assert!(backend.probe_open("Other").expect("installed").is_none(), "another driver while one is open: nothing");
    assert_eq!(created(), 1, "queried in place, never a second instance");
    loaded.close().expect("close");
    backend.probe("Other").expect("control: a probe loads");
    assert_eq!(created(), 2, "control: the probe instantiated the driver");
    assert_released_once_each(&observed, 2);
}

/// The end of the process: the shared host is never dropped, so it is shut down. The loaded driver
/// is released on the host thread, and a thread blocked in `init` is waited for (bounded) and
/// releases it before the shutdown returns. Past a bound too short it is left to finish by itself.
#[test]
fn shutting_down_releases_the_loaded_driver_and_waits_for_a_blocked_init() {
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), spec())]);
    let host = Host::spawn(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("host");
    let e = entry("Driver", 1);
    let _held = host.call("load", move |s| s.load(&e)).expect("load");
    assert_eq!(observed.live.load(Ordering::Acquire), 1, "without the shutdown the process would end with it initialized");
    assert!(host.shut_down(crate::RELEASE_WAIT), "the thread ended");
    assert_released_once_each(&observed, 1);
    assert!(host.call("list", |s| s.list()).is_err(), "later requests fail at once");
    drop(host);

    let gate = Arc::new(Gate::default());
    let mut slow = spec();
    slow.init_gate = Some(Arc::clone(&gate));
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), slow)]);
    let mut host = Host::spawn(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("host");
    host.timeout = Duration::from_millis(200);
    let e = entry("Driver", 1);
    assert!(host.call("load", move |s| s.load(&e)).is_err(), "the caller gave up");
    assert!(!host.shut_down(Duration::from_millis(100)), "a bound shorter than the blocked init");
    let opener = Arc::clone(&gate);
    let late = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        opener.open();
    });
    assert!(host.shut_down(crate::RELEASE_WAIT), "the init returned within the bound");
    assert_released_once_each(&observed, 1);
    late.join().expect("opener");
}

/// The process-wide slot's shutdown never starts a backend, and ends the one it holds.
#[test]
fn shutting_down_the_shared_slot_never_starts_a_backend() {
    let slot = Mutex::new(None);
    assert!(crate::shut_down_slot(&slot, crate::RELEASE_WAIT), "nothing was started: nothing to end");
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 1), spec())]);
    let backend = AsioBackend::with_loader(Box::new(loader), Slot::default(), Duration::from_secs(60)).expect("backend");
    let held = backend.load("Driver").expect("load");
    *slot.lock().unwrap() = Some(backend);
    assert!(crate::shut_down_slot(&slot, crate::RELEASE_WAIT));
    assert!(slot.lock().unwrap().is_none(), "taken out");
    assert_released_once_each(&observed, 1);
    drop(held);
    assert_released_once_each(&observed, 1);
}
