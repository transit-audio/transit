#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use ab::{Backend, BackendFactory};

const PERIOD: Duration = Duration::from_nanos(10_666_666);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

#[test]
fn blocks_are_due_one_block_length_apart_with_no_drift() {
    let pacer = Pacer::new(ms(5), FRAMES, RATE);
    assert_eq!(pacer.deadline(), ms(5), "the first block is due at once");
    assert_eq!(block_ns(1, FRAMES, RATE), 10_666_666);
    assert_eq!(block_ns(3, FRAMES, RATE), 32_000_000, "computed from the block count, not summed");
    assert_eq!(block_ns(4_500, FRAMES, RATE), 48_000_000_000);
}

#[test]
fn a_wake_renders_one_block_only_once_it_is_due_and_stamps_it() {
    let mut pacer = Pacer::new(ms(100), FRAMES, RATE);
    assert_eq!(pacer.wake(ms(99)), None, "not due yet");
    let first = pacer.wake(ms(100)).unwrap();
    assert_eq!(first.position, 0);
    assert_eq!(first.stamp.quality, ab::Quality::BackendEstimate);
    assert_eq!((first.stamp.host_callback_ns, first.stamp.backend_callback_ns, first.stamp.backend_frame_ns), (100_000_000, 0, 0));
    assert_eq!(pacer.wake(ms(105)), None, "the next block is due a block later");
    let second = pacer.wake(ms(111)).unwrap();
    assert_eq!(second.position, u64::from(FRAMES));
    assert_eq!((second.stamp.backend_callback_ns, second.stamp.backend_frame_ns), (11_000_000, 10_666_666), "woken late, due on time");
}

#[test]
fn a_late_wake_catches_up_back_to_back_and_a_long_stall_reanchors() {
    let mut pacer = Pacer::new(Duration::ZERO, FRAMES, RATE);
    pacer.wake(Duration::ZERO).unwrap();
    // At 43 ms, blocks 1 to 4 (due at 10.67, 21.33, 32.00 and 42.67 ms) are all due at once, then
    // the schedule is back on time.
    let now = ms(43);
    let caught: Vec<Tick> = std::iter::from_fn(|| pacer.wake(now)).collect();
    assert_eq!(caught.len(), 4);
    assert!(caught.iter().all(|t| t.stamp.quality == ab::Quality::BackendEstimate));
    assert_eq!(caught.last().unwrap().position, 4 * u64::from(FRAMES));
    // Far later than four blocks: one block now, an invalid stamp, and the schedule anchored here.
    let stalled = ms(500);
    let tick = pacer.wake(stalled).unwrap();
    assert_eq!(tick.stamp.quality, ab::Quality::Invalid, "the clock fit restarts at the stall");
    assert_eq!(tick.position, 5 * u64::from(FRAMES), "frames stay continuous");
    assert_eq!(pacer.wake(stalled), None, "no backlog of missed blocks");
    assert_eq!(pacer.deadline(), stalled + PERIOD);
    let next = pacer.wake(stalled + PERIOD).unwrap();
    assert_eq!(next.stamp.quality, ab::Quality::BackendEstimate);
}

#[test]
fn the_backend_offers_one_fixed_output_and_no_input() {
    let backend = NullBackend::new();
    assert_eq!(backend.system(), SYSTEM);
    let outputs = backend.endpoints(ab::Direction::Output).unwrap();
    assert_eq!(outputs.len(), 1);
    assert_eq!((outputs[0].name.as_str(), outputs[0].inputs, outputs[0].outputs), (DEVICE, 0, 2));
    assert!(backend.endpoints(ab::Direction::Input).unwrap().is_empty());
    let wish = ab::OutputWish { device: Some("Speakers".into()), rates: vec![96_000], channels: 8, period: Some(64) };
    let plan = backend.plan_render(&wish).unwrap();
    assert_eq!(plan.attempts.len(), 1);
    let request = plan.attempts[0];
    assert_eq!((request.format.rate, request.format.channels, request.format.sample), (48_000, 2, ab::SampleFormat::F32), "whatever was wished");
    assert_eq!(request.period, ab::Period::Frames(512));
    assert_eq!(backend.plan_capture(&ab::InputWish::default()).unwrap_err(), NO_INPUT);
    assert!(!backend.capture_with_render(&plan.endpoint));
    let capture = backend.open_capture(&plan.endpoint, &request, Box::new(|_| {}), Box::new(|_| {}));
    assert_eq!(capture.err(), Some(ab::OpenError::Other(NO_INPUT.into())));
    assert!(BackendFactory::backend(&backend, Some("ASIO")).is_some_and(|b| b.system() == SYSTEM), "every system name gets the silent device");
}

