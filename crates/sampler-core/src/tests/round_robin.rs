//! Round-robin selection in key-map groups (ADR 0041, "Playback, controls and modulation", and its
//! amendment of 2026-11-08, items 4 and 10). Every alternative plays a constant source of its own
//! number, so the first output frame of a trigger names the alternative it played (key tracking
//! off, unity level, velocity sensitivity 0: the frame is the source value exactly). The random
//! references below are written here, independently of the core.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

use super::{SR, unity};
use crate::event::{Context, Event, EventKind};
use crate::patch::{GroupMode, Membership, Pad, PatchError, PlayMode, Zone, check_patch};
use crate::playhead::Region;
use crate::reader::SliceSource;
use crate::{GROUPS, PADS, SamplerCore, ZONES, selection};

const LEN: usize = 64;

/// One zone of a test patch: membership (group, layer, alternative), keys, velocities, source.
#[derive(Clone, Copy)]
struct Z {
    m: Option<(u8, u8, u8)>,
    keys: (u8, u8),
    vels: (u8, u8),
    source: u16,
}

fn member(g: u8, l: u8, a: u8, vels: (u8, u8), source: u16) -> Z {
    Z { m: Some((g, l, a)), keys: (0, 127), vels, source }
}

fn plain(vels: (u8, u8), source: u16) -> Z {
    Z { m: None, keys: (0, 127), vels, source }
}

fn zone(z: &Z) -> Zone {
    let playback = crate::patch::Playback { velocity_sensitivity: 0.0, ..unity(PlayMode::OneShot, 60, 0) };
    Zone {
        keys: z.keys,
        velocities: z.vels,
        membership: z.m.map(|(group, layer, alternative)| Membership { group, layer, alternative }),
        playback,
        ..Zone::new(Region::whole(z.source, LEN as u64))
    }
}

fn table(zones: &[Z]) -> Vec<Option<Zone>> {
    let mut t = vec![None; ZONES];
    for (i, z) in zones.iter().enumerate() {
        t[i] = Some(zone(z));
    }
    t
}

fn modes(entries: &[(u8, GroupMode)]) -> [GroupMode; GROUPS] {
    let mut m = [GroupMode::Sequential; GROUPS];
    for &(g, mode) in entries {
        m[usize::from(g)] = mode;
    }
    m
}

/// A core at 48 kHz with `seed`, the zones from slot 0, default pads and the group modes.
fn build(zones: &[Z], entries: &[(u8, GroupMode)], seed: u32) -> SamplerCore {
    let mut c = SamplerCore::new();
    c.prepare(SR, seed);
    c.set_patch(&table(zones), &core::array::from_fn(Pad::new)).unwrap();
    c.set_group_modes(&modes(entries));
    c
}

/// Constant sources, source `i` holding `values[i]`.
fn data(values: &[f32]) -> Vec<Vec<f32>> {
    values.iter().map(|&v| vec![v; LEN]).collect()
}

fn bank(data: &[Vec<f32>]) -> Vec<SliceSource<'_>> {
    data.iter().map(|d| SliceSource::mono(d, SR)).collect()
}

const STOPPED: Context = Context { frame: 0, tempo: 120.0, quarters: None };

fn playing(q: f64) -> Context {
    Context { quarters: Some(q), ..STOPPED }
}

/// One note-on at frame 0 of a block in `ctx`: the first output frame, then every voice cut.
fn hit(c: &mut SamplerCore, sources: &[SliceSource<'_>], ctx: &Context, key: u8, velocity: u8) -> f32 {
    let (mut l, mut r) = ([0.0f32; 2], [0.0f32; 2]);
    c.process(ctx, sources, &[Event::on(0, key, velocity)], &mut [&mut l, &mut r]);
    c.process(ctx, sources, &[Event::new(0, EventKind::AllSoundOff)], &mut []);
    l[0]
}

/// The independent reference of item 4: group `group`'s first `n` alternatives among `count`.
fn reference(seed: u32, group: u8, count: u32, n: usize) -> Vec<u32> {
    let s = if seed == 0 { 1 } else { seed };
    let mut x = s ^ (u32::from(group) + 1).wrapping_mul(0x9E37_79B9);
    if x == 0 {
        x = 1;
    }
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x % count
        })
        .collect()
}

/// A one-layer group `g` of `count` alternatives on sources `first..first + count`.
fn one_layer(g: u8, count: u8, first: u16) -> Vec<Z> {
    (0..count).map(|a| member(g, 0, a, (1, 127), first + u16::from(a))).collect()
}

