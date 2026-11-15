//! The teardown and rebuild ordering of a driver session (#524), through the fake driver of
//! [`crate::com::fake`] only: the test thread (or a thread of its own) plays the driver's callback
//! thread. A poisoning `disposeBuffers` keeps the buffers allocated and filled with
//! `POISON`, so a buffer switch that still writes through a disposed buffer's address shows as a
//! changed byte instead of a write into freed memory.

use crate::abi;
use crate::callback::testing;
use crate::com::fake::{FakeLoader, FakeSpec, Observed, entry};
use crate::host::Slot;
use crate::session::Limits;
use crate::stream_tests::lock;
use crate::{AsioBackend, AsioStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use transit_audio_backend::{self as ab, Backend};

struct Rig {
    backend: AsioBackend,
    observed: Arc<Observed>,
    endpoint: ab::Endpoint,
}

/// Two inputs and two outputs of 32-bit samples; 64-frame buffers at 48 kHz.
fn spec() -> FakeSpec {
    let mut spec = FakeSpec::new("Fake ASIO", 2, 2, abi::ASIOST_INT32_LSB);
    spec.buffer = (16, 1024, 256, 16);
    spec
}

fn rig_with(loader: FakeLoader, observed: Arc<Observed>) -> Rig {
    // The watchdog waits an hour: the tests call back when they like.
    let backend =
        AsioBackend::with_loader_watching(Box::new(loader), Slot::default(), Duration::from_secs(10), Duration::from_secs(3600)).expect("backend");
    backend.probe("Driver").expect("probe");
    let endpoint = backend.endpoints(ab::Direction::Output).expect("endpoints").remove(0);
    Rig { backend, observed, endpoint }
}

fn rig(spec: FakeSpec) -> Rig {
    let (loader, observed) = FakeLoader::new(vec![(entry("Driver", 52), spec)]);
    rig_with(loader, observed)
}

/// A rig whose sessions wait only 20 ms for a callback (a callback the test holds never leaves in
/// time; the waits decide nothing else).
fn impatient_rig() -> Rig {
    let (mut loader, observed) = FakeLoader::new(vec![(entry("Driver", 52), spec())]);
    loader.limits = Limits { quiesce: Duration::from_millis(20), drain: Duration::from_millis(20) };
    rig_with(loader, observed)
}

fn plan(rig: &Rig, direction: ab::Direction) -> ab::OpenRequest {
    let device = Some("Driver".to_owned());
    let plan = match direction {
        ab::Direction::Output => rig.backend.plan_render(&ab::OutputWish { device, rates: vec![48_000], channels: 2, period: Some(64) }),
        ab::Direction::Input => rig.backend.plan_capture(&ab::InputWish { device, rate: Some(48_000), period: Some(64) }),
    };
    plan.expect("plan").attempts[0]
}

/// Opens the render stream with `render` as its callback.
fn open_render(rig: &Rig, render: ab::RenderCallback) -> Result<AsioStream, ab::OpenError> {
    let request = plan(rig, ab::Direction::Output);
    rig.backend.open_stream(&rig.endpoint, &request, crate::session::Callback::Render(render), Box::new(|_| {})).map(|(s, _)| s)
}

fn open_capture(rig: &Rig) -> Result<AsioStream, ab::OpenError> {
    let request = plan(rig, ab::Direction::Input);
    let capture: ab::CaptureCallback = Box::new(|block| block.chunks(&mut |_, _| {}));
    rig.backend.open_stream(&rig.endpoint, &request, crate::session::Callback::Capture(capture), Box::new(|_| {})).map(|(s, _)| s)
}

/// A render callback that writes 0.5 everywhere.
fn loud() -> ab::RenderCallback {
    Box::new(|block| block.render(&mut |chunk| chunk.fill(0.5)))
}

/// The `createBuffers` and `disposeBuffers` calls so far, in order.
fn buffer_calls(observed: &Observed) -> Vec<&'static str> {
    observed.call_names().into_iter().filter(|n| matches!(*n, "createBuffers" | "disposeBuffers")).collect()
}

