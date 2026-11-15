//! #587: on a separate input (the ALSA path) a recording start is placed, approximately, where it
//! used to be refused. Each case runs over a duplex start (it carries the input's own frame
//! ordinal) and a separate-input start (it carries none).
use super::*;
use crate::device_clock::{self, ClockFitter, ClockKey, ClockMap, Direction, Observation, Quality, Stamp};
use transit_audio_backend as ab;

const RATE: u64 = 48_000;
const BLOCK: u64 = 2048;

fn ns(frames: u64) -> u64 {
    (u128::from(frames) * 1_000_000_000 / u128::from(RATE)) as u64
}

/// Input block `k` holds frames [k*BLOCK, (k+1)*BLOCK) and is delivered at its end. `delay` is the
/// capture delay the stream reports, in frames: 0 stamps the first frame at the callback.
fn block(key: ClockKey, k: u64, delay: u64, late_ns: u64) -> Observation {
    let callback = ns((k + 1) * BLOCK) + late_ns;
    Observation {
        key,
        first_frame: k * BLOCK,
        frames: BLOCK as u32,
        nominal_rate: RATE as u32,
        stamp: Stamp {
            host_callback_ns: 1_000_000_000 + callback,
            backend_callback_ns: callback as i64,
            backend_frame_ns: (callback - ns(delay)) as i64,
            quality: Quality::BackendEstimate,
            offset_uncertainty_ns: None,
        },
    }
}

fn fitted(delay: u64, blocks: u64) -> (ClockKey, ClockFitter, ClockMap) {
    let (_, clock) = device_clock::stream_clock(Direction::Input, RATE as u32).unwrap();
    let key = clock.key();
    drop(clock);
    let mut fitter = ClockFitter::new(key);
    for k in 0..blocks {
        let _ = fitter.observe(block(key, k, delay, 0));
    }
    let map = fitter.map().unwrap();
    (key, fitter, map)
}

fn point(host_ns: u64) -> start::Point {
    start::Point { host_ns, output_frame: 9_000, position: crate::timeline_map::Position { sample: 0, fraction: 0, denominator: 1 } }
}

/// Mono frames whose value is their device frame, scaled into range.
fn frames(range: std::ops::Range<u64>) -> Vec<f32> {
    range.map(|f| f as f32 / 1e6).collect()
}

/// Case 1 of #587, through the recorder's real input callback on a scripted separate input: the
/// stream reports a capture delay of 0 at a 2048-frame period, and the start is applied 100 frames
/// after arming. The start then maps 1848 frames before the first frame captured after arm (less
/// than one block): the take starts at that first retained frame and is marked approximate.
#[test]
fn a_start_less_than_one_block_before_the_first_frame_after_arm_starts_the_take_there_approximately() {
    let armed_at = 21;
    let (key, _, map) = fitted(0, armed_at);
    let start_host = 1_000_000_000 + ns(armed_at * BLOCK + 200);
    let now = 1_000_000_000 + ns(armed_at * BLOCK) + 5_000_000;
    let mapped = map.host_to_frame(key, start_host, now).unwrap();
    assert_eq!(mapped, (armed_at - 1) * BLOCK + 200, "a zero delay maps one period early");
    for duplex in [false, true] {
        let fake =
            ab::fake::FakeBackend::new(ab::Format::new(RATE as u32, 1, ab::SampleFormat::F32)).named("ALSA", "pipewire").with_capture(1, false);
        let mut recorder = Recorder::open_through(&crate::AudioConfig::default(), &fake).unwrap();
        let format = ab::Format::new(RATE as u32, 1, ab::SampleFormat::F32);
        let pump = |k: u64| {
            let data = frames(k * BLOCK..(k + 1) * BLOCK);
            assert!(fake.pump_capture(BLOCK as usize, ab::Stamp::INVALID, format, &mut ab::fake::SliceSource { data: &data, channels: 1 }));
        };
        (0..armed_at).for_each(pump);
        recorder.shared.start_capture();
        (armed_at..armed_at + 3).for_each(pump);
        recorder.shared.stop_capture();

        let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(587));
        let reader = control.prepare().unwrap();
        let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
        assert!(publisher.start_placed(reader.ticket(), point(start_host), duplex.then_some(mapped), false));
        assert!(mapping.service(Some(map), Some(now)));
        assert_eq!(input_start.frame(), Ok(Some(mapped)));

        let path = transit_test_support::TestDir::file(&format!("start-before-history-{duplex}"), "wav");
        let _ = std::fs::remove_file(&path);
        let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.to_path_buf(), RATE as u32).unwrap()];
        let history = start::History::new(1, RATE as usize * 4).with_early_cap(RATE / 10);
        let gate = WriterStart { reader: reader.clone(), history, input_start, placed: false, next: None };
        let (takes, error) = write_takes_started(&recorder.shared, takes, 1, Some(gate));
        assert!(error.is_none(), "duplex {duplex}: {error:?}");
        assert_eq!(takes.len(), 1, "duplex {duplex}");
        assert_eq!(decode(&path)[0], frames(armed_at * BLOCK..(armed_at + 3) * BLOCK), "duplex {duplex}");
        assert_eq!(reader.fallback().map(|f| f.approximate), Some(true), "duplex {duplex}: the moved start must be marked approximate");
        std::fs::remove_file(path).unwrap();
        recorder.finish();
    }
}

