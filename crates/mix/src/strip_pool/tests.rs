//! Unit tests of the fan-out's own pieces; the worker set itself is exercised end to end in
//! `crates/mix/tests/render_workers.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::time::Instant;
use transit_dsp::event::NoteId;

/// Serializes the unit tests that flip the process-wide serial switch.
pub(crate) static SWITCH: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn event(message: Message, channel: u8) -> InstrumentEvent {
    InstrumentEvent {
        engine: EngineEpoch(1),
        target: (TrackId(1), INSTRUMENT_SLOT, SlotGen::VACANT),
        due: Due::Next,
        transport: None,
        source: Source { port: 0, channel, generation: 1 },
        message,
        arrival: Arrival { host_ns: 0, sequence: 0 },
    }
}

#[test]
fn the_ring_sees_the_classes_its_rules_need() {
    let id = NoteId::NONE;
    let on = to_event(&event(Message::NoteOn { key: 60, velocity: 90, id }, 3), 7);
    assert!(on.is_note_on() && on.channel() == 3 && on.offset == 7 && on.data1 == 60);
    let off = to_event(&event(Message::NoteOff { key: 60, velocity: 0, id }, 3), 9);
    assert!(off.is_note_off() && off.is_release());
    assert!(to_event(&event(Message::Control { cc: 64, value: 0 }, 0), 0).is_release(), "sustain off");
    assert!(!to_event(&event(Message::Control { cc: 64, value: 127 }, 0), 0).is_release(), "sustain on");
    assert!(!to_event(&event(Message::PitchBend { value: 100 }, 0), 0).is_release());
    assert!(!to_event(&event(Message::Program { program: 4 }, 0), 0).is_release());
    assert_eq!(on.tag, Event::NO_TAG, "the caller tags it");
}

#[test]
fn a_block_period_follows_frames_and_rate() {
    assert_eq!(block_period(128, 48_000), Duration::from_nanos(2_666_666));
    assert_eq!(block_period(64, 48_000), Duration::from_nanos(1_333_333));
    assert_eq!(block_period(64, 0), Duration::from_millis(1), "an unknown rate has a floor");
}

#[test]
fn sending_a_strip_away_keeps_its_routing_state_and_identity_in_the_table() {
    let mut table = Strip::new();
    table.latency = 64;
    table.pdc_out = 99;
    table.pdc_pos = 3;
    table.generation = SlotGen(7);
    table.track = 2;
    table.id = TrackId(42);
    table.sink_routes = vec![Some(1)];
    table.meter.peaks = vec![0.5];
    table.buf = vec![vec![0.25; 4]];
    let mut cell = Strip::new();
    send_away(&mut table, &mut cell);
    assert_eq!((table.latency, table.pdc_out, table.pdc_pos), (64, 99, 3), "the delay walk reads the table");
    assert_eq!((table.generation, table.track, table.id), (SlotGen(7), 2, TrackId(42)), "event lookups find the strip");
    assert_eq!(table.sink_routes, [Some(1)]);
    assert_eq!(table.meter.peaks, [0.5]);
    assert!(table.buf.is_empty(), "the render state went with the strip");
    assert_eq!(cell.buf, [vec![0.25; 4]]);
    table.pdc_out = 100;
    take_home(&mut table, &mut cell);
    assert_eq!((table.pdc_out, table.latency, table.buf.len()), (100, 64, 1), "home again with what the table updated");
    assert_eq!(table.sink_routes, [Some(1)]);
    assert_eq!(cell.generation, SlotGen::VACANT, "the spare carries no identity");
    assert!(cell.sink_routes.is_empty() && cell.buf.is_empty());
}

/// Test hook: the strip index whose every worker render sleeps past the deadline and then
/// panics (a runner fault that is also a late result). `usize::MAX`: none.
pub(crate) static LATE_PANIC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(usize::MAX);

pub(crate) fn late_panic(strip: usize) {
    if LATE_PANIC.load(Ordering::Relaxed) == strip {
        std::thread::sleep(Duration::from_millis(20));
        std::panic::panic_any("injected late runner fault");
    }
}

