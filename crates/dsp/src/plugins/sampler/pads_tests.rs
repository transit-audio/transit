//! What the core path publishes per pad (#326, 326-S, `pads.rs`): an admitted trigger counts with
//! its velocity, a choke group's trigger counts a cut on the sounding peer only, and a muted pad or
//! a pad a solo excludes neither counts a trigger nor cuts (ADR 0041). Events and buffers are
//! injected; nothing waits on a clock.

use super::*;
use crate::event::{Message, NoteId};
use crate::{PAD_SLOTS, PadWord, SampleBuffer};

const SR: f32 = 48_000.0;

fn source(frames: usize, offset: f32) -> Arc<dyn SampleSource> {
    Arc::new(SampleBuffer { sample_rate: SR, channels: vec![(0..frames).map(|n| 0.25 + offset + n as f32 * 1e-6).collect()] })
}

fn one_shot(note: u8) -> Playback {
    Playback {
        mode: PlayMode::OneShot,
        key_tracking: false,
        root: note,
        attack_ms: 0.0,
        decay_ms: 0.0,
        sustain: 1.0,
        release_ms: 0.0,
        ..Playback::pad(note)
    }
}

/// Pads 0..n bound to zones 0..n, each its own 4800-frame source, choke groups as given, and the
/// pads' mute and solo.
fn instance(chokes: &[u8], mute: &[usize], solo: &[usize]) -> Sampler {
    let mut s = Sampler::new();
    s.prepare(SR, 512, 2);
    let mut p = s.enable_core(1);
    let n = chokes.len();
    let mut spec = PatchSpec {
        sources: (0..n).map(|i| source(4800, i as f32 * 0.01)).collect(),
        zones: (0..n).map(|i| Some(Zone { playback: one_shot(60 + i as u8), ..Zone::new(Region::whole(i as u16, 4800)) })).collect(),
        ..PatchSpec::default()
    };
    for (i, c) in chokes.iter().enumerate() {
        spec.pads[i].zone = Some(i as u16);
        spec.pads[i].choke = *c;
        spec.pads[i].mute = mute.contains(&i);
        spec.pads[i].solo = solo.contains(&i);
    }
    p.publish(spec).unwrap();
    // The first block adopts the patch.
    block(&mut s, 64);
    s
}

fn on(s: &mut Sampler, offset: u32, key: u8, velocity: u8) {
    let ev = PluginEvent { channel: 0, note_id: -1, message: Message::NoteOn { key, velocity, id: NoteId::NONE } };
    assert!(s.event(offset, &ev));
}

fn block(s: &mut Sampler, frames: usize) -> Vec<f32> {
    let mut io = vec![vec![0.0; frames]; 2];
    s.process(&mut io, frames);
    io.swap_remove(0)
}

fn words(s: &Sampler) -> [PadWord; PAD_SLOTS] {
    let mut out = [0u32; PAD_SLOTS];
    assert!(s.pad_activity(&mut out), "the core path has pads");
    out.map(PadWord)
}

#[test]
fn an_admitted_trigger_counts_with_its_velocity_and_a_legacy_instance_has_no_pads() {
    let mut s = instance(&[0, 0], &[], &[]);
    assert_eq!(words(&s)[0], PadWord(0));
    on(&mut s, 3, 60, 90);
    block(&mut s, 256);
    let w = words(&s);
    assert_eq!((w[0].hits(), w[0].velocity(), w[0].chokes()), (1, 90, 0));
    assert_eq!(w[1], PadWord(0), "the other pad did nothing");
    // A retrigger counts again with the new velocity.
    on(&mut s, 0, 60, 30);
    block(&mut s, 64);
    assert_eq!((words(&s)[0].hits(), words(&s)[0].velocity()), (2, 30));
    // A note no pad has, and a zero-velocity note-on (a note-off), count nothing.
    on(&mut s, 0, 100, 90);
    on(&mut s, 1, 61, 0);
    block(&mut s, 64);
    assert_eq!(words(&s)[1], PadWord(0));
    // Negative control: a legacy instance publishes no pads.
    let legacy = Sampler::new();
    let mut out = [7u32; PAD_SLOTS];
    assert!(!legacy.pad_activity(&mut out));
}

