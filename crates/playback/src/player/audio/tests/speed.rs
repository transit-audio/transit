//! #522: recording runs at normal speed only, and a command runs at the speed requested when the
//! audio thread applies it, wherever in a callback that happens: never at one the callback read
//! before it.
use super::*;

/// Half-Speed playing in 2,048-frame callbacks with 512-sample render blocks. After the callback
/// has read 0.5 at its top, the host stores 1x and sends Stop, then Record from stopped, and the
/// callback services them at a block boundary inside itself, as its frame loop does. The latch is
/// published at 1x, and the rest of the callback (1,024 output frames) moves the timeline 1,024
/// samples, so the latch, the frames recorded and the host's lap clock agree exactly. Before,
/// those frames moved it 512 samples and the lap clock stayed 512 samples off for good.
#[test]
fn stop_and_record_serviced_inside_a_half_speed_callback_record_at_normal_speed() {
    let (mut control, mut audio, shared) = fixture(48_000);
    fill(&mut audio, &mut [0.0; 8]);
    control.service(&mut make);
    recording_clock(&mut audio, 48_000);
    let (mut start, publisher) = start::pair(transit_mix::EngineEpoch(1));
    audio.transport.record_start = publisher;
    shared.speed.store(500, Ordering::Relaxed);
    control.send_action(Command::Play { from: 0, end: None, looped: None }, true).unwrap();
    fill(&mut audio, &mut [0.0; 4_096]);
    assert!(audio.transport.playing);
    assert_eq!(audio.transport.speed, 500, "Half-Speed is applied");
    shared.speed.store(1_000, Ordering::Relaxed);
    control.stop().unwrap();
    let reader = start.prepare().unwrap();
    control.send_action(Command::Record { ticket: reader.ticket(), from: Some(96_000), end: None, looped: None, count_in: None }, true).unwrap();
    let ((), heap) = measure(|| audio.service_actions(&mut action_lane::callback_scope()));
    assert!(heap.is_zero(), "{heap:?}");
    let start::Status::Started(point) = reader.status() else { panic!("Record did not apply") };
    assert_eq!(audio.transport.speed, 1_000, "Record ran at the speed requested when it was applied");
    // The rate keeps a common denominator across the speed change: compare the value.
    assert_eq!((point.position.sample, point.position.fraction), (96_000, 0));
    // The rest of the callback.
    audio.transport.consumed(1_024, &shared).unwrap();
    let frames = shared.output_frame.load(Ordering::Relaxed) - point.output_frame;
    let moved = audio.transport.timeline.position().unwrap().sample - point.position.sample;
    assert_eq!((frames, moved), (1_024, 1_024), "frames recorded and timeline samples agree");
    assert_eq!(shared.position.load(Ordering::Relaxed), 96_000 + 1_024);
    start.cancel();
    control.retire(audio.plans, audio.actions);
}

/// The backstop: if the applied speed is ever not 1x when Record is applied, no latch is
/// published; the start waits for 1x at later block boundaries, and fails when its second is up.
/// The requested speed is never changed.
#[test]
fn a_latch_is_never_published_at_another_speed() {
    let (mut control, mut audio, shared) = fixture(48_000);
    fill(&mut audio, &mut [0.0; 8]);
    control.service(&mut make);
    recording_clock(&mut audio, 48_000);
    let (mut start, publisher) = start::pair(transit_mix::EngineEpoch(1));
    audio.transport.record_start = publisher;
    shared.speed.store(500, Ordering::Relaxed);
    let reader = start.prepare().unwrap();
    control.send_action(Command::Record { ticket: reader.ticket(), from: Some(0), end: None, looped: None, count_in: None }, true).unwrap();
    let ((), heap) = measure(|| fill(&mut audio, &mut [0.0; 4_096]));
    assert!(heap.is_zero(), "{heap:?}");
    assert_eq!(reader.status(), start::Status::Pending, "no latch at 0.5x");
    assert!(!audio.transport.recording);
    // A second of 2,048-frame callbacks: still no latch, then the start fails.
    for _ in 0..24 {
        fill(&mut audio, &mut [0.0; 4_096]);
    }
    assert_ne!(reader.status(), start::Status::Pending);
    assert!(!matches!(reader.status(), start::Status::Started(_)), "it never records at another rate");
    assert_eq!(shared.speed.load(Ordering::Relaxed), 500, "the requested speed is left alone");
    start.cancel();
    control.retire(audio.plans, audio.actions);
}
