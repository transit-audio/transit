use super::*;

#[test]
fn low_latency_adoption_during_capture_preserves_samples_and_placement() {
    use crate::player::RuntimeControl;
    use transit_mix::{DeviceGeneration, PlanLimits, RenderTarget};
    use transit_model::{ChannelFormat, Insert, Session, TrackKind};
    transit_mix::MixEngine::force_serial(true);
    let mut s = Session::default();
    let m = s.add_track(TrackKind::Master, ChannelFormat::Stereo, None);
    let mut ins = Insert::new("compressor");
    ins.params.insert("lookahead".into(), 4.0);
    s.track_mut(m).unwrap().mixer.inserts[0] = Some(ins);
    let s = Arc::new(s);
    let target = RenderTarget { device: DeviceGeneration(321), sample_rate: 48_000, max_block: 64, out_channels: 2, in_channels: 0 };
    let (mut runtime, mut audio) = RuntimeControl::<u8, u8>::new(Arc::clone(&s), None, target, PlanLimits::default()).unwrap();
    let shared = Shared::new(CaptureRing::new(256));
    shared.start_capture();
    let (mut latch, mut publisher) = start::pair(transit_mix::EngineEpoch(321));
    let reader = latch.prepare().unwrap();
    let point =
        start::Point { host_ns: 1234, output_frame: 100, position: crate::timeline_map::Position { sample: 700, fraction: 0, denominator: 1 } };
    assert!(publisher.start_placed(reader.ticket(), point, Some(900), false));
    let generation = shared.capture_state.load(Ordering::Acquire);
    let expected: Vec<f32> = (0..128).map(|n| n as f32 / 128.0).collect();
    for (part, block) in expected.chunks(32).enumerate() {
        if part == 1 {
            runtime.set_low_latency(Some(0.0));
            runtime.update_session(Arc::clone(&s)).unwrap();
        }
        if part == 3 {
            runtime.set_low_latency(None);
            runtime.update_session(Arc::clone(&s)).unwrap();
        }
        runtime.service();
        audio.plans.service(&mut crate::plan_bridge::callback_scope(), true);
        audio.plans.set_processing(false, true, false);
        {
            let _lease = shared.begin_capture().unwrap();
            shared.capture_at(900 + part as u64 * 32, block);
        }
        audio.plans.render(part as i64 * 32, 32);
        runtime.service();
        if part == 1 {
            assert_eq!(runtime.low_latency_bypasses().len(), 1);
        }
        assert_eq!(reader.status(), start::Status::Started(point));
        assert_eq!(shared.capture_state.load(Ordering::Acquire), generation);
    }
    let mut got = Vec::new();
    shared.ring.pop_all(&mut got);
    assert_eq!(got, expected);
    assert_eq!(shared.first_frame.load(Ordering::Acquire), 900);
    assert_eq!(shared.dropped.load(Ordering::Acquire), 0);
    runtime.retire(audio);
}

thread_local! {
    // Deterministic control-thread scheduling point. Never runs in the input callback;
    // absent from production builds and private to each test's control thread.
    static CAPTURE_WAIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static PLACEMENT_WAIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn on_capture_wait() {
    let hook = CAPTURE_WAIT_HOOK.with(|h| h.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

pub(super) fn on_placement_wait() {
    if let Some(hook) = PLACEMENT_WAIT_HOOK.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

thread_local! {
    static FINALIZE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn on_finalize_gate() {
    if let Some(hook) = FINALIZE_HOOK.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
}

#[test]
fn mapping_failure_between_write_and_stop_check_reports_failure() {
    let (key, map, now) = recording_map();
    let path = std::env::temp_dir().join(format!("clock-failure-{}.wav", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.clone(), 48_000).unwrap()];
    let shared = Shared::new(CaptureRing::new(32));
    shared.start_capture();
    {
        let _lease = shared.begin_capture().unwrap();
        shared.capture_at(100, &[0.5; 12]);
    }
    shared.stop_capture();
    let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(5));
    let reader = control.prepare().unwrap();
    let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
    let point = start::Point {
        host_ns: map.frame_to_host(key, 104, now).unwrap(),
        output_frame: 900,
        position: crate::timeline_map::Position { sample: 500, fraction: 0, denominator: 1 },
    };
    assert!(publisher.start(reader.ticket(), point));
    let gate = WriterStart { reader, history: start::History::new(1, 24), input_start, placed: false, next: None };
    FINALIZE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(mapping.service(None, Some(now)));
        }))
    });
    let (takes, error) = write_takes_started(&shared, takes, 1, Some(gate));
    assert!(takes.is_empty());
    assert!(!path.exists());
    assert!(error.is_some(), "input mapping failed during Stop, but the take was deleted and success returned");
}