#[test]
fn sequential_cycles_alternatives() {
    // Layer 0 (velocities 1-63) plays 1, 2, 3; layer 1 (64-127) plays 10, 20, 30.
    let mut zones: Vec<Z> = (0..3).map(|a| member(0, 0, a, (1, 63), u16::from(a))).collect();
    zones.extend((0..3).map(|a| member(0, 1, a, (64, 127), 3 + u16::from(a))));
    let d = data(&[1.0, 2.0, 3.0, 10.0, 20.0, 30.0]);
    let s = bank(&d);
    let mut c = build(&zones, &[], 1);
    let (mut low, mut high) = (Vec::new(), Vec::new());
    for high_note in [false, true, false, false, true, false, false, true, false, false] {
        if high_note {
            high.push(hit(&mut c, &s, &STOPPED, 60, 100));
        } else {
            low.push(hit(&mut c, &s, &STOPPED, 60, 30));
        }
    }
    assert_eq!(low, [1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0]);
    assert_eq!(high, [10.0, 20.0, 30.0], "each layer keeps its own counter");
}

#[test]
fn random_is_deterministic_for_a_seed() {
    let d = data(&[1.0, 2.0, 3.0, 4.0, 5.0]);
    let s = bank(&d);
    let sequence = |seed: u32| -> Vec<u32> {
        let mut c = build(&one_layer(0, 5, 0), &[(0, GroupMode::Random)], seed);
        (0..32).map(|_| hit(&mut c, &s, &STOPPED, 60, 100) as u32 - 1).collect()
    };
    for seed in [1, 12345, 0] {
        assert_eq!(sequence(seed), reference(seed, 0, 5, 32), "seed {seed}");
    }
    assert_eq!(sequence(0), sequence(1), "seed 0 plays as seed 1");
    assert_ne!(sequence(12345), sequence(1));
}

#[test]
fn silent_missing_media_still_counts() {
    // Alternative 1 names a source that is not there: it plays silence, and still takes its turn.
    let zones = [member(0, 0, 0, (1, 127), 0), member(0, 0, 1, (1, 127), 9), member(0, 0, 2, (1, 127), 2)];
    let d = data(&[1.0, 2.0, 3.0]);
    let s = bank(&d);
    let mut c = build(&zones, &[], 1);
    let got: Vec<f32> = (0..4).map(|_| hit(&mut c, &s, &STOPPED, 60, 100)).collect();
    assert_eq!(got, [1.0, 0.0, 3.0, 1.0]);
    assert_eq!(c.diagnostics().refused_starts, 1);
}

#[test]
fn refused_trigger_does_not_advance() {
    // No voice for the group: eight silent plain zones (velocities 1-63, before the group in zone
    // order) fill the note's eight layers, so a soft note admits no layer of the group.
    let mut zones: Vec<Z> = (0..8).map(|_| plain((1, 63), 0)).collect();
    zones.extend((0..3).map(|a| member(0, 0, a, (1, 127), 1 + u16::from(a))));
    let d = data(&[0.0, 1.0, 2.0, 3.0]);
    let s = bank(&d);
    let mut c = build(&zones, &[], 1);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 30), 0.0, "the group is left out");
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 1.0, "and its counter did not move");
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 2.0);

    // Muted: a muted pad on the note still takes one of its layers, so seven plain zones fill the
    // rest and the group is refused, without advancing.
    let mut zones: Vec<Z> = (0..7).map(|_| plain((1, 63), 0)).collect();
    zones.extend((0..3).map(|a| member(0, 0, a, (1, 127), 1 + u16::from(a))));
    zones.push(Z { m: None, keys: (60, 60), vels: (1, 127), source: 4 });
    let d = data(&[0.0, 1.0, 2.0, 3.0, 50.0]);
    let s = bank(&d);
    let mut c = SamplerCore::new();
    c.prepare(SR, 1);
    let mut pads: [Pad; PADS] = core::array::from_fn(Pad::new);
    pads[0] = Pad { zone: Some(10), mute: true, ..Pad::new(0) };
    c.set_patch(&table(&zones), &pads).unwrap();
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 30), 0.0, "the muted pad and seven zones leave no layer");
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 1.0, "the counter did not move");
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 2.0);
    c.process(&STOPPED, &s[..], &[Event::new(0, EventKind::PadPress { pad: 0, velocity: 100 })], &mut []);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 3.0, "a refused press of the muted pad advances nothing");
}

