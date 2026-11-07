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

/// ADR 0046: a native WASAPI open that fails is the open's error; the backend in the old cpal
/// fallback position is never asked to open.
#[test]
fn a_native_backend_that_fails_to_open_is_the_error_and_nothing_else_opens() {
    let (native, fallback) = (fake_factory(true), fake_factory(false));
    let error = Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig::default(), &native, &fallback).err();
    assert_eq!(error.as_deref(), Some("scripted"));
    assert!(fallback.opens().is_empty(), "no second open");
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
fn a_period_error_at_the_device_period_has_no_step_left_and_is_the_error() {
    let native = fake_factory(false);
    native.fail_next_open(ab::OpenError::PeriodRefused {
        requested_frames: 128,
        min_frames: Some(144),
        default_frames: Some(480),
        message: "Initialize(exclusive): the period is outside the range the device accepts".into(),
    });
    let fallback = fake_factory(false);
    let error =
        Player::open_native_or_cpal(Arc::new(Session::default()), &AudioConfig { share_device: false, ..Default::default() }, &native, &fallback)
            .err();
    assert!(error.as_deref().is_some_and(|e| e.contains("the period is outside the range the device accepts")), "{error:?}");
    assert_eq!((native.opens().len(), fallback.opens().len()), (1, 0), "the ladder's last step is the error (ADR 0046)");
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
fn any_other_exclusive_error_and_any_shared_error_are_the_error_with_no_other_open() {
    for (share_device, error) in [
        (false, ab::OpenError::DeviceNotAvailable("gone".into())),
        (false, ab::OpenError::Other("the device takes no 48000 Hz, 2-channel format in exclusive mode".into())),
        (true, ab::OpenError::InUse("busy".into())),
    ] {
        let native = fake_factory(false);
        native.fail_next_open(error.clone());
        let fallback = fake_factory(false);
        let cfg = AudioConfig { share_device, ..Default::default() };
        let failed = Player::open_native_or_cpal(Arc::new(Session::default()), &cfg, &native, &fallback).err();
        assert_eq!((native.opens().len(), fallback.opens().len()), (1, 0), "{error}");
        assert_eq!(failed, Some(error.to_string()));
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

#[test]
fn the_plan_table_follows_where_the_input_carries_system_audio() {
    // System Audio (#268) is the pair after the device's inputs; the Player's table says where, a
    // table the app hands back keeps it, and an input without the pair clears it.
    let (player, _output) = super::prepared();
    let wide = record::InputRing::new(4, 48_000);
    wide.set_system_channel(2);
    player.set_input(wide).unwrap();
    assert_eq!(player.io_map().system_audio, Some(2));
    player.set_io(transit_model::IoMap::for_device("WASAPI", "Speakers", 2, 2)).unwrap();
    assert_eq!(player.io_map().system_audio, Some(2), "a saved table never carries it; the open input does");
    player.set_input(record::InputRing::new(2, 48_000)).unwrap();
    assert_eq!(player.io_map().system_audio, None);
}

/// The #366 reopen note: a device opens with the app's saved monitoring set already in its
/// controls and its monitor stage at rest there, so with Dim on its first sounding block is
/// dimmed, not unity until the next UI frame stores the set (and then a ramp down). The same
/// session through the same fake, opened at unity, is the reference: every frame from the first
/// one that sounds is the reference times the Dim gain, with no ramp anywhere.
#[test]
fn a_device_opened_with_dim_on_plays_dimmed_from_its_first_sounding_block() {
    use crate::monitor_path::{DIM_DB, MonitorSettings};
    use ab::fake::SliceSink;
    use transit_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
    const BLOCK: usize = 256;
    let sounding = |monitor: &MonitorSettings| {
        let mut s = Session::default();
        s.edit.click = false;
        let len = 48_000;
        let src = SourceId(s.next_id + 10_000);
        let audio = transit_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; len], vec![0.25; len]] };
        s.pool.insert(src, Arc::new(SourceAudio::new(audio)));
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Tone"));
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "tone", src, 0, 0, len as i64));
        let fake = fake_factory(false);
        let player = Player::open_native_monitored(
            Arc::new(s),
            &AudioConfig::default(),
            Box::new(fake.clone()),
            &fake_factory(false),
            NativeKind::Wasapi,
            monitor,
        )
        .unwrap();
        player.play(0, None, None).unwrap();
        let mut out = Vec::new();
        let mut block = vec![0.0f32; BLOCK * 2];
        for _ in 0..60 {
            assert!(fake.pump_render(BLOCK, ab::Stamp::INVALID, &mut SliceSink { data: &mut block, channels: 2 }));
            out.extend_from_slice(&block);
            player.service().unwrap();
        }
        let first = out.iter().position(|x| x.abs() > 1e-6).expect("the clip reaches the output");
        out.split_off(first - first % 2)
    };
    let unity = sounding(&MonitorSettings::default());
    let dimmed = sounding(&MonitorSettings { dim: true, ..MonitorSettings::default() });
    let dim = transit_dsp::db_to_gain(DIM_DB);
    assert!(unity.len() >= 2 * BLOCK * 4 && dimmed.len() >= 2 * BLOCK * 4, "several sounding blocks");
    assert!(unity.iter().any(|x| (x - 0.5).abs() < 1e-3), "the reference plays at unity");
    for (i, (d, u)) in dimmed.iter().zip(&unity).take(2 * BLOCK * 4).enumerate() {
        assert!((d - u * dim).abs() < 1e-5, "frame {} channel {}: {d} against {} (unity {u})", i / 2, i % 2, u * dim);
    }
}
