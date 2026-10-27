#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use transit_mix::{
    Adoption, Arrival, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, MAX_PEDALS, Message, MixEngine,
    NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderFrame, Shed, SlotGen, TransportEpoch,
};
mod common;
use common::*;
#[path = "common/voice_releases.rs"]
mod voice_releases;
use voice_releases::*;
#[path = "common/pedal_edges.rs"]
mod pedal_edges;
use pedal_edges::*;
#[path = "common/pedal_values.rs"]
mod pedal_values;
use pedal_values::*;

#[test]
fn pedals_from_two_ports_are_ored_and_a_revocation_releases_the_last_holder() {
    let mut rig = Rig::new();
    let (a, b) = (port(1, 0), port(2, 0));
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(a, now, cc(64, 127)), EventOutcome::Queued);
    assert_eq!(rig.send(b, now + 10, cc(64, 100)), EventOutcome::Queued);
    assert_eq!(rig.send(a, now + 20, cc(64, 0)), EventOutcome::Queued);
    rig.render(1);
    assert_eq!(pedal_values(&rig.heard_by(1), 64), vec![(now, 127), (now + 20, 100)], "B still holds the pedal: A's up leaves B's value");
    rig.mix.revoke_source(2, 1);
    rig.render(1);
    assert_eq!(pedal_values(&rig.heard_by(1), 64), vec![(now, 127), (now + 20, 100), (now + BLOCK as u64, 0)], "B's revocation lifts it");
    let st = rig.mix.event_status();
    assert_eq!(st.absorbed_pedal_edges, 1, "B's down, below A's value");
    assert!(st.revoked >= 1);
}

#[test]
fn future_pedals_follow_due_order() {
    let mut rig = Rig::new();
    let (a, b) = (port(1, 0), port(2, 0));
    let now = rig.mix.next_frame().0;
    for (src, at, v) in [(a, 0, 127), (a, 100, 0), (b, 200, 127), (b, 300, 0)] {
        assert_eq!(rig.send(src, now + at, cc(66, v)), EventOutcome::Queued);
    }
    rig.render(2);
    assert_eq!(pedal_edges(&rig.heard_by(1), 66), vec![(now, true), (now + 100, false), (now + 200, true), (now + 300, false)]);
}

#[test]
fn the_pedal_limit_refuses_untracked_downs_and_resets_release_everything_at_a_full_queue() {
    let mut rig = Rig::new();
    let now = rig.mix.next_frame().0;
    // 32 pedal entries (sources × channels × kinds), then the 33rd.
    let mut n = 0;
    'fill: for p in 10..30u16 {
        for pedal in [64, 66, 69] {
            if n == MAX_PEDALS {
                break 'fill;
            }
            assert_eq!(rig.send(port(p, 3), now, cc(pedal, 127)), EventOutcome::Queued);
            n += 1;
        }
    }
    assert_eq!(rig.send(port(30, 3), now, cc(64, 127)), EventOutcome::Refused(Shed::PedalsFull));
    rig.render(1);
    let ch3: usize = rig.heard_by(1).iter().filter(|(_, e)| e.channel == 3).count();
    assert_eq!(ch3, 3, "one down per pedal kind reached the plug-in; the 33rd was never delivered");
    rig.mix.revoke_source(30, 1);
    for p in 10..30u16 {
        rig.mix.revoke_source(p, 1);
    }
    rig.render(1);

    // One source holds three pedals and notes on one channel; the queue is full of future
    // controllers; then CC 121 and CC 123.
    let src = port(40, 0);
    let now = rig.mix.next_frame().0;
    for pedal in [64, 66, 69] {
        assert_eq!(rig.send(src, now, cc(pedal, 127)), EventOutcome::Queued);
    }
    for k in 0..20 {
        assert_eq!(rig.send(src, now, on(k)), EventOutcome::Queued);
    }
    let mut filled = 0;
    while rig.send(src, now + 5000 + filled, cc(1, 1)) == EventOutcome::Queued {
        filled += 1;
    }
    assert!(filled > 200, "the non-release budget is taken: {filled}");
    assert_eq!(rig.send(src, now + 1, cc(121, 0)), EventOutcome::Queued);
    assert_eq!(rig.send(src, now + 2, cc(123, 0)), EventOutcome::Queued);
    rig.render(30);
    let heard = rig.heard_by(1);
    let mine: Vec<_> = heard.iter().filter(|(f, e)| *f >= now && e.channel == 0).copied().collect();
    assert_eq!(every_on_has_its_off(&mine), 20, "CC 123 released every note");
    for pedal in [64, 66, 69] {
        assert_eq!(pedal_edges(&mine, pedal), vec![(now, true), (now + 1, false)], "CC 121 lifted pedal {pedal}");
    }
    assert_eq!(rig.mix.event_status().dropped_releases, 0);
}

