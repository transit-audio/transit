//! #724: at a converted device rate the live input reaches the plan a fixed few frames after it
//! does at equal rates (the two converters' look-ahead), whatever the device buffer, and the ring
//! never runs dry once the stream has started; at any rate a loop wrap inside a callback moves
//! nothing.

use super::*;

struct Probe {
    m: crate::hardware_probe::Measured,
    /// Live input underrun frames while the input converter starts (from the first callback that
    /// delivered input until the first callback that read none: its look-ahead is not queued yet)
    /// and in every callback after that.
    underrun_start: u64,
    underrun_later: u64,
    discarded: u64,
}

/// A paired duplex fake device at `device` Hz under a session at `session` Hz whose capture is the
/// render's channel 0 delayed by `delay` device frames, with callbacks of `sizes` frames in turn.
/// The hardware probe starts after `warm` callbacks with input. With `looped`, the transport plays
/// a loop of that many session frames from 0 throughout.
fn measure(session: u32, device: u32, delay: usize, sizes: &[usize], warm: usize, looped: Option<i64>) -> Probe {
    use ab::fake::{SliceSink, SliceSource};
    let widest = sizes.iter().copied().max().unwrap();
    let format = ab::Format::new(device, 2, ab::SampleFormat::F32);
    let fake = ab::fake::FakeBackend::new(format).with_capture(2, true);
    let cfg = AudioConfig::default();
    let doc = Session { sample_rate: transit_time::SampleRate::new(session).unwrap(), ..Session::default() };
    let player = Player::open_native_or_cpal(Arc::new(doc), &cfg, &fake, &fake_factory(false)).unwrap();
    let recorder = player.open_input(&cfg).expect("the duplex input");
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    if let Some(end) = looped {
        player.play(0, None, Some(transit_time::Range { start: 0, end })).unwrap();
    }
    let (mut output, mut input) = (vec![0.0f32; widest * 2], vec![0.0f32; widest * 2]);
    let mut sent: Vec<f32> = Vec::new();
    for i in 0..4 {
        let n = sizes[i % sizes.len()];
        assert!(fake.pump_render(n, ab::Stamp::INVALID, &mut SliceSink { data: &mut output[..n * 2], channels: 2 }));
        player.service().unwrap();
    }
    let mut last = recorder.monitor.telemetry().underrun_frames;
    let (mut start, mut later, mut starting) = (0, 0, true);
    // About five seconds of device frames: the probe's clicks and a margin.
    let cycle = sizes.iter().sum::<usize>();
    for i in 0..(warm + 5 * device as usize / cycle * sizes.len()) {
        if i == warm {
            player.start_hardware_probe(&[0], &[0]).unwrap_or_else(|e| panic!("{e}"));
        }
        let n = sizes[i % sizes.len()];
        let at = sent.len();
        for f in 0..n {
            input[f * 2] = (at + f).checked_sub(delay).and_then(|k| sent.get(k)).copied().unwrap_or(0.0);
            input[f * 2 + 1] = 0.0;
        }
        assert!(fake.pump_capture(n, ab::Stamp::INVALID, format, &mut SliceSource { data: &input[..n * 2], channels: 2 }));
        assert!(fake.pump_render(n, ab::Stamp::INVALID, &mut SliceSink { data: &mut output[..n * 2], channels: 2 }));
        sent.extend(output[..n * 2].as_chunks::<2>().0.iter().map(|f| f[0]));
        player.service().unwrap();
        let now = recorder.monitor.telemetry().underrun_frames;
        let silent = now - last;
        last = now;
        starting &= silent > 0 || i == 0;
        if starting {
            start += silent;
        } else {
            later += silent;
        }
        if let Some(r) = player.hardware_probe_result() {
            let m = r.unwrap_or_else(|e| panic!("{session} on {device}: {e}"));
            let discarded = recorder.monitor.telemetry().discarded_frames;
            return Probe { m, underrun_start: start, underrun_later: later, discarded };
        }
    }
    panic!("{session} on {device}: no result");
}

/// A loopback delay of whole 10 ms periods at `device` Hz, longer than any callback of `sizes`
/// (the fake loops back only frames already rendered), and the ideal round trip it is in plan
/// frames at `session` Hz (exact: a 10 ms period is a whole number of frames at every rate here).
fn delay(session: u32, device: u32, sizes: &[usize]) -> (usize, i64) {
    let widest = sizes.iter().copied().max().unwrap();
    let periods = widest * 100 / device as usize + 2;
    (periods * device as usize / 100, (periods * session as usize / 100) as i64)
}

/// The probe's round trip past the ideal one, and the probe.
fn extra(session: u32, device: u32, sizes: &[usize], warm: usize, looped: Option<i64>) -> (i64, Probe) {
    let (delay, ideal) = delay(session, device, sizes);
    let p = measure(session, device, delay, sizes, warm, looped);
    (p.m.round_trip_frames as i64 - ideal, p)
}

/// Callbacks with input before the probe starts: two whole periods of the 44.1/48 kHz rounding
/// pattern (147 callbacks), so the measurement sees no settling.
const WARM: usize = 300;

