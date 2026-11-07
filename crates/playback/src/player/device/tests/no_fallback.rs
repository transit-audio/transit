//! ADR 0046, decision 7: on Windows a native open failure is a visible error with the device left
//! closed, and nothing else is asked to open: not the output, not the on-demand input, not a
//! recorder. The counting factory stands in the old cpal fallback's position; re-adding a fallback
//! call there makes it count and these tests fail.

use super::*;
use crate::backend::native::NativeKind;

/// A factory that only counts how often it is asked for a backend, and never gives one.
#[derive(Default)]
struct Counting(AtomicUsize);

impl Counting {
    fn calls(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

impl ab::BackendFactory for Counting {
    fn backend(&self, _system: Option<&str>) -> Option<Box<dyn ab::Backend>> {
        self.0.fetch_add(1, Ordering::Relaxed);
        None
    }
}

/// A native fake whose device asks for a 128 frame period, the chosen buffer size.
fn fake_at_128() -> ab::fake::FakeBackend {
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let request =
        ab::OpenRequest { format, period: ab::Period::Frames(128), mode: ab::Mode::Shared, origin: ab::HostOrigin::default(), calibration: None };
    ab::fake::FakeBackend::new(format).with_attempts(vec![request])
}

/// Native WASAPI, exclusive at 128 frames: the device is held (the ladder steps down to shared), then
/// the shared stream does not start (no step left). The Player runs silent with the backend's
/// reason, and the factory in the old fallback position is never asked.
#[test]
fn a_native_wasapi_failure_after_the_ladder_leaves_the_device_closed_and_opens_nothing_else() {
    let native = fake_at_128();
    native.fail_next_open(ab::OpenError::InUse("Initialize(exclusive): in use".into()));
    let reason = ab::OpenError::DeviceNotAvailable("the endpoint stopped answering".into());
    native.fail_next_start(reason.clone());
    let fallback = Counting::default();
    let cfg = AudioConfig { buffer_size: Some(128), share_device: false, ..Default::default() };
    let player = Player::with_native_kind(Arc::new(Session::default()), &cfg, &native, &fallback, NativeKind::Wasapi);
    assert_eq!(
        modes(&native),
        [(ab::Direction::Output, ab::Mode::Exclusive, ab::Period::Frames(128)), (ab::Direction::Output, ab::Mode::Shared, ab::Period::Frames(128))],
        "the whole ladder ran"
    );
    assert!(player.silent && !player.native_backend, "the device stays closed");
    assert_eq!(player.initial_error(), Some(reason.to_string().as_str()), "the backend's own reason");
    assert_eq!(fallback.calls(), 0, "no other factory was asked");
    assert_eq!(player.backend_note, None, "no fallback note");
}

/// Native ASIO: an open the driver refuses is the error; cpal's default host (WASAPI shared) is
/// never asked, as it was before ADR 0046.
#[test]
fn a_native_asio_failure_leaves_the_device_closed_and_opens_nothing_else() {
    let native = fake_factory(false);
    native.fail_next_open(ab::OpenError::DeviceNotAvailable("ASIO driver \"X\": init failed: No device connected".into()));
    let fallback = Counting::default();
    let player = Player::with_native_kind(Arc::new(Session::default()), &AudioConfig::default(), &native, &fallback, NativeKind::Asio);
    assert!(player.silent && !player.native_backend);
    assert_eq!(player.initial_error(), Some("ASIO driver \"X\": init failed: No device connected"));
    assert_eq!(fallback.calls(), 0);
}

/// Control: native JACK (off Windows, ADR 0021) still falls back, so the counting factory in the
/// same position is asked. This is the call the Windows tests above must never see.
#[test]
fn control_native_jack_still_asks_the_fallback() {
    let native = fake_factory(true);
    let fallback = Counting::default();
    let player = Player::with_native_kind(Arc::new(Session::default()), &AudioConfig::default(), &native, &fallback, NativeKind::Jack);
    assert_eq!(fallback.calls(), 1, "the fallback was asked (and had no audio system)");
    assert!(player.silent);
    assert_eq!(player.initial_error(), Some("no audio system"));
}

/// The on-demand input of a native WASAPI output (no duplex input held): its error reaches the caller
/// and the cpal adapter's position records no call. Control: the same Player with a native JACK kind
/// would ask it (`open_input_with` falls back only for JACK).
#[test]
fn the_on_demand_input_returns_the_native_error_and_asks_nothing_else() {
    let native = fake_factory(false).with_capture(2, false);
    let mut player = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &Counting::default()).unwrap();
    assert!(player.native_backend && !player.silent);
    // Without the test-Player guard (#363), so the native kind alone decides; the counting fake
    // gives no backend, so nothing real opens.
    player.fake_only = false;
    native.fail_next_open(ab::OpenError::DeviceNotAvailable("the capture endpoint is gone".into()));
    let cpal = Counting::default();
    let error = player.open_input_with(&AudioConfig::default(), &cpal, &|_| None).err();
    assert_eq!(error.as_deref(), Some("the capture endpoint is gone"));
    assert_eq!(cpal.calls(), 0, "no cpal input was asked");
    // Control: a native JACK output falls back for its input, so the same position is asked.
    let jack = fake_factory(false).with_capture(2, false);
    let mut player = Player::with_native_kind(Arc::new(Session::default()), &AudioConfig::default(), &jack, &Counting::default(), NativeKind::Jack);
    player.fake_only = false; // the counting fake gives no backend: nothing real opens
    jack.fail_next_open(ab::OpenError::DeviceNotAvailable("gone".into()));
    let cpal = Counting::default();
    assert!(player.open_input_with(&AudioConfig::default(), &cpal, &|_| None).is_err());
    assert_eq!(cpal.calls(), 1);
}

/// `Recorder::open` and `open_with` on Windows open through the native factory of the effective
/// backend and never ask cpal; without a native factory the open is an error, still without cpal.
/// Control: off Windows (`windows: false`) cpal is the one asked.
#[test]
fn the_recorder_on_windows_never_asks_cpal() {
    let native = fake_factory(false).with_capture(2, false);
    let cpal = Counting::default();
    let recorder = record::Recorder::open_on(&AudioConfig::default(), true, Some(Box::new(native.clone())), &cpal);
    assert!(recorder.is_ok(), "{:?}", recorder.err());
    assert_eq!(modes(&native).first().map(|m| m.0), Some(ab::Direction::Input), "the native factory opened the input");
    assert_eq!(cpal.calls(), 0);
    let cfg = AudioConfig { system: Some("ASIO".into()), ..Default::default() };
    let error = record::Recorder::open_on(&cfg, true, None, &cpal).err();
    assert_eq!(error.as_deref(), Some("no native backend serves the audio system “ASIO” in this build"));
    assert_eq!(cpal.calls(), 0, "no native factory is an error, not cpal");
    // Control: off Windows cpal opens it.
    assert!(record::Recorder::open_on(&AudioConfig::default(), false, Some(Box::new(native)), &cpal).is_err());
    assert_eq!(cpal.calls(), 1);
}
