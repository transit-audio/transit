//! #724: at a converted device rate the live input reaches the plan a fixed few frames after it
//! does at equal rates (the two converters' look-ahead), whatever the device buffer, and the ring
//! never runs dry once the stream has started.

use super::*;

struct Probe {
    m: crate::hardware_probe::Measured,
    /// Live input underrun frames in the first callback that delivered input (the input
    /// converter's start: its look-ahead is not queued yet) and in every callback after it.
    underrun_first: u64,
    underrun_later: u64,
    discarded: u64,
}

/// A paired duplex fake device at `device` Hz under a session at `session` Hz whose capture is the
/// render's channel 0 delayed by `delay` device frames, with callbacks of `sizes` frames in turn.
/// The hardware probe starts after `warm` callbacks with input.
fn measure(session: u32, device: u32, delay: usize, sizes: &[usize], warm: usize) -> Probe {
    use ab::fake::{SliceSink, SliceSource};
    let widest = sizes.iter().copied().max().unwrap();
    let format = ab::Format::new(device, 2, ab::SampleFormat::F32);
    let fake = ab::fake::FakeBackend::new(format).with_capture(2, true);
    let cfg = AudioConfig::default();
    let doc = Session { sample_rate: transit_time::SampleRate::new(session).unwrap(), ..Session::default() };
    let player = Player::open_native_or_cpal(Arc::new(doc), &cfg, &fake, &fake_factory(false)).unwrap();
    let recorder = player.open_input(&cfg).expect("the duplex input");
    player.set_input(Arc::clone(&recorder.monitor)).unwrap();
    let (mut output, mut input) = (vec![0.0f32; widest * 2], vec![0.0f32; widest * 2]);
    let mut sent: Vec<f32> = Vec::new();
    for i in 0..4 {
        let n = sizes[i % sizes.len()];
        assert!(fake.pump_render(n, ab::Stamp::INVALID, &mut SliceSink { data: &mut output[..n * 2], channels: 2 }));
        player.service().unwrap();
    }
    let before = recorder.monitor.telemetry().underrun_frames;
    let mut first = None;
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
        let first = *first.get_or_insert(recorder.monitor.telemetry().underrun_frames - before);
        if let Some(r) = player.hardware_probe_result() {
            let m = r.unwrap_or_else(|e| panic!("{session} on {device}: {e}"));
            let t = recorder.monitor.telemetry();
            return Probe { m, underrun_first: first, underrun_later: t.underrun_frames - before - first, discarded: t.discarded_frames };
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
fn extra(session: u32, device: u32, sizes: &[usize], warm: usize) -> (i64, Probe) {
    let (delay, ideal) = delay(session, device, sizes);
    let p = measure(session, device, delay, sizes, warm);
    (p.m.round_trip_frames as i64 - ideal, p)
}

/// Session and device rates, and the extra plan frames the round trip takes at that pair over
/// the equal-rate one (the output converter's reach plus the input converter's look-ahead), at
/// most one frame apart: whole frames of the two converters' fractional reaches. One callback
/// is never in it.
const PAIRS: [(u32, u32, i64, i64); 3] = [(48_000, 44_100, 34, 35), (44_100, 48_000, 32, 33), (48_000, 96_000, 31, 32)];

/// Callbacks with input before the probe starts: two whole periods of the 44.1/48 kHz rounding
/// pattern (147 callbacks), so the measurement sees no settling.
const WARM: usize = 300;

fn check(sizes: &[usize], warm: usize) {
    for (session, device, low, high) in PAIRS {
        let (extra, p) = extra(session, device, sizes, warm);
        let at = format!("{session} on {device}, callbacks {sizes:?}, warm-up {warm}");
        assert!((low..=high).contains(&extra), "{at}: the round trip is {extra} plan frames over the equal-rate one");
        assert_eq!((p.m.found, p.m.spread_frames), (p.m.sent, 0), "{at}");
        // The first callback with input starts the input converter: its look-ahead is not queued
        // yet, so the plan reads that much silence, once. Never more than one frame after it
        // (the rounding of the reaches; none at all at 44.1 on 48 and 48 on 96 with a fixed
        // buffer), and never anything near a callback.
        assert!(p.underrun_first <= 17, "{at}: {} frames of silence at the start", p.underrun_first);
        assert!(p.underrun_later <= 1, "{at}: {} frames of silence after the start", p.underrun_later);
        assert_eq!(p.discarded, 0, "{at}");
    }
    let (extra, p) = extra(48_000, 48_000, sizes, warm);
    assert_eq!((extra, p.m.spread_frames, p.underrun_first + p.underrun_later, p.discarded), (0, 0, 0, 0), "equal rates, {sizes:?}");
}

#[test]
fn the_converted_rate_term_does_not_depend_on_the_device_buffer() {
    for block in [64usize, 128, 192, 256, 512] {
        check(&[block], WARM);
        // 44.1 on 48 and 48 on 96 with a fixed buffer: exactly the low figure, no later silence.
        for (session, device, low, _) in &PAIRS[1..] {
            let (extra, p) = extra(*session, *device, &[block], WARM);
            assert_eq!((extra, p.underrun_later), (*low, 0), "{session} on {device}, {block}");
        }
    }
}

#[test]
fn a_long_warm_up_at_a_slow_drift_settles_to_the_same_figure() {
    // 48 kHz on 44.1 kHz with a 192-frame buffer drifts by a fraction of a frame per callback.
    let (extra, p) = extra(48_000, 44_100, &[192], 4_000);
    assert_eq!((extra, p.m.spread_frames, p.underrun_later, p.discarded), (35, 0, 0, 0));
}

#[test]
fn variable_callbacks_and_callbacks_wider_than_the_largest_block_keep_the_term() {
    // The plan's largest block is 512 frames: 600 and 1,024 are rendered in several blocks.
    for sizes in [&[100usize, 37, 256][..], &[600], &[1024], &[768, 64], &[600, 100]] {
        check(sizes, WARM);
    }
}
