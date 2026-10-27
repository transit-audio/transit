#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use transit_mix::{
    Adoption, Arrival, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, Message, MixEngine, NewInstance,
    PlanCompiler, PlanGeneration, PlanLimits, RenderFrame, SlotGen,
};
use transit_rt::testing::{AllocStats, measure};
mod common;
use common::*;
#[path = "common/voice_releases.rs"]
mod voice_releases;
use voice_releases::*;
#[path = "common/pedal_values.rs"]
mod pedal_values;
use pedal_values::*;

fn add_event_clip(rig: &mut Rig, start: i64, seq: transit_midi::Sequence) {
    let mut s = rig.session.clone();
    let cid = s.new_clip_id();
    s.track_mut(rig.track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "timing", start, 48_000, seq));
    rig.publish(&s);
    rig.session = s;
}

#[test]
fn clip_pairs_in_time_order_release_within_and_across_blocks() {
    let mut rig = Rig::new();
    let mut seq = transit_midi::Sequence::default();
    // Deliberately unordered, all on the same key. The last pair spans a block boundary.
    for (start, length) in [(8, 1), (1, 1), (10, 2), (4, 1)] {
        seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 0, channel: 0, start, length });
    }
    add_event_clip(&mut rig, 0, seq);
    rig.pos = 0;
    let now = rig.mix.next_frame().0;
    rig.render(1);
    let notes: Vec<_> = rig.heard_by(1).into_iter().filter(|(_, e)| matches!(e.message, Message::NoteOn { .. } | Message::NoteOff { .. })).collect();
    assert_eq!(notes.iter().map(|(f, _)| f - now).collect::<Vec<_>>(), [25, 50, 100, 125, 200, 225, 250]);
    assert_eq!(every_on_has_its_off(&notes[..6]), 3);
    assert!(rig.mix.has_pending_events(), "only the cross-block note remains");
    rig.render(1);
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), 4);
    assert_eq!(heard.last().unwrap().0, now + 300);
    assert!(!rig.mix.has_pending_events());
}

#[test]
fn clip_same_offset_off_precedes_restrike_across_clips() {
    let mut rig = Rig::new();
    // Put the later note in the first clip to catch ordering that only sorts within a clip.
    for (start, length) in [(4, 2), (1, 3)] {
        let mut seq = transit_midi::Sequence::default();
        seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 0, channel: 0, start, length });
        add_event_clip(&mut rig, 0, seq);
    }
    rig.pos = 0;
    let now = rig.mix.next_frame().0;
    rig.render(1);
    let notes: Vec<_> = rig.heard_by(1).into_iter().filter(|(_, e)| matches!(e.message, Message::NoteOn { .. } | Message::NoteOff { .. })).collect();
    assert_eq!(notes.iter().map(|(f, _)| f - now).collect::<Vec<_>>(), [25, 100, 100, 150]);
    assert!(matches!(notes[1].1.message, Message::NoteOff { .. }));
    assert!(matches!(notes[2].1.message, Message::NoteOn { .. }));
    assert_ne!(notes[0].1.note_id, notes[2].1.note_id);
    assert_eq!(every_on_has_its_off(&notes), 2);
    assert!(!rig.mix.has_pending_events());
}

#[test]
fn clip_live_pedals_merge_in_both_time_orders() {
    for pedal in [64, 66, 67, 69] {
        for live_at in [10, 88, 140] {
            let mut rig = Rig::new();
            let src = port(1, 0);
            let now = rig.mix.next_frame().0;
            assert_eq!(rig.send(src, now, cc(pedal, 127)), EventOutcome::Queued);
            rig.render(1);
            let mut seq = transit_midi::Sequence::default();
            for (tick, value) in [(4, 127), (8, 63)] {
                seq.ctrls.push(transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::Cc(pedal), value });
            }
            let start = rig.pos;
            add_event_clip(&mut rig, start, seq);
            rig.heard.lock().unwrap().clear();
            let now = rig.mix.next_frame().0;
            assert_eq!(rig.send(src, now + live_at, cc(pedal, 63)), EventOutcome::Queued);
            rig.render(1);
            let expected = if live_at == 10 { vec![(now + 10, 63), (now + 88, 127), (now + 188, 63)] } else { vec![(now + 188, 63)] };
            assert_eq!(pedal_values(&rig.heard_by(1), pedal), expected, "CC{pedal}, live offset: {live_at}");
            assert!(!rig.mix.has_pending_events());
        }
    }
}

