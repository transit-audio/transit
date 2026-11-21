//! #709 (ADR 0015, Amendment 2026-11-16 on the converter term): through a paired fake device, with
//! capture starting into an empty ring after four callbacks without capture, the hardware probe's
//! reading at a converted device rate exceeds the equal-rate one by exactly the term T at every
//! rate pair the ADR lists and every callback pattern tested (it was a tested range before, #724),
//! the figure has term provenance, and the ring reads silence only while capture starts, within
//! rule 9's bound, never after; at any rate a loop wrap inside a callback moves nothing. The probe's
//! click goes through the output converter like the render (`HwProbe::add_click`).

use super::*;

pub(super) struct Probe {
    pub(super) m: crate::hardware_probe::Measured,
    /// Live input underrun frames while the input converter starts (from the first callback that
    /// delivered input until the first callback that read none: its look-ahead is not queued yet)
    /// and in every callback after that.
    underrun_start: u64,
    underrun_later: u64,
    discarded: u64,
    /// How the plan's input was aligned when the result came.
    pub(super) alignment: transit_model::Alignment,
}

/// A paired duplex fake device at `device` Hz under a session at `session` Hz whose capture is the
/// render's channel 0 delayed by `delay` device frames, with callbacks of `sizes` frames in turn.
/// Four callbacks run without capture, so capture starts into an empty ring. The hardware probe
/// starts after `warm` callbacks with input. With `looped`, the transport plays
/// a loop of that many session frames from 0 throughout.
fn measure(session: u32, device: u32, delay: usize, sizes: &[usize], warm: usize, looped: Option<i64>) -> Probe {
    measure_on(true, session, device, delay, sizes, warm, looped)
}

/// [`measure`] on a paired device (`paired`: the input delivered before each render in one
/// callback, native ASIO or JACK) or one whose input runs on its own callback (WASAPI, cpal).
pub(super) fn measure_on(paired: bool, session: u32, device: u32, delay: usize, sizes: &[usize], warm: usize, looped: Option<i64>) -> Probe {
    use ab::fake::{SliceSink, SliceSource};
    let widest = sizes.iter().copied().max().unwrap();
    let format = ab::Format::new(device, 2, ab::SampleFormat::F32);
    let fake = ab::fake::FakeBackend::new(format).with_capture(2, paired);
    let cfg = AudioConfig::default();
    let doc = Session { sample_rate: transit_time::SampleRate::new(session).unwrap(), ..Session::default() };
    let player = Player::open_native_or_cpal(Arc::new(doc), &cfg, &fake, &fake_factory(false)).unwrap();
    let recorder = player.open_input(&cfg).expect("the duplex input");
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    if !paired {
        // An input that opened on its own: the app's path table names its inputs.
        player.set_io(transit_model::IoMap::for_device("test", "fake", 2, 2)).unwrap();
    }
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
            return Probe { m, underrun_start: start, underrun_later: later, discarded, alignment: player.input_alignment() };
        }
    }
    panic!("{session} on {device}: no result");
}

/// A loopback delay of whole 10 ms periods at `device` Hz, longer than any callback of `sizes`
/// (the fake loops back only frames already rendered), and the ideal round trip it is in plan
/// frames at `session` Hz (exact: a 10 ms period is a whole number of frames at every rate here).
pub(super) fn delay(session: u32, device: u32, sizes: &[usize]) -> (usize, i64) {
    let widest = sizes.iter().copied().max().unwrap();
    let periods = widest * 100 / device as usize + 2;
    (periods * device as usize / 100, (periods * session as usize / 100) as i64)
}

/// The probe's round trip past the ideal one, and the probe.
pub(super) fn extra(session: u32, device: u32, sizes: &[usize], warm: usize, looped: Option<i64>) -> (i64, Probe) {
    let (delay, ideal) = delay(session, device, sizes);
    let p = measure(session, device, delay, sizes, warm, looped);
    (p.m.round_trip_frames as i64 - ideal, p)
}

/// Callbacks with input before the probe starts: two whole periods of the 44.1/48 kHz rounding
/// pattern (147 callbacks), so the measurement sees no settling.
pub(super) const WARM: usize = 300;

/// Session and device rates, and the term T the ADR lists for the pair (rule 2): the probe reads
/// exactly that many plan frames over the equal-rate reading.
const PAIRS: [(u32, u32, i64); 8] = [
    (48_000, 44_100, 34),
    (44_100, 48_000, 32),
    (48_000, 96_000, 32),
    (96_000, 48_000, 62),
    (48_000, 88_200, 32),
    (44_100, 88_200, 32),
    (48_000, 176_400, 32),
    (48_000, 192_000, 32),
];

/// Rule 9's bound on the silence read while capture starts into an empty ring, for the widest
/// callback: the first aligned read's position `y0` is at least `-1 - (r + T)/q` input frames from
/// the first frame kept (`r`, the first block's render frames, is at most the callback's frames
/// times q plus the converter's reach, 16·max(1, q) + 1), so `ceil(q·(h + 1) + reach + T)` plan
/// frames, h = halo(1/q).
fn start_bound(session: u32, device: u32) -> u64 {
    let q = f64::from(session) / f64::from(device);
    let h = transit_resample::halo(1.0 / q) as f64;
    let reach = 16.0 * q.max(1.0) + 1.0;
    (q * (h + 1.0) + reach + f64::from(transit_model::converter_term(session, device))).ceil() as u64
}

const SIZES: [&[usize]; 11] = [&[32], &[64], &[128], &[192], &[256], &[512], &[600], &[1024], &[100, 37, 256], &[768, 64], &[600, 100]];

#[test]
fn hardware_probe_round_trip_gains_exactly_the_term_at_every_pair_and_callback_pattern() {
    for sizes in SIZES {
        for (session, device, term) in PAIRS {
            let (extra, p) = extra(session, device, sizes, WARM, None);
            let at = format!("{session} on {device}, callbacks {sizes:?}");
            assert_eq!(i64::from(transit_model::converter_term(session, device)), term, "{at}: rule 2's figure");
            assert_eq!(extra, term, "{at}: the round trip is {extra} plan frames over the equal-rate one");
            assert_eq!((p.m.found, p.m.spread_frames, p.discarded), (p.m.sent, 0, 0), "{at}");
            assert!(p.m.term, "{at}: the term was in force for the whole run");
            assert_eq!(p.alignment, transit_model::Alignment::Valid, "{at}");
            let bound = start_bound(session, device);
            assert!(p.underrun_start <= bound, "{at}: {} frames of silence while capture starts, bound {bound}", p.underrun_start);
            assert_eq!(p.underrun_later, 0, "{at}: silence after the start");
        }
        let (extra, p) = extra(48_000, 48_000, sizes, WARM, None);
        assert_eq!((extra, p.m.spread_frames, p.underrun_start + p.underrun_later, p.discarded), (0, 0, 0, 0), "equal rates, {sizes:?}");
        assert!(p.m.term, "equal rates: the term (0) is in force");
    }
}

#[test]
fn a_long_warm_up_at_a_slow_drift_settles_to_the_same_figure() {
    // 48 kHz on 44.1 kHz with a 192-frame buffer drifts by a fraction of a frame per callback.
    let (extra, p) = extra(48_000, 44_100, &[192], 4_000, None);
    assert_eq!((extra, p.m.spread_frames, p.underrun_later, p.discarded), (34, 0, 0, 0));
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
