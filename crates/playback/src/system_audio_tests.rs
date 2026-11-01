use super::*;
use std::collections::VecDeque;
use transit_audio_backend::fake::{FakeBackend, SliceSource};
use transit_rt::testing::measure;

/// A ring, its observation queue and its counters as `try_open` makes them, for driving the reader
/// directly.
fn ring(rate: u32) -> (rtrb::Producer<f32>, rtrb::Producer<rate::Obs>, Source) {
    let (producer, queue) = rtrb::RingBuffer::new(ring_samples(rate));
    let (stamp_producer, stamps) = rtrb::RingBuffer::new(STAMPS);
    (producer, stamp_producer, Source { queue, stamps, status: Arc::new(Status::default()), rate })
}

/// Push `frames` frames of a `hz` sine at `rate`, continuing from frame `start`, both sides equal.
fn push_sine(producer: &mut rtrb::Producer<f32>, start: u64, frames: u64, hz: f64, rate: u32) {
    for n in start..start + frames {
        let x = (std::f64::consts::TAU * hz * n as f64 / f64::from(rate)).sin() as f32;
        producer.push(x).unwrap();
        producer.push(x).unwrap();
    }
}

/// Convert one second of a 1 kHz sine from `from` to `to` in 128-frame device blocks, the producer
/// keeping pace at the nominal ratio (deterministically: no threads, no clocks, no timestamps),
/// and return the left side of the output.
fn convert(from: u32, to: u32) -> (Vec<f32>, Arc<Status>) {
    let (mut producer, _, mut source) = ring(from);
    let status = Arc::clone(&source.status);
    let mut resampler = Resampler::new(from, to);
    let mut pushed = 0u64;
    let mut out = Vec::new();
    let mut block = vec![0.0; 256];
    // The endpoint is ahead by twice the margin: more than the target (margin, one packet and one
    // callback), so the reader primes on the first block.
    let lead = 2 * target_frames(from) as u64;
    for i in 0..(u64::from(to) / 128 + 20) {
        let due = (i + 1) * 128 * u64::from(from) / u64::from(to) + lead;
        push_sine(&mut producer, pushed, due - pushed, 1_000.0, from);
        pushed = due;
        resampler.begin(&mut source, 128, None);
        resampler.pull(&mut source.queue, &mut block, &status);
        out.extend(block.as_chunks::<2>().0.iter().map(|f| f[0]));
    }
    (out, status)
}

/// Rising zero crossings in `x`.
fn rising(x: &[f32]) -> usize {
    x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
}

#[test]
fn the_endpoint_rate_converts_to_the_device_rate_44k1_to_48k_and_96k() {
    for to in [48_000, 96_000] {
        let (out, status) = convert(44_100, to);
        // Skip the priming block; then exactly one second of output at the device rate.
        let second = &out[256..256 + to as usize];
        let cycles = rising(second);
        assert!((999..=1_001).contains(&cycles), "44.1 kHz to {to}: a 1 kHz sine has {cycles} cycles in a second");
        let peak = second.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((0.99..=1.0).contains(&peak), "44.1 kHz to {to}: peak {peak}");
        assert_eq!(status.underrun.load(Ordering::Relaxed), 0, "44.1 kHz to {to}: the reader never ran dry");
        let drift = f64::from(status.drift_deci_ppm.load(Ordering::Relaxed)) / 10.0;
        assert!(drift.abs() < 50.0, "44.1 kHz to {to}: nominal clocks show {drift} ppm");
    }
}

#[test]
fn equal_rates_pass_the_samples_through() {
    let (mut producer, _, mut source) = ring(48_000);
    let status = Arc::clone(&source.status);
    let mut resampler = Resampler::new(48_000, 48_000);
    // The target: the 30 ms margin, one 10 ms packet (none seen yet) and the 128-frame callback.
    let target = target_frames(48_000) + packet_frames(48_000) + 128;
    for n in 0..target {
        producer.push(n as f32).unwrap();
        producer.push(-(n as f32)).unwrap();
    }
    let mut block = vec![0.0; 2 * 128];
    resampler.begin(&mut source, 128, None);
    assert_eq!(resampler.target, target);
    resampler.pull(&mut source.queue, &mut block, &status);
    for (n, frame) in block.as_chunks::<2>().0.iter().enumerate() {
        // At the target fill the trim is zero: the frames come through as queued.
        assert!((frame[0] - n as f32).abs() < 1e-3 && (frame[1] + n as f32).abs() < 1e-3, "frame {n}: {frame:?}");
    }
    assert_eq!(status.discarded.load(Ordering::Relaxed), 0);
}