#[test]
fn clip_live_pedal_or_tracks_each_sources_value_through_the_block() {
    for pedal in [64, 66, 67, 69] {
        let mut rig = Rig::new();
        let (a, b) = (port(1, 0), port(2, 0));
        let now = rig.mix.next_frame().0;
        assert_eq!(rig.send(a, now, cc(pedal, 100)), EventOutcome::Queued);
        assert_eq!(rig.send(b, now, cc(pedal, 80)), EventOutcome::Queued);
        rig.render(1);
        let mut seq = transit_midi::Sequence::default();
        // Reverse storage order: delivery must still be down@88, up@188.
        for (tick, value) in [(8, 1), (4, 90)] {
            seq.ctrls.push(transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::Cc(pedal), value });
        }
        let start = rig.pos;
        add_event_clip(&mut rig, start, seq);
        rig.heard.lock().unwrap().clear();
        let now = rig.mix.next_frame().0;
        for (src, at, value) in [(a, 10, 63), (b, 120, 70), (a, 150, 110), (a, 200, 1), (b, 220, 63)] {
            assert_eq!(rig.send(src, now + at, cc(pedal, value)), EventOutcome::Queued);
        }
        rig.render(1);
        assert_eq!(pedal_values(&rig.heard_by(1), pedal), vec![(now + 10, 80), (now + 88, 90), (now + 150, 110), (now + 200, 70), (now + 220, 63)]);
        assert!(!rig.mix.has_pending_events());
    }
}

#[test]
fn dense_clip_events_keep_every_release_and_same_sample_order() {
    let mut rig = Rig::new();
    let mut seq = transit_midi::Sequence::default();
    // More events than the scheduler's scratch, including more than a buffer at one sample.
    for start in [8, 1, 4] {
        for _ in 0..300 {
            seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 0, channel: 0, start, length: 1 });
        }
    }
    add_event_clip(&mut rig, 0, seq);
    rig.pos = 0;
    rig.render(1);
    assert_eq!(every_on_has_its_off(&rig.heard_by(1)), 900);
    assert!(!rig.mix.has_pending_events());
    assert_eq!(rig.mix.event_status().shed.note_on, 0);
}

