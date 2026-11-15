//! The teardown and rebuild ordering of a driver session (#524), through the fake driver of
//! [`crate::com::fake`] only: the test thread (or a thread of its own) plays the driver's callback
//! thread. A poisoning `disposeBuffers` keeps the buffers allocated and filled with
//! `POISON`, so a buffer switch that still writes through a disposed buffer's address shows as a
//! changed byte instead of a write into freed memory.

use crate::abi;
use crate::callback::testing;
use crate::com::fake::{FakeLoader, FakeSpec, Observed, entry};
use crate::host::Slot;
use crate::stream_tests::lock;
use crate::{AsioBackend, AsioStream};
use std::sync::Arc;
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
    assert_eq!(rig.observed.live.load(std::sync::atomic::Ordering::Acquire), 0);
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
