//! Edge fades on unbound zones (ADR 0041, "Playback, controls and modulation", and its amendment of
//! 2026-11-08, item 3): the widths, the `(k + 1)/(n + 1)` factors scaling the velocity after
//! selection, the single rounding, the clamp at use, and old zone maps rendering their old bits.

use super::{SR, unity};
use crate::event::{Context, Event, EventKind};
use crate::patch::{Fades, GroupMode, Membership, Pad, PlayMode, Playback, Zone};
use crate::playhead::Region;
use crate::reader::SliceSource;
use crate::{GROUPS, PADS, SamplerCore, ZONES};

/// FNV-1a over the bits of both channels.
fn hash(l: &[f32], r: &[f32]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in l.iter().chain(r) {
        for byte in v.to_bits().to_le_bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// A zone map without fades: four pads, six plain zones with overlapping ranges, a sequential group
/// of two layers and a random group, offsets inside blocks. Pure arithmetic (no transcendental
/// functions), so the bits hold on every platform.
fn old_map_render() -> u64 {
    let mut x = 0x2468_ACE1u32;
    let mut noise = move || {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    };
    let a: Vec<f32> = (0..2000).map(|_| noise()).collect();
    let b: Vec<f32> = (0..2000).map(|_| noise() * 0.5).collect();
    let sources = [SliceSource::mono(&a, SR), SliceSource::mono(&b, 44_100.0)];
    let mut zones: Vec<Option<Zone>> = vec![None; ZONES];
    let mut pads: [Pad; PADS] = core::array::from_fn(Pad::new);
    let play = |i: usize, sensitivity: f32| {
        let mode = if i.is_multiple_of(2) { PlayMode::Gate } else { PlayMode::OneShot };
        Playback { velocity_sensitivity: sensitivity, ..unity(mode, 60, 300) }
    };
    for p in 0..4usize {
        zones[p] = Some(Zone { playback: play(p + 1, 0.8), ..Zone::new(Region::whole((p % 2) as u16, 1500)) });
        pads[p] = Pad { zone: Some(p as u16), ..Pad::new(p) };
    }
    let plain =
        [((0u8, 127u8), (1u8, 127u8)), ((40, 80), (1, 64)), ((60, 72), (65, 127)), ((0, 59), (1, 127)), ((61, 61), (100, 127)), ((50, 90), (30, 90))];
    for (i, (keys, velocities)) in plain.into_iter().enumerate() {
        zones[4 + i] = Some(Zone { keys, velocities, playback: play(i, 0.5), ..Zone::new(Region::whole((i % 2) as u16, 1500)) });
    }
    let mut next = 10;
    for (group, layers, count, keys) in [(0u8, 2u8, 3u8, (30u8, 50u8)), (1, 1, 4, (64, 90))] {
        for layer in 0..layers {
            for alternative in 0..count {
                let velocities = match (layers, layer) {
                    (1, _) => (1, 127),
                    (_, 0) => (1, 70),
                    _ => (71, 127),
                };
                let membership = Some(Membership { group, layer, alternative });
                let region = Region::whole(u16::from(alternative % 2), 1500);
                zones[next] = Some(Zone { keys, velocities, membership, playback: play(next, 0.6), ..Zone::new(region) });
                next += 1;
            }
        }
    }
    let mut modes = [GroupMode::Sequential; GROUPS];
    modes[1] = GroupMode::Random;
    let mut c = SamplerCore::new();
    c.prepare(SR, 5);
    c.set_patch(&zones, &pads).unwrap();
    c.set_group_modes(&modes);
    let mut sched: Vec<(u64, EventKind)> = (0..60u64)
        .flat_map(|i| {
            let key = 25 + (i * 17 % 70) as u8;
            let on = EventKind::NoteOn { channel: 0, key, velocity: 1 + (i * 41 % 127) as u8, id: i as i32 };
            let off = EventKind::NoteOff { channel: 0, key, velocity: 64, id: i as i32 };
            [(i * 89 + i % 7, on), (i * 89 + 350, off)]
        })
        .collect();
    sched.sort_by_key(|e| e.0);
    let (l, r, _) = super::blocks::run(&mut c, &sources[..], &sched, &[256], 6000);
    assert!(l.iter().filter(|x| **x != 0.0).count() > 3000, "a golden of silence proves nothing");
    hash(&l, &r)
}

/// The bits [`old_map_render`] gave, captured from the core before edge fades existed.
const OLD_MAP_GOLDEN: u64 = 0x9247_8994_7424_312d;

#[test]
fn old_zone_map_bits_unchanged() {
    assert_eq!(old_map_render(), OLD_MAP_GOLDEN, "a zone map without fades renders the bits it did before fades");
}

const LEN: usize = 64;
const STOPPED: Context = Context { frame: 0, tempo: 120.0, quarters: None };

/// A zone over source `source` with ranges `keys` and `velocities`, widths `w` (`[key_lo, key_hi,
/// vel_lo, vel_hi]`) and membership `m`; unity level, velocity sensitivity 1, so the first output
/// frame is the source value times the played velocity over 127.
fn faded(keys: (u8, u8), velocities: (u8, u8), w: [u8; 4], m: Option<(u8, u8, u8)>, source: u16) -> Zone {
    let playback = Playback { velocity_sensitivity: 1.0, ..unity(PlayMode::OneShot, 60, 0) };
    Zone {
        keys,
        velocities,
        fades: Fades { key_lo: w[0], key_hi: w[1], vel_lo: w[2], vel_hi: w[3] },
        membership: m.map(|(group, layer, alternative)| Membership { group, layer, alternative }),
        playback,
        ..Zone::new(Region::whole(source, LEN as u64))
    }
}

/// A core at 48 kHz with the zones from slot 0 and the pads.
fn build_with(zones: &[Zone], pads: &[Pad; PADS]) -> SamplerCore {
    let mut t = vec![None; ZONES];
    for (i, z) in zones.iter().enumerate() {
        t[i] = Some(*z);
    }
    let mut c = SamplerCore::new();
    c.prepare(SR, 1);
    c.set_patch(&t, pads).unwrap();
    c
}

fn build(zones: &[Zone]) -> SamplerCore {
    build_with(zones, &core::array::from_fn(Pad::new))
}

/// Constant sources, source `i` holding `values[i]`.
fn data(values: &[f32]) -> Vec<Vec<f32>> {
    values.iter().map(|&v| vec![v; LEN]).collect()
}

fn bank(data: &[Vec<f32>]) -> Vec<SliceSource<'_>> {
    data.iter().map(|d| SliceSource::mono(d, SR)).collect()
}

/// One event at frame 0: the first output frame, then every voice cut.
fn hit_event(c: &mut SamplerCore, s: &[SliceSource<'_>], kind: EventKind) -> f32 {
    let (mut l, mut r) = ([0.0f32; 2], [0.0f32; 2]);
    c.process(&STOPPED, s, &[Event::new(0, kind)], &mut [&mut l, &mut r]);
    c.process(&STOPPED, s, &[Event::new(0, EventKind::AllSoundOff)], &mut []);
    l[0]
}

fn hit(c: &mut SamplerCore, s: &[SliceSource<'_>], key: u8, velocity: u8) -> f32 {
    hit_event(c, s, EventKind::NoteOn { channel: 0, key, velocity, id: -1 })
}

/// The velocity a first frame `x` of a source of value `source` played (0: silence).
fn played(x: f32, source: f32) -> u32 {
    let v = (x / source * 127.0).round();
    assert!((0.0..=127.0).contains(&v), "frame {x}");
    v as u32
}

/// The independent reference: the factor of `x` over `lo..=hi` with fitting widths `nl`, `nh`.
fn reference(x: u32, lo: u32, hi: u32, nl: u32, nh: u32) -> (u32, u32) {
    if x < lo + nl {
        (x - lo + 1, nl + 1)
    } else if x + nh > hi {
        (hi - x + 1, nh + 1)
    } else {
        (1, 1)
    }
}

/// `max(1, round(v * a/b * c/d))`, ties upward, in integers.
fn expected(v: u32, (a, b): (u32, u32), (c, d): (u32, u32)) -> u32 {
    ((2 * v * a * c + b * d) / (2 * b * d)).max(1)
}

#[test]
fn fade_crosses_to_expected_gains_at_edges() {
    // Keys 40..60 with key fades 3 and 3; velocities 96..104 with velocity fades 4 and 4.
    let d = data(&[1.0]);
    let s = bank(&d);
    let mut c = build(&[faded((40, 60), (96, 104), [3, 3, 4, 4], None, 0)]);
    // Input velocity 100 (the velocity range's full level) across the key edges.
    let keys: Vec<u32> = (39..=61).map(|k| played(hit(&mut c, &s, k, 100), 1.0)).collect();
    let want = [0, 25, 50, 75, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 100, 75, 50, 25, 0];
    assert_eq!(keys, want, "keys 39..61 at velocity 100: 40 is 1/4, 43 the first full key, 60 mirrors 40");
    // Key 50 (full) across the velocity edges, selection by the original velocity.
    let vels: Vec<u32> = (95..=105).map(|v| played(hit(&mut c, &s, 50, v), 1.0)).collect();
    assert_eq!(vels, [0, 19, 39, 59, 79, 100, 81, 61, 41, 21, 0], "velocities 95..105 at key 50");
    // Every key and velocity against the integer table, both factors at once.
    for key in 36..=64u8 {
        for v in 1..=127u8 {
            let want = if (40..=60).contains(&key) && (96..=104).contains(&v) {
                let fk = reference(u32::from(key), 40, 60, 3, 3);
                let fv = reference(u32::from(v), 96, 104, 4, 4);
                assert!(fk.0 > 0 && fv.0 > 0, "no factor inside the zone is 0");
                expected(u32::from(v), fk, fv)
            } else {
                0
            };
            assert_eq!(played(hit(&mut c, &s, key, v), 1.0), want, "key {key}, velocity {v}");
        }
    }
    // The outermost step of both edges at once still sounds: 96 x 1/4 x 1/5 = 4.8, so 5.
    assert_eq!(played(hit(&mut c, &s, 40, 96), 1.0), 5);
    assert_eq!(played(hit(&mut c, &s, 60, 104), 1.0), 5, "104 x 1/4 x 1/5 = 5.2");
}

#[test]
fn fade_scales_velocity_minimum_one() {
    // Key 60 is 1/11 of a key fade of 10; velocity 1 is 1/127 of a velocity fade of 126.
    let d = data(&[1.0]);
    let s = bank(&d);
    let mut c = build(&[faded((60, 70), (1, 127), [10, 0, 126, 0], None, 0)]);
    for (key, v) in [(60, 1), (60, 5), (61, 1), (70, 1), (60, 40)] {
        let x = hit(&mut c, &s, key, v);
        assert!(x > 0.0, "key {key}, velocity {v}: a faded note never goes silent");
        assert_eq!(played(x, 1.0), 1, "key {key}, velocity {v}");
    }
    assert_eq!(played(hit(&mut c, &s, 70, 127), 1.0), 127, "key 70 and velocity 127 are full");
}

#[test]
fn key_and_velocity_factors_multiply_and_round_once() {
    let d = data(&[1.0]);
    let s = bank(&d);
    // Key 60 has 1/4 (key fade 3); velocity 10 has 2/4 (velocities from 9, velocity fade 3).
    let mut c = build(&[faded((60, 70), (9, 127), [3, 0, 3, 0], None, 0)]);
    // 10 x 1/4 x 1/2 = 1.25: once, 1. Rounded after each factor it would be round(2.5) = 3, then
    // round(1.5) = 2.
    assert_eq!(played(hit(&mut c, &s, 60, 10), 1.0), 1);
    // Ties go upward: 20 is full on the velocity axis, 20 x 1/4 = 5.
    assert_eq!(played(hit(&mut c, &s, 60, 20), 1.0), 5, "20 x 1/4 = 5");
    let mut c = build(&[faded((60, 70), (1, 127), [3, 0, 0, 0], None, 0)]);
    assert_eq!(played(hit(&mut c, &s, 60, 10), 1.0), 3, "10 x 1/4 = 2.5, ties upward");
    assert_eq!(played(hit(&mut c, &s, 60, 6), 1.0), 2, "6 x 1/4 = 1.5, ties upward");
    assert_eq!(played(hit(&mut c, &s, 61, 5), 1.0), 3, "5 x 2/4 = 2.5, ties upward");
    // Both factors, every velocity: one exact rounding, never a product of rounded steps.
    let mut c = build(&[faded((60, 70), (1, 127), [3, 0, 126, 0], None, 0)]);
    for v in 1..=127u32 {
        for key in 60..=63u8 {
            let fk = reference(u32::from(key), 60, 70, 3, 0);
            assert_eq!(played(hit(&mut c, &s, key, v as u8), 1.0), expected(v, fk, (v, 127)), "key {key}, velocity {v}");
        }
    }
}

#[test]
fn fade_scales_after_selection_never_moves_a_note_between_layers() {
    // Layer 0 (velocities 1..63) plays +1, layer 1 (64..127, a velocity fade of 63 at its low edge)
    // plays -1: the sign names the layer. The fade scales 64 down to 1, still on layer 1.
    let d = data(&[1.0, -1.0]);
    let s = bank(&d);
    let group = [faded((0, 127), (1, 63), [0; 4], Some((0, 0, 0)), 0), faded((0, 127), (64, 127), [0, 0, 63, 0], Some((0, 1, 0)), 1)];
    let plain = [faded((0, 127), (1, 63), [0; 4], None, 0), faded((0, 127), (64, 127), [0, 0, 63, 0], None, 1)];
    for (what, zones) in [("a key-map group", group), ("plain zones", plain)] {
        let mut c = build(&zones);
        for v in 1..=127u32 {
            let x = hit(&mut c, &s, 60, v as u8);
            if v < 64 {
                assert!(x > 0.0, "{what}: velocity {v} plays layer 0, got {x}");
                assert_eq!(played(x, 1.0), v, "{what}: layer 0 has no fade");
            } else {
                assert!(x < 0.0, "{what}: velocity {v} plays layer 1, got {x}");
                assert_eq!(played(x, -1.0), expected(v, (1, 1), reference(v, 64, 127, 63, 0)), "{what}: velocity {v}");
            }
        }
    }
}

#[test]
fn fade_applies_to_round_robin_members_equally() {
    // Three alternatives (sources 1, 2, 3), each with the key fade 4 over keys 60..80: key 61 is
    // 2/5, so velocity 100 plays 40 on every alternative, and the counter moves once per trigger.
    let d = data(&[1.0, 2.0, 3.0]);
    let s = bank(&d);
    let zones: Vec<Zone> = (0..3u8).map(|a| faded((60, 80), (1, 127), [4, 0, 0, 0], Some((0, 0, a)), u16::from(a))).collect();
    let mut c = build(&zones);
    let got: Vec<(f32, u32)> = (0..7)
        .map(|_| {
            let x = hit(&mut c, &s, 61, 100);
            let source = (x * 127.0 / 40.0).round();
            (source, played(x, source))
        })
        .collect();
    let want: Vec<(f32, u32)> = [1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0].into_iter().map(|a| (a, 40)).collect();
    assert_eq!(got, want);
    // A note in the fade and one outside it share the counter.
    assert_eq!(played(hit(&mut c, &s, 70, 100), 2.0), 100, "alternative 2 at full level");
    assert_eq!(played(hit(&mut c, &s, 60, 100), 3.0), 20, "alternative 3 at 1/5");
}

#[test]
fn stored_over_wide_and_pad_bound_fades_play_clamped_and_audible() {
    let d = data(&[1.0, 1.0]);
    let s = bank(&d);
    // Zone 0: keys 60..62 with key widths 5 and 5: used 2 and 0. Zone 1: bound to pad 4 (note 64),
    // with every width set: none is used.
    let zones = [faded((60, 62), (1, 127), [5, 5, 0, 0], None, 0), faded((64, 64), (1, 127), [3, 3, 9, 9], None, 1)];
    let mut pads: [Pad; PADS] = core::array::from_fn(Pad::new);
    pads[4] = Pad { zone: Some(1), ..Pad::new(4) };
    let mut t = vec![None; ZONES];
    t[0] = Some(zones[0]);
    t[1] = Some(zones[1]);
    assert_eq!(crate::patch::check_patch(&t, &pads), Ok(()), "the core accepts stored widths it clamps");
    let mut c = build_with(&zones, &pads);
    let keys: Vec<u32> = (60..=62).map(|k| played(hit(&mut c, &s, k, 90), 1.0)).collect();
    assert_eq!(keys, [30, 60, 90], "the low width clamped to the span 2, the high one to 0");
    assert_eq!(played(hit(&mut c, &s, 64, 90), 1.0), 90, "the pad's zone plays no fade");
    assert_eq!(played(hit(&mut c, &s, 64, 3), 1.0), 3, "not even inside its stored velocity fade");
    assert_eq!(played(hit_event(&mut c, &s, EventKind::PadPress { pad: 4, velocity: 7 }), 1.0), 7, "nor through a press");
}

/// One schedule over faded zones (plain, a key-map group, a pad) gives the same bits and state in
/// blocks of 64 and of 512, and the fades change what it plays.
#[test]
fn block_sizes_render_fades_the_same() {
    let values: Vec<Vec<f32>> = (0..4).map(|k| (0..LEN).map(|n| ((k * 31 + n * 7) % 17) as f32 / 17.0 - 0.5).collect()).collect();
    let s = bank(&values);
    let zones = |w: [u8; 4]| {
        vec![
            faded((20, 70), (1, 127), w, None, 0),
            faded((40, 100), (1, 90), w, Some((3, 0, 0)), 1),
            faded((40, 100), (1, 90), w, Some((3, 0, 1)), 2),
            faded((40, 100), (91, 127), w, Some((3, 1, 0)), 3),
            faded((0, 127), (1, 127), w, None, 0),
        ]
    };
    let mut pads: [Pad; PADS] = core::array::from_fn(Pad::new);
    pads[0] = Pad { zone: Some(4), ..Pad::new(0) };
    let mut sched = Vec::new();
    for i in 0..80u64 {
        let key = 15 + (i * 13 % 95) as u8;
        sched.push((i * 51 + i % 5, EventKind::NoteOn { channel: 0, key, velocity: 1 + (i * 29 % 127) as u8, id: i as i32 }));
        sched.push((i * 51 + 30, EventKind::NoteOff { channel: 0, key, velocity: 64, id: i as i32 }));
    }
    sched.sort_by_key(|e| e.0);
    let run = |w: [u8; 4], size: usize| {
        let mut c = build_with(&zones(w), &pads);
        super::blocks::run(&mut c, &s[..], &sched, &[size], 4096)
    };
    let w = [9, 7, 20, 11];
    let (a, ar, fa) = run(w, 64);
    let (b, br, fb) = run(w, 512);
    assert!(a.iter().filter(|x| **x != 0.0).count() > 1000, "the schedule sounds");
    let first = a.iter().chain(&ar).zip(b.iter().chain(&br)).position(|(x, y)| x.to_bits() != y.to_bits());
    assert_eq!(first, None, "the first sample where blocks of 64 and 512 differ");
    assert_eq!(fa, fb, "the state fingerprints differ");
    let (n, nr, _) = run([0; 4], 512);
    assert_ne!(hash(&a, &ar), hash(&n, &nr), "the fades reach the render");
}