/// A start further than one block before the retained history is still refused: the input did
/// not keep those frames, and moving the take further would misplace it by more than a block.
#[test]
fn a_start_a_block_or_more_before_the_retained_history_is_still_refused() {
    let mut history = start::History::new(1, 64).with_early_cap(1000);
    history.set_block(16);
    history.push(100, &[0.0; 16], |_| panic!("pending take wrote audio")).unwrap();
    history.set_start(84);
    assert_eq!(history.push(116, &[0.0; 16], |_| panic!("refused take wrote audio")), Err(start::OUTSIDE_HISTORY));
    assert!(!history.moved());
}

/// #587 review F1: the tolerance is capped (a recorder caps it at 100 ms) whatever block the
/// input once delivered; within the cap the start still moves.
#[test]
fn the_move_tolerance_is_capped() {
    for (start, moved) in [(90, false), (95, true)] {
        let mut history = start::History::new(1, 64).with_early_cap(8);
        history.set_block(16);
        history.push(100, &[0.0; 16], |_| panic!("pending take wrote audio")).unwrap();
        history.set_start(start);
        let mut written = 0;
        let result = history.push(116, &[0.0; 16], |data| written += data.len());
        assert_eq!(result.is_ok(), moved, "start {start}");
        assert_eq!(history.moved(), moved, "start {start}");
        assert_eq!(written, if moved { 32 } else { 0 }, "start {start}");
    }
}

/// #587 review F1: a writer late enough that the bounded history evicted frames is refused as
/// before, even within a block of the oldest retained frame: moving the start there would drop
/// audio the input captured.
#[test]
fn a_start_before_evicted_history_is_refused_not_moved() {
    let mut history = start::History::new(1, 16).with_early_cap(1000);
    history.set_block(16);
    history.push(100, &[0.0; 16], |_| panic!("pending take wrote audio")).unwrap();
    history.push(116, &[0.0; 16], |_| panic!("pending take wrote audio")).unwrap();
    history.set_start(110);
    assert_eq!(history.push(132, &[0.0; 16], |_| panic!("refused take wrote audio")), Err(start::OUTSIDE_HISTORY));
    assert!(!history.moved());
}

/// Case 2 of #587: one input observation arrives 200 ms late right after the start. The fit
/// rejects it and no current map remains. A separate input's placement waits its bounded time for
/// one, then places the start by the newest validated map it saw; a duplex start falls back to its
/// input ordinal at once, as before. Either way a take, approximate, never "could not be mapped".
#[test]
fn a_late_input_observation_after_the_start_still_gives_an_approximate_take() {
    let (key, mut fitter, map) = fitted(BLOCK, 21);
    let callback = 1_000_000_000 + ns(21 * BLOCK);
    let start_host = callback + 10_000_000;
    let expected = map.host_to_frame(key, start_host, callback + 1_000_000).unwrap();
    let late = block(key, 21, BLOCK, 200_000_000);
    let _ = fitter.observe(late);
    assert!(fitter.map().is_none(), "the late observation leaves no current map");
    let then = late.stamp.host_callback_ns + 1_000_000;
    for duplex in [false, true] {
        let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(587));
        let reader = control.prepare().unwrap();
        let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
        assert!(!mapping.service(Some(map), Some(callback + 1_000_000)), "nothing to place before the start");
        assert!(publisher.start_placed(reader.ticket(), point(start_host), duplex.then_some(expected - 300), false));
        let placed = if duplex {
            assert!(mapping.service(None, Some(then)), "a duplex start falls back to its ordinal at once");
            assert_eq!(input_start.frame(), Ok(Some(expected - 300)), "placed by the duplex ordinal");
            expected - 300
        } else {
            assert!(!mapping.service(None, Some(then)), "a separate input waits for a current map first");
            assert_eq!(input_start.frame(), Ok(None));
            assert!(mapping.service(None, Some(then + start::mapping::WAIT_NS)));
            assert_eq!(input_start.frame(), Ok(Some(expected)), "placed by the last validated map");
            expected
        };
        assert_eq!(reader.fallback().map(|f| f.approximate), Some(true), "duplex {duplex}");

        let shared = Shared::new(CaptureRing::new(1 << 16));
        shared.start_capture();
        {
            let _lease = shared.begin_capture().unwrap();
            shared.capture_at(placed - 1000, &frames(placed - 1000..placed + 2000), 1);
        }
        shared.stop_capture();
        let path = transit_test_support::TestDir::file(&format!("late-observation-{duplex}"), "wav");
        let _ = std::fs::remove_file(&path);
        let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.to_path_buf(), RATE as u32).unwrap()];
        let gate = WriterStart { reader, history: start::History::new(1, RATE as usize * 4), input_start, placed: false, next: None };
        let (takes, error) = write_takes_started(&shared, takes, 1, Some(gate));
        assert!(error.is_none(), "duplex {duplex}: {error:?}");
        assert_eq!(takes.len(), 1);
        assert_eq!(decode(&path)[0], frames(placed..placed + 2000));
        std::fs::remove_file(path).unwrap();
    }
}