#[test]
fn writer_delay_overflow_and_input_partitions_preserve_boundary() {
    let (key, map, now) = recording_map();
    for (case, parts) in [vec![12], vec![4, 4, 4], vec![1, 3, 5, 3]].into_iter().enumerate() {
        let path = std::env::temp_dir().join(format!("boundary-overflow-{}-{case}.wav", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.clone(), 48_000).unwrap()];
        let shared = Shared::new(CaptureRing::new(8));
        shared.start_capture();
        let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(5));
        let reader = control.prepare().unwrap();
        let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
        let gate = WriterStart { reader: reader.clone(), history: start::History::new(1, 24), input_start, placed: false, next: None };
        let data: Vec<f32> = (0..12).map(|n| n as f32 / 20.0).collect();
        let mut offset = 0;
        for frames in parts {
            let (_, heap) = transit_rt::testing::measure(|| {
                let _lease = shared.begin_capture().unwrap();
                shared.capture_at(100 + offset as u64, &data[offset..offset + frames]);
            });
            assert!(heap.is_zero(), "{heap:?}");
            offset += frames;
        }
        let point = start::Point {
            host_ns: map.frame_to_host(key, 104, now).unwrap(),
            output_frame: 900,
            position: crate::timeline_map::Position { sample: 500, fraction: 0, denominator: 1 },
        };
        assert!(publisher.start(reader.ticket(), point));
        assert!(mapping.service(Some(map), Some(now)));
        shared.stop_capture();
        let (takes, error) = write_takes_started(&shared, takes, 1, Some(gate));
        assert!(error.is_none(), "{error:?}");
        assert_eq!(takes.len(), 1);
        assert_eq!(decode(&path)[0], [0.2, 0.25, 0.3, 0.35, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(shared.dropped.load(Ordering::Relaxed), 4);
        std::fs::remove_file(path).unwrap();
    }
}

fn decode(path: &PathBuf) -> Vec<Vec<f32>> {
    transit_audio_io::decode(&std::fs::read(path).unwrap(), None).unwrap().1.channels
}

fn recording_map() -> (crate::device_clock::ClockKey, crate::device_clock::ClockMap, u64) {
    use crate::device_clock::{self, ClockFitter, Direction, Observation, Quality, Stamp};
    let (_, reader) = device_clock::stream_clock(Direction::Input, 48_000).unwrap();
    let key = reader.key();
    drop(reader);
    let mut fitter = ClockFitter::new(key);
    for i in 0..=30u64 {
        let ns = i * 4_000_000;
        let _ = fitter.observe(Observation {
            key,
            first_frame: i * 192,
            frames: 192,
            nominal_rate: 48_000,
            stamp: Stamp {
                host_callback_ns: 1_000_000_000 + ns,
                backend_callback_ns: ns as i64,
                backend_frame_ns: ns as i64,
                quality: Quality::BackendEstimate,
                offset_uncertainty_ns: None,
            },
        });
    }
    let map = fitter.map().unwrap();
    let now = 1_120_000_000;
    (key, map, now)
}

#[test]
fn applied_record_boundary_trims_real_wav_channels_across_input_order_and_partitions() {
    let (key, map, now) = recording_map();
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let data: Vec<f32> = (0..12).flat_map(|frame| [frame as f32 / 20.0, -(frame as f32) / 20.0]).collect();
    for (case, parts) in [vec![12], vec![4, 4, 4], vec![1, 2, 5, 1, 3]].iter().enumerate() {
        for published_after in 0..=parts.len() {
            let path = std::env::temp_dir().join(format!("transit-record-start-{}-{unique}-{case}-{published_after}.wav", std::process::id()));
            let mut takes = [TakeWriter::create(TrackId(1), vec![1, 0], path.clone(), 48_000).unwrap()];
            let shared = Shared::new(CaptureRing::new(64));
            shared.first_frame.store(100, Ordering::Release);
            let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(5));
            let reader = control.prepare().unwrap();
            let ticket = reader.ticket();
            let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
            let mut gate = WriterStart { reader, history: start::History::new(2, 24), input_start, placed: false, next: None };
            let point = start::Point {
                host_ns: map.frame_to_host(key, 104, now).unwrap(),
                output_frame: 900,
                position: crate::timeline_map::Position { sample: 500, fraction: 0, denominator: 1 },
            };
            let mut offset = 0;
            for (index, &frames) in parts.iter().enumerate() {
                if index == published_after {
                    assert!(publisher.start(ticket, point));
                    // Supply the deterministic device-clock snapshot at the
                    // exact placement seam; all WAV/history work stays real.
                    assert!(mapping.service(Some(map), Some(now)));
                }
                gate.write(&shared, &mut takes, &data[offset * 2..(offset + frames) * 2], 2).unwrap();
                if index < published_after {
                    assert_eq!(takes[0].frames, 0);
                }
                offset += frames;
            }
            if published_after == parts.len() {
                assert!(publisher.start(ticket, point));
                assert!(mapping.service(Some(map), Some(now)));
            }
            control.cancel(); // Stop retains the applied boundary for final drain.
            gate.write(&shared, &mut takes, &[], 2).unwrap();
            takes[0].write_header();
            assert_eq!(takes[0].frames, 8);
            assert!(takes[0].error.is_none());
            drop(takes);
            let channels = decode(&path);
            assert_eq!(channels[0], (4..12).map(|frame| -(frame as f32) / 20.0).collect::<Vec<_>>());
            assert_eq!(channels[1], (4..12).map(|frame| frame as f32 / 20.0).collect::<Vec<_>>());
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[test]
fn stopped_writer_waits_for_clock_ack_without_another_input_callback() {
    use std::sync::mpsc;
    let (key, map, now) = recording_map();
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    for success in [false, true] {
        let path = std::env::temp_dir().join(format!("transit-final-start-{}-{unique}-{success}.wav", std::process::id()));
        let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.clone(), 48_000).unwrap()];
        let shared = Arc::new(Shared::new(CaptureRing::new(32)));
        shared.start_capture();
        {
            let _lease = shared.begin_capture().unwrap();
            shared.capture_at(100, &(0..12).map(|i| i as f32 / 20.0).collect::<Vec<_>>());
        }
        shared.stop_capture();
        assert!(shared.quiescent());
        let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(5));
        let reader = control.prepare().unwrap();
        let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
        let gate = WriterStart { reader: reader.clone(), history: start::History::new(1, 24), input_start, placed: false, next: None };
        let point = start::Point {
            host_ns: map.frame_to_host(key, 104, now).unwrap(),
            output_frame: 900,
            position: crate::timeline_map::Position { sample: 500, fraction: 0, denominator: 1 },
        };
        assert!(publisher.start(reader.ticket(), point));
        control.cancel();
        let (entered, waiting) = mpsc::sync_channel(0);
        let (release, resume) = mpsc::sync_channel(0);
        let writer = std::thread::spawn(move || {
            PLACEMENT_WAIT_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    entered.send(()).unwrap();
                    resume.recv().unwrap();
                }))
            });
            write_takes_started(&shared, takes, 1, Some(gate))
        });
        let reached = waiting.recv_timeout(Duration::from_secs(30));
        if success {
            assert!(mapping.service(Some(map), Some(now)));
        }
        drop(mapping); // Failure/shutdown must also end finalization.
        let _ = release.send(());
        let (takes, error) = writer.join().unwrap();
        assert!(reached.is_ok(), "writer returned before acknowledging its applied start");
        if success {
            assert!(error.is_none(), "{error:?}");
            assert_eq!(takes.len(), 1);
            assert_eq!(decode(&path)[0], (4..12).map(|i| i as f32 / 20.0).collect::<Vec<_>>());
            std::fs::remove_file(path).unwrap();
        } else {
            assert!(takes.is_empty());
            assert!(error.is_some());
            assert!(!path.exists());
        }
    }
}