#[test]
fn reset_restarts_the_counter() {
    let d = data(&[1.0, 2.0, 3.0]);
    let s = bank(&d);
    let mut c = build(&one_layer(0, 3, 0), &[], 1);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 1.0);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 2.0);
    c.reset();
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 1.0, "reset()");
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 2.0);
    let (mut l, mut r) = ([0.0f32; 2], [0.0f32; 2]);
    c.process(&STOPPED, &s[..], &[Event::new(0, EventKind::Reset), Event::on(0, 60, 100)], &mut [&mut l, &mut r]);
    assert_eq!(l[0], 1.0, "the Reset event");
}

#[test]
fn prepare_restarts_the_rng() {
    let d = data(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let s = bank(&d);
    let mut c = build(&one_layer(0, 6, 0), &[(0, GroupMode::Random)], 12345);
    let first: Vec<u32> = (0..8).map(|_| hit(&mut c, &s, &STOPPED, 60, 100) as u32 - 1).collect();
    assert_eq!(first, reference(12345, 0, 6, 8));
    c.prepare(SR, 12345);
    let again: Vec<u32> = (0..8).map(|_| hit(&mut c, &s, &STOPPED, 60, 100) as u32 - 1).collect();
    assert_eq!(again, first, "prepare restarts the random state");
}

#[test]
fn counters_restart_at_stop_and_at_play_start() {
    let d = data(&[1.0, 2.0, 3.0, 4.0, 5.0]);
    let s = bank(&d);
    let mut c = build(&one_layer(0, 5, 0), &[], 1);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 1.0);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 2.0);
    // The first playing block restarts the counter before its note at frame 0.
    assert_eq!(hit(&mut c, &s, &playing(0.0), 60, 100), 1.0, "at play start");
    assert_eq!(hit(&mut c, &s, &playing(0.5), 60, 100), 2.0);
    assert_eq!(hit(&mut c, &s, &playing(1.0), 60, 100), 3.0);
    // And the first stopped block, before its note.
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 1.0, "at stop");
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 2.0);

    // The random state too.
    let mut c = build(&one_layer(0, 5, 0), &[(0, GroupMode::Random)], 77);
    let want = reference(77, 0, 5, 3);
    let draws = |c: &mut SamplerCore, ctx: &Context| -> Vec<u32> { (0..3).map(|_| hit(c, &s, ctx, 60, 100) as u32 - 1).collect() };
    assert_eq!(draws(&mut c, &STOPPED), want);
    assert_eq!(draws(&mut c, &playing(0.0)), want, "at play start");
    assert_eq!(draws(&mut c, &STOPPED), want, "at stop");
}

