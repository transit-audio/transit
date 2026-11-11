//! What the core path publishes per sounding voice for the Slices view's playheads (#470): each
//! voice's zone and source frame after the block ([`crate::Plugin::voice_positions`]), none off
//! the core path or once the voice ends, and reading them never changes a bit of the audio.

use super::*;
use crate::event::{Message, NoteId};
use crate::{SampleBuffer, SampleSource, VOICE_SLOTS, VoicePos};
use std::sync::Arc;

const SR: f32 = 48_000.0;

/// Pad 0 on zone 3 over a 4800-frame source at 44.1 kHz, held Gate with a ping-pong loop over
/// `1000..1500`, transposed down a fifth.
fn instance() -> Sampler {
    let mut s = Sampler::new();
    s.prepare(SR, 512, 2);
    let mut p = s.enable_core(1);
    let src: Arc<dyn SampleSource> =
        Arc::new(SampleBuffer { sample_rate: 44_100.0, channels: vec![(0..4800).map(|n| ((n as f32) * 0.01).sin() * 0.5).collect()] });
    let playback = Playback {
        mode: PlayMode::Gate,
        key_tracking: false,
        root: 60,
        transpose: -7.0,
        attack_ms: 0.0,
        decay_ms: 0.0,
        sustain: 1.0,
        release_ms: 5.0,
        ..Playback::pad(60)
    };
    let mut zones = vec![None; 4];
    zones[3] = Some(Zone { playback, ..Zone::new(Region::whole(0, 4800).looped(LoopMode::PingPong, 1000, 1500)) });
    let mut spec = PatchSpec { sources: vec![src], zones, ..PatchSpec::default() };
    spec.pads[0].zone = Some(3);
    p.publish(spec).unwrap();
    block(&mut s, 64);
    s
}

fn note(s: &mut Sampler, offset: u32, on: bool) {
    let message =
        if on { Message::NoteOn { key: 60, velocity: 100, id: NoteId::NONE } } else { Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE } };
    assert!(s.event(offset, &crate::event::PluginEvent { channel: 0, note_id: -1, message }));
}

fn block(s: &mut Sampler, frames: usize) -> Vec<Vec<f32>> {
    let mut io = vec![vec![0.0; frames]; 2];
    s.process(&mut io, frames);
    io
}

fn published(s: &Sampler) -> Vec<(u16, u64)> {
    let mut out = [0u64; VOICE_SLOTS];
    let n = s.voice_positions(&mut out);
    out.iter().take(n).filter_map(|w| VoicePos(*w).get()).collect()
}

#[test]
fn a_sounding_voice_publishes_its_zone_and_the_core_s_source_frame() {
    let mut s = instance();
    assert!(published(&s).is_empty(), "nothing sounds");
    note(&mut s, 0, true);
    // 20 blocks of 128 at 0.613 source frames a frame (44.1 kHz, a fifth down): about 1570.
    for _ in 0..20 {
        block(&mut s, 128);
        let core: Vec<(u16, u64)> = s.core.as_deref().unwrap().core().voice_positions().map(|p| (p.zone, p.phase.floor() as u64)).collect();
        assert_eq!(core.len(), 1);
        assert_eq!(published(&s), core, "the published frame is the core's phase, floored");
    }
    assert!(published(&s)[0].1 >= 1000, "it reached the loop: {:?}", published(&s));
    // Released: the tail sounds 5 ms, then the voice and its place are gone.
    note(&mut s, 0, false);
    block(&mut s, 1024);
    assert!(published(&s).is_empty(), "the voice ended");
    // Negative control: an instance off the core path publishes nothing.
    let mut out = [7u64; VOICE_SLOTS];
    assert_eq!(Sampler::new().voice_positions(&mut out), 0);
}

#[test]
fn reading_the_positions_never_changes_the_audio() {
    let (mut read, mut unread) = (instance(), instance());
    note(&mut read, 7, true);
    note(&mut unread, 7, true);
    let mut seen = 0;
    for b in 0..20 {
        if b == 15 {
            note(&mut read, 3, false);
            note(&mut unread, 3, false);
        }
        let x = block(&mut read, 97);
        seen += published(&read).len();
        let y = block(&mut unread, 97);
        for (c, (a, u)) in x.iter().zip(&y).enumerate() {
            assert!(a.iter().zip(u).all(|(p, q)| p.to_bits() == q.to_bits()), "block {b}, channel {c}");
        }
    }
    assert!(seen >= 15, "the read side read every sounding block: {seen}");
}

#[test]
fn the_words_pack_the_zone_and_the_whole_frame() {
    assert_eq!(VoicePos(0).get(), None, "zero is no voice");
    assert_eq!(VoicePos::new(3, 1234.99).get(), Some((3, 1234)));
    assert_eq!(VoicePos::new(255, 0.0).get(), Some((255, 0)), "frame 0 is a voice");
    assert_eq!(VoicePos::new(0, -0.5).get(), Some((0, 0)), "a crossfaded reverse loop's phase below 0");
    assert_eq!(VoicePos::new(1, f64::NAN).get(), Some((1, 0)));
    assert_eq!(VoicePos::new(1, 1e300).get(), Some((1, (1 << 48) - 1)), "clamped to 48 bits");
    assert_eq!(VoicePos::new(0xFFFF, 5.0).get(), Some((0x7FFF, 5)), "15 bits of zone");
}
