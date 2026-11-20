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
            shared.capture_at(900 + part as u64 * 32, block, 1);
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
    let path = transit_test_support::TestDir::file("clock-failure", "wav");
    let _ = std::fs::remove_file(&path);
    let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.to_path_buf(), 48_000).unwrap()];
    let shared = Shared::new(CaptureRing::new(32));
    shared.start_capture();
    {
        let _lease = shared.begin_capture().unwrap();
        shared.capture_at(100, &[0.5; 12], 1);
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
            // No current map: the placement waits its bounded time (#587), then fails, as it saw
            // no validated map to fall back to.
            assert!(!mapping.service(None, Some(now)));
            assert!(mapping.service(None, Some(now + start::mapping::WAIT_NS)));
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
        let path = transit_test_support::TestDir::file(&format!("boundary-overflow-{case}"), "wav");
        let _ = std::fs::remove_file(&path);
        let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.to_path_buf(), 48_000).unwrap()];
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
                shared.capture_at(100 + offset as u64, &data[offset..offset + frames], 1);
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

fn decode(path: &std::path::Path) -> Vec<Vec<f32>> {
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
    for parts in [vec![12], vec![4, 4, 4], vec![1, 2, 5, 1, 3]].iter() {
        for published_after in 0..=parts.len() {
            let path = transit_test_support::TestDir::file(&format!("record-start-{unique}"), "wav");
            let mut takes = [TakeWriter::create(TrackId(1), vec![1, 0], path.to_path_buf(), 48_000).unwrap()];
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
        let path = transit_test_support::TestDir::file(&format!("final-start-{unique}"), "wav");
        let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.to_path_buf(), 48_000).unwrap()];
        let shared = Arc::new(Shared::new(CaptureRing::new(32)));
        shared.start_capture();
        {
            let _lease = shared.begin_capture().unwrap();
            shared.capture_at(100, &(0..12).map(|i| i as f32 / 20.0).collect::<Vec<_>>(), 1);
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
        input_identity: ab::CaptureEndpoint { id: "Mic In".into(), name: "Mic In".into() },
        input_ports: Vec::new(),
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
    let dir = transit_test_support::TestDir::new("panic-rearm");
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
    let dir = transit_test_support::TestDir::new("final-capture");
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
    let dir = transit_test_support::TestDir::new("failed-take");
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
    let dir = transit_test_support::TestDir::new("takes");
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
    let dir = transit_test_support::TestDir::new("live-take");
    let _ = std::fs::remove_dir_all(&dir);
    let mut take = TakeWriter::create(TrackId(4), vec![2, 0], dir.join("Gtr_01.wav"), 48_000).unwrap();
    let input: Vec<f32> = (0..300).flat_map(|f| [f as f32, -1.0, -(f as f32)]).collect();
    take.write(&input[..3 * 100], 3);
    take.write(&input[3 * 100..], 3);
    let live = take.live.lock().unwrap().clone();
    assert_eq!(live, [[(-127.0, 0.0), (-255.0, -128.0), (-299.0, -256.0)], [(0.0, 127.0), (128.0, 255.0), (256.0, 299.0)]]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #429, at a 2048-frame period with a 32-channel input on a separate device (cpal ALSA). The start
/// is the output's playback position: the next output frame plus the output's latency, 115 ms past
/// the input's last callback here (a callback 30 ms after it and two periods queued). The input map
/// projects only 100 ms past its last block, so the placement must wait for the next input block
/// rather than fail; the take then starts exactly at the start's input frame.
#[test]
fn a_start_past_the_input_maps_projection_waits_for_the_next_input_block() {
    use crate::device_clock::{self, ClockFitter, Direction, Observation, Quality, Stamp};
    const RATE: u64 = 48_000;
    const BLOCK: u64 = 2048;
    const CHANNELS: usize = 32;
    let (_, clock) = device_clock::stream_clock(Direction::Input, RATE as u32).unwrap();
    let key = clock.key();
    drop(clock);
    let ns = |frames: u64| (u128::from(frames) * 1_000_000_000 / u128::from(RATE)) as u64;
    // Input block k holds frames [k*BLOCK, (k+1)*BLOCK), delivered at its end: its first frame was
    // captured one period before the callback.
    let block = |k: u64| Observation {
        key,
        first_frame: k * BLOCK,
        frames: BLOCK as u32,
        nominal_rate: RATE as u32,
        stamp: Stamp {
            host_callback_ns: 1_000_000_000 + ns((k + 1) * BLOCK),
            backend_callback_ns: ns((k + 1) * BLOCK) as i64,
            backend_frame_ns: ns(k * BLOCK) as i64,
            quality: Quality::BackendEstimate,
            offset_uncertainty_ns: None,
        },
    };
    let mut fitter = ClockFitter::new(key);
    let last = 20;
    for k in 0..=last {
        let _ = fitter.observe(block(k));
    }
    let callback = block(last).stamp.host_callback_ns;
    let (mut latch, mut publisher) = start::pair(transit_mix::EngineEpoch(429));
    let reader = latch.prepare().unwrap();
    let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
    let point = start::Point {
        host_ns: callback + 115_000_000,
        output_frame: 7_000,
        position: crate::timeline_map::Position { sample: 0, fraction: 0, denominator: 1 },
    };
    assert!(publisher.start(reader.ticket(), point));
    // The start's input frame: 115 ms (5520 frames) after the last block's end.
    let expected = (last + 1) * BLOCK + 5_520;
    let map = fitter.map().unwrap();
    assert!(!map.frame_range().contains(&expected), "the start lies past this map's projection");
    let done = mapping.service(Some(map), Some(callback + 5_000_000));
    assert!(!done, "the placement waits for the input to observe the start");
    assert_eq!(input_start.frame(), Ok(None));
    let _ = fitter.observe(block(last + 1));
    let next = block(last + 1).stamp.host_callback_ns;
    assert!(mapping.service(fitter.map(), Some(next + 5_000_000)));
    assert_eq!(input_start.frame(), Ok(Some(expected)));
    // The history (4 s at 48 kHz, 32 channels) holds every block since arm; the take starts there.
    let mut history = start::History::new(CHANNELS, 48_000 * 4);
    let frame = |f: u64| (0..CHANNELS).map(move |c| (f * 100 + c as u64) as f32);
    let mut written = Vec::new();
    for k in last - 2..=last + 4 {
        if k == last + 2 {
            history.set_start(expected);
        }
        let data: Vec<f32> = (k * BLOCK..(k + 1) * BLOCK).flat_map(frame).collect();
        history.push(k * BLOCK, &data, |d| written.extend_from_slice(d)).unwrap();
    }
    assert_eq!(written.len(), ((last + 5) * BLOCK - expected) as usize * CHANNELS);
    assert_eq!(written[..CHANNELS], frame(expected).collect::<Vec<_>>()[..]);
}

/// #429 review: the refused start is logged with both clocks' figures and both latencies; a
/// recorder without an applied start or a fitted map says so instead of inventing figures.
#[test]
fn a_refused_start_logs_both_clocks_and_latencies() {
    let mut r = Recorder::detached("pipewire");
    r.latency = Some(Duration::from_millis(43));
    let (_, map, _) = recording_map();
    let output = crate::device_clock::ClockReport { map: Some(map), publication: Default::default(), fitting: Default::default() };
    let line = r.start_figures(Some(output), Some(Duration::from_millis(85)));
    assert!(line.starts_with("recording start not applied; output clock: BackendEstimate, residual "), "{line}");
    assert!(line.contains(&format!("revision {}", map.revision)) && line.contains("offset "), "{line}");
    assert!(line.contains("latency Some(85ms); input clock: no map, latency Some(43ms)"), "{line}");
}

/// A recorder whose take writer starts only now, after the test queued its input: what the
/// writer finds and what Stop reports are then fixed, with no race against its drain cycle.
fn finish_after(shared: &Arc<Shared>, take: TakeWriter) -> Takes {
    let mut r = recorder(shared);
    let sh = Arc::clone(shared);
    r.writer = Some((std::thread::spawn(move || write_takes(&sh, vec![take], 1)), Vec::new()));
    r.finish()
}

/// #184: a take writer that fell behind long enough for the ring to fill leaves silence in the
/// take; Stop names how much and from where in the take, not only a total.
#[test]
fn a_writer_that_fell_behind_is_counted_located_and_reported_at_stop() {
    let dir = transit_test_support::TestDir::new("lost-overflow");
    let _ = std::fs::remove_dir_all(&dir);
    for (case, overflow) in [("kept", false), ("lost", true)] {
        let shared = Arc::new(Shared::new(CaptureRing::new(8192)));
        shared.start_capture();
        {
            // The take starts one second into the input, so its origin shows at two decimals.
            let _lease = shared.begin_capture().unwrap();
            shared.capture_at(48_000, &[0.5; 4800], 1);
            // 3,392 samples fit; with `overflow` the other 1,408 are refused and owed as silence,
            // from input frame 56,192: take frame 8,192, 0.17 s (1.17 s from the input's start).
            shared.capture_at(52_800, &vec![0.25; if overflow { 4800 } else { 3392 }], 1);
        }
        let path = dir.join(format!("{case}.wav"));
        let (done, problem) = finish_after(&shared, TakeWriter::create(TrackId(1), vec![0], path.clone(), 48_000).unwrap());
        assert_eq!(done.len(), 1);
        let audio = decode(&path).remove(0);
        if overflow {
            assert_eq!(
                problem.as_deref(),
                Some(
                    "29 ms of input lost from 0.17 s into the take: the take writer fell behind (a busy disk or a starved thread) and the take holds silence there"
                )
            );
            assert_eq!(audio, [vec![0.5; 4800], vec![0.25; 3392], vec![0.0; 1408]].concat(), "the take keeps its timing");
        } else {
            assert_eq!(problem, None, "nothing was lost, nothing is reported");
            assert_eq!(audio, [vec![0.5; 4800], vec![0.25; 3392]].concat());
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// #184: zeros a backend put in place of device input it lost (a padded WASAPI discontinuity)
/// reach the take as silence, so it keeps its timing, and are counted while recording and named
/// at Stop. The same zeros delivered as input (a quiet device) are not.
#[test]
fn zeros_a_backend_substituted_for_lost_input_are_counted_and_reported() {
    const BLOCK: usize = 64;
    let dir = transit_test_support::TestDir::new("lost-substituted");
    let _ = std::fs::remove_dir_all(&dir);
    for (case, substituted) in [("quiet", false), ("lost", true)] {
        let shared = Arc::new(Shared::new(CaptureRing::new(1 << 14)));
        let monitor = InputRing::new(1, 48_000);
        let (clock, _reader) = crate::device_clock::stream_clock(crate::device_clock::Direction::Input, 48_000).unwrap();
        let (mut capture, _error) = capture_callbacks(&shared, &monitor, clock, None, None, None);
        shared.start_capture();
        // The callback marks its thread as the audio thread: pump it on a thread of its own.
        std::thread::spawn(move || {
            let format = ab::Format::new(48_000, 1, ab::SampleFormat::F32);
            let (signal, zeros) = ([0.5f32; BLOCK], [0.0f32; BLOCK]);
            for k in 0..25 {
                let lost = (10..15).contains(&k);
                let mut source = ab::fake::SliceSource { data: if lost { &zeros } else { &signal }, channels: 1 };
                let mut block = ab::CaptureBlock::new(BLOCK, ab::Stamp::INVALID, format, &mut source);
                block.substituted = lost && substituted;
                capture(&mut block);
            }
        })
        .join()
        .unwrap();
        let path = dir.join(format!("{case}.wav"));
        let lost_ms = recorder(&shared).lost_ms();
        let (_, problem) = finish_after(&shared, TakeWriter::create(TrackId(1), vec![0], path.clone(), 48_000).unwrap());
        assert_eq!(decode(&path)[0], [vec![0.5; 10 * BLOCK], vec![0.0; 5 * BLOCK], vec![0.5; 10 * BLOCK]].concat());
        if substituted {
            assert_eq!(lost_ms, 6, "the status line counts it while recording");
            assert_eq!(problem.as_deref(), Some("6 ms of input lost by the audio device from 0.01 s into the take, recorded as silence"));
        } else {
            assert_eq!((lost_ms, problem), (0, None), "input that is silent is not lost input");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nan_and_infinite_input_is_counted_and_reported() {
    let shared = Shared::new(CaptureRing::new(8));
    shared.capture(&[0.5, -0.5]);
    assert_eq!(shared.losses.report(0, 48_000), None);
    shared.capture(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
    assert_eq!(shared.losses.report(0, 48_000).as_deref(), Some("3 invalid input samples (not a number or infinite) recorded as silence"));
    shared.start_capture();
    assert_eq!(shared.losses.report(0, 48_000), None, "a new take starts clean");
}

/// #184: a failed take file does not hide the input lost before it, nor the reverse.
#[test]
fn a_write_failure_and_lost_input_are_both_reported() {
    let dir = transit_test_support::TestDir::new("lost-and-failed");
    let _ = std::fs::remove_dir_all(&dir);
    let mut take = TakeWriter::create(TrackId(1), vec![0], dir.join("Vox_01.wav"), 48_000).unwrap();
    take.file = File::open(&take.path).unwrap();
    let shared = Arc::new(Shared::new(CaptureRing::new(8)));
    shared.start_capture();
    {
        let _lease = shared.begin_capture().unwrap();
        shared.capture_at(0, &[0.5; 4], 1);
        shared.capture_at(4, &[0.5; 4800], 1);
    }
    let problem = finish_after(&shared, take).1.unwrap();
    let (write, lost) = problem.split_once("; ").unwrap();
    assert!(write.contains("Vox_01.wav"), "{problem}");
    assert!(lost.starts_with("99 ms of input lost from 0.00 s into the take"), "{problem}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #184: the take writer measures each take's longest stretch of exact digital silence between
/// signal (a device that sent zeros while still calling back), and only a stretch of 100 ms or
/// more becomes a note; silence at either end of a take is not a stretch.
#[test]
fn a_take_measures_its_longest_exact_silence_between_signal() {
    let dir = transit_test_support::TestDir::new("zero-run");
    let _ = std::fs::remove_dir_all(&dir);
    // A stereo take of input channels 1 and 0 of a 3-channel input; channel 2 is not in it.
    let write = |name: &str, parts: &[(f32, f32, usize)]| {
        let mut take = TakeWriter::create(TrackId(1), vec![1, 0], dir.join(name), 48_000).unwrap();
        for &(left, right, n) in parts {
            let block: Vec<f32> = (0..n).flat_map(|_| [left, right, 0.75]).collect();
            take.write(&block, 3);
        }
        take.zeros
    };
    let lost = write("lost.wav", &[(0.0, 0.0, 1000), (0.1, 0.0, 500), (0.0, 0.0, 4800), (0.0, -0.1, 10), (0.0, 0.0, 9000)]);
    assert_eq!(lost.longest(), Some((1500, 4800)));
    assert_eq!(
        lost.note("lost.wav", 48_000).as_deref(),
        Some("lost.wav holds 0.100 s of exact digital silence between signal, from 0.031 s into the take (frames 1500 to 6300)")
    );
    let short = write("short.wav", &[(0.1, 0.1, 10), (0.0, 0.0, 4799), (0.1, 0.1, 10)]);
    assert_eq!((short.longest(), short.note("short.wav", 48_000)), (Some((10, 4799)), None), "under 100 ms is no note");
    let ends = write("ends.wav", &[(0.0, 0.0, 48_000), (0.2, 0.2, 10), (0.0, 0.0, 48_000)]);
    assert_eq!(ends.longest(), None, "silence before the first and after the last signal is not between signal");
    let _ = std::fs::remove_dir_all(&dir);
}

/// #184 review B1, the production path: a take placed by its applied start (the gated writer of
/// `prepare`) reports where its silence begins counted from that start, not from the first
/// captured frame or from zero, and the figure matches the silence in the file.
#[test]
fn a_gated_take_locates_its_silence_from_the_applied_start() {
    let (key, map, now) = recording_map();
    let data: Vec<f32> = (1..=12_000).map(|n| n as f32 / 12_000.0).collect();
    for (case, parts) in [vec![12_000], vec![4_000, 4_000, 4_000]].into_iter().enumerate() {
        let path = transit_test_support::TestDir::file(&format!("gated-overflow-{case}"), "wav");
        let _ = std::fs::remove_file(&path);
        let takes = vec![TakeWriter::create(TrackId(1), vec![0], path.to_path_buf(), 48_000).unwrap()];
        let shared = Shared::new(CaptureRing::new(8192));
        shared.start_capture();
        let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(5));
        let reader = control.prepare().unwrap();
        let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
        let gate = WriterStart { reader: reader.clone(), history: start::History::new(1, 2_000), input_start, placed: false, next: None };
        let mut offset = 0;
        for frames in parts {
            let _lease = shared.begin_capture().unwrap();
            shared.capture_at(100 + offset as u64, &data[offset..offset + frames], 1);
            offset += frames;
        }
        // The take starts at input frame 1,100. The ring kept input frames 100 to 8,291, so the
        // take's silence begins at take frame 7,192 (0.15 s; 0.17 s from the first captured frame
        // or from zero).
        let point = start::Point {
            host_ns: map.frame_to_host(key, 1_100, now).unwrap(),
            output_frame: 900,
            position: crate::timeline_map::Position { sample: 500, fraction: 0, denominator: 1 },
        };
        assert!(publisher.start(reader.ticket(), point));
        assert!(mapping.service(Some(map), Some(now)));
        shared.stop_capture();
        let (takes, error) = write_takes_started(&shared, takes, 1, Some(gate));
        assert!(error.is_none(), "{error:?}");
        assert_eq!(takes.len(), 1);
        let audio = decode(&path).remove(0);
        assert_eq!(audio.len(), 11_000, "the take keeps its timing");
        assert_eq!(audio[..7_192], data[1_000..8_192]);
        assert!(audio[7_192..].iter().all(|v| *v == 0.0));
        assert_eq!(
            shared.losses.report(shared.dropped.load(Ordering::Relaxed), 48_000).as_deref(),
            Some(
                "79 ms of input lost from 0.15 s into the take: the take writer fell behind (a busy disk or a starved thread) and the take holds silence there"
            )
        );
        std::fs::remove_file(path).unwrap();
    }
}

/// #184 review F1: input lost before the take's first frame (a count-in or pre-roll the take does
/// not keep) is not placed at 0.00 s in the take.
#[test]
fn input_lost_before_the_take_began_is_said_so() {
    let shared = Shared::new(CaptureRing::new(8));
    shared.losses.set_origin(48_000);
    shared.losses.overflow(47_000);
    assert_eq!(
        shared.losses.report(480, 48_000).as_deref(),
        Some(
            "10 ms of input lost starting before the take began: the take writer fell behind (a busy disk or a starved thread) and the take holds silence there"
        )
    );
    shared.start_capture();
    shared.losses.set_origin(48_000);
    shared.losses.overflow(48_480);
    assert!(shared.losses.report(480, 48_000).unwrap().starts_with("10 ms of input lost from 0.01 s into the take"));
}

/// #184 review F3: zeros a backend substituted that the full ring also refused are counted once,
/// as refused input, not again as input the device lost.
#[test]
fn substituted_zeros_the_ring_refused_are_counted_once() {
    const BLOCK: usize = 64;
    let shared = Arc::new(Shared::new(CaptureRing::new(2 * BLOCK)));
    let monitor = InputRing::new(1, 48_000);
    let (clock, _reader) = crate::device_clock::stream_clock(crate::device_clock::Direction::Input, 48_000).unwrap();
    let (mut capture, _error) = capture_callbacks(&shared, &monitor, clock, None, None, None);
    shared.start_capture();
    // One block of input, then two substituted blocks; nothing drains, so the ring takes the first
    // substituted block and refuses the second.
    std::thread::spawn(move || {
        let format = ab::Format::new(48_000, 1, ab::SampleFormat::F32);
        let (signal, zeros) = ([0.5f32; BLOCK], [0.0f32; BLOCK]);
        for k in 0..3 {
            let mut source = ab::fake::SliceSource { data: if k == 0 { &signal } else { &zeros }, channels: 1 };
            let mut block = ab::CaptureBlock::new(BLOCK, ab::Stamp::INVALID, format, &mut source);
            block.substituted = k > 0;
            capture(&mut block);
        }
    })
    .join()
    .unwrap();
    assert_eq!((shared.dropped.load(Ordering::Relaxed), shared.losses.substituted_frames()), (BLOCK, BLOCK));
    assert_eq!(recorder(&shared).lost_ms(), 2, "128 frames lost in all, not 192");
}

mod start_fallback;

/// ADR 0062 section 6: a Failed stage finishes the writer's start as Cancelled does, without
/// waiting for an input mapping, and stops the capture; CountingIn and Waiting wait as Pending.
#[test]
fn failed_finishes_the_writer_start() {
    let (key, map, now) = recording_map();
    let shared = Shared::new(CaptureRing::new(32));
    shared.start_capture();
    let (mut control, mut publisher) = start::pair(transit_mix::EngineEpoch(5));
    let reader = control.prepare().unwrap();
    let ticket = reader.ticket();
    let (mut mapping, input_start) = start::mapping::pair(reader.clone(), key);
    let mut gate = WriterStart { reader: reader.clone(), history: start::History::new(1, 24), input_start, placed: false, next: None };
    let mut takes: [TakeWriter; 0] = [];
    assert_eq!(gate.finished(), Ok(false), "a pending start waits");
    assert!(publisher.counting_in(ticket));
    gate.write(&shared, &mut takes, &[], 1).unwrap();
    assert_eq!(gate.finished(), Ok(false), "a count-in waits as Pending does");
    assert!(shared.armed());
    assert!(publisher.waiting(ticket));
    gate.write(&shared, &mut takes, &[], 1).unwrap();
    assert_eq!(gate.finished(), Ok(false), "a clock wait waits as Pending does");
    assert!(!mapping.service(Some(map), Some(now)), "nothing to map before the start");
    assert!(publisher.fail(ticket, Some(start::StartFailure::Clock)));
    assert_eq!(reader.status(), start::Status::Failed(start::StartFailure::Clock));
    // No mapping was serviced since: the writer does not wait for one.
    assert_eq!(gate.finished(), Ok(true), "Failed finishes the writer's start");
    gate.write(&shared, &mut takes, &[], 1).unwrap();
    assert!(!shared.armed(), "Failed stops the capture, as Cancelled does");
    assert!(mapping.service(Some(map), Some(now)), "the mapping ends too");
}