/// Defect 1 of #524: a rebuild whose `createBuffers` fails left the old (disposed) buffer
/// addresses installed, and a buffer switch then wrote silence through them.
#[test]
fn a_failed_rebuild_leaves_no_disposed_buffer_address_for_a_late_switch() {
    let _guard = lock();
    let mut spec = spec();
    spec.poison_on_dispose = true;
    spec.create_fails_from = Some(2);
    let rig = rig(spec);
    let render = open_render(&rig, loud()).expect("render: the first createBuffers");
    let refused = open_capture(&rig).err().expect("capture: the rebuild's createBuffers fails");
    assert!(refused.to_string().contains("createBuffers"), "{refused}");
    assert_eq!(buffer_calls(&rig.observed), ["createBuffers", "disposeBuffers", "createBuffers", "createBuffers"], "dispose, fail, retry, fail");
    assert_eq!(rig.observed.poison(), (2, true), "the render stream's two output channels were disposed");
    // A driver that calls back now (nothing started, so it should not) must find no buffer.
    testing::switch_time_info(0, Some(0), None);
    testing::switch_time_info(1, Some(64), None);
    assert_eq!(rig.observed.poison(), (2, true), "a buffer switch wrote through a disposed buffer's address");
    drop(render);
    assert_eq!(rig.observed.live.load(Ordering::Acquire), 0);
}

/// Defect 1 of #524: the rebuild and the teardown disposed the buffers while their addresses were
/// still installed, so a buffer switch racing `disposeBuffers` wrote through them.
#[test]
fn a_switch_inside_dispose_buffers_finds_no_buffer_address_at_the_rebuild_or_the_teardown() {
    let _guard = lock();
    let mut spec = spec();
    spec.poison_on_dispose = true;
    spec.switch_in_dispose = true;
    let rig = rig(spec);
    let render = open_render(&rig, loud()).expect("render");
    let capture = open_capture(&rig).expect("capture: the rebuild for both directions");
    assert_eq!(rig.observed.poison(), (2, true), "the rebuild: a switch inside disposeBuffers wrote through the old render buffers");
    ab::Stream::start(&capture).expect("start capture");
    ab::Stream::start(&render).expect("start render");
    drop(capture);
    drop(render);
    assert_eq!(buffer_calls(&rig.observed), ["createBuffers", "disposeBuffers", "createBuffers", "disposeBuffers"]);
    assert_eq!(rig.observed.poison(), (6, true), "the teardown: a switch inside disposeBuffers wrote through the buffers");
    assert!(!testing::installed());
}

/// A render callback that blocks inside its first call, after its fill, until `release` is set
/// (a plug-in that does not return); `entered` says it is inside.
fn blocking(entered: &Arc<AtomicBool>, release: &Arc<AtomicBool>) -> ab::RenderCallback {
    let (entered, release) = (Arc::clone(entered), Arc::clone(release));
    Box::new(move |block| {
        block.render(&mut |chunk| chunk.fill(0.5));
        entered.store(true, Ordering::SeqCst);
        while !release.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
    })
}

/// Defect 2 of #524: after the waits for a callback still inside timed out, the close disposed the
/// buffers and released the driver anyway, and the one process-wide in-flight count kept every
/// later close waiting for that callback too. Now the buffers and the driver are leaked, the
/// driver is refused until the process restarts, and another session still closes cleanly.
#[test]
fn a_callback_still_inside_at_the_close_leaks_the_buffers_and_the_driver_and_later_sessions_close_cleanly() {
    let _guard = lock();
    let stuck = impatient_rig();
    let (entered, release) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let capture = open_capture(&stuck).expect("capture");
    let render = open_render(&stuck, blocking(&entered, &release)).expect("render");
    ab::Stream::start(&capture).expect("start capture");
    ab::Stream::start(&render).expect("start render");
    std::thread::scope(|scope| {
        let driver = scope.spawn(|| testing::switch_time_info(0, Some(0), None));
        while !entered.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        // Both streams go while the render callback is inside its buffer switch.
        drop(capture);
        drop(render);
        assert!(stuck.observed.call_names().contains(&"stop"), "the driver was stopped");
        assert!(!testing::installed(), "the callback state was uninstalled");
        assert_eq!(buffer_calls(&stuck.observed), ["createBuffers", "disposeBuffers", "createBuffers"], "no disposeBuffers under a callback");
        assert_eq!(
            (stuck.observed.live.load(Ordering::Acquire), stuck.observed.released.load(Ordering::Acquire)),
            (1, 1),
            "the probe's driver was released, the session's is left loaded"
        );
        let refused = open_render(&stuck, loud()).err().expect("the driver is refused");
        assert!(refused.to_string().contains("until transit restarts"), "{refused}");
        // With that callback still inside its (leaked) state, another driver's session closes
        // cleanly: the stuck callback is counted in its own state, not in a process-wide count.
        let other = impatient_rig();
        let stream = open_render(&other, loud()).expect("another session");
        ab::Stream::start(&stream).expect("start");
        testing::switch_time_info(0, Some(0), None);
        drop(stream);
        assert_eq!(buffer_calls(&other.observed), ["createBuffers", "disposeBuffers"], "the other session disposed its buffers");
        assert_eq!(other.observed.live.load(Ordering::Acquire), 0, "and released its driver");
        // The stuck callback resumes: it writes into buffers and calls a driver that still exist.
        release.store(true, Ordering::SeqCst);
        driver.join().expect("the driver thread");
    });
    assert_eq!(stuck.observed.live.load(Ordering::Acquire), 1, "the driver stays loaded for the life of the process");
}

