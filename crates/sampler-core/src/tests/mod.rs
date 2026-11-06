//! The core's behavior tests: ADR 0041 Gate 1 at the core level (`gate1`), the reader and loops
//! against independent scalar references (`loops`), and Gate 6's block-size invariance with state
//! fingerprints (`blocks`). The allocation probe is the integration test `tests/realtime.rs`.

mod blocks;
mod gate1;
mod loops;
mod seams;
mod shaped;

use crate::event::{Context, Event};
use crate::patch::{Pad, PlayMode, Playback, Zone};
use crate::playhead::Region;
use crate::reader::SliceSource;
use crate::{PADS, SamplerCore};

pub(crate) const SR: f32 = 48_000.0;

/// Gate 1's controls: level unity, attack/decay/declick zero, sustain 1, release `release` frames
/// (at 48 kHz), key tracking off with the root at `note`.
pub(crate) fn unity(mode: PlayMode, note: u8, release: u32) -> Playback {
    Playback {
        mode,
        key_tracking: false,
        root: note,
        attack_ms: 0.0,
        decay_ms: 0.0,
        sustain: 1.0,
        release_ms: release as f32 * 1000.0 / SR,
        declick_ms: 0.0,
        ..Playback::pad(note)
    }
}

/// A core at 48 kHz.
pub(crate) fn core() -> SamplerCore {
    let mut c = SamplerCore::new();
    c.prepare(SR, 7);
    c
}

/// Binds pad `pad` (trigger note `60 + pad`) to zone `pad` over the whole of source `source`.
pub(crate) fn bind(c: &mut SamplerCore, pad: usize, source: u16, frames: u64, playback: Playback, choke: u8) {
    let zone = Zone { playback, ..Zone::new(Region::whole(source, frames)) };
    c.set_zone(pad, Some(zone)).unwrap();
    c.set_pad(pad, Pad { zone: Some(pad as u16), choke, ..Pad::new(pad) }).unwrap();
}

/// The trigger note of pad `pad`.
pub(crate) fn note(pad: usize) -> u8 {
    assert!(pad < PADS);
    60 + pad as u8
}

/// Renders `frames` frames in one block from frame 0, stereo; returns the left channel (and checks
/// it equals the right one when `centered`).
pub(crate) fn render(c: &mut SamplerCore, sources: &[SliceSource<'_>], events: &[Event], frames: usize) -> Vec<f32> {
    let mut l = vec![f32::NAN; frames];
    let mut r = vec![f32::NAN; frames];
    c.process(&Context::default(), sources, events, &mut [&mut l, &mut r]);
    for (n, (a, b)) in l.iter().zip(&r).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "center pan: left and right differ at frame {n}");
    }
    l
}