/// A late result whose runner panicked makes its strip suspect at `begin_block`; the pool keeps
/// that same instance silent (never republished) until a plan replaces it.
#[test]
fn a_late_runner_fault_keeps_the_same_instance_suspect() {
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::default();
    for _ in 0..6 {
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    }
    let mut eng = RangeRender::new(&s, None, transit_time::Range::new(0, 512 * 4000), 512).eng;
    if eng.start_render_workers(16) == 0 {
        return;
    }
    eng.set_strip_deadline(Some(Duration::from_millis(2)));
    MixEngine::force_serial(false);
    LATE_PANIC.store(0, Ordering::Relaxed);
    let eng = std::thread::spawn(move || {
        mark_audio_thread();
        let mut out = vec![vec![0.0f32; 512]; 2];
        let start = Instant::now();
        let panics = |e: &MixEngine| e.pool.as_ref().map_or(0, |p| p.set.stats().runner_panics);
        let mut b = 0i64;
        while panics(&eng) == 0 && start.elapsed() < Duration::from_secs(10) {
            eng.render_active(b * 512, 512, &mut out);
            b += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
        // timing: sleep-ok paces the blocks so the late result is retired, not a measurement
        for k in 0..60 {
            eng.render_active((b + k) * 512, 512, &mut out);
            std::thread::sleep(Duration::from_millis(1));
        }
        unmark_audio_thread();
        eng
    })
    .join()
    .unwrap();
    LATE_PANIC.store(usize::MAX, Ordering::Relaxed);
    MixEngine::force_serial(true);
    let pool = eng.pool.as_ref().unwrap();
    let st = pool.set.stats();
    let suspect = pool.counters.suspect.load(Ordering::Relaxed);
    assert_eq!(st.runner_panics, 1, "the faulted instance was republished: {st:?}");
    assert!(suspect > 0, "the strip stayed silent as suspect");
    assert!(pool.set.is_suspect(0));
    drop(eng.into_retired());
}

/// #162 with render workers: a strip the callback renders inline (here a set with no workers,
/// so every strip is reclaimed inline) marks the phase marker with its own phase and index, as
/// the serial path does; the block's longest stretch is a strip render, not the mixdown before it.
#[test]
fn a_strip_rendered_inline_marks_the_phase_marker() {
    use transit_audio_io::AudioBuffer;
    use transit_model::{Clip, Insert, SourceAudio, SourceId};
    use transit_rt::phase_marker::{Phase, PhaseMarker};
    let _switch = SWITCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut s = Session::default();
    let len = 48_000usize;
    for k in 0..6u32 {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let signal: Vec<f32> = (0..len).map(|i| (i as f32 * 0.01 * (k + 1) as f32).sin() * 0.2).collect();
        let src = SourceId(9100 + u64::from(k));
        s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![signal] })));
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", src, 0, 0, len as i64));
        tr.mixer.inserts[0] = Some(Insert::new("room_reverb"));
    }
    let mut eng = RangeRender::new(&s, None, transit_time::Range::new(0, 512 * 40), 512).eng;
    eng.pool = Some(StripPool::new(0, 16, false, DEFAULT_STRIP_DEADLINE_PERCENT).unwrap());
    // A cost history for every strip: a strip without one is never rendered inline (#168).
    eng.seed_strip_costs(Duration::from_micros(1));
    eng.set_strip_deadline(Some(Duration::from_secs(30)));
    let marker = Arc::new(PhaseMarker::new());
    eng.set_phase_marker(Arc::clone(&marker));
    MixEngine::force_serial(false);
    let m = Arc::clone(&marker);
    let (eng, stamp, inline) = std::thread::spawn(move || {
        mark_audio_thread();
        let mut out = vec![vec![0.0f32; 512]; 2];
        for b in 0..4 {
            eng.render_active(b * 512, 512, &mut out);
        }
        m.begin();
        eng.render_active(4 * 512, 512, &mut out);
        m.end();
        let inline = eng.pool.as_ref().map_or(0, |p| p.set.stats().inline_renders);
        unmark_audio_thread();
        (eng, m.read(), inline)
    })
    .join()
    .unwrap();
    MixEngine::force_serial(true);
    assert_eq!(inline, 5 * 6, "every strip was rendered inline");
    assert_eq!(stamp.gap_phase, Phase::StripRender, "the inline strip renders marked their phase: {stamp:?}");
    assert!(stamp.gap_strip < 6, "with the strip's track index: {stamp:?}");
    drop(eng.into_retired());
}

/// #172: the strips' deadline share decides whether a strip whose render is longer than three
/// quarters of the period but shorter than one and a half is mixed. One strip, a render of 1.1
/// periods (128 frames at 48 kHz) begun at the block's start: at 75% its worker is still
/// rendering at the deadline (`Overrun(Late)`), at 150% it is done before it (`Mixed`).
///
/// Driven by construction, not by the clock: the pool has no worker and the test claims the
/// slot itself as the worker. The block's start is placed one budget in the past (the pool's
/// own `strip_budget`, the budget `render` hands `finish`), so `finish`'s deadline has already
/// come when it is called and it returns at once, and the slot shows the render's state at the
/// deadline: completed if the render took no longer than the budget, still claimed if not.
#[test]
fn a_render_of_1_1_periods_is_late_at_75_percent_and_mixed_at_150() {
    let (frames, rate) = (128, 48_000);
    let render = block_period(frames, rate) * 11 / 10;
    for (percent, expected) in [(75, Outcome::Overrun(OverrunCause::Late)), (150, Outcome::Mixed)] {
        let mut pool = StripPool::new(0, 1, false, percent).unwrap();
        let budget = pool.strip_budget(frames, rate);
        assert_eq!(budget, block_period(frames, rate) * percent / 100, "the pool's share of the period");
        assert_eq!(pool.reader().snapshot().deadline_percent, u64::from(percent), "the report's share");
        assert!(pool.set.begin_block(1));
        assert_eq!(pool.set.publish(0), Ok(Published::Published));
        let start = Instant::now().checked_sub(budget).unwrap();
        match pool.set.jobs().slot(0).and_then(transit_rt::strip_jobs::JobSlot::try_claim) {
            Some(token) if render <= budget => token.complete(),
            // Still rendering at the deadline: a token left uncompleted keeps the slot claimed.
            Some(_still_rendering) => {}
            None => panic!("the test claims the slot as the worker"),
        }
        pool.set.finish(start + budget, |_| panic!("a claimed strip is never rendered inline"));
        assert_eq!(pool.set.outcome(0), Some(expected), "at {percent}% of the period");
    }
}
