#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;
use transit_mix::{Message, NoteId, PluginEvent};
use transit_rt::testing::{AllocStats, measure};
mod common;
use common::*;

#[test]
fn refused_note_release_stays_pending_until_accepted() {
    let mut rig = Rig::new();
    let src = port(1, 3);
    let now = rig.mix.next_frame().0;
    assert!(rig.send(src, now, on(60)).accepted());
    rig.render(1);
    let voice = rig.heard_by(1).last().unwrap().1.note_id;
    let off = Message::NoteOff { key: 60, velocity: 47, id: NoteId::NONE };
    let now = rig.mix.next_frame().0;
    assert!(rig.send(src, now + 100, off).accepted());
    rig.refuse_releases.store(true, Ordering::Relaxed);
    for _ in 0..3 {
        let ((), stats) = measure(|| rig.render(1));
        assert_eq!(stats, AllocStats::default(), "refused release retries must not allocate");
        assert!(rig.mix.has_pending_events(), "a refused off must keep its identity and wake reservation");
    }
    assert_eq!(rig.mix.event_status().host_refused[1], 3);
    assert_eq!(rig.heard_by(1).len(), 1);
    rig.refuse_releases.store(false, Ordering::Relaxed);
    let retry_at = rig.mix.next_frame().0;
    let ((), stats) = measure(|| rig.render(1));
    assert_eq!(stats, AllocStats::default(), "accepting a retry must not allocate");
    assert_eq!(rig.heard_by(1).last(), Some(&(retry_at, PluginEvent { channel: 3, note_id: voice, message: off })));
    assert!(!rig.mix.has_pending_events());
    rig.render(1);
    assert_eq!(rig.heard_by(1).len(), 2, "accepted releases are never retried");
}

#[test]
fn refused_pedal_down_never_becomes_a_holder() {
    for pedal in [64, 66, 67, 69] {
        let mut rig = Rig::new();
        let src = port(1, 0);
        let now = rig.mix.next_frame().0;
        rig.refuse_downs.store(true, Ordering::Relaxed);
        assert!(rig.send(src, now + 10, cc(pedal, 127)).accepted());
        assert!(rig.send(src, now + 20, cc(pedal, 63)).accepted());
        rig.render(1);
        assert!(rig.heard_by(1).is_empty(), "a refused down needs no host up");
        assert!(!rig.mix.has_pending_events());
        rig.refuse_downs.store(false, Ordering::Relaxed);
        let now = rig.mix.next_frame().0;
        assert!(rig.send(src, now, cc(pedal, 127)).accepted());
        rig.render(1);
        assert_eq!(rig.heard_by(1).last().unwrap().1.message, cc(pedal, 127), "the refused down must not update sent");
        assert!(rig.send(src, rig.mix.next_frame().0, cc(pedal, 0)).accepted());
        rig.render(1);
        assert!(!rig.mix.has_pending_events());
    }
}

#[test]
fn refused_pedal_release_stays_pending_until_accepted() {
    for pedal in [64, 66, 67, 69] {
        let mut rig = Rig::new();
        let src = port(1, 2);
        let now = rig.mix.next_frame().0;
        assert!(rig.send(src, now, cc(pedal, 127)).accepted());
        rig.render(1);
        rig.refuse_releases.store(true, Ordering::Relaxed);
        let now = rig.mix.next_frame().0;
        assert!(rig.send(src, now + 10, cc(pedal, 63)).accepted());
        for _ in 0..3 {
            rig.render(1);
            assert!(rig.mix.has_pending_events());
        }
        assert_eq!(rig.mix.event_status().host_refused[1], 3);
        assert_eq!(rig.heard_by(1).len(), 1);
        rig.refuse_releases.store(false, Ordering::Relaxed);
        let retry_at = rig.mix.next_frame().0;
        rig.render(1);
        assert_eq!(rig.heard_by(1).last(), Some(&(retry_at, PluginEvent { channel: 2, note_id: PluginEvent::NO_ID, message: cc(pedal, 63) })));
        assert!(!rig.mix.has_pending_events());
        rig.render(1);
        assert_eq!(rig.heard_by(1).len(), 2);
    }
}

#[test]
fn refused_restrike_off_keeps_the_old_voice_for_retry() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    assert!(rig.send(src, now, on(60)).accepted());
    rig.render(1);
    let voice = rig.heard_by(1)[0].1.note_id;
    rig.refuse_releases.store(true, Ordering::Relaxed);
    let now = rig.mix.next_frame().0;
    assert!(rig.send(src, now + 10, on(60)).accepted());
    rig.render(1);
    assert_eq!(rig.heard_by(1).len(), 1, "a refused old off must not start an untracked new voice");
    assert!(rig.mix.has_pending_events());
    rig.refuse_releases.store(false, Ordering::Relaxed);
    rig.render(1);
    assert_eq!(rig.heard_by(1).last().unwrap().1.note_id, voice);
    assert!(matches!(rig.heard_by(1).last().unwrap().1.message, Message::NoteOff { .. }));
    assert!(!rig.mix.has_pending_events());
}

#[test]
fn reset_and_revocation_keep_refused_releases_pending() {
    for reset in [false, true] {
        let mut rig = Rig::new();
        let src = port(1, 0);
        let now = rig.mix.next_frame().0;
        assert!(rig.send(src, now, on(60)).accepted());
        assert!(rig.send(src, now, cc(64, 127)).accepted());
        rig.render(1);
        rig.refuse_releases.store(true, Ordering::Relaxed);
        if reset {
            rig.mix.reset();
        } else {
            rig.mix.revoke_source(src.port, src.generation);
        }
        rig.render(1);
        assert!(rig.mix.has_pending_events(), "reset={reset}");
        rig.refuse_releases.store(false, Ordering::Relaxed);
        rig.render(1);
        assert!(!rig.mix.has_pending_events());
        let heard = rig.heard_by(1);
        assert!(heard.iter().any(|(_, e)| matches!(e.message, Message::NoteOff { key: 60, .. })));
        assert!(heard.iter().any(|(_, e)| e.message == cc(64, 0)));
    }
}

#[test]
fn refused_restrike_preserves_a_future_on_and_its_release() {
    let mut rig = Rig::new();
    let src = port(1, 0);
    let now = rig.mix.next_frame().0;
    assert!(rig.send(src, now, on(60)).accepted());
    rig.render(1);
    let now = rig.mix.next_frame().0;
    assert!(rig.send(src, now + 10, on(60)).accepted());
    assert!(rig.send(src, now + 2 * BLOCK as u64 + 10, on(60)).accepted());
    rig.refuse_releases.store(true, Ordering::Relaxed);
    rig.render(1);
    rig.refuse_releases.store(false, Ordering::Relaxed);
    rig.render(2);
    assert!(rig.send(src, rig.mix.next_frame().0, off(60)).accepted());
    rig.render(1);
    let heard = rig.heard_by(1);
    assert_eq!(heard.len(), 4);
    assert_eq!(heard[0].1.note_id, heard[1].1.note_id);
    assert_eq!(heard[2].1.note_id, heard[3].1.note_id);
    assert_ne!(heard[0].1.note_id, heard[2].1.note_id);
    assert!(!rig.mix.has_pending_events());
}
