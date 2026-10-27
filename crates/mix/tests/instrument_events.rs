#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;
use transit_mix::{
    Arrival, Due, EngineEpoch, EventLimits, EventOutcome, InstrumentEvent, MAX_PORTS, Message, MixEngine, RenderFrame, Source, TransportEpoch,
};
use transit_model::Insert;
mod common;
use common::*;
#[path = "common/voice_ids.rs"]
mod voice_ids;
use voice_ids::*;
#[path = "common/voice_releases.rs"]
mod voice_releases;
use voice_releases::*;
#[path = "common/pedal_values.rs"]
mod pedal_values;
use pedal_values::*;

#[test]
fn a_new_document_refuses_old_slot_events_and_drops_queued_notes() {
    let mut rig = Rig::new();
    let old = rig.binding();
    let event = rig.event(port(1, 0), Due::Next, on(60));
    assert_eq!(rig.mix.queue_event(&event), EventOutcome::Queued);
    rig.publish_with_mode(&rig.session.clone(), transit_mix::CompileMode::NewDocument);
    assert_ne!(rig.binding(), old);
    assert_eq!(rig.mix.queue_event(&event), EventOutcome::StaleBinding);
    rig.render(2);
    assert!(!rig.heard_by(2).iter().any(|(_, e)| matches!(e.message, Message::NoteOn { .. })), "old queued notes cannot reach the new document");
}

#[test]
fn identity_reaches_the_plug_in_and_the_tap_sees_each_admission() {
    let mut rig = Rig::new();
    rig.mix.install_event_tap(4096);
    let start = rig.mix.next_frame().0;
    let mut sequence = 0;
    // Three ports × 16 channels, the same key and the same sender id on every one.
    for p in 0..3u16 {
        for ch in 0..16u8 {
            for (k, msg) in [on_id(60, 1), off_id(60, 1)].into_iter().enumerate() {
                sequence += 1;
                let due = start + u64::from(p) * 100 + u64::from(ch) * 3 + k as u64 * 1000;
                let ev = InstrumentEvent {
                    arrival: Arrival { host_ns: sequence * 10, sequence },
                    ..rig.event(Source { port: p, channel: ch, generation: 7 + u64::from(p) }, Due::Frame(RenderFrame(due)), msg)
                };
                assert_eq!(rig.mix.queue_event(&ev), EventOutcome::Queued);
            }
        }
    }
    rig.render(8);
    let tap = rig.mix.event_tap();
    assert_eq!(tap.len(), 96);
    for (i, r) in tap.iter().enumerate() {
        let (p, ch, k) = (i / 32, (i / 2) % 16, i % 2);
        assert_eq!((r.source.port, r.source.channel, r.source.generation), (p as u16, ch as u8, 7 + p as u64));
        assert_eq!(r.arrival, Arrival { host_ns: (i as u64 + 1) * 10, sequence: i as u64 + 1 });
        assert_eq!(r.due, Some(RenderFrame(start + p as u64 * 100 + ch as u64 * 3 + k as u64 * 1000)));
    }
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), 48);
    // Each on arrives on its source's channel at its due frame, with a voice id of its own.
    let ons: Vec<_> = heard.iter().filter(|(_, e)| matches!(e.message, Message::NoteOn { .. })).collect();
    let mut ids: Vec<i32> = ons.iter().map(|(_, e)| e.note_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 48, "48 concurrent notes on one key, 48 voice ids");
    for (f, e) in &ons {
        let rel = f - start;
        let (p, ch) = (rel / 100, (rel % 100) / 3);
        assert_eq!(u64::from(e.channel), ch, "channel at frame {f}");
        assert!(p < 3);
    }
    let frames: Vec<u64> = heard.iter().map(|(f, _)| *f).collect();
    assert!(frames.windows(2).all(|w| w[0] <= w[1]), "delivered in due order");
}