fn dense_clip_pedal_trial(transitions: usize) -> (transit_mix::EventStatus, bool, f32) {
    let (mut session, track) = instrument_session("subtractive_synth");
    let mut seq = transit_midi::Sequence::default();
    for i in 0..transitions {
        seq.ctrls.push(transit_midi::CtrlEvent {
            tick: 22,
            channel: 0,
            kind: transit_midi::CtrlKind::Cc(64),
            value: if i % 2 == 0 { 127 } else { 0 },
        });
    }
    let cid = session.new_clip_id();
    session.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "dense pedals", 0, 48_000, seq));
    let mut mix = MixEngine::realtime_with_events(target(), EngineEpoch::FIRST, EventLimits::default());
    let mut comp = PlanCompiler::new(target(), PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| {
        let mut p = transit_dsp::create(&spec.id)?;
        p.prepare(SR as f32, BLOCK, spec.ch);
        p.set_param("amp_release", 1.0);
        Some(NewInstance { plugin: p, refused_state: false })
    };
    let mut env = comp.compile(Arc::new(session), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    let binding = comp.active_bindings().into_iter().find(|b| b.0 == track && b.1 == INSTRUMENT_SLOT).unwrap();
    let ev = |at, message| InstrumentEvent {
        engine: EngineEpoch::FIRST,
        target: binding,
        due: Due::Frame(RenderFrame(at)),
        transport: None,
        source: port(1, 0),
        message,
        arrival: Arrival { host_ns: 0, sequence: 0 },
    };
    let mut out = vec![vec![0.0; BLOCK]; 2];
    mix.render_active(0, BLOCK, &mut out);
    assert!(mix.queue_event(&ev(256, on(60))).accepted());
    mix.render_active(256, BLOCK, &mut out);
    assert!(mix.queue_event(&ev(612, off(60))).accepted());
    let ((), stats) = measure(|| {
        mix.render_active(512, BLOCK, &mut out);
        for i in 3..16 {
            mix.render_active(i * BLOCK as i64, BLOCK, &mut out);
        }
    });
    assert_eq!(stats, AllocStats::default(), "dense pedal traffic must not allocate");
    let energy: f32 = out.iter().flatten().map(|x| x * x).sum();
    println!("dense pedal clip: status={:?}; pending={}; energy={energy}", mix.event_status(), mix.has_pending_events());
    (mix.event_status(), mix.has_pending_events(), energy)
}

#[test]
fn refute_dense_clip_pedals_cannot_consume_a_live_notes_release_reserve() {
    let (control, pending, energy) = dense_clip_pedal_trial(1000);
    assert_eq!(control.host_refused[1], 0);
    assert!(!pending);
    assert!(energy < 0.000001, "control note must be silent: {energy}");
    let (st, pending, energy) = dense_clip_pedal_trial(1400);
    assert!(!pending);
    assert_eq!(st.host_refused[1], 0, "the conforming built-in must receive the live note release; energy={energy}");
    assert!(energy < 0.000001, "released note must be silent: {energy}");
}

// #94: a pedal a clip leaves held is released at the clip's end.

fn pedal_seq(events: &[(i64, i32)]) -> transit_midi::Sequence {
    let mut seq = transit_midi::Sequence::default();
    for &(tick, value) in events {
        seq.ctrls.push(transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::Cc(64), value });
    }
    seq
}

/// Adds a MIDI clip `length` samples long at `start`; a loop range makes it a looped clip.
fn add_pedal_clip(rig: &mut Rig, start: i64, length: i64, looping: Option<(i64, i64)>, seq: transit_midi::Sequence) {
    let mut s = rig.session.clone();
    let cid = s.new_clip_id();
    let mut clip = transit_model::Clip::midi(cid, "pedal", start, length, seq);
    clip.set_loop(looping.map(|(start, end)| transit_model::ClipLoop { start, end }));
    s.track_mut(rig.track).unwrap().playlist_mut().unwrap().clips.push(clip);
    rig.publish(&s);
    rig.session = s;
}

/// Plays `blocks` blocks from the current position; returns the first frame played and CC64 heard,
/// as offsets from that frame.
fn play_pedal(rig: &mut Rig, blocks: usize) -> Vec<(u64, u8)> {
    rig.heard.lock().unwrap().clear();
    let now = rig.mix.next_frame().0;
    rig.render(blocks);
    pedal_values(&rig.heard_by(1), 64).into_iter().map(|(f, v)| (f - now, v)).collect()
}

#[test]
fn a_clip_that_ends_with_the_pedal_down_releases_it_once_at_its_end() {
    let mut rig = Rig::new();
    rig.pos = 0;
    // Down at tick 4 (sample 100), no up. The clip ends at sample 2600, inside block 10.
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    assert_eq!(play_pedal(&mut rig, 14), vec![(100, 127), (2600, 0)]);
    assert!(!rig.mix.has_pending_events());
}

#[test]
fn a_clip_whose_pedal_comes_up_inside_it_sends_no_extra_release() {
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127), (8, 0)]));
    assert_eq!(play_pedal(&mut rig, 14), vec![(100, 127), (200, 0)]);
    // A clip that never touches the pedal sends nothing either.
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[]));
    assert_eq!(play_pedal(&mut rig, 14), vec![]);
}

