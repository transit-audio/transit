use super::*;
use transit_audio_backend::fake::{FakeBackend, SliceSource};
use transit_rt::testing::measure;

/// A ring and its counters as `try_open` makes them, for driving the reader directly.
fn ring(rate: u32) -> (rtrb::Producer<f32>, Source) {
    let (producer, queue) = rtrb::RingBuffer::new(ring_samples(rate));
    (producer, Source { queue, status: Arc::new(Status::default()), rate })
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
/// keeping pace at the nominal ratio (deterministically: no threads, no clocks), and return the
/// left side of the output.
fn convert(from: u32, to: u32) -> (Vec<f32>, Arc<Status>) {
    let (mut producer, mut source) = ring(from);
    let status = Arc::clone(&source.status);
    let mut resampler = Resampler::new(from, to);
    let mut pushed = 0u64;
    let mut out = Vec::new();
    let mut block = vec![0.0; 256];
    for i in 0..(u64::from(to) / 128 + 20) {
        // The endpoint's frames due by the end of this block, plus the target the reader primes on.
        let due = (i + 1) * 128 * u64::from(from) / u64::from(to) + target_frames(from) as u64;
        push_sine(&mut producer, pushed, due - pushed, 1_000.0, from);
        pushed = due;
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
    let (mut producer, mut source) = ring(48_000);
    let status = Arc::clone(&source.status);
    let mut resampler = Resampler::new(48_000, 48_000);
    for n in 0..target_frames(48_000) {
        producer.push(n as f32).unwrap();
        producer.push(-(n as f32)).unwrap();
    }
    let mut block = vec![0.0; 2 * 128];
    resampler.pull(&mut source.queue, &mut block, &status);
    for (n, frame) in block.as_chunks::<2>().0.iter().enumerate() {
        // At the target fill the trim is a fraction of a ppm: the frames come through as queued.
        assert!((frame[0] - n as f32).abs() < 1e-3 && (frame[1] + n as f32).abs() < 1e-3, "frame {n}: {frame:?}");
    }
    assert_eq!(status.discarded.load(Ordering::Relaxed), 0);
}

#[test]
fn a_silent_endpoint_reads_as_zeros_without_waiting_or_allocating() {
    let (mut state, mut tap) = reserve(2, 48_000);
    let (mut producer, source) = ring(48_000);
    let status = Arc::clone(&source.status);
    let device: Vec<f32> = (0..2 * 480).map(|n| n as f32).collect();
    let mut seen = Vec::new();
    // Before the capture is attached, and after it with nothing queued: every widened frame
    // carries zeros on the pair, the device's own channels pass through untouched, and taking the
    // capture from its slot allocates nothing.
    let silent_pass = |tap: &mut Tap| {
        let ((), heap) = measure(|| {
            for _ in 0..50 {
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
    push_sine(&mut producer, 0, target_frames(48_000) as u64 + 480, 1_000.0, 48_000);
    tap.widen(&device, &mut |_, wide| seen.extend(wide.as_chunks::<4>().0.iter().map(|f| f[2])));
    assert!(seen.iter().any(|x| x.abs() > 0.5), "the queued audio is read once the ring reaches its target");
    seen.clear();
    for _ in 0..4 {
        tap.widen(&device, &mut |_, wide| seen.extend(wide.as_chunks::<4>().0.iter().map(|f| f[2])));
    }
    assert!(seen[seen.len() - 480..].iter().all(|x| *x == 0.0), "a ring that ran dry reads as zeros");
    assert!(status.underrun.load(Ordering::Relaxed) > 0, "running dry after reading is counted");
}

#[test]
fn a_drifting_endpoint_is_trimmed_and_the_drift_reported() {
    // The endpoint's clock runs 200 ppm fast against the device's: it delivers 480-frame packets
    // that much more often. Simulated in device frames, deterministically.
    let (mut producer, mut source) = ring(48_000);
    let status = Arc::clone(&source.status);
    let mut resampler = Resampler::new(48_000, 48_000);
    let (mut produced, mut device_frames) = (0.0f64, 0u64);
    let mut block = vec![0.0; 2 * 128];
    let mut fills = Vec::new();
    for _ in 0..(60 * 48_000 / 128) {
        device_frames += 128;
        while produced + 480.0 <= device_frames as f64 * (1.0 + 200e-6) + target_frames(48_000) as f64 {
            for _ in 0..480 {
                let _ = producer.push(0.25);
                let _ = producer.push(0.25);
            }
            produced += 480.0;
        }
        resampler.pull(&mut source.queue, &mut block, &status);
        fills.push(source.queue.slots() / 2);
    }
    let drift = f64::from(status.drift_deci_ppm.load(Ordering::Relaxed)) / 10.0;
    assert!((150.0..=250.0).contains(&drift), "a 200 ppm fast endpoint reports {drift} ppm");
    let late = &fills[fills.len() / 2..];
    let most = late.iter().copied().max().unwrap_or(0);
    assert!(most < target_frames(48_000) * 2, "the ring stays near its target ({most} frames queued)");
    assert_eq!(status.discarded.load(Ordering::Relaxed), 0, "trimmed, never resynchronized");
    assert_eq!(status.underrun.load(Ordering::Relaxed), 0);
}

/// Run the reader against an endpoint whose clock runs `ppm(t)` fast (negative: slow) against the
/// device's, for `seconds` of simulated device time: 480-frame packets, 128-frame device blocks,
/// the producer ahead by the target at the start. Deterministic sample counts, no clocks. Returns
/// the counters, the fills after each block and the reported drift in ppm at the end.
fn simulate_drift(ppm: &dyn Fn(f64) -> f64, seconds: u64) -> (Arc<Status>, Vec<usize>, f64) {
    let (mut producer, mut source) = ring(48_000);
    let status = Arc::clone(&source.status);
    let mut resampler = Resampler::new(48_000, 48_000);
    let (mut produced, mut endpoint_clock, mut device_frames) = (0.0f64, 0.0f64, 0u64);
    let mut block = vec![0.0; 2 * 128];
    let mut fills = Vec::new();
    for _ in 0..(seconds * 48_000 / 128) {
        device_frames += 128;
        endpoint_clock += 128.0 * (1.0 + ppm(device_frames as f64 / 48_000.0) * 1e-6);
        while produced + 480.0 <= endpoint_clock + target_frames(48_000) as f64 {
            for _ in 0..480 {
                let _ = producer.push(0.25);
                let _ = producer.push(0.25);
            }
            produced += 480.0;
        }
        resampler.pull(&mut source.queue, &mut block, &status);
        fills.push(source.queue.slots() / 2);
    }
    let drift = f64::from(status.drift_deci_ppm.load(Ordering::Relaxed)) / 10.0;
    (status, fills, drift)
}

#[test]
fn the_controller_holds_the_ring_at_its_target_at_any_drift_inside_the_clamp() {
    let target = target_frames(48_000) as f64;
    for true_ppm in [1_000.0, -1_000.0, 800.0, -800.0, 500.0, -500.0, 200.0, -200.0] {
        let (status, fills, drift) = simulate_drift(&|_| true_ppm, 600);
        assert_eq!(status.underrun.load(Ordering::Relaxed), 0, "{true_ppm} ppm: frames of zeros");
        assert_eq!(status.discarded.load(Ordering::Relaxed), 0, "{true_ppm} ppm: resynchronized");
        let late = &fills[fills.len() / 2..];
        let (lo, hi) = (late.iter().copied().min().unwrap_or(0) as f64, late.iter().copied().max().unwrap_or(0) as f64);
        assert!(lo > target * 0.6 && hi < target * 1.4, "{true_ppm} ppm: the ring wanders {lo}..{hi} frames around {target}");
        assert!((drift - true_ppm).abs() <= true_ppm.abs() * 0.05, "{true_ppm} ppm reported as {drift}");
    }
}

#[test]
fn a_step_in_the_endpoint_clock_is_followed_without_running_dry() {
    // 0 ppm for two minutes, then the endpoint slows to -900 ppm for ten.
    let target = target_frames(48_000) as f64;
    let (status, fills, drift) = simulate_drift(&|t| if t < 120.0 { 0.0 } else { -900.0 }, 720);
    assert_eq!(status.underrun.load(Ordering::Relaxed), 0, "frames of zeros after the step");
    assert_eq!(status.discarded.load(Ordering::Relaxed), 0);
    let lowest = fills.iter().copied().min().unwrap_or(0) as f64;
    assert!(lowest > target * 0.4, "the fill dips to {lowest} frames around the step");
    let late = &fills[fills.len() * 9 / 10..];
    let mean = late.iter().sum::<usize>() as f64 / late.len() as f64;
    assert!((mean - target).abs() < target * 0.2, "settled at {mean} frames");
    assert!((drift + 900.0).abs() <= 45.0, "-900 ppm reported as {drift}");
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
    assert_eq!(path_latency(Some(Duration::from_millis(10)), 48_000), Duration::from_millis(40));
    assert_eq!(path_latency(None, 44_100), Duration::from_nanos(1_323 * 1_000_000_000 / 44_100));
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
    assert!((latency - 40.0).abs() < 0.1, "10 ms stream + 30 ms ring: {latency}");
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
    let browser: Vec<f32> = (0..1_920).flat_map(|_| [0.5, -0.5]).collect();
    assert!(fake.pump_loopback(1_920, &mut SliceSource { data: &browser, channels: 2 }));
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