#[test]
fn a_silent_endpoint_reads_as_zeros_without_waiting_or_allocating() {
    let (mut state, mut tap) = reserve(2, 48_000);
    let (mut producer, _stamps, source) = ring(48_000);
    let status = Arc::clone(&source.status);
    let device: Vec<f32> = (0..2 * 480).map(|n| n as f32).collect();
    let mut seen = Vec::new();
    // Before the capture is attached, and after it with nothing queued: every widened frame
    // carries zeros on the pair, the device's own channels pass through untouched, and taking the
    // capture from its slot allocates nothing.
    let silent_pass = |tap: &mut Tap| {
        let ((), heap) = measure(|| {
            for _ in 0..50 {
                tap.begin(480, ab::Stamp::INVALID);
                tap.widen(&device, &mut |offset, wide| {
                    assert_eq!(wide.len() % 4, 0);
                    for (k, frame) in wide.as_chunks::<4>().0.iter().enumerate() {
                        assert_eq!(&frame[..2], &device[(offset + k) * 2..(offset + k) * 2 + 2]);
                        assert_eq!(&frame[2..], &[0.0, 0.0]);
                    }
                });
            }
        });
        assert!(heap.is_zero(), "the input callback's System Audio work allocated: {heap:?}");
    };
    silent_pass(&mut tap);
    let SystemAudio::Pending(slot) = &mut state else { panic!("the pair is reserved") };
    assert!(slot.push(source).is_ok());
    silent_pass(&mut tap);
    // Packets arrive, then stop again (the endpoint fell silent): audio, then zeros, never a stall.
    // The target is the margin, one packet and the 480-frame callback.
    let target = (target_frames(48_000) + packet_frames(48_000) + 480) as u64;
    push_sine(&mut producer, 0, target + 480, 1_000.0, 48_000);
    tap.begin(480, ab::Stamp::INVALID);
    tap.widen(&device, &mut |_, wide| seen.extend(wide.as_chunks::<4>().0.iter().map(|f| f[2])));
    assert!(seen.iter().any(|x| x.abs() > 0.5), "the queued audio is read once the ring reaches its target");
    seen.clear();
    for _ in 0..8 {
        tap.begin(480, ab::Stamp::INVALID);
        tap.widen(&device, &mut |_, wide| seen.extend(wide.as_chunks::<4>().0.iter().map(|f| f[2])));
    }
    assert!(seen[seen.len() - 480..].iter().all(|x| *x == 0.0), "a ring that ran dry reads as zeros");
    assert!(status.underrun.load(Ordering::Relaxed) > 0, "running dry after reading is counted");
}

/// What one drift simulation saw.
#[derive(Debug, Default)]
struct Run {
    /// Times the reader ran dry after priming, and the frames of zeros it gave after priming.
    dry: u64,
    zeros: u64,
    /// Frames discarded after priming (priming itself discards down to the target).
    discarded: u64,
    /// The fill at callback start over the late half, as a fraction of the target.
    late_fill: (f64, f64),
    /// The largest of `|trim| - (1.3 |true drift| + 100 ppm)` after the settling time, in ppm
    /// (negative: inside the bound), and the largest `|trim|`.
    trim_excess: f64,
    max_trim: f64,
    /// The reported drift over the late half, in ppm (lowest, highest).
    reported: (f64, f64),
    target: usize,
}

