//! The forward-loop crossfade (ADR 0041 "Loop points and crossfade reference", amendment
//! 2026-11-08, item 2) against an independent scalar reference of the ADR's equations written
//! here with plain while-loops: forward and reverse, tiny loops, X = 0, the largest X and a stored
//! X past it, fractional steps, several wraps in one block and in one step, source rates unlike
//! the session's, and release; plus the mirror, the effective period, the seam, the clamp, the
//! intro, block sizes and the X = 0 bypass.

use super::blocks::{run, source_data};
use super::{SR, core, render};
use crate::event::{Event, EventKind};
use crate::patch::{Pad, PlayMode, Playback, Zone};
use crate::playhead::{LoopMode, Region};
use crate::reader::{SliceSource, cubic};
use crate::{MAX_STEP, MOD_BLOCK, PADS, ZONES};

/// The ADR's equations: the forward loop with crossfade X (already the used value), its mirror in
/// reverse, every tap mapped with while-loops. The outgoing read takes the playhead's taps (intro
/// taps on the entry side until the first wrap); the incoming read's taps are always loop taps.
fn reference(data: &[f32], r: &Region, reverse: bool, step: f64, frames: usize, x: u64) -> Vec<f32> {
    assert_eq!(r.loop_mode, LoopMode::Forward);
    let (start, end) = (r.start as i64, r.end as i64);
    let (ls, le) = (r.loop_start as i64, r.loop_end as i64);
    let (len, xi) = (le - ls, x as i64);
    let tap = |mut j: i64, looped: bool| -> f32 {
        let exit = if reverse { j < ls } else { j >= le };
        let entry = if reverse { j >= le } else { j < ls };
        if exit || (entry && looped) {
            while j >= le {
                j -= len;
            }
            while j < ls {
                j += len;
            }
            return data[j as usize];
        }
        if start <= j && j < end { data[j as usize] } else { 0.0 }
    };
    let read = |p: f64, looped: bool| -> f32 {
        let i = p.floor();
        let t = (p - i) as f32;
        let i = i as i64;
        cubic(tap(i - 1, looped), tap(i, looped), tap(i + 1, looped), tap(i + 2, looped), t)
    };
    let mut p = if reverse { (end - 1) as f64 } else { start as f64 };
    let mut wrapped = false;
    let mut out = vec![0.0; frames];
    for y in out.iter_mut() {
        let a = read(p, wrapped);
        *y = a;
        if x > 0 && !reverse && p >= (le - xi) as f64 {
            // The final X frames: w = (p - (loop_end - X)) / X, incoming loop_start + that.
            let d = p - (le - xi) as f64;
            let b = read(ls as f64 + d, true);
            *y = a + (b - a) * (d / x as f64) as f32;
        }
        if x > 0 && reverse && p <= (ls + xi - 1) as f64 {
            // The same in u = (loop_start + loop_end - 1) - p.
            let d = (ls + xi - 1) as f64 - p;
            let b = read((le - 1) as f64 - d, true);
            *y = a + (b - a) * (d / x as f64) as f32;
        }
        if reverse {
            p -= step;
            // Past the exit: continue L - X up (the mirror of loop_start + X).
            while if x > 0 { p <= (ls - 1) as f64 } else { p < ls as f64 } {
                p += (len - xi) as f64;
                wrapped = true;
            }
        } else {
            p += step;
            while p >= le as f64 {
                p -= (len - xi) as f64;
                wrapped = true;
            }
        }
    }
    out
}

/// The crossfade the ADR uses for stored `x`: `min(X, floor(L / 2))` on a forward loop.
fn used(r: &Region, x: u64) -> u64 {
    x.min((r.loop_end - r.loop_start) / 2)
}

/// Plays `region` of `data` as a held Gate zone at `pitch` (the key over root 60, a transpose and
/// the source rate), every gain unity; `off`: a note-off at that frame with a release of that many
/// milliseconds. Returns the output and the step.
fn play(data: &[f32], region: Region, reverse: bool, pitch: (u8, f32, f32), frames: usize, off: Option<(u32, f32)>) -> (Vec<f32>, f64) {
    let (key, transpose, source_rate) = pitch;
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
        release_ms: off.map_or(0.0, |o| o.1),
        ..Playback::zone()
    };
    c.set_zone(0, Some(Zone { playback, ..Zone::new(region) })).unwrap();
    let mut events = vec![Event::new(0, EventKind::NoteOn { channel: 0, key, velocity: 127, id: -1 })];
    if let Some((at, _)) = off {
        events.push(Event::new(at, EventKind::NoteOff { channel: 0, key, velocity: 64, id: -1 }));
    }
    let out = render(&mut c, &[SliceSource::mono(data, source_rate)], &events, frames);
    let semis = f64::from(key) - 60.0 + f64::from(transpose);
    let step = (f64::from(source_rate) / f64::from(SR) * 2f64.powf(semis / 12.0)).min(MAX_STEP);
    (out, step)
}

