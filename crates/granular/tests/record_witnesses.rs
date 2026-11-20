//! The processor's own transplant witnesses (ADR 0029, amendment for #742, sections 12 and 13;
//! feature `testing`, which this crate's tests turn on), for state the Time Stretcher never sets
//! and so its witnesses cannot reach: the granular mode (the grain player and the random
//! generator its grains draw from), bypass, silence, and a seeded generator.
//!
//! Each case: A reaches T, a fresh B (same memory sizes, quality and mode) restores A's record,
//! re-records exactly the same words, and both run the same continuation; the outputs must stay
//! identical and the records equal at every block, with B's buffer memory past its live extent
//! poisoned after the restore and after every block.

use std::hash::Hasher;
use transit_granular::record::{Poison, Reader, Record, Writer};
use transit_granular::{BufferSize, Frame, Granular, MAX_BLOCK_SIZE, Parameters, PlaybackMode, Quality};

#[derive(Default)]
struct Words(Vec<u64>);

impl Hasher for Words {
    fn write(&mut self, _: &[u8]) {
        unreachable!("the record writer emits words only");
    }
    fn write_u64(&mut self, w: u64) {
        self.0.push(w);
    }
    fn finish(&self) -> u64 {
        0
    }
}

fn record(v: &Granular) -> Vec<u64> {
    let mut sink = Words::default();
    v.record(&mut Writer::new(&mut sink));
    sink.0
}

fn parameters(k: usize) -> Parameters {
    Parameters {
        position: 0.3 + 0.002 * (k % 100) as f32,
        size: 0.5,
        pitch: 3.0,
        density: if k % 300 < 150 { 0.8 } else { 0.2 },
        texture: 0.6,
        dry_wet: 0.9,
        stereo_spread: 0.5,
        feedback: 0.3,
        reverb: 0.4,
        freeze: false,
        trigger: k.is_multiple_of(97),
        gate: false,
    }
}

fn block(v: &mut Granular, k: usize) -> Vec<Frame> {
    let input: Vec<Frame> =
        (0..MAX_BLOCK_SIZE).map(|i| [(((i + k * 32) * 31) % 4001) as i16 * 8 - 16000, (((i + k * 32) * 17) % 3001) as i16 * 9 - 13500]).collect();
    let mut out = vec![[0i16; 2]; MAX_BLOCK_SIZE];
    v.process(&parameters(k), &input, &mut out);
    out
}

fn make(quality: Quality, mode: PlaybackMode) -> Granular {
    Granular::new_in_mode(BufferSize::FIRMWARE, quality, mode)
}

/// Restores A's record into `b` and runs `blocks` blocks on both; `each` changes both alike before
/// a block.
fn transplant(a: &mut Granular, mut b: Granular, blocks: usize, each: fn(&mut Granular, usize)) {
    let rec = record(a);
    let mut r = Reader::new(&rec);
    b.restore(&mut r).expect("the restore takes the record");
    assert!(r.finish(), "the restore reads every word");
    assert_eq!(record(&b), rec, "the restored processor re-records the same words");
    let mut p = Poison::new(5);
    b.poison_unobservable(&mut p);
    for k in 0..blocks {
        each(a, k);
        each(&mut b, k);
        assert_eq!(block(a, k), block(&mut b, k), "block {k}: the outputs differ");
        assert_eq!(record(a), record(&b), "block {k}: the records differ");
        b.poison_unobservable(&mut p);
    }
}

/// The granular mode with a seeded generator, grains sounding at T.
#[test]
fn the_granular_mode_transplants() {
    let mut a = make(Quality::Stereo, PlaybackMode::Granular);
    a.seed(0x5eed);
    for k in 0..1500 {
        block(&mut a, k);
    }
    transplant(&mut a, make(Quality::Stereo, PlaybackMode::Granular), 1500, |_, _| {});
}

/// Bypassed and silenced at T (nothing recorded meanwhile); the continuation plays again.
#[test]
fn bypass_and_silence_transplant() {
    let mut a = make(Quality::Mono, PlaybackMode::Stretch);
    for k in 0..800 {
        block(&mut a, k);
    }
    a.set_bypass(true);
    a.set_silence(true);
    block(&mut a, 800);
    transplant(&mut a, make(Quality::Mono, PlaybackMode::Stretch), 1200, |v, k| {
        if k == 300 {
            v.set_bypass(false);
        }
        if k == 600 {
            v.set_silence(false);
        }
    });
}

/// A switch between the modes after T (the filters reset, the correlator's memory cleared, the
/// grain player's state kept from before).
#[test]
fn a_mode_switch_transplants() {
    let mut a = make(Quality::StereoLowFidelity, PlaybackMode::Granular);
    for k in 0..900 {
        block(&mut a, k);
    }
    a.set_playback_mode(PlaybackMode::Stretch);
    for k in 900..1400 {
        block(&mut a, k);
    }
    transplant(&mut a, make(Quality::StereoLowFidelity, PlaybackMode::Granular), 1200, |v, k| {
        if k == 400 {
            v.set_playback_mode(PlaybackMode::Granular);
        }
    });
}

/// A quality change between blocks (the Time Stretcher's restart sets it inside a host block, so a
/// host block can end with the buffer reset still pending): the next block clears and re-lays the
/// memory only if the restored flag says so.
#[test]
fn a_pending_buffer_reset_transplants() {
    let mut a = make(Quality::Stereo, PlaybackMode::Stretch);
    for k in 0..600 {
        block(&mut a, k);
    }
    a.set_quality(Quality::Mono);
    transplant(&mut a, make(Quality::Stereo, PlaybackMode::Stretch), 900, |_, _| {});
}