/// Run the reader for `seconds` of simulated device time against an endpoint whose clock runs
/// `ppm(t)` fast (negative: slow) against the device's: both at 48 kHz, the device's input in
/// `cb`-frame callbacks, the endpoint's capture in 480-frame packets, each delivered up to
/// `jitter_ms` (deterministic pseudo-random, in order) after its last frame was captured. The
/// packets carry the QPC time of their first frame and the callbacks the time of theirs, as the
/// WASAPI stamps do; the device is the time base. Deterministic sample counts and synthetic
/// timestamps, no clocks, no threads; the reader starts from an empty ring.
fn simulate(ppm: &dyn Fn(f64) -> f64, cb: usize, jitter_ms: f64, seconds: f64) -> Run {
    const RATE: f64 = 48_000.0;
    const PACKET: u64 = 480;
    const SETTLE_S: f64 = 30.0;
    let (mut producer, mut stamps, mut source) = ring(48_000);
    let status = Arc::clone(&source.status);
    let mut resampler = Resampler::new(48_000, 48_000);
    let mut out = vec![0.0f32; 2 * PIECE];
    // The endpoint's frame position at the last callback instant, and that instant.
    let (mut ep_at, mut t_at) = (0.0f64, 0.0f64);
    // The next packet to complete, the capture time of its first frame, the pending deliveries.
    let (mut next, mut first_time, mut last_delivery, mut seed) = (0u64, 0.0f64, 0.0f64, 0x2680_u64);
    let mut pending: VecDeque<(f64, rate::Obs)> = VecDeque::new();
    let mut run = Run { late_fill: (f64::MAX, 0.0), trim_excess: f64::MIN, reported: (f64::MAX, f64::MIN), ..Run::default() };
    let callbacks = (seconds * RATE / cb as f64) as u64;
    let mut primed_discard = None;
    for k in 0..callbacks {
        let t = (k * cb as u64) as f64 / RATE;
        // The endpoint's clock from the previous instant to this one, at the drift of the interval.
        let ep = ep_at + (t - t_at) * RATE * (1.0 + ppm(t_at) * 1e-6);
        // Packets completed by now: their completion time by interpolation, then their delivery.
        while ((next + 1) * PACKET) as f64 <= ep {
            let done = t_at + (((next + 1) * PACKET) as f64 - ep_at) / (ep - ep_at) * (t - t_at);
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let jitter = (seed >> 11) as f64 / (1u64 << 53) as f64 * jitter_ms * 1e-3;
            last_delivery = (done + jitter).max(last_delivery);
            let obs = rate::Obs { first_frame: next * PACKET, frames: PACKET as u32, time_ns: (first_time * 1e9) as i64, kind: rate::Kind::Position };
            pending.push_back((last_delivery, obs));
            (next, first_time) = (next + 1, done);
        }
        (ep_at, t_at) = (ep, t);
        while pending.front().is_some_and(|(due, _)| *due <= t) {
            let Some((_, obs)) = pending.pop_front() else { break };
            for _ in 0..PACKET {
                let _ = producer.push(0.25);
                let _ = producer.push(0.25);
            }
            let _ = stamps.push(obs);
        }
        let fill = source.queue.slots() / 2;
        let was_primed = resampler.primed;
        resampler.begin(&mut source, cb, Some((rate::Kind::Position, (t * 1e9) as i64)));
        let primed_now = resampler.primed;
        if primed_now && primed_discard.is_none() {
            primed_discard = Some(status.discarded.load(Ordering::Relaxed));
        }
        let mut left = cb;
        while left > 0 {
            let n = left.min(PIECE);
            resampler.pull(&mut source.queue, &mut out[..2 * n], &status);
            left -= n;
        }
        if (was_primed || primed_now) && !resampler.primed {
            run.dry += 1;
        }
        let true_ppm = ppm(t);
        let trim = resampler.trim * 1e6;
        run.max_trim = run.max_trim.max(trim.abs());
        if t >= SETTLE_S {
            run.trim_excess = run.trim_excess.max(trim.abs() - (1.3 * true_ppm.abs() + 100.0));
        }
        if t >= seconds / 2.0 {
            let f = fill as f64 / resampler.target as f64;
            run.late_fill = (run.late_fill.0.min(f), run.late_fill.1.max(f));
            let reported = f64::from(status.drift_deci_ppm.load(Ordering::Relaxed)) / 10.0;
            run.reported = (run.reported.0.min(reported), run.reported.1.max(reported));
        }
    }
    run.zeros = status.underrun.load(Ordering::Relaxed);
    run.discarded = status.discarded.load(Ordering::Relaxed) - primed_discard.unwrap_or(0);
    run.target = resampler.target;
    run
}

/// The drifts every callback size is run at, in ppm (constant), plus the step from 0 to -900 ppm
/// at two minutes (`None`).
const DRIFTS: [Option<f64>; 10] =
    [Some(0.0), Some(50.0), Some(-50.0), Some(200.0), Some(-200.0), Some(500.0), Some(-500.0), Some(1_000.0), Some(-1_000.0), None];