/// Session and device rates; the extra plan frames the round trip takes at that pair over the
/// equal-rate one (the output converter's reach plus the input converter's look-ahead), the low
/// figure or one more; and the most silence the input converter's start reads. The one frame is
/// the rounding of whole render frames against the converters' fractional reaches: the input's
/// place is set by how far the first read falls short, and a later callback can need up to one
/// input frame more, read as one frame of silence once (measured; a hold-back of one session frame
/// in the first read changes nothing, as that read's stop sets the place either way). One callback
/// is never in the figure.
const PAIRS: [(u32, u32, i64, u64); 8] = [
    (48_000, 44_100, 34, 18),
    (44_100, 48_000, 32, 16),
    (48_000, 96_000, 31, 16),
    (96_000, 48_000, 61, 31),
    (48_000, 88_200, 32, 16),
    (44_100, 88_200, 31, 16),
    (48_000, 176_400, 32, 16),
    (48_000, 192_000, 31, 16),
];

const SIZES: [&[usize]; 11] = [&[32], &[64], &[128], &[192], &[256], &[512], &[600], &[1024], &[100, 37, 256], &[768, 64], &[600, 100]];

#[test]
fn the_converted_rate_term_does_not_depend_on_the_device_buffer() {
    for sizes in SIZES {
        for (session, device, low, start) in PAIRS {
            let (extra, p) = extra(session, device, sizes, WARM, None);
            let at = format!("{session} on {device}, callbacks {sizes:?}");
            assert!((low..=low + 1).contains(&extra), "{at}: the round trip is {extra} plan frames over the equal-rate one");
            assert_eq!((p.m.found, p.m.spread_frames, p.discarded), (p.m.sent, 0, 0), "{at}");
            assert!(p.underrun_start <= start, "{at}: {} frames of silence while the converter starts", p.underrun_start);
            assert!(p.underrun_later <= 1, "{at}: {} frames of silence after the start", p.underrun_later);
        }
        let (extra, p) = extra(48_000, 48_000, sizes, WARM, None);
        assert_eq!((extra, p.m.spread_frames, p.underrun_start + p.underrun_later, p.discarded), (0, 0, 0, 0), "equal rates, {sizes:?}");
    }
}

#[test]
fn a_long_warm_up_at_a_slow_drift_settles_to_the_same_figure() {
    // 48 kHz on 44.1 kHz with a 192-frame buffer drifts by a fraction of a frame per callback.
    let (extra, p) = extra(48_000, 44_100, &[192], 4_000, None);
    assert_eq!((extra, p.m.spread_frames, p.underrun_later, p.discarded), (35, 0, 0, 0));
}

/// A monitored input on a paired ASIO-style fake at 48 kHz, with the transport playing a loop of
/// `looped` frames that wraps inside callbacks of `block` frames: an impulse captured every 50
/// callbacks, each one's frames from capture to output, and the ring's silence after the start.
fn monitored_through_loop_wraps(block: usize, looped: Option<i64>) -> (Vec<usize>, u64) {
    use ab::fake::{SliceSink, SliceSource};
    use transit_model::{ChannelFormat, TrackKind};
    let fake = model12();
    let cfg = asio_config();
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Mic"));
    s.track_mut(t).unwrap().mixer.input_monitor = true;
    let player = Player::open_native(Arc::new(s), &cfg, Box::new(fake.clone()), &fake_factory(false), NativeKind::Asio).unwrap();
    let recorder = player.open_input(&cfg).unwrap();
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    if let Some(end) = looped {
        player.play(0, None, Some(transit_time::Range { start: 0, end })).unwrap();
    }
    let format = ab::Format::new(48_000, 12, ab::SampleFormat::F32);
    let (mut output, mut input) = (vec![0.0f32; block * 10], vec![0.0f32; block * 12]);
    let (mut latencies, mut pending, mut started) = (Vec::new(), None, None);
    for i in 0..1_200 {
        input.fill(0.0);
        if i % 50 == 10 {
            input[..12].fill(0.5);
            pending = Some(i * block);
        }
        assert!(fake.pump_capture(block, ab::Stamp::INVALID, format, &mut SliceSource { data: &input, channels: 12 }));
        assert!(fake.pump_render(block, ab::Stamp::INVALID, &mut SliceSink { data: &mut output, channels: 10 }));
        if let Some(at) = pending
            && let Some(f) = output.chunks(10).position(|frame| frame.iter().any(|x| x.abs() > 0.01))
        {
            latencies.push(i * block + f - at);
            pending = None;
        }
        player.service().unwrap();
        started.get_or_insert(recorder.monitor.telemetry().underrun_frames);
    }
    (latencies, recorder.monitor.telemetry().underrun_frames - started.unwrap_or(0))
}

/// The live input stays where it was after a loop wrap inside a callback, at the session's rate
/// with ordinary buffers: each impulse reaches the output as soon as the first did, and the ring
/// never reads silence. A wrap used to leave the render blocks out of step with the callbacks for
/// the rest of playback, so each wrap moved the input later until it was about one callback
/// late, reading that much silence on the way.
#[test]
fn a_loop_wrap_inside_a_callback_leaves_the_live_input_where_it_was() {
    for block in [64usize, 100, 128, 256] {
        let (reference, silent) = monitored_through_loop_wraps(block, None);
        let (looped, silent_looped) = monitored_through_loop_wraps(block, Some(10_007));
        assert_eq!(silent, 0, "{block}-frame callbacks without a loop");
        assert_eq!(looped.len(), 24, "{block}: every impulse is heard");
        assert!(looped.iter().all(|l| *l == reference[0]), "{block}-frame callbacks: {looped:?} against {}", reference[0]);
        assert_eq!(silent_looped, 0, "{block}-frame callbacks through the loop wraps");
    }
}