/// A capture callback that blocks inside its call number `nth` (counted from 1) until `release` is
/// set; `entered` says it is inside.
fn capture_blocking_at(nth: usize, entered: &Arc<AtomicBool>, release: &Arc<AtomicBool>) -> ab::CaptureCallback {
    let (entered, release, calls) = (Arc::clone(entered), Arc::clone(release), AtomicUsize::new(0));
    Box::new(move |block| {
        block.chunks(&mut |_, _| {});
        if calls.fetch_add(1, Ordering::SeqCst) + 1 == nth {
            entered.store(true, Ordering::SeqCst);
            while !release.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
        }
    })
}

fn open_capture_with(rig: &Rig, capture: ab::CaptureCallback, error: ab::ErrorCallback) -> AsioStream {
    let request = plan(rig, ab::Direction::Input);
    rig.backend.open_stream(&rig.endpoint, &request, crate::session::Callback::Capture(capture), error).map(|(s, _)| s).expect("open capture")
}

fn silent(bytes: &[u8]) -> bool {
    bytes.iter().all(|b| *b == 0)
}

/// Defect 3 of #524: a buffer switch that found the engine busy did nothing, so its half played
/// the block it held from two switches ago at full level. It now writes silence into its half.
#[test]
fn a_buffer_switch_that_finds_the_engine_busy_writes_silence_into_its_half() {
    let _guard = lock();
    let rig = rig(spec());
    let (entered, release) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let capture = open_capture_with(&rig, capture_blocking_at(3, &entered, &release), Box::new(|_| {}));
    let render = open_render(&rig, loud()).expect("render");
    ab::Stream::start(&capture).expect("start capture");
    ab::Stream::start(&render).expect("start render");
    testing::switch_time_info(0, Some(0), None);
    testing::switch_time_info(1, Some(64), None);
    assert!(!silent(&rig.observed.read_output(0, 1)), "half 1 holds a rendered block");
    std::thread::scope(|scope| {
        // The third switch (half 0) is held inside its capture callback, holding the engine.
        let held = scope.spawn(|| testing::switch_time_info(0, Some(128), None));
        while !entered.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        testing::switch_time_info(1, Some(192), None);
        for channel in 0..2 {
            assert!(silent(&rig.observed.read_output(channel, 1)), "channel {channel}: the skipped half replays the block from two switches ago");
        }
        release.store(true, Ordering::SeqCst);
        held.join().expect("the held switch");
    });
    assert_eq!(ab::Stream::stats(&render).expect("stats").busy_skips, 1, "the skip is counted");
    assert!(!silent(&rig.observed.read_output(0, 0)), "the held switch rendered its own half");
    drop((capture, render));
}

