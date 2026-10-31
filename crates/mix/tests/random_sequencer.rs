//! The Random Sequencer MIDI effect (#202) through a realtime engine: its notes reach the
//! instrument on the transport's grid at sample-accurate frames, the same whatever the block size,
//! through a tempo change; a Stop ends them; and the slot with it allocates nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_mix::{Message, MixEngine, PluginEvent, mark_audio_thread};
use transit_model::{MidiEffectSlot, Session};
use transit_rt::testing::{AllocStats, measure};
#[allow(dead_code)] // The shared rig carries helpers this file does not use.
mod common;
use common::*;

fn with_sequencer(rig: &mut Rig, params: &[(&str, f32)]) {
    let mut s: Session = rig.session.clone();
    let t = s.track_mut(rig.track).unwrap();
    let mut fx = MidiEffectSlot::new("random_sequencer");
    for (k, v) in params {
        fx.params.insert((*k).into(), *v);
    }
    t.midi_effects = vec![fx];
    rig.publish(&s);
    rig.session = s;
}

/// Renders `total` frames from the timeline's start in blocks of `block`: what the instrument
/// heard, by frame from the start.
fn play(rig: &mut Rig, total: usize, block: usize) -> Vec<(u64, PluginEvent)> {
    rig.heard.lock().unwrap().clear();
    rig.pos = 0;
    let now = rig.mix.next_frame().0;
    let mut out = vec![vec![0.0; BLOCK]; 2];
    let mut done = 0;
    while done < total {
        let n = block.min(total - done);
        rig.mix.render_active(rig.pos, n, &mut out);
        rig.pos += n as i64;
        done += n;
    }
    rig.heard_by(1).into_iter().map(|(f, e)| (f - now, e)).collect()
}

fn notes(heard: &[(u64, PluginEvent)]) -> Vec<(u64, bool, u8, u8)> {
    heard
        .iter()
        .filter_map(|(f, e)| match e.message {
            Message::NoteOn { key, velocity, .. } => Some((*f, true, key, velocity)),
            Message::NoteOff { key, .. } => Some((*f, false, key, 0)),
            _ => None,
        })
        .collect()
}

/// No key starts twice while it sounds, no key ends before it starts, and (with `ended`) every key
/// started is ended.
fn paired(notes: &[(u64, bool, u8, u8)], ended: bool) -> bool {
    let mut on = [None::<u64>; 128];
    for &(f, start, key, _) in notes {
        let slot = &mut on[usize::from(key)];
        if start {
            if slot.is_some() {
                return false;
            }
            *slot = Some(f);
        } else if let Some(s) = slot.take()
            && s > f
        {
            return false;
        }
    }
    !ended || on.iter().all(Option::is_none)
}

#[test]
fn the_clock_voice_plays_every_sixteenth_on_the_grid() {
    let mut rig = Rig::new();
    with_sequencer(&mut rig, &[("play", 3.0)]);
    // 120 BPM at 48 kHz: a sixteenth is 6000 frames, each note half of it.
    let heard = notes(&play(&mut rig, 48_000, 256));
    let ons: Vec<u64> = heard.iter().filter(|n| n.1).map(|n| n.0).collect();
    assert_eq!(ons, (0..8).map(|k| k * 6000).collect::<Vec<_>>());
    assert!(heard.iter().filter(|n| !n.1).all(|n| n.0 % 6000 == 3000), "{heard:?}");
}

#[test]
fn the_notes_are_the_same_bits_at_64_and_256_frame_blocks() {
    for params in [&[][..], &[("model", 2.0), ("play", 4.0), ("jitter", 40.0)][..], &[("model", 4.0), ("gate_bias", 90.0), ("deja_vu", 75.0)][..]] {
        let mut a = Rig::new();
        with_sequencer(&mut a, params);
        let small = play(&mut a, 48_000 * 6, 64);
        let mut b = Rig::new();
        with_sequencer(&mut b, params);
        let large = play(&mut b, 48_000 * 6, 256);
        assert!(notes(&small).len() > 20, "{params:?}: {} events", small.len());
        assert_eq!(small, large, "{params:?}");
        assert!(paired(&notes(&small), false), "{params:?}");
    }
}

#[test]
fn a_tempo_change_mid_run_keeps_the_grid() {
    let mut rig = Rig::new();
    let mut s = rig.session.clone();
    // 120 BPM, then 90 BPM from beat 4 (sample 96 000).
    s.tempo.set_tempo(4 * transit_time::TICKS_PER_QUARTER, 90.0).unwrap();
    rig.publish(&s);
    rig.session = s;
    with_sequencer(&mut rig, &[("play", 3.0)]);
    let heard = notes(&play(&mut rig, 48_000 * 4, 256));
    let ons: Vec<u64> = heard.iter().filter(|n| n.1).map(|n| n.0).collect();
    // Sixteen sixteenths at 6000 frames, then sixteenths of 8000 frames from 96 000.
    let mut want: Vec<u64> = (0..16).map(|k| k * 6000).collect();
    want.extend((0..12).map(|k| 96_000 + k * 8000));
    assert_eq!(ons, want);
}

#[test]
fn a_stop_ends_every_note() {
    let mut rig = Rig::new();
    with_sequencer(&mut rig, &[("play", 4.0), ("gate_length", 100.0)]);
    let before = notes(&play(&mut rig, 48_000, 256));
    assert!(!paired(&before, true), "notes sound when the transport stops");
    let at = rig.mix.next_frame().0;
    let start = at - 48_000;
    rig.mix.reset();
    rig.render(1);
    // The reset ends every sounding note at the next block's start (the rig keeps playing, so the
    // sequencer then starts again on the grid there: those notes are new).
    let mut all = before.clone();
    all.extend(notes(&rig.heard_by(1).into_iter().map(|(f, e)| (f - start, e)).collect::<Vec<_>>()).into_iter().filter(|n| n.0 >= 48_000 && !n.1));
    assert!(paired(&all, true), "{all:?}");
}

#[test]
fn the_slot_with_the_sequencer_allocates_nothing() {
    MixEngine::force_serial(true);
    mark_audio_thread();
    let mut rig = Rig::new();
    let edited = {
        let mut s = rig.session.clone();
        let mut fx = MidiEffectSlot::new("random_sequencer");
        fx.params.insert("play".into(), 4.0);
        s.track_mut(rig.track).unwrap().midi_effects = vec![fx];
        s
    };
    rig.heard.lock().unwrap().reserve(8192);
    rig.pos = 0;
    // The chain edit makes the effect in place, on the audio thread.
    rig.publish(&edited);
    rig.session = edited;
    let ((), stats) = measure(|| {
        rig.render(200);
        rig.mix.reset();
        rig.render(2);
    });
    assert_eq!(stats, AllocStats::default(), "the sequencer is a value and its schedule fixed storage");
    assert!(notes(&rig.heard_by(1)).len() > 20);
}