/// Run every drift at callback size `cb`, with and without 0 to 3 ms of packet jitter, for ten
/// simulated minutes (twelve for the step), and assert: no dry event and no frame of zeros after
/// priming, the fill at callback start inside 0.6 to 1.4 of the target over the late half, the
/// trim inside 1.3 times the true drift plus 100 ppm after 30 s, and the reported drift within 5 %
/// of the true figure or 20 ppm, whichever is larger, over the late half.
fn assert_drifts_at(cb: usize) {
    for jitter_ms in [0.0, 3.0] {
        for drift in DRIFTS {
            let (ppm, seconds, label): (Box<dyn Fn(f64) -> f64>, f64, String) = match drift {
                Some(p) => (Box::new(move |_| p), 600.0, format!("cb {cb}, {p:+} ppm, jitter {jitter_ms} ms")),
                None => {
                    (Box::new(|t| if t < 120.0 { 0.0 } else { -900.0 }), 720.0, format!("cb {cb}, 0 to -900 ppm at 120 s, jitter {jitter_ms} ms"))
                }
            };
            let end = ppm(seconds);
            let run = simulate(&*ppm, cb, jitter_ms, seconds);
            assert_eq!((run.dry, run.zeros), (0, 0), "{label}: ran dry after priming: {run:?}");
            assert_eq!(run.discarded, 0, "{label}: resynchronized: {run:?}");
            assert!(run.late_fill.0 > 0.6 && run.late_fill.1 < 1.4, "{label}: the fill wanders: {run:?}");
            assert!(run.trim_excess <= 0.0, "{label}: the trim exceeds 1.3 x the drift + 100 ppm: {run:?}");
            let tolerance = (end.abs() * 0.05).max(20.0);
            assert!(run.reported.0 >= end - tolerance && run.reported.1 <= end + tolerance, "{label}: reported drift off: {run:?}");
        }
    }
}

#[test]
fn drift_at_128_frame_callbacks() {
    assert_drifts_at(128);
}

#[test]
fn drift_at_256_frame_callbacks() {
    assert_drifts_at(256);
}

#[test]
fn drift_at_480_frame_callbacks_lined_up_with_the_packets() {
    assert_drifts_at(480);
}

#[test]
fn drift_at_512_frame_callbacks() {
    assert_drifts_at(512);
}

#[test]
fn drift_at_960_frame_callbacks_lined_up_with_the_packets() {
    assert_drifts_at(960);
}

#[test]
fn drift_at_1024_frame_callbacks() {
    assert_drifts_at(1_024);
}

#[test]
fn drift_at_1440_frame_callbacks() {
    assert_drifts_at(1_440);
}

#[test]
fn drift_at_2048_frame_callbacks() {
    assert_drifts_at(2_048);
}

#[test]
fn drift_at_4096_frame_callbacks() {
    assert_drifts_at(4_096);
}

#[test]
fn the_target_covers_one_callback_one_packet_and_the_margin() {
    let run = simulate(&|_| 0.0, 2_048, 0.0, 5.0);
    assert_eq!(run.target, 1_440 + 2_048 + 480);
    let run = simulate(&|_| 0.0, 128, 0.0, 5.0);
    assert_eq!(run.target, 1_440 + 128 + 480);
}

#[test]
fn the_feed_folds_the_mix_to_stereo_and_counts_a_full_ring_only_once_read() {
    assert_eq!(fold_to_stereo(&[]), (0.0, 0.0));
    assert_eq!(fold_to_stereo(&[0.5]), (0.5, 0.5));
    assert_eq!(fold_to_stereo(&[0.1, 0.2, 0.3, 0.4, 0.5, 0.6]), (0.1, 0.2), "the front pair of a 5.1 mix");
    let status = Status::default();
    let (mut producer, mut queue) = rtrb::RingBuffer::<f32>::new(4);
    push_frames(&mut producer, &status, &[0.1, 0.2, 0.3, f32::NAN, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1, 1.2], 6);
    assert_eq!((queue.pop(), queue.pop(), queue.pop(), queue.pop()), (Ok(0.1), Ok(0.2), Ok(0.7), Ok(0.8)));
    assert_eq!(status.overflow.load(Ordering::Relaxed), 0, "nothing reads yet: a full ring before the input starts is not a loss");
    status.reading.store(true, Ordering::Relaxed);
    push_frames(&mut producer, &status, &[0.1, 0.2, 0.3, 0.4, 0.5, 0.6], 2);
    assert_eq!(status.overflow.load(Ordering::Relaxed), 1);
    assert_eq!(f32::from_bits(status.peak[0].load(Ordering::Relaxed)), 0.7);
}