#[test]
fn a_choke_group_trigger_cuts_its_sounding_peer_only() {
    // Pads 0 and 1 share group 1; pad 2 is in group 2.
    let mut s = instance(&[1, 1, 2], &[], &[]);
    on(&mut s, 0, 60, 100);
    on(&mut s, 0, 62, 100);
    block(&mut s, 128);
    // Pad 1 cuts pad 0 (sounding, its one-shot runs 4800 frames); pad 2 is in another group.
    on(&mut s, 10, 61, 80);
    let out = block(&mut s, 128);
    let w = words(&s);
    assert_eq!((w[0].chokes(), w[1].hits(), w[1].chokes(), w[2].chokes()), (1, 1, 0, 0));
    assert!(out.iter().any(|x| *x != 0.0), "pad 1 sounds");
    // Negative control: a peer that is not sounding is not cut. Pad 0's voice was cut; pad 1
    // retriggers itself and pad 0 is still silent, so its count stays.
    on(&mut s, 0, 61, 80);
    block(&mut s, 64);
    assert_eq!(words(&s)[0].chokes(), 1, "a silent peer is not cut again");
    // Within one block: a trigger and its peer's trigger after it count the cut too.
    let mut b = instance(&[3, 3], &[], &[]);
    on(&mut b, 0, 60, 100);
    on(&mut b, 20, 61, 100);
    block(&mut b, 128);
    assert_eq!((words(&b)[0].chokes(), words(&b)[1].chokes()), (1, 0));
}

#[test]
fn a_muted_or_solo_excluded_pad_neither_counts_a_trigger_nor_cuts() {
    // Pad 1 is muted, in pad 0's group: pad 0 sounds, pad 1's note admits nothing and cuts nothing.
    let mut s = instance(&[1, 1], &[1], &[]);
    on(&mut s, 0, 60, 100);
    block(&mut s, 64);
    on(&mut s, 0, 61, 100);
    let out = block(&mut s, 128);
    let w = words(&s);
    assert_eq!((w[1].hits(), w[0].chokes()), (0, 0), "the muted pad admits and cuts nothing");
    assert!(out.iter().all(|x| *x != 0.0), "pad 0 plays on, uncut");
    // Pad 2 is solo: pad 0 (not solo) admits nothing, and does not cut pad 1 of its group.
    let mut x = instance(&[1, 1, 0], &[], &[2]);
    on(&mut x, 0, 60, 100);
    block(&mut x, 64);
    assert_eq!(words(&x)[0].hits(), 0, "excluded by the solo");
    on(&mut x, 0, 62, 100);
    block(&mut x, 64);
    assert_eq!(words(&x)[2].hits(), 1, "the solo pad plays");
    // Negative control: unmuted, the same trigger cuts its peer.
    let mut y = instance(&[1, 1], &[], &[]);
    on(&mut y, 0, 60, 100);
    block(&mut y, 64);
    on(&mut y, 0, 61, 100);
    block(&mut y, 64);
    assert_eq!((words(&y)[1].hits(), words(&y)[0].chokes()), (1, 1));
}

#[test]
fn the_words_pack_and_wrap() {
    let w = PadWord(0).hit(200);
    assert_eq!((w.velocity(), w.hits(), w.chokes()), (127, 1, 0), "velocity clamps to 127");
    let c = w.choked();
    assert_eq!((c.velocity(), c.hits(), c.chokes()), (127, 1, 1));
    let mut x = PadWord(0);
    for _ in 0..256 {
        x = x.hit(5).choked();
    }
    assert_eq!((x.hits(), x.chokes(), x.velocity()), (0, 0, 5), "both counts wrap at 256");
    assert_ne!(PadWord(0).hit(5), PadWord(0), "negative control: a hit changes the word");
}