#[test]
fn counters_survive_a_locate_a_cycle_wrap_and_a_patch_edit() {
    let zones = one_layer(0, 6, 0);
    let d = data(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let s = bank(&d);
    let mut c = build(&zones, &[], 1);
    assert_eq!(hit(&mut c, &s, &playing(0.0), 60, 100), 1.0);
    assert_eq!(hit(&mut c, &s, &playing(100.0), 60, 100), 2.0, "a locate");
    assert_eq!(hit(&mut c, &s, &playing(4.0), 60, 100), 3.0, "a cycle wrap");
    let mut edited = table(&zones);
    for z in edited.iter_mut().flatten() {
        z.playback.release_ms = 20.0;
    }
    c.set_patch(&edited, &core::array::from_fn(Pad::new)).unwrap();
    assert_eq!(hit(&mut c, &s, &playing(4.5), 60, 100), 4.0, "a patch edit");
    c.set_group_modes(&modes(&[]));
    assert_eq!(hit(&mut c, &s, &playing(5.0), 60, 100), 5.0, "a mode edit");
}

#[test]
fn per_group_random_start_state_matches_the_reference() {
    // Item 4's start state, against the formula written out here.
    let zero = 0x9E37_79B9u32; // seed whose xor with group 0's term is 0
    for seed in [1u32, 12345, 0, zero, 2u32.wrapping_mul(zero)] {
        for g in [0u8, 1, 255] {
            let s = seed.max(1);
            let x = s ^ (u32::from(g) + 1).wrapping_mul(0x9E37_79B9);
            assert_eq!(selection::start_state(seed, g), if x == 0 { 1 } else { x }, "seed {seed}, group {g}");
        }
    }
    assert_eq!(selection::start_state(zero, 0), 1, "a zero start state is 1");
    assert_eq!(selection::start_state(2u32.wrapping_mul(zero), 1), 1);

    // Groups 0, 1 and 255 on their own key ranges, seven alternatives each, interleaved.
    let mut zones = Vec::new();
    for (i, g) in [0u8, 1, 255].into_iter().enumerate() {
        for a in 0..7u8 {
            zones.push(Z { m: Some((g, 0, a)), keys: (40 * i as u8, 40 * i as u8 + 39), vels: (1, 127), source: (i * 7) as u16 + u16::from(a) });
        }
    }
    let values: Vec<f32> = (0..21).map(|k| (k + 1) as f32).collect();
    let d = data(&values);
    let s = bank(&d);
    for seed in [1u32, 12345, 0, zero] {
        let mut c = build(&zones, &[(0, GroupMode::Random), (1, GroupMode::Random), (255, GroupMode::Random)], seed);
        let mut got = [Vec::new(), Vec::new(), Vec::new()];
        for k in 0..30usize {
            let i = [0usize, 1, 2, 2, 0, 1, 1][k % 7];
            let v = hit(&mut c, &s, &STOPPED, 40 * i as u8 + 5, 100) as u32;
            got[i].push(v - 1 - 7 * i as u32);
        }
        for (i, g) in [0u8, 1, 255].into_iter().enumerate() {
            assert_eq!(got[i], reference(seed, g, 7, got[i].len()), "seed {seed}, group {g}");
        }
    }

    // One note admits two layers of group 5: layer 0 draws first, then layer 1, whatever the zone
    // order (layer 1's zones come first here).
    let mut zones: Vec<Z> = (0..3).map(|a| member(5, 1, a, (1, 127), 4 + u16::from(a))).collect();
    zones.extend((0..4).map(|a| member(5, 0, a, (1, 127), u16::from(a))));
    let d = data(&[1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0]);
    let s = bank(&d);
    let mut c = build(&zones, &[(5, GroupMode::Random)], 4242);
    let mut x = 4242u32 ^ 6u32.wrapping_mul(0x9E37_79B9);
    for _ in 0..12 {
        let mut step = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        let (w0, w1) = (step(), step());
        let want = (w0 % 4 + 1) as f32 + 10.0 * (w1 % 3 + 1) as f32;
        assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), want);
    }
}

#[test]
fn different_groups_have_independent_state() {
    // Sequential: groups 0 (1, 2, 3) and 1 (10, 20) on the same keys; one note admits both.
    let mut zones = one_layer(0, 3, 0);
    zones.extend(one_layer(1, 2, 3));
    let d = data(&[1.0, 2.0, 3.0, 10.0, 20.0]);
    let s = bank(&d);
    let mut c = build(&zones, &[], 1);
    let got: Vec<f32> = (0..4).map(|_| hit(&mut c, &s, &STOPPED, 60, 100)).collect();
    assert_eq!(got, [11.0, 22.0, 13.0, 21.0]);

    // Random: group 4's draws are its own whatever group 3 draws in between.
    let mut zones: Vec<Z> = (0..5).map(|a| Z { m: Some((3, 0, a)), keys: (0, 59), vels: (1, 127), source: u16::from(a) }).collect();
    zones.extend((0..5).map(|a| Z { m: Some((4, 0, a)), keys: (60, 127), vels: (1, 127), source: 5 + u16::from(a) }));
    let values: Vec<f32> = (0..10).map(|k| (k + 1) as f32).collect();
    let d = data(&values);
    let s = bank(&d);
    let mut c = build(&zones, &[(3, GroupMode::Random), (4, GroupMode::Random)], 999);
    let mut four = Vec::new();
    for k in 0..20 {
        for _ in 0..(k % 3) {
            hit(&mut c, &s, &STOPPED, 30, 100);
        }
        four.push(hit(&mut c, &s, &STOPPED, 70, 100) as u32 - 6);
    }
    assert_eq!(four, reference(999, 4, 5, 20));
}