#[test]
fn overlapping_clips_release_the_pedal_when_the_clip_that_wrote_it_last_ends() {
    // A (0..2600) presses at 100; B (1000..5000) presses at 1100 and never lets go: only B's end releases.
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    add_pedal_clip(&mut rig, 1000, 4000, None, pedal_seq(&[(4, 127)]));
    assert_eq!(play_pedal(&mut rig, 24), vec![(100, 127), (5000, 0)]);
    // B lifts the pedal at 1500, after A's press: A's end at 2600 must not send a second release.
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    add_pedal_clip(&mut rig, 1000, 4000, None, pedal_seq(&[(20, 0)]));
    assert_eq!(play_pedal(&mut rig, 24), vec![(100, 127), (1500, 0)]);
    // Two clips ending at the same sample, both pressed: one release.
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    add_pedal_clip(&mut rig, 500, 2100, None, pedal_seq(&[(4, 127)]));
    assert_eq!(play_pedal(&mut rig, 14), vec![(100, 127), (2600, 0)]);
}

#[test]
fn a_looped_clip_releases_a_pedal_its_last_cycle_leaves_held() {
    let mut rig = Rig::new();
    rig.pos = 0;
    // Loop 0..2400 (96 ticks): down at tick 4, up at tick 60, every cycle. The clip ends at 10000,
    // 400 samples into the fifth cycle, after its press and before its lift.
    add_pedal_clip(&mut rig, 0, 10_000, Some((0, 2400)), pedal_seq(&[(4, 127), (60, 0)]));
    let want = vec![(100, 127), (1500, 0), (2500, 127), (3900, 0), (4900, 127), (6300, 0), (7300, 127), (8700, 0), (9700, 127), (10_000, 0)];
    assert_eq!(play_pedal(&mut rig, 42), want);
}

#[test]
fn a_pedal_held_by_live_input_is_not_released_by_a_clip_end() {
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(port(1, 0), now + 10, cc(64, 127)), EventOutcome::Queued);
    // The live press goes out first; the clip's press and its release at 2600 change nothing the
    // instrument can hear while the live source holds the pedal.
    assert_eq!(play_pedal(&mut rig, 14), vec![(10, 127)]);
    rig.mix.revoke_source(1, 1);
    rig.render(1);
    assert_eq!(pedal_values(&rig.heard_by(1), 64).last().map(|(_, v)| *v), Some(0));
}

/// CC64 as the plug-in hears it, as (session sample, value), over `blocks` blocks from `rig.pos`.
fn pedal_at_session_samples(rig: &mut Rig, blocks: usize) -> Vec<(i64, u8)> {
    rig.heard.lock().unwrap().clear();
    let (frame0, pos0) = (rig.mix.next_frame().0 as i64, rig.pos);
    rig.render(blocks);
    pedal_values(&rig.heard_by(1), 64).into_iter().map(|(f, v)| (f as i64 - frame0 + pos0, v)).collect()
}

#[test]
fn a_locate_past_a_clip_that_left_the_pedal_down_does_not_press_it() {
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    rig.mix.reset();
    rig.pos = 5120;
    let got = pedal_at_session_samples(&mut rig, 8);
    assert!(got.iter().all(|(_, v)| *v < 64), "the chase pressed a pedal whose clip had ended: {got:?}");
    // Both orders of last writer: a later clip that ended holding wins over an earlier one.
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    add_pedal_clip(&mut rig, 1000, 1000, None, pedal_seq(&[(4, 0)]));
    rig.mix.reset();
    rig.pos = 5120;
    assert!(pedal_at_session_samples(&mut rig, 8).iter().all(|(_, v)| *v < 64));
}

#[test]
fn a_locate_into_a_clip_with_the_pedal_down_presses_it_and_releases_it_once_at_the_clip_end() {
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    rig.render(2);
    rig.mix.reset();
    rig.pos = 1024;
    let got = pedal_at_session_samples(&mut rig, 10);
    // The locate's own release, the chase's press, then exactly one release, at the clip end.
    assert_eq!(got, vec![(1024, 0), (1024, 127), (2600, 0)]);
}

#[test]
fn the_release_at_a_clip_end_precedes_the_press_of_a_clip_that_starts_there() {
    let mut rig = Rig::new();
    rig.pos = 0;
    // B starts on a tick (sample 2600 is tick 104), so its press at its tick 0 is at 2600 too.
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    add_pedal_clip(&mut rig, 2600, 2400, None, pedal_seq(&[(0, 127), (40, 0)]));
    assert_eq!(pedal_at_session_samples(&mut rig, 24), vec![(100, 127), (2600, 0), (2600, 127), (3600, 0)]);
}