/// What the render callback saw: frames, position and stamp quality of each block.
type Seen = Arc<Mutex<Vec<(usize, Option<u64>, u16, ab::Quality)>>>;

fn open(clock: &Arc<StepClock>) -> (Box<dyn ab::Stream>, ab::Achieved, Seen) {
    let backend = NullBackend::new().with_clock(Arc::clone(clock) as Arc<dyn PaceClock>);
    let plan = backend.plan_render(&ab::OutputWish::default()).unwrap();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    let render: ab::RenderCallback = Box::new(move |block| {
        let mut samples = 0;
        block.render(&mut |chunk| {
            chunk.fill(0.5);
            samples += chunk.len();
        });
        assert_eq!(samples, block.frames * usize::from(block.channels), "the whole block is lent to the callback");
        log.lock().unwrap().push((block.frames, block.device_position, block.channels, block.stamp.quality));
    });
    let (stream, achieved) = backend.open_render(&plan.endpoint, &plan.attempts[0], render, Box::new(|_| {})).unwrap();
    (stream, achieved, seen)
}

#[test]
fn the_stream_renders_fixed_blocks_paced_by_its_clock_and_restarts_after_a_stop() {
    let clock = StepClock::new();
    let (stream, achieved, seen) = open(&clock);
    assert_eq!(achieved.period, ab::Period::Frames(512));
    assert_eq!(achieved.buffer_frames, Some(512));
    assert_eq!(achieved.latency, Some(PERIOD));
    assert_eq!(stream.latency(), Some(PERIOD));
    assert!(seen.lock().unwrap().is_empty(), "nothing runs before start");
    let promotions = transit_rt::testing::promotion_calls();
    stream.start().unwrap();
    clock.settle();
    assert_eq!(seen.lock().unwrap().len(), 1, "the first block at once");
    assert!(transit_rt::testing::promotion_calls() > promotions, "the thread asked for realtime scheduling");
    assert_ne!(stream.thread_priority(), ab::WorkerPriority::Pending, "the promotion's outcome is observed");
    clock.advance(PERIOD / 2);
    assert_eq!(seen.lock().unwrap().len(), 1, "half a block later nothing more is due");
    clock.advance(PERIOD / 2 + Duration::from_nanos(1));
    // To 32.67 ms: the blocks due at 21.33 and 32.00 ms.
    clock.advance(ms(22));
    let blocks = seen.lock().unwrap().clone();
    assert_eq!(blocks.len(), 4);
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!(*block, (512, Some(512 * index as u64), 2, ab::Quality::BackendEstimate), "block {index}");
    }
    stream.stop().unwrap();
    clock.advance(PERIOD * 10);
    assert_eq!(seen.lock().unwrap().len(), 4, "a stopped stream renders nothing");
    stream.start().unwrap();
    clock.settle();
    assert_eq!(seen.lock().unwrap().len(), 5, "started again with its callback");
    drop(stream);
}

#[test]
fn a_panic_in_the_render_callback_is_reported_once_through_the_error_callback() {
    let clock = StepClock::new();
    let backend = NullBackend::new().with_clock(Arc::clone(&clock) as Arc<dyn PaceClock>);
    let plan = backend.plan_render(&ab::OutputWish::default()).unwrap();
    let blocks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&blocks);
    let render: ab::RenderCallback = Box::new(move |_block| {
        if counted.fetch_add(1, Ordering::Relaxed) + 1 == 2 {
            panic!("a fault in the render callback (expected by this test)");
        }
    });
    let errors: Arc<Mutex<Vec<ab::StreamError>>> = Arc::default();
    let reported = Arc::clone(&errors);
    let error: ab::ErrorCallback = Box::new(move |e| reported.lock().unwrap().push(e));
    let (stream, _) = backend.open_render(&plan.endpoint, &plan.attempts[0], render, error).unwrap();
    stream.start().unwrap();
    clock.settle();
    assert_eq!(blocks.load(Ordering::Relaxed), 1);
    assert!(errors.lock().unwrap().is_empty(), "nothing to report before the fault");
    // The second block panics: the thread reports it and ends (the clock's settle sees it end).
    clock.advance(PERIOD);
    assert_eq!(*errors.lock().unwrap(), vec![ab::StreamError::Other], "the panic is reported once, as a stream error");
    clock.advance(PERIOD * 4);
    assert_eq!(blocks.load(Ordering::Relaxed), 2, "the stream ended at the panic");
    assert_eq!(errors.lock().unwrap().len(), 1, "reported once only");
    // The callbacks were given back: the stream starts again with them.
    stream.stop().unwrap();
    stream.start().unwrap();
    clock.settle();
    assert_eq!(blocks.load(Ordering::Relaxed), 3, "started again with its callbacks");
    drop(stream);
}