#[test]
fn the_latency_report_is_the_stream_plus_the_ring_target() {
    // Before the reader has seen a callback: the 30 ms margin and one 10 ms packet.
    assert_eq!(path_latency(Some(Duration::from_millis(10)), 48_000, 0), Duration::from_millis(50));
    assert_eq!(path_latency(None, 44_100, 0), Duration::from_nanos(1_764 * 1_000_000_000 / 44_100));
    // Then the reader's target: with 2048-frame callbacks, 1440 + 2048 + 480 frames at 48 kHz.
    assert_eq!(path_latency(Some(Duration::from_millis(10)), 48_000, 3_968), Duration::from_nanos(10_000_000 + 3_968 * 1_000_000_000 / 48_000));
    let off = SystemAudio::Off.report(48_000);
    assert_eq!((off.state, off.latency_ms), ("off", None));
}

fn loopback_fake() -> FakeBackend {
    FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32))
        .with_loopback(ab::Format::new(44_100, 2, ab::SampleFormat::F32))
        .with_latency(Duration::from_millis(10))
}

#[test]
fn open_starts_a_loopback_of_the_chosen_output_and_reports_it() {
    let fake = loopback_fake();
    let opened = open_on(&fake, Some("Speakers"));
    assert_eq!(fake.loopback_opens(), [Some("Speakers".to_owned())]);
    let (SystemAudio::Running(_), Some(source)) = (&opened.state, opened.source) else { panic!("System Audio runs") };
    assert_eq!(source.rate, 44_100, "the endpoint's mix rate");
    let frames: Vec<f32> = (0..2 * 2_000).map(|n| if n % 2 == 0 { 0.5 } else { -0.25 }).collect();
    assert!(fake.pump_loopback(2_000, &mut SliceSource { data: &frames, channels: 2 }), "the loopback stream was started");
    let report = opened.state.report(48_000);
    assert_eq!((report.state, report.text.as_str(), report.rate, report.device_rate), ("running", "Speakers", 44_100, 48_000));
    assert_eq!(report.peak, [0.5, 0.25]);
    let latency = report.latency_ms.unwrap();
    assert!((latency - 50.0).abs() < 0.1, "10 ms stream + 30 ms margin + one 10 ms packet: {latency}");
    assert_eq!(source.queue.slots(), 2 * 2_000);
}

#[test]
fn an_exclusive_owner_refuses_and_audio_devices_says_why() {
    let fake = loopback_fake();
    let why = "Windows refuses to capture Speakers while it is open in exclusive mode (by transit or another app). Open the device shared, or choose another output for System Audio.";
    fake.fail_next_loopback(ab::OpenError::InUse(why.into()));
    let opened = open_on(&fake, None);
    assert!(opened.source.is_none());
    let report = opened.state.report(48_000);
    assert_eq!(report.state, "refused");
    assert!(report.text.contains("open in exclusive mode"), "{}", report.text);
    assert!(report.text.contains("choose another output for System Audio"), "{}", report.text);
}

#[test]
fn off_unless_asked_for() {
    let opened = open(&AudioConfig::default());
    assert!(matches!(opened.state, SystemAudio::Off) && opened.source.is_none());
}

#[test]
fn linux_monitor_sources_are_named_as_system_audio() {
    let label = |name: &str| monitor_label(name, &[]);
    assert_eq!(label("Monitor of Built-in Audio Analog Stereo").as_deref(), Some("System Audio (monitor of Built-in Audio Analog Stereo)"));
    assert_eq!(
        label("alsa_output.pci-0000_00_1f.3.analog-stereo.monitor").as_deref(),
        Some("System Audio (monitor of pci-0000_00_1f.3.analog-stereo)")
    );
    assert_eq!(label("bluez_output.AA_BB.1.monitor").as_deref(), Some("System Audio (monitor of bluez_output.AA_BB.1)"));
    let ports = vec!["monitor_FL".to_owned(), "monitor_FR".to_owned()];
    assert_eq!(monitor_label("Built-in Audio Analog Stereo", &ports).as_deref(), Some("System Audio (monitor of Built-in Audio Analog Stereo)"));
    let mixed = vec!["capture_FL".to_owned(), "monitor_FR".to_owned()];
    assert_eq!(monitor_label("Scarlett 2i2 USB", &mixed), None, "a group with real capture ports is an interface");
    for plain in ["pulse", "default", "Scarlett 2i2 USB", "system", "Monitor of ", ".monitor"] {
        assert_eq!(label(plain), None, "{plain}");
    }
}

