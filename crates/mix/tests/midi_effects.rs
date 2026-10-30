//! The MIDI effect slot (#201, ADR 0036) through a realtime engine: clips and live input reach the
//! instrument through the track's MIDI effects in chain order at their sample-accurate frames; a
//! bypassed chain is transparent; events an effect moves past the block arrive in a later block;
//! and none of it allocates on the audio thread.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_mix::{Due, EventOutcome, Message, MixEngine, PluginEvent, RenderFrame, mark_audio_thread};
use transit_model::{MidiEffectSlot, Session};
use transit_rt::testing::{AllocStats, measure};
mod common;
use common::*;
#[path = "common/voice_releases.rs"]
mod voice_releases;
use voice_releases::*;

fn with_chain(rig: &mut Rig, chain: &[(&str, &[(&str, f32)], bool)]) {
    let mut s: Session = rig.session.clone();
    let t = s.track_mut(rig.track).unwrap();
    t.midi_effects.clear();
    for (id, params, bypass) in chain {
        let mut fx = MidiEffectSlot::new(*id);
        for (k, v) in *params {
            fx.params.insert((*k).into(), *v);
        }
        fx.bypass = *bypass;
        t.midi_effects.push(fx);
    }
    rig.publish(&s);
    rig.session = s;
}

fn add_clip(rig: &mut Rig, notes: &[(u8, i64, i64)]) {
    let mut s = rig.session.clone();
    let mut seq = transit_midi::Sequence::default();
    for &(pitch, start, length) in notes {
        seq.notes.push(transit_midi::Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length });
    }
    let cid = s.new_clip_id();
    s.track_mut(rig.track).unwrap().playlist_mut().unwrap().clips.push(transit_model::Clip::midi(cid, "fx", 0, 48_000, seq));
    rig.publish(&s);
    rig.session = s;
}

/// A clip (ticks: 960 per quarter, 120 BPM, so 25 frames a tick at 48 kHz) and live notes over two
/// blocks, with `chain`: what the instrument heard, by frame from the start.
fn play(chain: &[(&str, &[(&str, f32)], bool)]) -> Vec<(u64, PluginEvent)> {
    let mut rig = Rig::new();
    with_chain(&mut rig, chain);
    add_clip(&mut rig, &[(60, 1, 2), (64, 4, 9), (67, 8, 1)]);
    rig.pos = 0;
    let now = rig.mix.next_frame().0;
    let src = port(1, 0);
    assert_eq!(rig.send(src, now + 30, on(72)), EventOutcome::Queued);
    assert_eq!(rig.send(src, now + 30, cc(1, 99)), EventOutcome::Queued);
    assert_eq!(rig.send(src, now + 400, off(72)), EventOutcome::Queued);
    rig.render(2);
    rig.mix.reset();
    rig.render(1);
    rig.heard_by(1).into_iter().map(|(f, e)| (f - now, e)).collect()
}

fn summary(heard: &[(u64, PluginEvent)]) -> Vec<(u64, &'static str, u8, u8)> {
    heard
        .iter()
        .filter_map(|(f, e)| match e.message {
            Message::NoteOn { key, velocity, .. } => Some((*f, "on", key, velocity)),
            Message::NoteOff { key, .. } => Some((*f, "off", key, 0)),
            _ => None,
        })
        .collect()
}

#[test]
fn clips_and_live_notes_go_through_the_chain_in_order_at_their_frames() {
    let plain = play(&[]);
    let fx = play(&[("midi_transpose", &[("semitones", 12.0)], false), ("midi_velocity", &[("scale", 50.0)], false)]);
    let (plain, fx) = (summary(&plain), summary(&fx));
    assert_eq!(plain.len(), fx.len());
    assert!(plain.len() >= 8, "three clip notes and a live one: {plain:?}");
    for (p, f) in plain.iter().zip(&fx) {
        assert_eq!((p.0, p.1), (f.0, f.1), "same frame, same kind");
        assert_eq!(f.2, p.2 + 12, "transposed");
        if p.1 == "on" {
            assert_eq!(f.3, p.3 / 2, "velocity scaled");
        }
    }
    assert_eq!(every_on_has_its_off(&play(&[("midi_transpose", &[("semitones", 12.0)], false)])), 4);
}

#[test]
fn a_bypassed_chain_gives_the_instrument_exactly_what_no_chain_does() {
    let plain = play(&[]);
    let bypassed = play(&[("midi_transpose", &[("semitones", 12.0)], true), ("midi_velocity", &[("scale", 0.0)], true)]);
    assert_eq!(bypassed, plain);
    let unknown = play(&[("clap:not.a.midi.effect", &[], false)]);
    assert_eq!(unknown, plain, "an effect this build does not know passes events through");
}