fn data() -> Vec<f32> {
    (0..64).map(|i| ((i * 37 % 64) as f32 - 31.5) / 32.0).collect()
}

fn assert_bits(got: &[f32], want: &[f32], what: &str) {
    assert_eq!(got.len(), want.len());
    for (n, (a, b)) in got.iter().zip(want).enumerate() {
        assert_eq!(a.to_bits(), b.to_bits(), "{what} frame {n}: {a} != {b}");
    }
}

/// The forward loops and the stored crossfades the reference cases play (some past the maximum).
fn loops() -> Vec<(Region, u64)> {
    let base = Region { start: 2, end: 50, ..Region::whole(0, 64) };
    let from_zero = Region { start: 0, end: 50, ..Region::whole(0, 64) };
    let mut v = Vec::new();
    for (r, xs) in [
        (base.looped(LoopMode::Forward, 10, 30), &[0u64, 1, 4, 10, 25][..]),
        (base.looped(LoopMode::Forward, 20, 22), &[1, 5]),
        (base.looped(LoopMode::Forward, 20, 23), &[1, 2]),
        (base.looped(LoopMode::Forward, 2, 50), &[7, 24]),
        (from_zero.looped(LoopMode::Forward, 0, 40), &[3, 20]),
    ] {
        v.extend(xs.iter().map(|&x| (r.crossfaded(x), x)));
    }
    v
}