#[test]
fn a_full_ring_owes_the_lost_samples_as_silence() {
    let shared = Shared::new(CaptureRing::new(8));
    let mut out = Vec::new();
    shared.capture(&[1.0; 6]);
    shared.capture(&[2.0; 4]);
    shared.ring.pop_all(&mut out);
    shared.capture(&[3.0; 2]);
    shared.ring.pop_all(&mut out);
    assert_eq!(out, [vec![1.0; 6], vec![2.0; 2], vec![0.0; 2], vec![3.0; 2]].concat());
    assert_eq!((shared.pending_silence.load(Ordering::Relaxed), shared.dropped.load(Ordering::Relaxed)), (0, 2));
}

#[test]
fn nan_and_infinite_input_records_as_silence() {
    let shared = Shared::new(CaptureRing::new(8));
    let mut out = Vec::new();
    shared.capture(&[0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5]);
    shared.ring.pop_all(&mut out);
    assert_eq!(out, [0.5, 0.0, 0.0, 0.0, -0.5]);
}

fn recorder(shared: &Arc<Shared>) -> Recorder {
    Recorder {
        shared: Arc::clone(shared),
        monitor: InputRing::new(1, 48_000),
        #[cfg(not(target_os = "freebsd"))]
        _stream: None,
        clock: None,
        record_start: None,
        device_name: "Mic In".into(),
        sample_rate: 48_000,
        channels: 1,
        system_audio: crate::system_audio::SystemAudio::Off,
        buffer_frames: None,
        latency: None,
        sample_format: None,
        writer: None,
        seen: (0, Instant::now()),
    }
}