#[test]
fn group_shape_refused_at_set_patch() {
    let valid = || {
        let mut z: Vec<Z> = (0..3).map(|a| member(0, 0, a, (1, 63), 0)).collect();
        z.extend((0..2).map(|a| member(0, 1, a, (64, 127), 0)));
        z
    };
    let pads: [Pad; PADS] = core::array::from_fn(Pad::new);
    let mut c = SamplerCore::new();
    c.prepare(SR, 1);
    assert_eq!(c.set_patch(&table(&valid()), &pads), Ok(()));
    let cases: [(&str, Box<dyn Fn(&mut Vec<Z>)>, PatchError); 7] = [
        ("nine alternatives", Box::new(|z| z.extend((3..9).map(|a| member(0, 0, a, (1, 63), 0)))), PatchError::Range),
        ("nine layers", Box::new(|z| z.extend((2..9).map(|l| member(0, l, 0, (1, 127), 0)))), PatchError::Range),
        ("unequal velocity ranges in a layer", Box::new(|z| z[1].vels = (1, 64)), PatchError::GroupShape),
        ("unequal key ranges in a group", Box::new(|z| z[4].keys = (0, 100)), PatchError::GroupShape),
        ("unequal key ranges in a layer", Box::new(|z| z[0].keys = (10, 127)), PatchError::GroupShape),
        ("a gap in the alternatives", Box::new(|z| z[1].m = Some((0, 0, 3))), PatchError::GroupShape),
        ("a repeated alternative", Box::new(|z| z[2].m = Some((0, 0, 1))), PatchError::GroupShape),
    ];
    for (what, edit, err) in cases {
        let mut z = valid();
        edit(&mut z);
        assert_eq!(check_patch(&table(&z), &pads), Err(err), "{what}: check_patch");
        assert_eq!(c.set_patch(&table(&z), &pads), Err(err), "{what}: set_patch");
    }
    // A refused patch leaves the old one in place.
    let d = data(&[1.0]);
    let s = bank(&d);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 30), 1.0);
    // set_zone keeps the shape too: removing alternative 1 of three leaves a gap.
    assert_eq!(c.set_zone(1, None), Err(PatchError::GroupShape));
    assert_eq!(c.set_zone(2, None), Ok(()), "removing the last alternative keeps it contiguous");
    let mut gap = zone(&member(0, 0, 4, (1, 63), 0));
    assert_eq!(c.set_zone(9, Some(gap)), Err(PatchError::GroupShape));
    gap.membership = Some(Membership { group: 0, layer: 0, alternative: 2 });
    assert_eq!(c.set_zone(9, Some(gap)), Ok(()), "alternative 2 again, now in slot 9");
}

#[test]
fn pad_bound_zones_never_enter_the_scan() {
    // Pad 0 (note 60) binds zone 0, whose own ranges also hold the note: it plays once, through
    // its pad, beside the group's alternative.
    let mut zones = vec![plain((1, 127), 0)];
    zones.extend(one_layer(0, 2, 1));
    let d = data(&[1.0, 10.0, 20.0]);
    let s = bank(&d);
    let mut c = SamplerCore::new();
    c.prepare(SR, 1);
    let mut pads: [Pad; PADS] = core::array::from_fn(Pad::new);
    pads[0] = Pad { zone: Some(0), ..Pad::new(0) };
    c.set_patch(&table(&zones), &pads).unwrap();
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 11.0);
    assert_eq!(hit(&mut c, &s, &STOPPED, 60, 100), 21.0);
    // A pad cannot bind a group member.
    pads[1] = Pad { zone: Some(1), ..Pad::new(1) };
    assert_eq!(c.set_patch(&table(&zones), &pads), Err(PatchError::BoundMembership));
}