#[test]
fn crossfade_matches_reference_equations_sample_for_sample() {
    let data = data();
    // (key, transpose, source rate): steps 1, 0.5, 0.75, 1.5 and 3.3 from the rate, a 44.1 kHz
    // source, a fractional pitch, and steps past a loop's length (several wraps in one step).
    let pitches = [
        (60, 0.0, 48_000.0),
        (60, 0.0, 24_000.0),
        (60, 0.0, 36_000.0),
        (60, 0.0, 72_000.0),
        (60, 0.0, 158_400.0),
        (60, 0.0, 44_100.0),
        (67, -10.3, 44_100.0),
        (100, 0.0, 48_000.0),
        (127, 12.0, 96_000.0),
    ];
    let mut cases = 0;
    for (region, x) in loops() {
        for reverse in [false, true] {
            for (key, transpose, rate) in pitches {
                let frames = 200 + MOD_BLOCK;
                let (out, step) = play(&data, region, reverse, (key, transpose, rate), frames, None);
                let want = reference(&data, &region, reverse, step, frames, used(&region, x));
                assert_bits(&out, &want, &format!("{region:?} reverse {reverse} key {key} transpose {transpose} rate {rate} step {step}"));
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 13 * 2 * 9);
    // Release: a note-off inside the loop, released over 2 ms; the output is the reference times
    // the envelope (read from a constant source over the same loop).
    let ones = vec![1.0f32; 64];
    for (region, x) in loops().into_iter().filter(|(_, x)| *x > 0) {
        for reverse in [false, true] {
            for rate in [48_000.0, 36_000.0] {
                let frames = 300;
                let off = Some((150, 10.0));
                let (out, step) = play(&data, region, reverse, (60, 0.0, rate), frames, off);
                let (env, _) = play(&ones, region, reverse, (60, 0.0, rate), frames, off);
                let want = reference(&data, &region, reverse, step, frames, used(&region, x));
                let want: Vec<f32> = want.iter().zip(&env).enumerate().map(|(n, (y, g))| if n < 150 { *y } else { y * g }).collect();
                assert!(env[150..].iter().all(|g| *g > 0.0) && env[299] < 1.0, "the release sounds through the render");
                assert_bits(&out, &want, &format!("release: {region:?} reverse {reverse} rate {rate}"));
            }
        }
    }
}

/// The golden render of a patch with every kind of loop (forward and reverse forward loops, a
/// three-frame loop, ping-pong) and plain regions, one-shot and Gate, envelopes, declick, pan and
/// choke, over integer-noise sources at 44.1 and 48 kHz (fractional steps). Every value on its path
/// is IEEE arithmetic: no shaper, no matrix, key tracking off with no transpose (the step is
/// `pow(2, 0) = 1` times an exact rate ratio), 0 dB, so the bits are the same on every platform.
/// Captured on the tree before crossfades existed.
fn golden_render() -> u64 {
    let mut x = 0x1357_9BDFu32;
    let mut noise = move || {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    };
    let mono: Vec<f32> = (0..3000).map(|_| noise()).collect();
    let left: Vec<f32> = (0..3000).map(|_| noise() * 0.5).collect();
    let right: Vec<f32> = (0..3000).map(|_| noise() * 0.25).collect();
    let sources = [SliceSource::mono(&mono, 44_100.0), SliceSource::stereo(&left, &right, 48_000.0)];
    let mut c = crate::SamplerCore::new();
    c.prepare(SR, 99);
    for p in 0..24usize {
        // Each kind of region on both sources, so every loop runs at a whole and a fractional step.
        let source = ((p / 4) % 2) as u16;
        let region = match p % 4 {
            0 => Region::whole(source, 3000),
            1 => Region { start: 100, end: 2900, ..Region::whole(source, 3000) }.looped(LoopMode::Forward, 400, 1400),
            2 => Region::whole(source, 3000).looped(LoopMode::PingPong, 50, 300),
            _ => Region { start: 7, end: 900, ..Region::whole(source, 3000) }.looped(LoopMode::Forward, 8, 11),
        };
        let playback = Playback {
            mode: if p.is_multiple_of(3) { PlayMode::OneShot } else { PlayMode::Gate },
            reverse: p.is_multiple_of(5) || (p % 4 == 1 && p > 12),
            pan: (p % 9) as f32 / 4.0 - 1.0,
            attack_ms: (p % 4) as f32,
            decay_ms: 5.0 + (p % 3) as f32,
            sustain: 0.6,
            release_ms: 2.0 + (p % 5) as f32,
            declick_ms: (p % 3) as f32,
            velocity_sensitivity: 0.5,
            ..Playback::pad(60 + p as u8)
        };
        c.set_zone(p, Some(Zone { playback, ..Zone::new(region) })).unwrap();
        c.set_pad(p, Pad { zone: Some(p as u16), choke: (p % 4) as u8, ..Pad::new(p) }).unwrap();
    }
    let mut sched = Vec::new();
    for i in 0..64u64 {
        let key = 60 + (i * 5 % 24) as u8;
        sched.push((i * 37, EventKind::NoteOn { channel: 0, key, velocity: 30 + (i * 13 % 98) as u8, id: i as i32 }));
        sched.push((i * 37 + 700, EventKind::NoteOff { channel: 0, key, velocity: 64, id: i as i32 }));
    }
    sched.sort_by_key(|e| e.0);
    let (l, r, _) = run(&mut c, &sources[..], &sched, &[64], 4000);
    assert!(l.iter().filter(|v| **v != 0.0).count() > 3000, "the golden schedule sounds");
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in l.iter().chain(&r) {
        for byte in v.to_bits().to_le_bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

#[test]
fn x_zero_bypasses() {
    // A patch with every kind of loop renders the bits it rendered before the crossfade existed
    // (a platform-independent render: see `golden_render`).
    assert_eq!(golden_render(), GOLDEN, "the loops' render changed");
    // Every forward loop with X = 0 matches the plain loop's own reference from `loops`, and its
    // phase walks the plain loop's recurrence bit for bit (reverse wraps below loop_start).
    let data = data();
    for (region, _) in loops() {
        let region = region.crossfaded(0);
        for reverse in [false, true] {
            for (key, transpose, rate) in [(60, 0.0, 48_000.0), (60, -17.2, 44_100.0), (127, 12.0, 48_000.0)] {
                let (out, step) = play(&data, region, reverse, (key, transpose, rate), 200, None);
                let want = super::loops::reference(&data, &region, reverse, true, step, 200);
                assert_bits(&out, &want, &format!("{region:?} reverse {reverse} key {key}"));
                let mut head = crate::playhead::Playhead::start(&region, reverse, true);
                let (ls, le) = (region.loop_start as f64, region.loop_end as f64);
                let mut p = head.pos;
                for n in 0..200 {
                    head.advance(&region, step);
                    if reverse {
                        p -= step;
                        while p < ls {
                            p += le - ls;
                        }
                    } else {
                        p += step;
                        while p >= le {
                            p -= le - ls;
                        }
                    }
                    assert_eq!(head.pos.to_bits(), p.to_bits(), "{region:?} reverse {reverse} key {key}: the phase after {n} steps");
                }
            }
        }
    }
}

/// [`golden_render`] on the base tree (captured on a fresh copy of it, in its own target).
const GOLDEN: u64 = 0xa714_8932_472b_8b4f;

#[test]
fn reverse_mirrors() {
    // At whole steps reverse playback of the data is forward playback of the mirrored data over the
    // mirrored region (frame k becomes 63 - k): crossfade, wrap and intro, bit for bit.
    let data = data();
    let mirrored: Vec<f32> = data.iter().rev().copied().collect();
    let n = data.len() as u64;
    let mirror = |r: Region| Region { start: n - r.end, end: n - r.start, loop_start: n - r.loop_end, loop_end: n - r.loop_start, ..r };
    let mut blended = 0;
    for (region, _) in loops() {
        for rate in [48_000.0, 96_000.0, 144_000.0] {
            let (rev, _) = play(&data, region, true, (60, 0.0, rate), 300, None);
            let (fwd, _) = play(&mirrored, mirror(region), false, (60, 0.0, rate), 300, None);
            assert_bits(&rev, &fwd, &format!("{region:?} rate {rate}"));
            let (plain, _) = play(&data, region.crossfaded(0), true, (60, 0.0, rate), 300, None);
            blended += usize::from(rev != plain);
        }
    }
    assert!(blended > 20, "the crossfade reaches the reverse renders ({blended})");
}

#[test]
fn effective_period_is_loop_minus_x() {
    // A 20-frame loop with X = 6 repeats every 14 frames after its first wrap (28 at half speed),
    // not every 20.
    let data = data();
    let region = Region { start: 2, end: 50, ..Region::whole(0, 64) }.looped(LoopMode::Forward, 10, 30).crossfaded(6);
    for reverse in [false, true] {
        for (rate, period, plain) in [(48_000.0, 14, 20), (24_000.0, 28, 40)] {
            let (out, _) = play(&data, region, reverse, (60, 0.0, rate), 300, None);
            let body = &out[100..];
            assert!(body.iter().zip(&body[period..]).all(|(a, b)| a.to_bits() == b.to_bits()), "reverse {reverse}: period {period}");
            assert!(body.iter().zip(&body[plain..]).any(|(a, b)| a.to_bits() != b.to_bits()), "reverse {reverse}: not the plain period");
        }
    }
}

#[test]
fn crossfade_seam_is_click_free() {
    // A sine whose 20-frame loop ends far from where it starts: without a crossfade the wrap jumps.
    // With X = 10 no output step, the seam's included, exceeds what the blend itself can move in one
    // frame: the source's largest step plus its largest value span over X.
    let data: Vec<f32> = (0..64).map(|i| (i as f32 * 0.11).sin()).collect();
    let max_step = data.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    let span = data.iter().fold(0.0f32, |m, v| m.max(v.abs())) * 2.0;
    let x = 10u64;
    let bound = max_step + span / x as f32;
    let region = Region { start: 2, end: 50, ..Region::whole(0, 64) }.looped(LoopMode::Forward, 10, 30);
    for reverse in [false, true] {
        let jump = |r: Region| {
            let (out, _) = play(&data, r, reverse, (60, 0.0, 48_000.0), 300, None);
            out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max)
        };
        let plain = jump(region);
        let faded = jump(region.crossfaded(x));
        assert!(plain > bound, "reverse {reverse}: the uncrossfaded seam clicks ({plain} against {bound})");
        assert!(faded <= bound, "reverse {reverse}: the crossfaded seam steps {faded}, more than {bound}");
    }
}

#[test]
fn slice_edit_leaving_crossfade_above_half_plays_the_used_value() {
    // A loop shortened to 20 frames under a stored X of 15: the playback uses floor(20 / 2) = 10.
    let data = data();
    let region = Region { start: 2, end: 50, ..Region::whole(0, 64) }.looped(LoopMode::Forward, 10, 30).crossfaded(15);
    assert_eq!(region.crossfade_used(), 10);
    assert_eq!(region.crossfade, 15, "the stored value stands");
    for reverse in [false, true] {
        for rate in [48_000.0, 36_000.0, 72_000.0] {
            let (out, step) = play(&data, region, reverse, (60, 0.0, rate), 260, None);
            assert_bits(&out, &reference(&data, &region, reverse, step, 260, 10), &format!("reverse {reverse} rate {rate}"));
        }
    }
}

#[test]
fn crossfade_used_is_zero_unless_forward() {
    let base = Region { start: 2, end: 50, ..Region::whole(0, 64) };
    let pp = base.looped(LoopMode::PingPong, 10, 30);
    assert_eq!(pp.crossfaded(6).crossfade_used(), 0);
    assert_eq!(base.crossfaded(6).crossfade_used(), 0, "no loop");
    assert_eq!(base.looped(LoopMode::Forward, 10, 30).crossfaded(6).crossfade_used(), 6);
    let data = data();
    for reverse in [false, true] {
        for rate in [48_000.0, 36_000.0] {
            let (with, _) = play(&data, pp.crossfaded(6), reverse, (60, 0.0, rate), 260, None);
            let (without, _) = play(&data, pp, reverse, (60, 0.0, rate), 260, None);
            assert_bits(&with, &without, &format!("ping-pong reverse {reverse} rate {rate}"));
        }
    }
    // A one-shot never loops, so it never crossfades.
    let fwd = base.looped(LoopMode::Forward, 10, 30);
    let one_shot = |r: Region| {
        let mut c = core();
        let playback = Playback { mode: PlayMode::OneShot, attack_ms: 0.0, ..Playback::pad(60) };
        c.set_zone(0, Some(Zone { playback, ..Zone::new(r) })).unwrap();
        c.set_pad(0, Pad { zone: Some(0), ..Pad::new(0) }).unwrap();
        render(&mut c, &[SliceSource::mono(&data, SR)], &[Event::new(0, EventKind::NoteOn { channel: 0, key: 60, velocity: 127, id: -1 })], 100)
    };
    assert_bits(&one_shot(fwd.crossfaded(6)), &one_shot(fwd), "one-shot");
}

#[test]
fn the_first_traversal_intro_is_unaffected() {
    // Before the phase reaches loop_end - X (reverse: loop_start + X - 1) a crossfaded loop plays
    // exactly what the plain loop plays.
    let data = data();
    let region = Region { start: 2, end: 50, ..Region::whole(0, 64) }.looped(LoopMode::Forward, 20, 40);
    for (reverse, untouched) in [(false, 20 - 2 + 20 - 6), (true, 49 - (20 + 6 - 1))] {
        let (plain, _) = play(&data, region, reverse, (60, 0.0, 48_000.0), 100, None);
        let (faded, _) = play(&data, region.crossfaded(6), reverse, (60, 0.0, 48_000.0), 100, None);
        // Through the zone's first frame, whose weight is 0.
        assert_bits(&faded[..=untouched], &plain[..=untouched], &format!("reverse {reverse}: the intro"));
        assert_ne!(faded[untouched + 1].to_bits(), plain[untouched + 1].to_bits(), "reverse {reverse}: the blend starts there");
    }
}

#[test]
fn block_sizes_render_crossfades_the_same() {
    let (mono, left, right) = source_data();
    let sources = [SliceSource::mono(&mono, 44_100.0), SliceSource::stereo(&left, &right, 48_000.0)];
    let zones = |x: u64| {
        let mut t = vec![None; ZONES];
        for p in 0..8usize {
            let r = Region { start: 50 + p as u64, end: 2900, ..Region::whole((p % 2) as u16, 3000) };
            let r = r.looped(LoopMode::Forward, 300 + 7 * p as u64, 300 + 7 * p as u64 + [3, 40, 401, 1000][p % 4]).crossfaded(x * (p as u64 + 1));
            let playback = Playback { reverse: p % 3 == 1, transpose: p as f32 * 1.7 - 5.0, release_ms: 3.0, ..Playback::zone() };
            t[p] = Some(Zone { playback, keys: (40 + 10 * p as u8, 49 + 10 * p as u8), ..Zone::new(r) });
        }
        t
    };
    let mut sched = Vec::new();
    for i in 0..60u64 {
        let key = 40 + (i * 13 % 80) as u8;
        sched.push((i * 61 + i % 7, EventKind::NoteOn { channel: 0, key, velocity: 100, id: i as i32 }));
        sched.push((i * 61 + 900, EventKind::NoteOff { channel: 0, key, velocity: 64, id: i as i32 }));
    }
    sched.sort_by_key(|e| e.0);
    let pads: [Pad; PADS] = core::array::from_fn(Pad::new);
    let render_with = |x: u64, size: usize| {
        let mut c = crate::SamplerCore::new();
        c.prepare(SR, 3);
        c.set_patch(&zones(x), &pads).unwrap();
        run(&mut c, &sources[..], &sched, &[size], 5000)
    };
    let (a, ar, fa) = render_with(37, 64);
    let (b, br, fb) = render_with(37, 512);
    assert!(a.iter().filter(|v| **v != 0.0).count() > 3000, "the schedule sounds");
    let first = a.iter().chain(&ar).zip(b.iter().chain(&br)).position(|(x, y)| x.to_bits() != y.to_bits());
    assert_eq!(first, None, "the first sample where blocks of 64 and 512 differ");
    assert_eq!(fa, fb, "the state fingerprints differ");
    let (n, ..) = render_with(0, 512);
    assert!(n.iter().zip(&b).any(|(x, y)| x.to_bits() != y.to_bits()), "the crossfades reach the render");
}
