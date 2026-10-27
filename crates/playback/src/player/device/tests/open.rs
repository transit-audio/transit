use super::*;

pub(super) fn fake_factory(failing: bool) -> ab::fake::FakeBackend {
    let fake = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32));
    if failing {
        fake.fail_next_open(ab::OpenError::DeviceNotAvailable("scripted".into()));
    }
    fake
}

#[test]
fn a_native_backend_that_opens_is_marked_and_says_nothing() {
    let (native, fallback) = (fake_factory(false), fake_factory(false));
    let player = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &fallback).unwrap();
    assert!(player.native_backend && player.backend_note.is_none() && !player.silent);
    assert_eq!((native.opens().len(), fallback.opens().len()), (1, 0), "cpal was not touched");
}

#[test]
fn a_native_backend_that_fails_to_open_falls_back_and_says_why() {
    let (native, fallback) = (fake_factory(true), fake_factory(false));
    let player = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &fallback).unwrap();
    assert!(!player.native_backend && !player.silent, "the fallback backend serves the device");
    assert_eq!(player.backend_note.as_deref(), Some("Native WASAPI failed: scripted; using the cpal backend"));
    assert_eq!(fallback.opens().len(), 1);
}

/// The fake's `set_host_blocked` is the hook for a stuck ASIO host thread (a driver settings panel
/// that is still open): the open fails and the same driver must not be loaded again through cpal.
#[test]
fn a_native_open_refused_by_a_stuck_host_does_not_fall_back_to_cpal() {
    let (native, fallback) = (fake_factory(true), fake_factory(false));
    native.set_host_blocked(true);
    let error = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &fallback).err();
    assert_eq!(error.as_deref(), Some("the ASIO driver's settings panel is open; close it and reconnect"));
    assert!(fallback.opens().is_empty(), "cpal was not asked to open the driver again");
}

#[test]
fn both_backends_failing_reports_the_fallbacks_error() {
    let (native, fallback) = (fake_factory(true), fake_factory(true));
    let error = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &fallback).err();
    assert_eq!(error.as_deref(), Some("scripted"));
}

pub(super) fn modes(fake: &ab::fake::FakeBackend) -> Vec<(ab::Direction, ab::Mode, ab::Period)> {
    fake.opens().iter().map(|(direction, request)| (*direction, request.mode, request.period)).collect()
}

/// A native fake whose device asks for a 128 frame period, the chosen buffer size.
fn fake_at_128() -> ab::fake::FakeBackend {
    let format = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let request =
        ab::OpenRequest { format, period: ab::Period::Frames(128), mode: ab::Mode::Shared, origin: ab::HostOrigin::default(), calibration: None };
    ab::fake::FakeBackend::new(format).with_attempts(vec![request])
}

#[test]
fn the_native_backend_is_asked_for_exclusive_unless_the_user_shares_the_device() {
    for (share_device, mode) in [(false, ab::Mode::Exclusive), (true, ab::Mode::Shared)] {
        let native = fake_factory(false);
        let cfg = AudioConfig { share_device, ..Default::default() };
        let player = Player::open_native_or_cpal(Arc::new(Session::default()), &cfg, &native, &fake_factory(false)).unwrap();
        assert!(player.native_backend && player.backend_note.is_none());
        assert_eq!(modes(&native), [(ab::Direction::Output, mode, ab::Period::Default)], "share_device {share_device}");
        assert_eq!(player.device_exclusive, mode == ab::Mode::Exclusive);
    }
    // The cpal backend has only shared streams: it is never asked for anything else.
    let fallback = fake_factory(false);
    let _ = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &fake_factory(true), &fallback).unwrap();
    assert_eq!(modes(&fallback), [(ab::Direction::Output, ab::Mode::Shared, ab::Period::Default)]);
}

#[test]
fn the_telemetry_carries_the_backend_snapshot_of_the_output_stream() {
    let native = fake_factory(false);
    let player = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &fake_factory(false)).unwrap();
    assert_eq!(player.callback_telemetry().backend, Some(ab::StreamStats::default()), "the fake backend keeps counters");
    let scripted = ab::StreamStats { wakes: 9, skipped_wakes: 2, max_wake_late_us: 700, ..Default::default() };
    native.set_stream_stats(ab::Direction::Output, scripted);
    assert_eq!(player.callback_telemetry().backend, Some(scripted));
    assert_eq!(player.backend_stats(), Some(scripted));
}

#[test]
fn a_refused_period_retries_exclusive_at_the_device_period_and_says_so() {
    let native = fake_at_128();
    native.fail_next_open(ab::OpenError::PeriodRefused {
        requested_frames: 128,
        min_frames: Some(144),
        default_frames: Some(480),
        message: "Initialize(exclusive): the period is outside the range the device accepts".into(),
    });
    let cfg = AudioConfig { buffer_size: Some(128), share_device: false, ..Default::default() };
    let fallback = fake_factory(false);
    let player = Player::open_native_or_cpal(Arc::new(Session::default()), &cfg, &native, &fallback).unwrap();
    assert_eq!(
        modes(&native),
        [(ab::Direction::Output, ab::Mode::Exclusive, ab::Period::Frames(128)), (ab::Direction::Output, ab::Mode::Exclusive, ab::Period::Default)]
    );
    assert!(player.native_backend && player.device_exclusive && fallback.opens().is_empty());
    assert_eq!(player.backend_note.as_deref(), Some("Exclusive mode could not use 128 frames; using the device period"));
}

