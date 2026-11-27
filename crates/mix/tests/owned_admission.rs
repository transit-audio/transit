//! ADR 0059 R1 through the mix, with a contract-conforming recording instrument (it admits through
//! the shared ledger and settles each call): clip notes reach the instrument through the mix's table
//! without the queue's quotas, so the ledger is that path's only limit.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;
use transit_mix::Message;
#[allow(dead_code)] // The shared rig carries helpers this file does not use.
mod common;
use common::*;

/// Decision 1's replay: 481 clip note cycles in one block are all admitted and each on gets its
/// off. Ticks are 25 frames (960 per quarter at 120 BPM, 48 kHz); 48 notes start on each of the
/// first ten ticks of the 256-frame block, each one tick long, 48 keys per tick.
#[test]
fn four_hundred_eighty_one_clip_note_cycles_in_one_block_are_all_admitted() {
    let mut rig = Rig::new();
    rig.owned.store(true, Ordering::Relaxed);
    let mut s = rig.session.clone();
    let mut seq = transit_midi::Sequence::default();
    for i in 0..481i64 {
        let pitch = 30 + (i / 10) as u8;
        seq.notes.push(transit_midi::Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start: i % 10, length: 1 });
    }
    // A new instrument id makes a new instance, which admits through the ledger.
    s.track_mut(rig.track).unwrap().instrument = Some(transit_model::Insert::new("clap:test.owned"));
    let cid = s.new_clip_id();
    s.track_mut(rig.track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "cycles", 0, 48_000, seq));
    rig.publish(&s);
    rig.session = s;
    rig.pos = 0;
    rig.heard.lock().unwrap().clear();
    rig.render(1);
    assert_eq!(rig.refused.load(Ordering::Relaxed), 0, "no start refused");
    let heard = rig.heard_by(2);
    let ons = heard.iter().filter(|(_, e)| matches!(e.message, Message::NoteOn { .. })).count();
    let offs = heard.iter().filter(|(_, e)| matches!(e.message, Message::NoteOff { .. })).count();
    assert_eq!((ons, offs), (481, 481), "every cycle delivered in the one block");
}

/// Replays what the instrument applied: the voices still held (by the mix's voice id) and whether
/// the channel's sustain pedal is down.
fn replay(heard: &[(u64, transit_mix::PluginEvent)]) -> (Vec<i32>, bool) {
    let mut voices = Vec::new();
    let mut pedal = false;
    for (_, e) in heard {
        match e.message {
            Message::NoteOn { velocity, .. } if velocity > 0 => voices.push(e.note_id),
            Message::NoteOn { .. } | Message::NoteOff { .. } => voices.retain(|v| *v != e.note_id),
            Message::Control { cc: 64, value } => pedal = value >= 64,
            _ => {}
        }
    }
    (voices, pedal)
}

/// ADR 0059, Decision 2 through the mix: a call carrying [pedal up, pedal down, off A, on B] is
/// applied up to its k-th event and then reports an error, for every k. The mix contains the
/// instrument (nothing more reaches it), and after Stop the owed releases leave no voice sounding,
/// no pedal held and nothing owed.
#[test]
fn a_call_that_applies_its_first_k_events_and_fails_leaves_nothing_after_stop() {
    for k in 0..=4usize {
        let mut rig = Rig::new();
        rig.owned.store(true, Ordering::Relaxed);
        let mut s = rig.session.clone();
        s.track_mut(rig.track).unwrap().instrument = Some(transit_model::Insert::new("clap:test.owned"));
        rig.publish(&s);
        rig.session = s;
        rig.heard.lock().unwrap().clear();
        let src = port(1, 0);
        let now = rig.mix.next_frame().0;
        rig.send(src, now, cc(64, 127));
        rig.send(src, now, on(60));
        rig.render(1);
        assert_eq!(replay(&rig.heard_by(2)), (vec![voice(&rig, 60)], true), "A held under the pedal");
        let now = rig.mix.next_frame().0;
        rig.send(src, now, cc(64, 0));
        rig.send(src, now, cc(64, 127));
        rig.send(src, now, off(60));
        rig.send(src, now, on(62));
        rig.apply.store(k, Ordering::Relaxed);
        rig.err.store(true, Ordering::Relaxed);
        rig.render(1);
        rig.err.store(false, Ordering::Relaxed);
        let contained = rig.heard_by(2).len();
        rig.render(3);
        assert_eq!(rig.heard_by(2).len(), contained, "nothing reaches the contained instrument (k = {k})");
        rig.mix.reset();
        rig.render(2);
        let (voices, pedal) = replay(&rig.heard_by(2));
        assert!(voices.is_empty() && !pedal, "k = {k}: voices {voices:?}, pedal {pedal}");
        assert_eq!(rig.refused.load(Ordering::Relaxed), 0);
    }
}

fn voice(rig: &Rig, key: u8) -> i32 {
    rig.heard_by(2)
        .iter()
        .find_map(|(_, e)| if let Message::NoteOn { key: k, .. } = e.message { (k == key).then_some(e.note_id) } else { None })
        .unwrap()
}