#[test]
fn events_moved_past_the_block_arrive_in_a_later_block() {
    let mut rig = Rig::new();
    with_chain(&mut rig, &[("midi_test_delay", &[("samples", 300.0)], false), ("midi_transpose", &[("semitones", 2.0)], false)]);
    let now = rig.mix.next_frame().0;
    let src = port(1, 0);
    assert_eq!(rig.send(src, now + 100, on(60)), EventOutcome::Queued);
    assert_eq!(rig.send(src, now + 200, off(60)), EventOutcome::Queued);
    rig.render(1);
    assert!(rig.heard_by(1).is_empty());
    assert!(rig.mix.has_pending_events(), "the carried events are pending");
    rig.render(2);
    assert_eq!(
        summary(&rig.heard_by(1)).into_iter().map(|(f, k, key, _)| (f - now, k, key)).collect::<Vec<_>>(),
        [(400, "on", 62), (500, "off", 62)]
    );
    assert!(!rig.mix.has_pending_events());
    assert_eq!(rig.mix.event_status().midi_fx, transit_mix::MidiFxCounts::default());
}

#[test]
fn an_edit_while_a_note_sounds_leaves_nothing_hanging() {
    let mut rig = Rig::new();
    with_chain(&mut rig, &[("midi_transpose", &[("semitones", 5.0)], false)]);
    let now = rig.mix.next_frame().0;
    let src = port(1, 0);
    assert_eq!(rig.send(src, now + 10, on(60)), EventOutcome::Queued);
    rig.render(1);
    with_chain(&mut rig, &[("midi_transpose", &[("semitones", 5.0)], true)]);
    let later = rig.mix.next_frame().0;
    assert_eq!(rig.send(src, later + 10, off(60)), EventOutcome::Queued);
    rig.render(1);
    let heard = summary(&rig.heard_by(1));
    assert_eq!(heard.iter().map(|(f, k, key, _)| (f - now, *k, *key)).collect::<Vec<_>>(), [(10, "on", 65), (256, "off", 65)]);
    assert_eq!(rig.mix.event_status().midi_fx.released_on_edit, 1);
    assert!(!rig.mix.has_pending_events());
}

#[test]
fn a_chain_with_events_flowing_allocates_nothing() {
    MixEngine::force_serial(true);
    mark_audio_thread();
    let mut rig = Rig::new();
    with_chain(&mut rig, &[("midi_transpose", &[("semitones", 3.0)], false), ("midi_velocity", &[("offset", 5.0)], false)]);
    add_clip(&mut rig, &[(60, 1, 2), (64, 4, 30), (67, 8, 1)]);
    rig.pos = 0;
    let src = port(1, 0);
    let edited = {
        let mut s = rig.session.clone();
        s.track_mut(rig.track).unwrap().midi_effects.reverse();
        s
    };
    rig.render(1);
    // The rig's own helpers allocate (the binding lookup, the recorder's list): kept outside.
    let binding = rig.binding();
    let event = |at: u64, message: Message| transit_mix::InstrumentEvent {
        target: binding,
        due: Due::Frame(RenderFrame(at)),
        ..rig.event(src, Due::Next, message)
    };
    let ons: Vec<_> = (0..40u8).map(|k| event(rig.mix.next_frame().0 + u64::from(k) * 7, on(40 + k))).collect();
    let offs: Vec<_> = (0..40u8).map(|k| event(rig.mix.next_frame().0 + 2 * 256 + u64::from(k), off(40 + k))).collect();
    let late = event(rig.mix.next_frame().0 + 3 * 256, on(50));
    rig.heard.lock().unwrap().reserve(4096);
    let ((), stats) = measure(|| {
        for e in &ons {
            assert!(rig.mix.queue_event(e).accepted());
        }
        rig.render(2);
        for e in &offs {
            assert!(rig.mix.queue_event(e).accepted());
        }
        rig.render(1);
    });
    assert_eq!(stats, AllocStats::default(), "the chain's block, carry and note tables are fixed storage");
    // An edit (a reorder) and a Stop, on the audio thread, allocate nothing either.
    rig.publish(&edited);
    rig.session = edited;
    let ((), stats) = measure(|| {
        assert!(rig.mix.queue_event(&late).accepted());
        rig.render(2);
        rig.mix.reset();
        rig.render(1);
    });
    assert_eq!(stats, AllocStats::default(), "a chain edit builds the built-ins in place");
    assert_eq!(rig.mix.event_status().midi_fx.overflow, 0);
}
