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
use std::sync::atomic::{AtomicBool, Ordering};
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