#[test]
fn a_replaced_instrument_takes_no_events_meant_for_the_old_one() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let old = rig.binding();
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(src, now, on(60)), EventOutcome::Queued);
    assert_eq!(rig.send(src, now + 5000, on(62)), EventOutcome::Queued, "a future note, still queued at the replacement");
    rig.render(1);
    // Replace the instrument (another plug-in id: a fresh instance).
    let mut s = rig.session.clone();
    s.track_mut(rig.track).unwrap().instrument = Some(Insert::new("clap:test.rec2"));
    rig.publish(&s);
    assert_ne!(rig.binding(), old);
    let ev = InstrumentEvent { target: old, ..rig.event(src, Due::Next, off(60)) };
    assert_eq!(rig.mix.queue_event(&ev), EventOutcome::StaleBinding, "a release for the old binding is refused, never retargeted");
    rig.render(30);
    assert!(rig.heard_by(2).is_empty(), "the new instance heard nothing: {:?}", rig.heard_by(2));
    let st = rig.mix.event_status();
    assert_eq!(st.refused_stale[1], 1);
    assert_eq!(st.retired_with_instance, [1, 0], "the queued note-on retired with its instance");
    assert_eq!(st.affected, Some(old));
}

#[test]
fn a_new_engine_epoch_refuses_an_aliased_binding() {
    let mut first = Rig::with_epoch(EngineEpoch(5));
    let mut second = Rig::with_epoch(EngineEpoch(6));
    assert_eq!(first.binding().2, second.binding().2, "a fresh compiler issues the same SlotGen (aliased)");
    let ev = first.event(port(1, 0), Due::Next, on(60));
    assert_eq!(second.mix.queue_event(&ev), EventOutcome::StaleEngine);
    second.render(4);
    assert!(second.heard.lock().unwrap().is_empty());
    assert_eq!(second.mix.event_status().refused_stale[0], 1);
    assert_eq!(first.mix.queue_event(&ev), EventOutcome::Queued);
}

#[test]
fn an_engine_without_an_epoch_refuses_every_event() {
    let rig = Rig::new();
    let ev = rig.event(port(1, 0), Due::Next, on(60));
    let mut eng = MixEngine::realtime(target());
    assert_eq!(eng.queue_event(&ev), EventOutcome::StaleEngine);
    assert_eq!(eng.queue_event(&InstrumentEvent { message: off(60), ..ev }), EventOutcome::StaleEngine);
}

#[test]
fn a_transport_change_refuses_and_cancels_clip_derived_events() {
    let mut rig = Rig::new();
    let (t1, t2) = (TransportEpoch(1), TransportEpoch(2));
    rig.mix.set_transport_epoch(t1);
    let now = rig.mix.next_frame().0;
    let clip = |rig: &Rig, at: u64, m: Message, t| InstrumentEvent {
        transport: Some(t),
        ..rig.event(port(Source::CLIP, 0), Due::Frame(RenderFrame(at)), m)
    };
    assert_eq!(rig.mix.queue_event(&clip(&rig, now + 1000, on(60), t1)), EventOutcome::Queued);
    assert_eq!(rig.mix.queue_event(&clip(&rig, now + 2000, off(60), t1)), EventOutcome::Queued);
    // A live note keeps going through the change.
    assert_eq!(rig.send(port(1, 0), now + 10, on(64)), EventOutcome::Queued);
    rig.mix.set_transport_epoch(t2);
    assert_eq!(rig.mix.queue_event(&clip(&rig, now + 1000, on(61), t1)), EventOutcome::StaleTransport);
    rig.render(16);
    let heard = rig.heard_by(1);
    assert_eq!(heard.len(), 1, "only the live note: {heard:?}");
    assert!(matches!(heard[0].1.message, Message::NoteOn { key: 64, .. }));
    let st = rig.mix.event_status();
    assert_eq!(st.cancelled_transport, 1);
    assert_eq!(st.refused_stale[2], 1);
}

#[test]
fn a_revoked_source_stays_revoked_until_a_newer_generation() {
    let mut rig = Rig::new();
    let (g, newer) = (Source { port: 4, channel: 0, generation: 7 }, Source { port: 4, channel: 0, generation: 8 });
    rig.mix.revoke_source(4, 7);
    let now = rig.mix.next_frame().0;
    // A note-on that was still in flight when the revocation was acknowledged.
    assert_eq!(rig.send(g, now, on(60)), EventOutcome::RevokedSource);
    assert_eq!(rig.send(Source { generation: 6, ..g }, now, on(61)), EventOutcome::RevokedSource, "older generations too");
    rig.render(2);
    assert!(rig.heard_by(1).is_empty(), "nothing from a revoked source reaches the plug-in");
    assert_eq!(rig.send(newer, now + 2 * BLOCK as u64, on(62)), EventOutcome::Queued);
    assert_eq!(rig.send(port(5, 0), now + 2 * BLOCK as u64, on(63)), EventOutcome::Queued, "other ports are untouched");
    rig.render(1);
    assert_eq!(rig.heard_by(1).len(), 2);
    assert_eq!(rig.mix.event_status().refused_revoked, 2);
}

