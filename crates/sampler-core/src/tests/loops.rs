//! The reader: the cubic pinned to the legacy sampler's `hermite` by a reference table, and playback
//! through regions and loops (none, forward, ping-pong; forward and reverse; tiny loops, fractional
//! steps, multiple wraps in one step, a source rate unlike the session's) compared bit for bit
//! with an independent scalar reference written here with plain while-loops.

use super::{SR, core, render};
use crate::event::{Event, EventKind};
use crate::patch::{PlayMode, Playback, Zone};
use crate::playhead::{LoopMode, Playhead, Region};
use crate::reader::{SliceSource, cubic, split};
use crate::{MAX_STEP, MOD_BLOCK};

const DATA: [f32; 12] = [0.25, -0.5, 0.875, 0.1, -0.9, 0.333, 0.71, -0.2, 0.05, 0.6, -0.45, 0.3];

/// `hermite(&DATA, pos)` of crates/dsp/src/plugins/sampler.rs, printed from that function at the
/// extraction (positions with a whole frame of at least 1, where both read the same taps).
const TABLE: [(f64, u32); 15] = [
    (1.0, 0xbf00_0000),
    (1.25, 0xbe82_b333),
    (2.5, 0x3f22_cccd),
    (3.1, 0xba75_5d80),
    (4.999, 0x3eaa_1528),
    (5.0001, 0x3eaa_8987),
    (6.73, 0xbb65_d500),
    (7.5, 0xbe2a_3d70),
    (8.123456789, 0x3dea_3606),
    (9.9, 0xbed2_4748),
    (1.75, 0x3f21_f332),
    (2.0625, 0x3f62_d29a),
    (3.875, 0xbf61_5c48),
    (6.3333333, 0x3eeb_06e8),
    (8.5, 0x3ed0_0000),
];

#[test]
fn the_cubic_is_the_legacy_hermite_bit_for_bit() {
    for (pos, bits) in TABLE {
        let (i, t) = split(pos);
        let i = i as usize;
        let y = cubic(DATA[i - 1], DATA[i], DATA[i + 1], DATA[i + 2], t);
        assert_eq!(y.to_bits(), bits, "at {pos}: {y} != {}", f32::from_bits(bits));
    }
    assert_eq!(cubic(1.0, f32::NAN, 1.0, 1.0, 0.0), 0.0);
    assert_eq!(cubic(1.0, f32::INFINITY, 1.0, 1.0, 0.5), 0.0);
}

/// The ADR's rule where the legacy reader differs: a tap before the region reads zero (the legacy
/// `hermite` repeated frame 0 for tap -1).
#[test]
fn taps_outside_the_region_read_zero() {
    let r = Region { start: 4, end: 9, ..Region::whole(0, 12) };
    let h = Playhead::start(&r, false, false);
    assert_eq!(h.taps(&r).0, [None, Some(4), Some(5), Some(6)]);
    let h = Playhead { pos: 7.5, ..h };
    assert_eq!(h.taps(&r).0, [Some(6), Some(7), Some(8), None]);
}