/// #587 review F2: an input clock that invalidates (a discontinuity) after the placement saw a map
/// drops that map, so a separate input's fallback never projects across the lost frames.
#[test]
fn an_input_clock_invalidation_drops_the_remembered_map() {
    let (key, _, map) = fitted(BLOCK, 21);
    let callback = 1_000_000_000 + ns(21 * BLOCK);
    let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(587));
    let reader = control.prepare().unwrap();
    let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
    assert!(!mapping.service_counted(Some(map), Some(callback + 1_000_000), Some(0)));
    assert!(publisher.start_placed(reader.ticket(), point(callback + 10_000_000), None, false));
    let then = callback + 300_000_000;
    assert!(!mapping.service_counted(None, Some(then), Some(1)), "waits for a current map first");
    assert!(mapping.service_counted(None, Some(then + start::mapping::WAIT_NS), Some(1)));
    assert!(input_start.frame().is_err(), "no map since the discontinuity: the start must not be projected across it");
}

/// #587 with #184: a take whose start was moved to the first frame after arm reports a later ring
/// overflow from that frame, the take's actual first sample, not from the mapped start.
#[test]
fn a_moved_start_measures_its_losses_from_the_takes_first_frame() {
    let (arm_first, mapped) = (21 * BLOCK, 20 * BLOCK + 200);
    let path = transit_test_support::TestDir::file("moved-start-losses", "wav");
    let _ = std::fs::remove_file(&path);
    let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.to_path_buf(), RATE as u32).unwrap()];
    let shared = Shared::new(CaptureRing::new(8192));
    shared.start_capture();
    shared.block_frames.store(BLOCK, Ordering::Relaxed);
    {
        let _lease = shared.begin_capture().unwrap();
        assert_eq!(shared.capture_at(arm_first, &frames(arm_first..arm_first + 2 * BLOCK), 1), 0);
        // The ring has room for two more blocks: the second half of these four is refused.
        assert_eq!(shared.capture_at(arm_first + 2 * BLOCK, &frames(arm_first + 2 * BLOCK..arm_first + 6 * BLOCK), 1), 2 * BLOCK as usize);
    }
    shared.stop_capture();
    let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(587));
    let reader = control.prepare().unwrap();
    let (mut mapping, input_start) = start::mapping::pair(reader.clone(), ClockKey { generation: 9, domain: 9, direction: Direction::Input });
    // A start the output placed without a host time and the input by its ordinal: 1848 frames
    // before the first frame captured after arm, so the writer moves it there.
    assert!(publisher.start_placed(reader.ticket(), point(0), Some(mapped), false));
    assert!(mapping.service(None, None));
    let history = start::History::new(1, RATE as usize * 4).with_early_cap(RATE / 10);
    let gate = WriterStart { reader: reader.clone(), history, input_start, placed: false, next: None };
    let (takes, error) = write_takes_started(&shared, takes, 1, Some(gate));
    assert!(error.is_none(), "{error:?}");
    assert_eq!(takes.len(), 1);
    let audio = decode(&path).remove(0);
    assert_eq!(audio.len(), 6 * BLOCK as usize, "the take keeps its timing from the first frame after arm");
    assert_eq!(audio[..4 * BLOCK as usize], frames(arm_first..arm_first + 4 * BLOCK)[..]);
    // The loss begins at input frame 21*2048 + 4*2048, 0.17 s after the take's first frame
    // (from the mapped start it would read 0.21 s).
    assert_eq!(
        shared.losses.report(shared.dropped.load(Ordering::Relaxed), RATE as u32).as_deref(),
        Some(
            "85 ms of input lost from 0.17 s into the take: the take writer fell behind (a busy disk or a starved thread) and the take holds silence there"
        )
    );
    std::fs::remove_file(path).unwrap();
}