#[test]
fn ports_beyond_the_table_are_refused_and_the_ui_port_is_not() {
    let mut rig = Rig::new();
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(port(MAX_PORTS as u16, 0), now, on(60)), EventOutcome::PortOutOfRange);
    assert_eq!(rig.send(port(0x8000, 0), now, on(60)), EventOutcome::PortOutOfRange);
    rig.mix.revoke_source(0x8000, 1);
    assert_eq!(rig.send(port(Source::UI, 0), now, on(61)), EventOutcome::Queued);
    assert_eq!(rig.send(port(MAX_PORTS as u16 - 1, 0), now, on(62)), EventOutcome::Queued);
    rig.mix.revoke_source(Source::UI, 1);
    assert_eq!(rig.send(port(Source::UI, 0), now, on(63)), EventOutcome::RevokedSource);
    assert_eq!(rig.send(Source { generation: 2, ..port(Source::UI, 0) }, now, on(63)), EventOutcome::Queued);
}

/// Review F10-4a: revoking generation MAX refuses generation MAX.
#[test]
fn revoking_the_last_generation_refuses_it() {
    let mut rig = Rig::new();
    let now = rig.mix.next_frame().0;
    let last = Source { port: 2, channel: 0, generation: u64::MAX };
    rig.mix.revoke_source(2, u64::MAX);
    assert_eq!(rig.send(last, now, on(60)), EventOutcome::RevokedSource);
    assert_eq!(rig.send(Source { generation: u64::MAX - 1, ..last }, now, on(60)), EventOutcome::RevokedSource);
    rig.mix.revoke_source(3, 5);
    assert_eq!(rig.send(Source { port: 3, generation: 5, ..last }, now, on(60)), EventOutcome::RevokedSource);
    assert_eq!(rig.send(Source { port: 3, generation: 6, ..last }, now, on(60)), EventOutcome::Queued);
}

/// Review F10-4b: an events engine with no adopted plan has no instrument to take an event.
#[test]
fn an_events_engine_without_a_plan_refuses_events() {
    let rig = Rig::new();
    let ev = rig.event(port(1, 0), Due::Next, on(60));
    let mut eng = MixEngine::realtime_with_events(target(), rig.epoch, EventLimits::default());
    assert_eq!(eng.queue_event(&ev), EventOutcome::NoInstrument);
    assert!(!eng.has_pending_events());
}

/// Review F10-4c: an instrument that fails keeps nothing pending; its queue is voided (counted)
/// and its sounding note and pedal released.
#[test]
fn a_failed_instrument_keeps_nothing_pending() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    rig.send(src, now, on(60));
    rig.send(src, now, cc(64, 127));
    rig.send(src, now + 3000, on(62));
    rig.send(src, now + 4000, off(62));
    rig.render(1);
    rig.fail.store(true, Ordering::Relaxed);
    rig.render(1);
    assert!(!rig.mix.has_pending_events(), "the failed instrument's queue is void");
    assert_eq!(rig.mix.event_status().voided, [1, 1], "the future on and its off");
    assert_eq!(rig.send(src, rig.mix.next_frame().0, on(64)), EventOutcome::NoInstrument);
    rig.fail.store(false, Ordering::Relaxed);
    rig.mix.reset();
    rig.render(1);
    let heard = rig.heard_by(1);
    assert_eq!(every_on_has_its_off(&heard), 1);
    assert_eq!(pedal_values(&heard, 64).last().map(|x| x.1), Some(0));
}

/// Review F10-4c: an instrument whose track becomes inactive keeps nothing pending either, and
/// takes no new events.
#[test]
fn an_inactive_instrument_keeps_nothing_pending() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    rig.send(src, now, on(60));
    rig.send(src, now + 3000, on(62));
    rig.render(1);
    let mut s = rig.session.clone();
    s.track_mut(rig.track).unwrap().inactive = true;
    rig.publish(&s);
    assert!(!rig.mix.has_pending_events());
    assert_eq!(rig.mix.event_status().voided, [1, 0]);
    assert_eq!(rig.send(src, rig.mix.next_frame().0, on(64)), EventOutcome::NoInstrument);
    rig.render(1);
}