/// Review F10-3: one holder's half-pedal values pass through exactly.
#[test]
fn half_pedal_values_pass_through() {
    let mut rig = Rig::new();
    let now = rig.mix.next_frame().0;
    for (at, v) in [(0, 127), (10, 90), (20, 70), (30, 0)] {
        assert_eq!(rig.send(port(1, 0), now + at, cc(64, v)), EventOutcome::Queued);
    }
    rig.render(1);
    assert_eq!(pedal_values(&rig.heard_by(1), 64), vec![(now, 127), (now + 10, 90), (now + 20, 70), (now + 30, 0)]);
    assert!(!rig.mix.has_pending_events());
}

/// A source resting below 64 is not a holder: A100/B60/A0/B0 emits only 100/0.
#[test]
fn a_low_resting_source_does_not_hold_the_pedal() {
    let mut rig = Rig::new();
    let (a, b) = (port(1, 0), port(2, 0));
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(a, now, cc(64, 100)), EventOutcome::Queued);
    assert_eq!(rig.send(b, now + 10, cc(64, 60)), EventOutcome::Unmatched);
    assert_eq!(rig.send(a, now + 20, cc(64, 0)), EventOutcome::Queued);
    assert_eq!(rig.send(b, now + 30, cc(64, 0)), EventOutcome::Unmatched);
    rig.render(1);
    assert_eq!(pedal_values(&rig.heard_by(1), 64), vec![(now, 100), (now + 20, 0)]);
    assert!(!rig.mix.has_pending_events());
}

/// CC 121 cannot be undone by re-sending a pedal down: suppress it while another source holds.
#[test]
fn reset_controllers_preserves_other_sources_pedals_without_a_glitch() {
    let mut rig = Rig::new();
    let (a, b) = (port(1, 0), port(2, 0));
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(b, now, cc(64, 100)), EventOutcome::Queued);
    assert_eq!(rig.send(a, now + 10, cc(121, 0)), EventOutcome::Queued);
    rig.render(1);
    let ctl: Vec<(u64, u8, u8)> = rig
        .heard_by(1)
        .iter()
        .filter_map(|(f, e)| if let Message::Control { cc, value } = e.message { Some((*f, cc, value)) } else { None })
        .collect();
    assert_eq!(ctl, vec![(now, 64, 100)]);
}
fn synth_energy_after_seek(seek: bool, pedal: u8, clip_pedal: bool) -> f32 {
    let (mut session, track) = instrument_session("subtractive_synth");
    if clip_pedal {
        let mut seq = transit_midi::Sequence::default();
        for (tick, value) in [(0, 110), (100, 63)] {
            seq.ctrls.push(transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::Cc(64), value });
        }
        let cid = session.new_clip_id();
        session.track_mut(track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "pedal", 0, 48_000 * 8, seq));
    }
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
    let mut out = vec![vec![0.0; BLOCK]; 2];
    mix.set_transport_epoch(TransportEpoch(1));
    mix.render_active(0, BLOCK, &mut out);
    let mut send = |at, message| {
        assert!(
            mix.queue_event(&InstrumentEvent {
                engine: EngineEpoch::FIRST,
                target: binding,
                due: Due::Frame(RenderFrame(at)),
                transport: None,
                source: port(1, 0),
                message,
                arrival: Arrival { host_ns: 0, sequence: 0 },
            })
            .accepted()
        );
    };
    send(BLOCK as u64, on(60));
    send(BLOCK as u64 + 10, cc(pedal, 127));
    send(BLOCK as u64 + 128, off(60));
    mix.render_active(BLOCK as i64, BLOCK, &mut out);
    if seek {
        mix.set_transport_epoch(TransportEpoch(2));
    }
    let start = if seek { 100_000 } else { (2 * BLOCK) as i64 };
    for i in 0..12 {
        mix.render_active(start + i * BLOCK as i64, BLOCK, &mut out);
    }
    out.iter().flatten().map(|x| x * x).sum::<f32>()
}

#[test]
fn seek_keeps_live_pedal_sustained_audio() {
    for pedal in [64, 66] {
        let normal = synth_energy_after_seek(false, pedal, false);
        let seek = synth_energy_after_seek(true, pedal, false);
        println!("CC{pedal}: normal energy {normal}, sought {seek}");
        assert!(normal > 0.01, "normal CC{pedal}: {normal}");
        assert!(seek > normal * 0.9, "CC{pedal}: normal energy {normal}, after seek {seek}");
    }
}

#[test]
fn seek_keeps_live_audio_when_a_clip_also_holds_the_pedal() {
    let normal = synth_energy_after_seek(false, 64, true);
    let seek = synth_energy_after_seek(true, 64, true);
    println!("Shared live/clip sustain: normal energy {normal}, sought {seek}");
    assert!(normal > 0.01);
    assert!(seek > normal * 0.9, "normal energy {normal}, after seek {seek}");
}