/// Defect 3 of #524: an error was delivered under the flag a buffer switch needs, so an error
/// callback that took long made the switches skip. Errors now have a flag of their own.
#[test]
fn an_error_callback_that_takes_long_does_not_make_a_buffer_switch_skip() {
    let _guard = lock();
    let rig = rig(spec());
    let (entered, release) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let slow_error: ab::ErrorCallback = {
        let (entered, release) = (Arc::clone(&entered), Arc::clone(&release));
        Box::new(move |_| {
            entered.store(true, Ordering::SeqCst);
            while !release.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
        })
    };
    let capture = open_capture_with(&rig, Box::new(|block| block.chunks(&mut |_, _| {})), slow_error);
    let renders = Arc::new(AtomicUsize::new(0));
    let counted: ab::RenderCallback = {
        let renders = Arc::clone(&renders);
        Box::new(move |block| {
            block.render(&mut |chunk| chunk.fill(0.5));
            renders.fetch_add(1, Ordering::SeqCst);
        })
    };
    let render = open_render(&rig, counted).expect("render");
    ab::Stream::start(&capture).expect("start capture");
    ab::Stream::start(&render).expect("start render");
    std::thread::scope(|scope| {
        // The driver asks for a reset from a thread of its own; the capture's error callback takes
        // its time.
        let message = scope.spawn(|| testing::message(abi::K_ASIO_RESET_REQUEST, 0));
        while !entered.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        testing::switch_time_info(0, Some(0), None);
        testing::switch_time_info(1, Some(64), None);
        assert_eq!(renders.load(Ordering::SeqCst), 2, "the buffer switches ran while the error callback was inside");
        assert_eq!(ab::Stream::stats(&render).expect("stats").busy_skips, 0);
        release.store(true, Ordering::SeqCst);
        assert_eq!(message.join().expect("the message thread"), 1);
    });
    drop((capture, render));
}

/// Defect 4 of #524: the control side spun without bound for the engine's flag, so a callback that
/// never returned hung the host thread (at raised priority) in a start. The spin is bounded and
/// fails the start; a start after the callback returned works.
#[test]
fn a_start_while_a_callback_still_holds_the_engine_fails_instead_of_spinning() {
    let _guard = lock();
    let rig = impatient_rig();
    let (entered, release) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let capture = open_capture_with(&rig, capture_blocking_at(1, &entered, &release), Box::new(|_| {}));
    let render = open_render(&rig, loud()).expect("render");
    ab::Stream::start(&capture).expect("start capture: the driver starts");
    std::thread::scope(|scope| {
        let held = scope.spawn(|| testing::switch_time_info(0, Some(0), None));
        while !entered.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        // The capture stops (its wait for the held callback times out) and the driver stops; the
        // callback still holds the engine.
        ab::Stream::stop(&capture).expect("stop capture");
        assert_eq!(rig.observed.call_names().last(), Some(&"stop"), "the driver stopped");
        let refused = ab::Stream::start(&render).expect_err("the start fails while the engine is held");
        assert!(refused.to_string().contains("still inside the driver's callback"), "{refused}");
        release.store(true, Ordering::SeqCst);
        held.join().expect("the held switch");
    });
    ab::Stream::start(&render).expect("the start works once the callback returned");
    assert_eq!(rig.observed.call_names().iter().filter(|n| **n == "start").count(), 2, "the driver started again");
    drop((capture, render));
}

/// Defect 5 of #524: `stop()` asked the host thread first, so while that thread was blocked (in a
/// driver's control panel, say) the request failed and the stream kept playing after `stop()`
/// returned its error. The stop now takes the direction out at once and the host thread finishes
/// it when it is free.
#[test]
fn a_stop_while_the_host_thread_is_blocked_stops_the_direction_at_once_and_the_host_finishes_it() {
    let _guard = lock();
    let rig = rig(spec());
    let renders = Arc::new(AtomicUsize::new(0));
    let counted: ab::RenderCallback = {
        let renders = Arc::clone(&renders);
        Box::new(move |block| {
            block.render(&mut |chunk| chunk.fill(0.5));
            renders.fetch_add(1, Ordering::SeqCst);
        })
    };
    let render = open_render(&rig, counted).expect("render");
    ab::Stream::start(&render).expect("start");
    testing::switch_time_info(0, Some(0), None);
    assert_eq!(renders.load(Ordering::SeqCst), 1);
    let host = &*rig.backend.host;
    std::thread::scope(|scope| {
        let release = crate::stream_tests::hold_host(scope, host);
        host.set_stuck(true);
        let refused = ab::Stream::stop(&render).expect_err("the host thread is blocked");
        assert!(refused.to_string().contains("still blocked"), "{refused}");
        testing::switch_time_info(1, Some(64), None);
        assert_eq!(renders.load(Ordering::SeqCst), 1, "a render callback ran after stop() returned");
        assert!(silent(&rig.observed.read_output(0, 1)), "the stopped direction plays silence");
        host.set_stuck(false);
        drop(release);
    });
    // A stall bound (#87, E49), not a timing claim: the host thread finishes the stop between
    // requests once it is free, and the driver stops (this was its last running stream).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !rig.observed.call_names().contains(&"stop") && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(rig.observed.call_names().contains(&"stop"), "the host thread stopped the driver once it was free");
    drop(render);
}