#[test]
fn a_period_error_at_the_device_period_has_no_step_left_and_falls_back_to_cpal() {
    let native = fake_factory(false);
    native.fail_next_open(ab::OpenError::PeriodRefused {
        requested_frames: 128,
        min_frames: Some(144),
        default_frames: Some(480),
        message: "Initialize(exclusive): the period is outside the range the device accepts".into(),
    });
    let fallback = fake_factory(false);
    let player =
        Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig { share_device: false, ..Default::default() }, &native, &fallback)
            .unwrap();
    assert!(!player.native_backend);
    assert!(player.backend_note.as_deref().is_some_and(|n| n.starts_with("Native WASAPI failed: ") && n.ends_with("; using the cpal backend")));
}

#[test]
fn a_device_held_by_another_app_or_closed_to_exclusive_use_opens_shared_and_says_so() {
    let held = ab::OpenError::InUse("Initialize(exclusive): in use".into());
    let off = ab::OpenError::ExclusiveDisallowed("Initialize(exclusive): not allowed".into());
    for (error, note) in [
        (held, "Exclusive mode unavailable: another app holds the device; opened shared (higher latency)"),
        (off, "Exclusive mode is turned off for this device in Windows; opened shared"),
    ] {
        let native = fake_at_128();
        native.fail_next_open(error);
        let cfg = AudioConfig { buffer_size: Some(128), share_device: false, ..Default::default() };
        let fallback = fake_factory(false);
        let player = Player::open_native_or_cpal(Arc::new(Session::default()), &cfg, &native, &fallback).unwrap();
        assert_eq!(
            modes(&native),
            [
                (ab::Direction::Output, ab::Mode::Exclusive, ab::Period::Frames(128)),
                (ab::Direction::Output, ab::Mode::Shared, ab::Period::Frames(128))
            ],
            "shared keeps the chosen buffer size"
        );
        assert!(player.native_backend && !player.device_exclusive && fallback.opens().is_empty());
        assert_eq!(player.backend_note.as_deref(), Some(note));
    }
}

#[test]
fn any_other_exclusive_error_and_any_shared_error_fall_back_to_cpal() {
    for (share_device, error) in [
        (false, ab::OpenError::DeviceNotAvailable("gone".into())),
        (false, ab::OpenError::Other("the device takes no 48000 Hz, 2-channel format in exclusive mode".into())),
        (true, ab::OpenError::InUse("busy".into())),
    ] {
        let native = fake_factory(false);
        native.fail_next_open(error.clone());
        let fallback = fake_factory(false);
        let cfg = AudioConfig { share_device, ..Default::default() };
        let player = Player::open_native_or_cpal(Arc::new(Session::default()), &cfg, &native, &fallback).unwrap();
        assert_eq!((native.opens().len(), fallback.opens().len()), (1, 1), "{error}");
        assert!(!player.native_backend);
        assert_eq!(player.backend_note, Some(NativeKind::Wasapi.fallback_note(&error.to_string())));
    }
}

#[test]
fn a_duplex_input_that_cannot_open_exclusive_reopens_both_streams_in_the_fallback_mode() {
    let native = fake_factory(false).with_capture(2, true);
    native.fail_next_open(ab::OpenError::InUse("in use".into()));
    let player = Player::open_native_or_cpal(
        Arc::new(Session::default()),
        &AudioConfig { share_device: false, ..Default::default() },
        &native,
        &fake_factory(false),
    )
    .unwrap();
    assert_eq!(
        modes(&native),
        [
            (ab::Direction::Input, ab::Mode::Exclusive, ab::Period::Default),
            (ab::Direction::Input, ab::Mode::Shared, ab::Period::Default),
            (ab::Direction::Output, ab::Mode::Shared, ab::Period::Default),
        ],
        "the output never opened exclusive beside a shared input"
    );
    assert!(player.native_backend && !player.device_exclusive && !player.silent);
    assert_eq!(player.backend_note.as_deref(), Some("Exclusive mode unavailable: another app holds the device; opened shared (higher latency)"));
    assert!(native.running(ab::Direction::Input) && native.running(ab::Direction::Output));
}

#[test]
fn a_duplex_input_failure_the_ladder_does_not_answer_keeps_the_output_exclusive() {
    let cfg = AudioConfig { share_device: false, ..Default::default() };
    let held = fake_factory(false).with_capture(2, true);
    held.fail_next_open(ab::OpenError::Other("not an exclusive-mode error".into()));
    let player = Player::open_native_or_cpal(Arc::new(Session::default()), &cfg, &held, &fake_factory(false)).unwrap();
    assert!(player.native_backend && player.device_exclusive, "an input failure the ladder does not answer leaves the output exclusive");
    assert_eq!(held.opens().len(), 2);
}