#[test]
fn the_release_precedes_the_next_clips_press_whatever_the_clip_order_in_the_track() {
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 2600, 2400, None, pedal_seq(&[(0, 127), (40, 0)]));
    add_pedal_clip(&mut rig, 0, 2600, None, pedal_seq(&[(4, 127)]));
    assert_eq!(pedal_at_session_samples(&mut rig, 24), vec![(100, 127), (2600, 0), (2600, 127), (3600, 0)]);
}

#[test]
fn an_off_tick_clip_starting_at_the_end_of_a_clip_keeps_the_pedal_down() {
    let mut rig = Rig::new();
    rig.pos = 0;
    // B starts at 2560 (tick 102.4), so its events sit on the grid at 2550, before A's end: B wrote
    // the pedal last and A's end must not lift it. B's lift is at its tick 40.
    add_pedal_clip(&mut rig, 0, 2560, None, pedal_seq(&[(4, 127)]));
    add_pedal_clip(&mut rig, 2560, 2400, None, pedal_seq(&[(0, 127), (40, 0)]));
    assert_eq!(pedal_at_session_samples(&mut rig, 24), vec![(100, 127), (3550, 0)]);
}

// #95: a clip whose tick grid starts before its start sample.

/// A clip with a note-on and a CC64 press at tick 0.
fn tick0_seq() -> transit_midi::Sequence {
    let mut seq = pedal_seq(&[(0, 127)]);
    seq.notes.push(transit_midi::Note { pitch: 60, velocity: 100, release_velocity: 0, channel: 0, start: 0, length: 40 });
    seq
}

/// (session sample, event) of every note-on and CC64 the plug-in heard over `blocks` blocks.
fn tick0_heard(rig: &mut Rig, blocks: usize) -> Vec<(i64, String)> {
    rig.heard.lock().unwrap().clear();
    let (frame0, pos0) = (rig.mix.next_frame().0 as i64, rig.pos);
    rig.render(blocks);
    let heard = rig.heard.lock().unwrap();
    heard
        .iter()
        .filter_map(|(_, f, e)| {
            let at = *f as i64 - frame0 + pos0;
            match (*e)?.message {
                Message::NoteOn { key, .. } => Some((at, format!("on {key}"))),
                Message::Control { cc: 64, value } if value >= 64 => Some((at, format!("cc64 {value}"))),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn tick_zero_events_of_an_off_tick_clip_on_a_block_boundary_are_heard_once() {
    // 2560 is tick 102.4: the clip's grid starts at 2550, in the block before the clip start.
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 2560, 2400, None, tick0_seq());
    assert_eq!(tick0_heard(&mut rig, 24), vec![(2550, "on 60".to_string()), (2550, "cc64 127".to_string())]);
}

#[test]
fn tick_zero_events_of_an_off_tick_clip_in_the_middle_of_a_block_are_heard_once() {
    let mut rig = Rig::new();
    rig.pos = 0;
    // 2600 is tick 104 exactly; 2610 is tick 104.4, grid 2600, inside block 10 (2560..2816).
    add_pedal_clip(&mut rig, 2610, 2400, None, tick0_seq());
    assert_eq!(tick0_heard(&mut rig, 24), vec![(2600, "on 60".to_string()), (2600, "cc64 127".to_string())]);
}

#[test]
fn a_locate_onto_an_off_tick_clip_start_presses_the_pedal_once() {
    let mut rig = Rig::new();
    rig.pos = 0;
    add_pedal_clip(&mut rig, 2560, 2400, None, tick0_seq());
    rig.mix.reset();
    rig.pos = 2560;
    let got = tick0_heard(&mut rig, 12);
    assert_eq!(got.iter().filter(|(_, e)| e.starts_with("cc64")).count(), 1, "{got:?}");
    assert!(got.iter().filter(|(_, e)| e.starts_with("on")).count() <= 1, "{got:?}");
}