#[test]
fn nonzero_pedal_ups_use_the_release_reserve() {
    for pedal in [64, 66, 67, 69] {
        for up in [1, 63] {
            let mut rig = Rig::new();
            let src = port(1, 0);
            let down_at = rig.mix.next_frame().0;
            assert_eq!(rig.send(src, down_at, cc(pedal, 127)), EventOutcome::Queued);
            rig.render(1);
            let now = rig.mix.next_frame().0;
            for k in 0..256 {
                assert_eq!(rig.send(src, now + 1000 + k, cc(1, 1)), EventOutcome::Queued);
            }
            assert_eq!(rig.send(src, now, cc(pedal, up)), EventOutcome::Queued, "CC{pedal}={up}");
            rig.render(1);
            assert_eq!(pedal_values(&rig.heard_by(1), pedal), vec![(down_at, 127), (now, up)]);
            assert_eq!(rig.mix.event_status().dropped_releases, 0);
            rig.render(5);
            assert!(!rig.mix.has_pending_events(), "an up must free its holder");
        }
    }
}

#[test]
fn nonzero_pedal_ups_survive_later_transport_changes() {
    for pedal in [64, 66, 67, 69] {
        let mut rig = Rig::new();
        let src = port(1, 0);
        rig.mix.set_transport_epoch(TransportEpoch(1));
        let down_at = rig.mix.next_frame().0;
        rig.send(src, down_at, cc(pedal, 127));
        rig.render(1);
        let now = rig.mix.next_frame().0;
        let event = InstrumentEvent { transport: Some(TransportEpoch(1)), ..rig.event(src, Due::Frame(RenderFrame(now + 20)), cc(pedal, 63)) };
        assert_eq!(rig.mix.queue_event(&event), EventOutcome::Queued);
        rig.mix.set_transport_epoch(TransportEpoch(2));
        rig.mix.set_transport_epoch(TransportEpoch(3));
        rig.render(1);
        assert_eq!(pedal_values(&rig.heard_by(1), pedal), vec![(down_at, 127), (now + 20, 63)]);
        assert_eq!(rig.mix.event_status().cancelled_transport, 0);
        assert!(!rig.mix.has_pending_events());
    }
}

#[test]
fn resting_low_values_cannot_exhaust_pedal_holders() {
    for pedal in [64, 66, 67, 69] {
        let mut rig = Rig::new();
        let now = rig.mix.next_frame().0;
        for value in 1..64 {
            assert_eq!(rig.send(port(value as u16, 0), now, cc(pedal, value)), EventOutcome::Unmatched);
        }
        rig.render(1);
        assert!(!rig.mix.has_pending_events());
        let now = rig.mix.next_frame().0;
        for source in 0..MAX_PEDALS {
            assert_eq!(rig.send(port(source as u16, 0), now, cc(pedal, 64)), EventOutcome::Queued);
        }
        assert_eq!(rig.send(port(MAX_PEDALS as u16, 0), now, cc(pedal, 64)), EventOutcome::Refused(Shed::PedalsFull));
    }
}

#[test]
fn down_sources_give_their_maximum_then_the_last_sources_exact_up() {
    for pedal in [64, 66, 67, 69] {
        let mut rig = Rig::new();
        let (a, b) = (port(1, 0), port(2, 0));
        let now = rig.mix.next_frame().0;
        for (src, at, value) in [(a, 0, 100), (b, 10, 80), (a, 20, 70), (b, 30, 63), (a, 40, 1)] {
            assert_eq!(rig.send(src, now + at, cc(pedal, value)), EventOutcome::Queued);
        }
        rig.render(1);
        assert_eq!(pedal_values(&rig.heard_by(1), pedal), vec![(now, 100), (now + 20, 80), (now + 30, 70), (now + 40, 1)]);
        assert!(!rig.mix.has_pending_events());
    }
}

#[test]
fn seek_releases_clip_pedals_to_live_values_without_resetting_the_channel() {
    for pedal in [64, 66, 67, 69] {
        let mut rig = Rig::new();
        let mut s = rig.session.clone();
        let mut seq = transit_midi::Sequence::default();
        seq.notes.push(transit_midi::Note { pitch: 48, velocity: 100, release_velocity: 0, channel: 0, start: 0, length: 9600 });
        for (tick, value) in [(0, 127), (100, 63)] {
            seq.ctrls.push(transit_midi::CtrlEvent { tick, channel: 0, kind: transit_midi::CtrlKind::Cc(pedal), value });
        }
        let cid = s.new_clip_id();
        s.track_mut(rig.track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "pedal", 0, 48_000 * 8, seq));
        rig.publish(&s);
        rig.pos = 0;
        let now = rig.mix.next_frame().0;
        rig.send(port(1, 0), now, cc(pedal, 90));
        rig.render(1);
        rig.heard.lock().unwrap().clear();
        rig.mix.set_transport_epoch(TransportEpoch(2));
        rig.pos = 100_000;
        let seek_at = rig.mix.next_frame().0;
        rig.render(1);
        let heard = rig.heard_by(1);
        assert_eq!(pedal_values(&heard, pedal), vec![(seek_at, 90)]);
        assert!(!heard.iter().any(|(_, e)| matches!(e.message, Message::Control { cc: 121, .. })));
        assert!(heard.iter().any(|(_, e)| matches!(e.message, Message::NoteOff { key: 48, .. })));
        rig.mix.revoke_source(1, 1);
        rig.render(1);
        assert_eq!(pedal_values(&rig.heard_by(1), pedal).last().unwrap().1, 0);
    }
}
