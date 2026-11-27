//! ADR 0059 R1, Decision 2: an instrument that reports a processing error is contained like one
//! that puts out NaN. Its output is silenced from that call, every note and pedal it held is
//! released at once (the MIDI effect chain's notes, carries and devices included, whether or not
//! the mix's own tables hold anything), no event reaches it until Stop, and after Stop it plays
//! again. Also the stated same-source limit across Stop: an old anonymous note-off can cut short a
//! new note of its source and key, never hang one.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::Ordering;
use transit_mix::{EventOutcome, INSTRUMENT_SLOT, Message, PluginEvent};
use transit_model::{MidiEffectSlot, Session};
#[allow(dead_code)] // The shared rig carries helpers this file does not use.
mod common;
use common::*;
#[path = "common/voice_ids.rs"]
mod voice_ids;
use voice_ids::*;

fn admitted(rig: &Rig) -> Vec<(u64, PluginEvent)> {
    rig.admitted.lock().unwrap().clone()
}

#[test]
fn a_processing_error_contains_the_instrument_and_resets_the_chain_at_once() {
    let mut rig = Rig::new();
    // The Random Sequencer plays a note at frame 0 for 3,000 frames and the next at 6,000: a note
    // the chain generated and still owns while the mix's own tables and queue are empty.
    let mut s: Session = rig.session.clone();
    let mut fx = MidiEffectSlot::new("random_sequencer");
    fx.params.insert("play".into(), 3.0);
    s.track_mut(rig.track).unwrap().midi_effects = vec![fx];
    rig.publish(&s);
    rig.session = s;
    rig.pos = 0;
    rig.admitted.lock().unwrap().clear();
    rig.render(1);
    let ons: Vec<u8> =
        admitted(&rig).iter().filter_map(|(_, e)| if let Message::NoteOn { key, .. } = e.message { Some(key) } else { None }).collect();
    assert_eq!(ons.len(), 1, "the sequencer's first note");
    let key = ons[0];
    // The instrument reports an error in the next block.
    rig.err.store(true, Ordering::Relaxed);
    let before = admitted(&rig).len();
    rig.render(1);
    rig.err.store(false, Ordering::Relaxed);
    assert!(rig.out.iter().all(|c| c.iter().all(|x| *x == 0.0)), "its output silenced from that call");
    let at_error = admitted(&rig).split_off(before);
    assert!(
        at_error.iter().any(|(_, e)| matches!(e.message, Message::NoteOff { key: k, .. } if k == key)),
        "the generated note's note-off reached the adapter at the error, before Stop: {at_error:?}"
    );
    let meter = rig.mix.meters().find(|(id, _)| *id == rig.track).map(|(_, m)| m.plugin_failures).unwrap();
    assert_eq!((meter.errors, meter.error_slot, meter.count), (1, INSTRUMENT_SLOT, 0), "counted as an error, not as invalid audio");
    // Until Stop nothing reaches it: not the sequencer's next notes, not a live note.
    let contained = admitted(&rig).len();
    rig.render(30);
    assert_eq!(admitted(&rig).len(), contained, "no event reaches a contained instrument");
    assert!(rig.out.iter().all(|c| c.iter().all(|x| *x == 0.0)));
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(port(1, 0), now, on(70)), EventOutcome::NoInstrument);
    // Stop ends the containment: the instrument and the chain play again.
    rig.mix.reset();
    rig.pos = 0;
    rig.render(1);
    assert!(admitted(&rig).len() > contained, "after Stop the chain reaches the instrument again");
}

/// Renders until the heard log holds a note-on of `key`; returns its voice id.
fn voice_of(rig: &Rig, key: u8) -> i32 {
    rig.heard_by(1)
        .iter()
        .rev()
        .find_map(|(_, e)| if let Message::NoteOn { key: k, .. } = e.message { (k == key).then_some(e.note_id) } else { None })
        .unwrap()
}

fn offs_of(rig: &Rig, voice: i32) -> usize {
    rig.heard_by(1).iter().filter(|(_, e)| e.note_id == voice && matches!(e.message, Message::NoteOff { .. })).count()
}

/// A on key 60 from source S, an error, Stop, then B on key 60 and A's late anonymous note-off.
fn across_stop(b_source: transit_mix::Source, b: Message) -> (Rig, i32) {
    let mut rig = Rig::new();
    let s = port(1, 0);
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(s, now, on(60)), EventOutcome::Queued);
    rig.render(1);
    rig.err.store(true, Ordering::Relaxed);
    rig.render(1);
    rig.err.store(false, Ordering::Relaxed);
    rig.mix.reset();
    rig.render(1);
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(b_source, now, b), EventOutcome::Queued);
    rig.render(1);
    let vb = voice_of(&rig, 60);
    let now = rig.mix.next_frame().0;
    let _ = rig.send(s, now, off(60));
    rig.render(1);
    (rig, vb)
}

#[test]
fn a_late_note_off_from_another_source_leaves_the_new_note_sounding() {
    let (mut rig, vb) = across_stop(port(2, 0), on(60));
    assert_eq!(offs_of(&rig, vb), 0, "B sounds until its own off");
    let now = rig.mix.next_frame().0;
    rig.send(port(2, 0), now, off(60));
    rig.render(1);
    assert_eq!(offs_of(&rig, vb), 1);
}

#[test]
fn a_late_anonymous_note_off_of_the_same_source_cuts_the_new_note_short() {
    // The stated limit (Decision 2): B from S, anonymous or carrying a sender id, ends at A's late
    // anonymous note-off; B's own off then matches nothing; nothing is left sounding.
    for b in [on(60), on_id(60, 42)] {
        let (mut rig, vb) = across_stop(port(1, 0), b);
        assert_eq!(offs_of(&rig, vb), 1, "B cut short at A's late note-off");
        let own = if matches!(b, Message::NoteOn { id, .. } if id == transit_mix::NoteId::NONE) { off(60) } else { off_id(60, 42) };
        let now = rig.mix.next_frame().0;
        rig.send(port(1, 0), now, own);
        rig.render(1);
        assert_eq!(offs_of(&rig, vb), 1, "B's own off matches nothing");
        rig.mix.reset();
        rig.render(1);
        assert_eq!(offs_of(&rig, vb), 1, "nothing left sounding: Stop owes nothing more");
    }
}