#[test]
fn a_recorder_reports_whether_its_pass_started_without_a_validated_clock() {
    use crate::record::start::{Point, pair};
    use crate::timeline_map::Position;
    let point = Point { host_ns: 0, output_frame: 100, position: Position { sample: 0, fraction: 0, denominator: 1 } };
    for approximate in [false, true] {
        let mut r = recorder(&Arc::new(Shared::new(CaptureRing::new(8))));
        assert!(!r.start_approximate(), "no pass");
        let (mut control, mut publisher) = pair(transit_mix::EngineEpoch(1));
        let reader = control.prepare().unwrap();
        r.record_start = Some(reader.clone());
        assert!(!r.start_approximate(), "not started yet");
        assert!(publisher.start_placed(reader.ticket(), point, Some(64), approximate));
        assert_eq!(r.start_approximate(), approximate);
        r.finish();
        assert_eq!(r.start_approximate(), approximate, "still known after Stop collected the take");
    }
}

#[test]
fn stop_waits_for_an_admitted_push_before_the_final_drain() {
    use std::sync::mpsc::channel;
    let shared = Arc::new(Shared::new(CaptureRing::new(8)));
    shared.start_capture();
    let (entered_tx, entered_rx) = channel();
    let (resume_tx, resume_rx) = channel();
    let producer = Arc::clone(&shared);
    let worker = std::thread::spawn(move || {
        let _lease = producer.begin_capture().unwrap();
        producer.ring.push(4, |i| {
            if i == 0 {
                entered_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            }
            (i + 1) as f32
        });
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    shared.stop_capture();
    let mut block = Vec::new();
    let premature_exit = shared.drain_capture(&mut block);
    // Release and join even if the assertion fails, so a negative-control run leaves no
    // parked producer. This is the writer's production exit predicate, not a timing sleep.
    resume_tx.send(()).unwrap();
    worker.join().unwrap();
    assert!(!premature_exit, "writer exited before the admitted producer published its head");
    assert!(shared.drain_capture(&mut block));
    assert_eq!(block, [1.0, 2.0, 3.0, 4.0]);
}

#[test]
fn stop_closes_admission_and_rearm_rejects_an_old_generation() {
    let shared = Shared::new(CaptureRing::new(8));
    shared.start_capture();
    let before_stop = shared.capture_state.load(Ordering::Acquire);
    shared.stop_capture();
    assert!(shared.admit_capture(before_stop).is_none());
    assert!(shared.begin_capture().is_none());
    assert!(shared.quiescent(), "no future device callback is needed");
    shared.start_capture();
    assert!(shared.admit_capture(before_stop).is_none(), "an old callback must not enter the new take");
    let lease = shared.begin_capture().unwrap();
    assert!(shared.begin_capture().is_none(), "there can only be one producer");
    shared.stop_capture();
    assert!(!shared.quiescent());
    drop(lease);
    assert!(shared.quiescent());
}

#[test]
fn a_panicking_producer_releases_its_capture_lease() {
    let shared = Shared::new(CaptureRing::new(8));
    shared.start_capture();
    shared.guarded(|| {
        let _lease = shared.begin_capture().unwrap();
        std::panic::panic_any("input failure after admission");
    });
    shared.stop_capture();
    assert!(shared.quiescent());
    assert_eq!(shared.fault.load(Ordering::Relaxed), BROKEN);
}

#[test]
fn stop_before_any_input_callback_does_not_need_an_ack_callback() {
    let shared = Arc::new(Shared::new(CaptureRing::new(8)));
    let mut r = recorder(&shared);
    r.arm(Vec::new()).unwrap();
    assert_eq!(r.finish(), (Vec::new(), None));
    assert!(shared.quiescent());
}

#[test]
fn a_writer_panic_is_reported_with_capture_closed() {
    let shared = Arc::new(Shared::new(CaptureRing::new(8)));
    shared.start_capture();
    let mut r = recorder(&shared);
    r.writer = Some((std::thread::spawn(|| std::panic::panic_any("writer failure")), Vec::new()));
    let (takes, problem) = r.finish();
    assert!(takes.is_empty());
    assert!(problem.is_some_and(|e| e.contains("writer stopped unexpectedly")));
    assert!(shared.quiescent());
    assert!(shared.begin_capture().is_none());
}

#[test]
fn writer_panic_waits_for_admitted_capture_and_discards_old_debt_before_rearm() {
    use std::sync::mpsc::channel;
    let dir = std::env::temp_dir().join(format!("transit-panic-rearm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("new.wav");
    let shared = Arc::new(Shared::new(CaptureRing::new(8)));
    shared.start_capture();
    let mut r = recorder(&shared);
    let (admitted_tx, admitted_rx) = channel();
    let (resume_tx, resume_rx) = channel();
    let producer_shared = Arc::clone(&shared);
    let producer = std::thread::spawn(move || {
        let _lease = producer_shared.begin_capture().unwrap();
        // No healthy writer drains this take. Overflow must not become silence in the next.
        producer_shared.capture(&[0.5; 12]);
        admitted_tx.send(()).unwrap();
        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        producer_shared.capture(&[0.75; 2]);
    });
    admitted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    r.writer = Some((std::thread::spawn(|| std::panic::panic_any("writer failure with capture in flight")), Vec::new()));
    let hook_resume = resume_tx.clone();
    CAPTURE_WAIT_HOOK.with(|h| {
        *h.borrow_mut() = Some(Box::new(move || {
            let _ = hook_resume.send(());
        }))
    });
    let (old_takes, problem) = r.finish();
    let quiescent_at_return = shared.quiescent();
    CAPTURE_WAIT_HOOK.with(|h| h.borrow_mut().take());
    // If a mutation bypasses the wait, still release/join the producer and clean up, then
    // fail on the observed state. No elapsed quiet-time window determines correctness.
    let _ = resume_tx.send(());
    producer.join().unwrap();
    let old_debt = shared.pending_silence.load(Ordering::Relaxed);

    r.arm(vec![(TrackId(1), vec![0], path.clone())]).unwrap();
    {
        let _lease = shared.begin_capture().unwrap();
        shared.capture(&[0.25, -0.25]);
    }
    let (new_takes, new_problem) = r.finish();
    let audio = decode(&path);
    std::fs::remove_dir_all(&dir).unwrap();

    assert!(quiescent_at_return, "finish returned while the old callback still held its lease");
    assert!(old_takes.is_empty());
    assert!(problem.is_some_and(|e| e.contains("writer stopped unexpectedly")));
    assert_eq!(old_debt, 6, "the panicked writer could not perform its final silence drain");
    assert_eq!((new_takes.len(), new_problem), (1, None));
    assert_eq!(audio, [vec![0.25, -0.25]], "old-take silence debt entered the new WAV");
}

#[test]
fn final_overflow_silence_preserves_multichannel_length_and_does_not_leak_on_rearm() {
    let dir = std::env::temp_dir().join(format!("transit-final-capture-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let shared = Arc::new(Shared::new(CaptureRing::new(8)));
    let mut r = recorder(&shared);
    r.channels = 3;
    let first_path = dir.join("first.wav");
    // Hold the producer lease through Stop: an admitted callback must finish as part of
    // this take. Pre-fill before starting the writer to force the overflow deterministically.
    shared.start_capture();
    {
        let _lease = shared.begin_capture().unwrap();
        shared.stop_capture();
        shared.capture(&(1..=12).map(|i| i as f32 / 16.0).collect::<Vec<_>>());
    }
    let take = TakeWriter::create(TrackId(1), vec![0, 1, 2], first_path.clone(), 48_000).unwrap();
    let (done, problem) = write_takes(&shared, vec![take], 3);
    assert_eq!((done.len(), problem), (1, None));
    assert_eq!(decode(&first_path), [vec![0.0625, 0.25, 0.4375, 0.0], vec![0.125, 0.3125, 0.5, 0.0], vec![0.1875, 0.375, 0.0, 0.0]]);
    assert_eq!(shared.pending_silence.load(Ordering::Relaxed), 0);
    r.finish();
    // No idle input callback between takes: the next take still starts with no silence debt.
    let second_path = dir.join("second.wav");
    r.arm(vec![(TrackId(2), vec![0, 1, 2], second_path.clone())]).unwrap();
    {
        let _lease = shared.begin_capture().unwrap();
        shared.capture(&[0.25, 0.5, 0.75]);
    }
    assert!(r.finish().1.is_none());
    assert_eq!(decode(&second_path), [vec![0.25], vec![0.5], vec![0.75]]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bug_a_stall_or_a_lost_device_is_trouble() {
    let shared = Arc::new(Shared::new(CaptureRing::new(64)));
    let mut r = recorder(&shared);
    shared.start_capture();
    assert_eq!(r.trouble(), None);
    r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
    assert_eq!(r.trouble(), Some(Trouble::Device("“Mic In” stopped sending audio".into())));
    shared.guarded(|| {});
    assert_eq!(r.trouble(), None, "it sent again");
    shared.stop_capture();
    r.seen.1 = Instant::now().checked_sub(STALL).unwrap();
    assert_eq!(r.trouble(), None, "a quiet input that isn't recording is fine");
    shared.guarded(|| std::panic::panic_any("a bug in the input"));
    let mut ran = false;
    shared.guarded(|| ran = true);
    assert!(!ran);
    assert_eq!(r.trouble(), Some(Trouble::Device("the audio input hit an internal error (see the log)".into())));
    let gone = Arc::new(Shared::new(CaptureRing::new(64)));
    gone.fault.store(GONE, Ordering::Relaxed);
    assert_eq!(recorder(&gone).trouble(), Some(Trouble::Device("“Mic In” was disconnected".into())));
}

#[test]
fn a_failed_write_is_trouble_while_still_recording() {
    let dir = std::env::temp_dir().join(format!("transit-failed-take-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut take = TakeWriter::create(TrackId(1), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
    take.file = File::open(&take.path).unwrap();
    let shared = Arc::new(Shared::new(CaptureRing::new(1 << 12)));
    shared.start_capture();
    let mut r = recorder(&shared);
    let sh = Arc::clone(&shared);
    let writer = std::thread::spawn(move || write_takes(&sh, vec![take], 1));
    shared.capture(&[0.5; 480]);
    let trouble = (0..300).find_map(|_| {
        std::thread::sleep(Duration::from_millis(10));
        r.trouble()
    });
    assert!(matches!(&trouble, Some(Trouble::Disk(e)) if e.contains("Vox_01.wav")), "{trouble:?}");
    assert_eq!(r.trouble(), None, "reported once");
    shared.stop_capture();
    assert!(writer.join().unwrap().1.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn takes_stream_into_their_files() {
    let dir = std::env::temp_dir().join(format!("transit-takes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let gtr = TakeWriter::create(TrackId(1), vec![1, 2], dir.join("Gtr_01.wav"), 48_000).unwrap();
    let mut vox = TakeWriter::create(TrackId(2), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
    assert!(TakeWriter::create(TrackId(3), vec![0], dir.join("Vox_01.wav"), 48_000).is_err(), "never overwrites a file");
    vox.write(&[0.5; 300], 3);
    vox.write_header();
    vox.write(&[0.25; 150], 3);
    assert_eq!(decode(&dir.join("Vox_01.wav"))[0].len(), 100, "the header covers what it has seen");

    let shared = Arc::new(Shared::new(CaptureRing::new(1 << 16)));
    shared.start_capture();
    let sh = Arc::clone(&shared);
    let writer = std::thread::spawn(move || write_takes(&sh, vec![gtr, vox], 3));
    let input: Vec<f32> = (0..3 * 4_800).map(|i| i as f32 / 20_000.0).collect();
    input.chunks(3 * 480).for_each(|block| shared.capture(block));
    shared.stop_capture();
    let (done, problem) = writer.join().unwrap();
    let channel = |c: usize| input.iter().skip(c).step_by(3).copied().collect::<Vec<f32>>();
    assert_eq!((done.len(), problem), (2, None));
    assert_eq!(decode(&done[0].1), vec![channel(1), channel(2)]);
    assert_eq!(decode(&done[1].1), vec![[vec![0.5; 100], vec![0.25; 50], channel(0)].concat()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_take_sums_itself_up_for_drawing_while_it_records() {
    let dir = std::env::temp_dir().join(format!("transit-live-take-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut take = TakeWriter::create(TrackId(4), vec![2, 0], dir.join("Gtr_01.wav"), 48_000).unwrap();
    let input: Vec<f32> = (0..300).flat_map(|f| [f as f32, -1.0, -(f as f32)]).collect();
    take.write(&input[..3 * 100], 3);
    take.write(&input[3 * 100..], 3);
    let live = take.live.lock().unwrap().clone();
    assert_eq!(live, [[(-127.0, 0.0), (-255.0, -128.0), (-299.0, -256.0)], [(0.0, 127.0), (128.0, 255.0), (256.0, 299.0)]]);
    let _ = std::fs::remove_dir_all(&dir);
}