/// Plain zones only (no membership): overlapping key and velocity ranges, Gate and One-shot, a
/// source at another rate, offsets inside blocks. Pure arithmetic (no transcendental functions),
/// so the bits hold on every platform.
fn plain_render(c: &mut SamplerCore) -> u64 {
    let mut x = 0x1357_9BDFu32;
    let mut noise = move || {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    };
    let a: Vec<f32> = (0..2000).map(|_| noise()).collect();
    let b: Vec<f32> = (0..2000).map(|_| noise() * 0.5).collect();
    let sources = [SliceSource::mono(&a, SR), SliceSource::mono(&b, 44_100.0)];
    let mut ranges = [((0u8, 127u8), (1u8, 127u8)); 6];
    ranges[1] = ((40, 80), (1, 64));
    ranges[2] = ((60, 72), (65, 127));
    ranges[3] = ((0, 59), (1, 127));
    ranges[4] = ((60, 60), (100, 127));
    ranges[5] = ((50, 90), (30, 90));
    for (i, (keys, velocities)) in ranges.into_iter().enumerate() {
        let mode = if i % 2 == 0 { PlayMode::Gate } else { PlayMode::OneShot };
        let playback = crate::patch::Playback { velocity_sensitivity: 0.5, ..unity(mode, 60, 300) };
        let zone = Zone { keys, velocities, playback, ..Zone::new(Region::whole((i % 2) as u16, 1500)) };
        c.set_zone(i, Some(zone)).unwrap();
    }
    let sched: Vec<(u64, EventKind)> = (0..40u64)
        .flat_map(|i| {
            let key = 30 + (i * 11 % 60) as u8;
            let on = EventKind::NoteOn { channel: 0, key, velocity: 1 + (i * 37 % 127) as u8, id: i as i32 };
            let off = EventKind::NoteOff { channel: 0, key, velocity: 64, id: i as i32 };
            [(i * 97, on), (i * 97 + 400, off)]
        })
        .collect();
    let mut sched = sched;
    sched.sort_by_key(|e| e.0);
    let (l, r, _) = super::blocks::run(c, &sources[..], &sched, &[256], 5000);
    assert!(l.iter().filter(|x| **x != 0.0).count() > 2000, "a golden of silence proves nothing");
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in l.iter().chain(&r) {
        for byte in v.to_bits().to_le_bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// The bits plain zones render, captured from the core before round robin existed.
const PLAIN_GOLDEN: u64 = 0x9215_829d_50de_2a1d;

#[test]
fn plain_zones_unchanged() {
    let mut c = SamplerCore::new();
    c.prepare(SR, 5);
    assert_eq!(plain_render(&mut c), PLAIN_GOLDEN, "plain zones render the bits they did before round robin");
    // A random group on keys no plain note reaches leaves them alone.
    let mut c = SamplerCore::new();
    c.prepare(SR, 5);
    for a in 0..3u8 {
        let z = Zone { keys: (120, 127), membership: Some(Membership { group: 9, layer: 0, alternative: a }), ..Zone::new(Region::whole(0, 100)) };
        c.set_zone(200 + usize::from(a), Some(z)).unwrap();
    }
    c.set_group_modes(&modes(&[(9, GroupMode::Random)]));
    assert_eq!(plain_render(&mut c), PLAIN_GOLDEN);
}

/// One schedule with two groups playing (one random, one sequential, two layers each) gives the
/// same bits and state in blocks of 64 and of 512, the transport playing throughout.
#[test]
fn block_sizes_render_a_group_the_same() {
    let mut zones = Vec::new();
    for g in 0..2u8 {
        for l in 0..2u8 {
            for a in 0..3u8 {
                let source = u16::from(g * 6 + l * 3 + a);
                zones.push(Z { m: Some((g, l, a)), keys: (48 * g, 48 * g + 60), vels: if l == 0 { (1, 90) } else { (60, 127) }, source });
            }
        }
    }
    let values: Vec<Vec<f32>> = (0..12).map(|k| (0..LEN).map(|n| ((k * 31 + n * 7) % 17) as f32 / 17.0 - 0.5).collect()).collect();
    let s = bank(&values);
    let mut sched = Vec::new();
    for i in 0..60u64 {
        let key = 20 + (i * 13 % 90) as u8;
        sched.push((i * 67 + i % 5, EventKind::NoteOn { channel: 0, key, velocity: 1 + (i * 29 % 127) as u8, id: i as i32 }));
        sched.push((i * 67 + 40, EventKind::NoteOff { channel: 0, key, velocity: 64, id: i as i32 }));
    }
    sched.sort_by_key(|e| e.0);
    let run = |size: usize| -> (Vec<f32>, u64) {
        let mut c = build(&zones, &[(0, GroupMode::Random)], 31337);
        let total = 4096;
        let mut out = vec![0.0f32; total];
        let mut at = 0;
        while at < total {
            let n = size.min(total - at);
            let events: Vec<Event> =
                sched.iter().filter(|e| e.0 >= at as u64 && e.0 < (at + n) as u64).map(|e| Event::new((e.0 - at as u64) as u32, e.1)).collect();
            let ctx = Context { frame: at as u64, tempo: 120.0, quarters: Some(at as f64 / 24_000.0) };
            c.process(&ctx, &s[..], &events, &mut [&mut out[at..at + n]]);
            at += n;
        }
        let mut h = DefaultHasher::new();
        c.fingerprint(&mut h);
        (out, h.finish())
    };
    let (a, fa) = run(64);
    let (b, fb) = run(512);
    assert!(a.iter().filter(|x| **x != 0.0).count() > 1000, "the schedule sounds");
    let first = a.iter().zip(&b).position(|(x, y)| x.to_bits() != y.to_bits());
    assert_eq!(first, None, "the first frame where blocks of 64 and 512 differ");
    assert_eq!(fa, fb, "the state fingerprints differ");
}
