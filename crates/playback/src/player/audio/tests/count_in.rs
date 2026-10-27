use super::*;

/// The clock service keeps the map current as callbacks run; a test that runs thousands of frames
/// refits one that covers the frames around `frame`.
fn track_clock(audio: &mut Audio, rate: u32, frame: u64) {
    use crate::device_clock::{ClockFitter, Observation, Quality, Stamp};
    let key = audio.transport.clock_key;
    let mut fitter = ClockFitter::new(key);
    let step = u64::from(rate / 250);
    let n = (frame / step).max(30);
    for i in n - 30..=n {
        let ns = i * 4_000_000;
        let _ = fitter.observe(Observation {
            key,
            first_frame: i * step,
            frames: rate / 250,
            nominal_rate: rate,
            stamp: Stamp {
                host_callback_ns: 1_000_000_000 + ns,
                backend_callback_ns: ns as i64,
                backend_frame_ns: ns as i64,
                quality: Quality::BackendEstimate,
                offset_uncertainty_ns: None,
            },
        });
    }
    audio.transport.clock_map = fitter.map();
    audio.transport.clock_now = Some(1_000_000_000 + n * 4_000_000);
}

/// A count-in fixture: a clocked recorder waiting for a ticket, the default session (120 bpm, 4/4).
fn counting(from: Samples, bars: i64) -> (Control<Command, ActionResult>, Audio, Arc<Shared>, start::Control, start::Reader) {
    let (mut control, mut audio, shared) = fixture(48_000);
    fill(&mut audio, &mut [0.0; 8]);
    control.service(&mut make);
    let (mut start, publisher) = start::pair(transit_mix::EngineEpoch(1));
    audio.transport.record_start = publisher;
    recording_clock(&mut audio, 48_000);
    let reader = start.prepare().unwrap();
    reader.mark_audio();
    let count_in = CountIn { frames: bars * 4 * 24_000, beat: 24_000, per_bar: 4 };
    let command = Command::Record { ticket: reader.ticket(), from: Some(from), end: None, looped: None, count_in: Some(count_in) };
    control.send_action(command, true).unwrap();
    (control, audio, shared, start, reader)
}

/// Audible frames (either channel) in a stereo callback buffer.
fn loud(out: &[f32]) -> usize {
    out.as_chunks::<2>().0.iter().filter(|f| f[0].abs() > 1e-6 || f[1].abs() > 1e-6).count()
}

#[test]
fn count_in_clicks_for_exactly_the_bar_then_play_and_the_recording_latch_start_on_that_sample() {
    // 700 frames per callback does not divide 96,000: the hand-over happens inside a callback.
    for (from, callback) in [(0, 480usize), (0, 700), (96_000, 700)] {
        let (mut control, mut audio, shared, _start, reader) = counting(from, 1);
        let first = audio.transport.timeline.next_output();
        let mut out = vec![0.0f32; callback * 2];
        let mut frames = 0usize;
        let mut clicked = 0usize;
        let mut after = 0usize;
        while reader.status() == start::Status::Pending && frames < 200_000 {
            let now = audio.transport.timeline.next_output();
            track_clock(&mut audio, 48_000, now);
            fill(&mut audio, &mut out);
            control.service(&mut make);
            if reader.status() == start::Status::Pending {
                // Parked at the start: the position does not move and nothing is playing.
                assert!(!audio.transport.playing && !audio.transport.recording);
                assert_eq!(shared.position.load(Ordering::Relaxed), from);
                let (left, total) = (shared.metro.count_in_left.load(Ordering::Relaxed), shared.metro.count_in_total.load(Ordering::Relaxed));
                assert_eq!(total, 96_000);
                // Rendered block by block, so it runs ahead of the callbacks by at most one block.
                assert!((0..=96_000).contains(&left), "{left}");
                clicked += loud(&out);
            } else {
                after = loud(&out);
            }
            frames += callback;
        }
        let start::Status::Started(point) = reader.status() else {
            panic!(
                "never latched: {:?} frames={frames} from={from} cb={callback} left={}",
                reader.status(),
                shared.metro.count_in_left.load(Ordering::Relaxed)
            )
        };
        // The latch is the output frame 96,000 samples after the command was applied, at the start.
        assert_eq!(point.output_frame, first + 96_000);
        assert_eq!(point.position, timeline_map::Position { sample: from, fraction: 0, denominator: 1 });
        assert!(audio.transport.playing && audio.transport.recording);
        assert!(clicked > 4 * 100, "four beats of click, got {clicked} audible frames");
        assert!(clicked <= 4 * 1_440 + 8, "click blips only, got {clicked}");
        // Material from the start sample: the default session has none and the click option is off.
        assert_eq!(after, 0);
        assert_eq!(shared.metro.count_in_left.load(Ordering::Relaxed), 0);
        control.retire(audio.plans, audio.actions);
    }
}