#[test]
fn the_recorder_appends_the_pair_after_the_device_inputs_without_allocating() {
    let device = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let fake = FakeBackend::new(device).with_capture(2, false).with_loopback(ab::Format::new(48_000, 2, ab::SampleFormat::F32));
    let plan = ab::Backend::plan_capture(&fake, &ab::InputWish::default()).unwrap();
    let mut recorder = crate::record::Recorder::build(&fake, &plan.endpoint, &plan.attempts[0], None, None, true).unwrap();
    recorder.start().unwrap();
    assert_eq!(recorder.system_audio_report().state, "pending", "the pair is reserved; the loopback opens after the output");
    recorder.attach_system_audio(open_on(&fake, None));
    assert_eq!((recorder.channels, recorder.device_channels(), recorder.system_channel()), (4, 2, Some(2)));
    assert_eq!(recorder.monitor.system_channel(), Some(2), "the plan's table learns where the pair is from the ring");
    // The target is the margin, one packet and the 480-frame callback (2400 frames): six packets.
    let browser: Vec<f32> = (0..480).flat_map(|_| [0.5, -0.5]).collect();
    for _ in 0..6 {
        assert!(fake.pump_loopback(480, &mut SliceSource { data: &browser, channels: 2 }));
    }
    let mic: Vec<f32> = (0..480).flat_map(|_| [0.1, 0.2]).collect();
    let (pumped, heap) = measure(|| fake.pump_capture(480, ab::Stamp::INVALID, device, &mut SliceSource { data: &mic, channels: 2 }));
    assert!(pumped);
    assert!(heap.is_zero(), "the input callback with System Audio allocated: {heap:?}");
    let mut out = vec![vec![0.0; 480]; 4];
    recorder.monitor.pop_into(&mut out, 480, 48_000);
    assert_eq!([out[0][100], out[1][100], out[2][100], out[3][100]], [0.1, 0.2, 0.5, -0.5]);
    let refused = SystemAudio::Refused("no".into());
    assert_eq!(refused.report(48_000).state, "refused");
}

#[test]
fn a_refused_loopback_leaves_the_pair_silent_and_the_input_running() {
    let device = ab::Format::new(48_000, 2, ab::SampleFormat::F32);
    let fake = FakeBackend::new(device).with_capture(2, false).with_loopback(device);
    fake.fail_next_loopback(ab::OpenError::InUse("held exclusively".into()));
    let plan = ab::Backend::plan_capture(&fake, &ab::InputWish::default()).unwrap();
    let mut recorder = crate::record::Recorder::build(&fake, &plan.endpoint, &plan.attempts[0], None, None, true).unwrap();
    recorder.start().unwrap();
    recorder.attach_system_audio(open_on(&fake, None));
    assert_eq!(recorder.system_audio_report().state, "refused", "Audio Devices says why; the session's input is not failed");
    let mic: Vec<f32> = (0..480).flat_map(|_| [0.1, 0.2]).collect();
    assert!(fake.pump_capture(480, ab::Stamp::INVALID, device, &mut SliceSource { data: &mic, channels: 2 }));
    let mut out = vec![vec![1.0; 480]; 4];
    recorder.monitor.pop_into(&mut out, 480, 48_000);
    assert_eq!([out[0][10], out[1][10], out[2][10], out[3][10]], [0.1, 0.2, 0.0, 0.0], "the device's inputs run; the pair is silence");
    let plain = crate::record::Recorder::build(&fake, &plan.endpoint, &plan.attempts[0], None, None, false).unwrap();
    assert_eq!((plain.channels, plain.system_channel(), plain.system_audio_report().state), (2, None, "off"));
}