/// The independent reference: the phase recurrence and the tap rules with while-loops.
pub(super) fn reference(data: &[f32], r: &Region, reverse: bool, looping: bool, step: f64, frames: usize) -> Vec<f32> {
    let looping = looping && r.loop_mode != LoopMode::Off;
    let (start, end) = (r.start as i64, r.end as i64);
    let (lo, hi) = (r.loop_start as i64, r.loop_end as i64 - 1);
    let len = hi + 1 - lo;
    let mut p = if reverse { (r.end - 1) as f64 } else { r.start as f64 };
    let mut up = !reverse;
    let mut wrapped = false;
    let mut out = vec![0.0; frames];
    for y in out.iter_mut() {
        let i = p.floor() as i64;
        let t = (p - i as f64) as f32;
        let tap = |mut j: i64| -> f32 {
            if looping {
                let exit = if reverse { j < lo } else { j > hi };
                let entry = if reverse { j > hi } else { j < lo };
                if exit || (entry && wrapped) {
                    if r.loop_mode == LoopMode::PingPong {
                        while j < lo || j > hi {
                            j = if j > hi { 2 * hi - j } else { 2 * lo - j };
                        }
                    } else {
                        while j > hi {
                            j -= len;
                        }
                        while j < lo {
                            j += len;
                        }
                    }
                    return data[j as usize];
                }
            }
            if start <= j && j < end { data[j as usize] } else { 0.0 }
        };
        *y = cubic(tap(i - 1), tap(i), tap(i + 1), tap(i + 2), t);
        if !looping {
            p = if reverse { p - step } else { p + step };
            if (reverse && p < start as f64) || (!reverse && p >= end as f64) {
                break;
            }
        } else if r.loop_mode == LoopMode::PingPong {
            p = if up { p + step } else { p - step };
            loop {
                if up && p > hi as f64 {
                    p = 2.0 * hi as f64 - p;
                    up = false;
                } else if !up && p < lo as f64 {
                    p = 2.0 * lo as f64 - p;
                    up = true;
                } else {
                    break;
                }
                wrapped = true;
            }
        } else if reverse {
            p -= step;
            while p < lo as f64 {
                p += len as f64;
                wrapped = true;
            }
        } else {
            p += step;
            while p >= (hi + 1) as f64 {
                p -= len as f64;
                wrapped = true;
            }
        }
    }
    out
}

/// Plays `region` of `data` (at `source_rate`) as a held Gate zone through the core with the key
/// `key` over root 60 plus `transpose` semitones, every gain unity; returns the output and the step.
fn play(data: &[f32], region: Region, reverse: bool, key: u8, transpose: f32, source_rate: f32, frames: usize) -> (Vec<f32>, f64) {
    let mut c = core();
    let playback = Playback {
        mode: PlayMode::Gate,
        key_tracking: true,
        root: 60,
        transpose,
        reverse,
        attack_ms: 0.0,
        decay_ms: 0.0,
        sustain: 1.0,
        release_ms: 0.0,
        ..Playback::zone()
    };
    c.set_zone(0, Some(Zone { playback, ..Zone::new(region) })).unwrap();
    let out = render(
        &mut c,
        &[SliceSource::mono(data, source_rate)],
        &[Event::new(0, EventKind::NoteOn { channel: 0, key, velocity: 127, id: -1 })],
        frames,
    );
    let semis = f64::from(key) - 60.0 + f64::from(transpose) + f64::from(0.0f32) / 100.0;
    let step = (f64::from(source_rate) / f64::from(SR) * 2f64.powf(semis / 12.0)).min(MAX_STEP);
    (out, step)
}

#[test]
fn regions_and_loops_match_the_scalar_reference_bit_for_bit() {
    let data: Vec<f32> = (0..64).map(|i| ((i * 37 % 64) as f32 - 31.5) / 32.0).collect();
    let base = Region { start: 2, end: 50, ..Region::whole(0, 64) };
    let regions = [
        base,
        base.looped(LoopMode::Forward, 10, 30),
        base.looped(LoopMode::PingPong, 10, 30),
        base.looped(LoopMode::Forward, 20, 22),
        base.looped(LoopMode::PingPong, 20, 22),
        base.looped(LoopMode::Forward, 2, 50),
        base.looped(LoopMode::PingPong, 2, 50),
    ];
    // (key, transpose): unity, fractional down and up, a fifth, and steps past a loop's length.
    let pitches = [(60, 0.0), (60, -17.2), (60, 7.0), (67, 10.3), (100, 0.0), (127, 12.0)];
    let mut cases = 0;
    for region in regions {
        for reverse in [false, true] {
            for (key, transpose) in pitches {
                for source_rate in [48_000.0, 44_100.0, 96_000.0] {
                    let frames = 200 + MOD_BLOCK;
                    let (out, step) = play(&data, region, reverse, key, transpose, source_rate, frames);
                    let want = reference(&data, &region, reverse, true, step, frames);
                    for (n, (a, b)) in out.iter().zip(&want).enumerate() {
                        assert_eq!(
                            a.to_bits(),
                            b.to_bits(),
                            "{region:?} reverse {reverse} key {key} transpose {transpose} rate {source_rate} step {step} frame {n}: {a} != {b}"
                        );
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 7 * 2 * 6 * 3);
}