#[test]
fn count_in_click_is_four_beats_of_one_bar_with_the_accent_first() {
    let (mut control, mut audio, _, _start, reader) = counting(0, 1);
    let mut out = vec![0.0f32; 480 * 2];
    let mut blips = Vec::new();
    for call in 0..200 {
        fill(&mut audio, &mut out);
        control.service(&mut make);
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        if peak > 0.0 && call % 50 == 0 {
            blips.push(peak);
        }
    }
    assert_eq!(blips.len(), 4, "a click on each beat");
    assert!(blips[0] > blips[1] && (blips[1] - blips[2]).abs() < 1e-3, "beat 1 is accented: {blips:?}");
    assert!(matches!(reader.status(), start::Status::Pending | start::Status::Started(_)));
    control.retire(audio.plans, audio.actions);
}

#[test]
fn stopping_during_the_count_in_leaves_no_take_and_no_sounding_click() {
    let (mut control, mut audio, shared, start, reader) = counting(0, 2);
    let mut out = vec![0.0f32; 480 * 2];
    for _ in 0..60 {
        fill(&mut audio, &mut out);
        control.service(&mut make);
    }
    assert!(shared.metro.count_in_left.load(Ordering::Relaxed) > 0);
    // Main: cancel the start, then Stop.
    start.cancel();
    control.stop().unwrap();
    fill(&mut audio, &mut out);
    assert_eq!(reader.status(), start::Status::Cancelled);
    assert!(audio.transport.count_in.is_none() && !audio.transport.playing && !audio.transport.recording);
    assert_eq!(shared.metro.count_in_left.load(Ordering::Relaxed), 0);
    // Well past where the count-in would have ended: still silent, still cancelled, still stopped.
    for _ in 0..400 {
        fill(&mut audio, &mut out);
        assert_eq!(loud(&out), 0, "no click after Stop");
    }
    assert_eq!(reader.status(), start::Status::Cancelled);
    assert!(!audio.transport.playing && !audio.transport.recording);
    control.retire(audio.plans, audio.actions);
}

#[test]
fn a_cancelled_ticket_stops_the_count_in_from_starting_play() {
    let (control, mut audio, _shared, start, reader) = counting(0, 1);
    let mut out = vec![0.0f32; 480 * 2];
    for _ in 0..10 {
        fill(&mut audio, &mut out);
    }
    start.cancel();
    for _ in 0..300 {
        fill(&mut audio, &mut out);
    }
    assert_eq!(reader.status(), start::Status::Cancelled);
    assert!(!audio.transport.playing && !audio.transport.recording);
    control.retire(audio.plans, audio.actions);
}

#[test]
fn a_failed_start_at_the_end_of_the_count_in_is_reported_and_leaves_the_transport_parked() {
    let (control, mut audio, shared, _start, reader) = counting(0, 1);
    let mut out = vec![0.0f32; 480 * 2];
    for _ in 0..20 {
        fill(&mut audio, &mut out);
    }
    assert_eq!(shared.metro.start_failure.load(Ordering::Relaxed), 0);
    // The clock map is lost during the count-in, and never comes back.
    audio.transport.clock_map = None;
    for _ in 0..300 {
        fill(&mut audio, &mut out);
    }
    assert_eq!(reader.status(), start::Status::Cancelled);
    assert!(!audio.transport.playing && !audio.transport.recording && audio.transport.count_in.is_none());
    assert_eq!(shared.metro.start_failure.load(Ordering::Relaxed), StartFailure::Clock as u8);
    control.retire(audio.plans, audio.actions);
}

#[test]
fn a_loop_set_during_the_count_in_is_the_one_the_recording_starts_with() {
    let (mut control, mut audio, _shared, _start, reader) = counting(0, 1);
    let mut out = vec![0.0f32; 480 * 2];
    for _ in 0..10 {
        fill(&mut audio, &mut out);
    }
    control.send_action(Command::SetLoop(Some(Range { start: 0, end: 4_800 })), true).unwrap();
    for call in 0..300 {
        let now = audio.transport.timeline.next_output();
        track_clock(&mut audio, 48_000, now);
        fill(&mut audio, &mut out);
        control.service(&mut make);
        if matches!(reader.status(), start::Status::Started(_)) {
            let _ = call;
            break;
        }
    }
    assert!(matches!(reader.status(), start::Status::Started(_)), "{:?}", reader.status());
    assert_eq!(audio.transport.looped, Some(Range { start: 0, end: 4_800 }));
    control.retire(audio.plans, audio.actions);
}

#[test]
fn metronome_gain_scales_the_click_and_changing_it_allocates_nothing() {
    let peak_of_beat_two = |gain: f32| {
        let (control, mut audio, shared, _start, _reader) = counting(0, 1);
        let mut out = vec![0.0f32; 480 * 2];
        let mut peak = 0.0f32;
        for call in 0..60 {
            if call == 49 {
                // Between callbacks, as Main would: one atomic store.
                let ((), heap) = measure(|| shared.metro.gain.store(gain.to_bits(), Ordering::Relaxed));
                assert!(heap.is_zero());
            }
            let ((), heap) = measure(|| fill(&mut audio, &mut out));
            assert!(heap.is_zero(), "{heap:?}");
            if call >= 50 {
                peak = peak.max(out.iter().fold(0.0f32, |m, v| m.max(v.abs())));
            }
        }
        control.retire(audio.plans, audio.actions);
        peak
    };
    let (full, quarter) = (peak_of_beat_two(1.0), peak_of_beat_two(0.25));
    assert!(full > 0.0 && (quarter / full - 0.25).abs() < 0.02, "{full} {quarter}");
}
