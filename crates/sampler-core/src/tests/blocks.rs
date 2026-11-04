//! Gate 6 (realtime determinism): one absolute schedule of events rendered in host blocks of 1, 7,
//! 64, 127 and 512 frames and in an irregular partition gives the same output bits and the same
//! state fingerprint, with shaped stereo and mono voices, every matrix row active, loops, choke,
//! the pedal, controllers, steals and a reset inside the schedule.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

use transit_instrument_core::adsr::AdsrRates;
use transit_instrument_core::matrix::{Matrix, ROWS, Row, dst, src};
use transit_instrument_core::shaper::{LfoSettings, Settings, filter_mode};

use super::{SR, note};
use crate::event::{Context, Control, Event, EventKind};
use crate::patch::{Pad, PlayMode, Playback, Zone};
use crate::playhead::{LoopMode, Region};
use crate::reader::{SliceSource, SourceBank};
use crate::{PADS, SamplerCore};

/// Every one of the 16 rows active: each source at least once, each sampler destination.
pub(crate) fn full_matrix() -> Matrix {
    let rows: [(usize, usize, usize, f32); ROWS] = [
        (src::LFO1, dst::PITCH, src::OFF, 0.05),
        (src::LFO2, dst::CUTOFF, src::OFF, 0.3),
        (src::AMP_ENV, dst::LEVEL, src::OFF, -0.2),
        (src::FILTER_ENV, dst::RESONANCE, src::OFF, 0.2),
        (src::VELOCITY, dst::ENV_AMOUNT, src::OFF, 0.4),
        (src::KEY, dst::PAN, src::OFF, 0.5),
        (src::RELEASE_VELOCITY, dst::CUTOFF, src::OFF, -0.3),
        (src::AFTERTOUCH, dst::LFO1_RATE, src::OFF, 0.5),
        (src::POLY_AFTERTOUCH, dst::LFO2_RATE, src::OFF, -0.5),
        (src::MOD_WHEEL, dst::LFO1_AMOUNT, src::OFF, 0.6),
        (src::PITCH_BEND, dst::PITCH, src::OFF, 0.08),
        (src::RANDOM, dst::PAN, src::OFF, 0.3),
        (src::MACRO1, dst::LFO2_AMOUNT, src::OFF, -0.4),
        (src::MACRO1 + 1, dst::LEVEL, src::OFF, 0.25),
        (src::LFO1, dst::CUTOFF, src::MOD_WHEEL, 0.7),
        (src::KEY, dst::RESONANCE, src::VELOCITY, 0.1),
    ];
    Matrix::new(rows.iter().enumerate().map(|(i, &(source, dest, via, amount))| Row { source, via, dest, curve: i % 5, amount }))
}

pub(crate) fn shaper_settings(rate: f32) -> Settings {
    Settings {
        on: true,
        filter_env: AdsrRates::new(5.0, 50.0, 0.3, 80.0, rate),
        mode: filter_mode(0),
        slope24: true,
        cutoff_log2: 2000f32.log2(),
        resonance: 0.4,
        drive: 1.5,
        key_track: 0.5,
        env_amount: 0.5,
        lfo: [
            LfoSettings::new(0, 3.0, false, 4, true, 20.0, 0.8, 4.0, (0, 0.0), rate),
            LfoSettings::new(5, 0.7, false, 4, false, 0.0, 0.6, 4.0, (0, 0.0), rate),
        ],
        sr: rate,
        ..Settings::default()
    }
}

/// A patch over two sources (mono 0, stereo 1): every pad bound, half shaped with the full
/// matrix, loops of each kind, choke groups, and one unbound zone on keys 0..=59.
pub(crate) fn rich_core(rate: f32) -> SamplerCore {
    let mut c = SamplerCore::new();
    c.prepare(rate, 99);
    for p in 0..PADS {
        let region = match p % 4 {
            0 => Region::whole((p % 2) as u16, 3000),
            1 => Region { start: 100, end: 2900, ..Region::whole((p % 2) as u16, 3000) }.looped(LoopMode::Forward, 400, 1400),
            2 => Region::whole((p % 2) as u16, 3000).looped(LoopMode::PingPong, 50, 300),
            _ => Region { start: 7, end: 900, ..Region::whole((p % 2) as u16, 3000) }.looped(LoopMode::Forward, 8, 11),
        };
        let playback = Playback {
            mode: if p.is_multiple_of(3) { PlayMode::OneShot } else { PlayMode::Gate },
            reverse: p.is_multiple_of(5),
            transpose: (p % 7) as f32 - 3.0,
            cents: (p % 11) as f32 * 9.0 - 45.0,
            pan: (p % 9) as f32 / 4.0 - 1.0,
            level_db: -((p % 6) as f32),
            attack_ms: (p % 4) as f32,
            decay_ms: 5.0 + (p % 3) as f32,
            sustain: 0.6,
            release_ms: 2.0 + (p % 5) as f32,
            declick_ms: (p % 3) as f32,
            ..Playback::pad(note(p))
        };
        let shaped = p.is_multiple_of(2);
        let zone = Zone {
            playback,
            shaper: if shaped { shaper_settings(rate) } else { Settings { sr: rate, ..Settings::default() } },
            matrix: if shaped { full_matrix() } else { Matrix::default() },
            ..Zone::new(region)
        };
        c.set_zone(p, Some(zone)).unwrap();
        c.set_pad(p, Pad { zone: Some(p as u16), choke: (p % 4) as u8, ..Pad::new(p) }).unwrap();
    }
    let zone = Zone { keys: (0, 59), shaper: shaper_settings(rate), matrix: full_matrix(), ..Zone::new(Region::whole(1, 3000)) };
    c.set_zone(PADS, Some(zone)).unwrap();
    c
}

pub(crate) fn source_data() -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut x = 0x2468_ACE1u32;
    let mut noise = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    };
    let mono = (0..3000).map(|_| noise()).collect();
    let left = (0..3000).map(|i| (i as f32 * 0.031).sin() * 0.5).collect();
    let right = (0..3000).map(|_| noise() * 0.25).collect();
    (mono, left, right)
}

/// The absolute schedule: more than 64 starts (steals), offsets inside blocks, every event kind.
fn schedule() -> Vec<(u64, EventKind)> {
    let mut s = Vec::new();
    let on = |key: u8, velocity: u8, id: i32| EventKind::NoteOn { channel: 0, key, velocity, id };
    let off = |key: u8, id: i32| EventKind::NoteOff { channel: 0, key, velocity: 90, id };
    s.push((0, EventKind::Control(Control::ModWheel(0.4))));
    for i in 0..80u64 {
        let key = 60 + (i * 7 % 64) as u8;
        s.push((i * 23 + (i % 3), on(key, 40 + (i % 80) as u8, i as i32)));
        if i % 4 == 1 {
            s.push((i * 23 + 150, off(key, i as i32)));
        }
    }
    s.push((300, EventKind::Control(Control::Sustain(true))));
    s.push((333, on(20, 100, -1)));
    s.push((333, on(21, 101, -1)));
    s.push((340, EventKind::Control(Control::PitchBend(-0.6))));
    s.push((500, EventKind::Control(Control::Aftertouch(0.7))));
    s.push((501, EventKind::Control(Control::PolyAftertouch { key: 67, value: 0.5 })));
    s.push((640, off(20, -1)));
    s.push((700, EventKind::Control(Control::Macro { index: 0, value: 0.9 })));
    s.push((710, EventKind::Control(Control::Macro { index: 1, value: 0.3 })));
    // A burst on the unbound zone while the pads sound: more than 64 voices, so some are stolen.
    for k in 0..60u64 {
        s.push((800 + k / 6, on(k as u8, 100, 1000 + k as i32)));
    }
    s.push((900, EventKind::Control(Control::Sustain(false))));
    s.push((950, EventKind::PadPress { pad: 9, velocity: 120 }));
    s.push((951, EventKind::PadPress { pad: 13, velocity: 30 }));
    s.push((1300, EventKind::PadRelease { pad: 9 }));
    s.push((1400, EventKind::AllNotesOff));
    s.push((1500, on(64, 127, 500)));
    s.push((1500, on(68, 127, 501)));
    s.push((1700, EventKind::Stop));
    s.push((1800, on(66, 127, 502)));
    s.push((2000, EventKind::Reset));
    for i in 0..30u64 {
        s.push((2001 + i * 31, on(60 + (i * 5 % 64) as u8, 127, 600 + i as i32)));
    }
    s.push((2600, EventKind::AllSoundOff));
    s.push((2601, on(30, 64, -1)));
    s.sort_by_key(|e| e.0);
    s
}

/// Renders `total` frames in blocks of `sizes` (cycled); returns both channels and the fingerprint.
pub(crate) fn run<B: SourceBank + ?Sized>(
    c: &mut SamplerCore,
    sources: &B,
    sched: &[(u64, EventKind)],
    sizes: &[usize],
    total: usize,
) -> (Vec<f32>, Vec<f32>, u64) {
    let (mut l, mut r) = (vec![0.0; total], vec![0.0; total]);
    let mut at = 0usize;
    let mut k = 0usize;
    let mut events = Vec::new();
    while at < total {
        let n = sizes[k % sizes.len()].min(total - at);
        k += 1;
        events.clear();
        events.extend(sched.iter().filter(|e| e.0 >= at as u64 && e.0 < (at + n) as u64).map(|e| Event::new((e.0 - at as u64) as u32, e.1)));
        let ctx = Context { frame: at as u64, ..Context::default() };
        c.process(&ctx, sources, &events, &mut [&mut l[at..at + n], &mut r[at..at + n]]);
        at += n;
    }
    let mut h = DefaultHasher::new();
    c.fingerprint(&mut h);
    (l, r, h.finish())
}

#[test]
fn every_block_partition_renders_the_same_bits_and_state() {
    let (mono, left, right) = source_data();
    let sources = [SliceSource::mono(&mono, 44_100.0), SliceSource::stereo(&left, &right, 48_000.0)];
    let sched = schedule();
    let total = 3000;
    let mut reference = rich_core(SR);
    let (rl, rr, rf) = run(&mut reference, &sources[..], &sched, &[total], total);
    assert!(rl.iter().chain(&rr).all(|x| x.is_finite()));
    assert!(rl.iter().filter(|x| **x != 0.0).count() > 2000, "the schedule sounds");
    let d = reference.diagnostics();
    assert!(d.steals > 0 && d.chokes > 0, "{d:?}");
    for sizes in [&[1usize][..], &[7], &[64], &[127], &[512], &[3, 100, 1, 250, 17, 64, 12, 11, 13, 500]] {
        let mut c = rich_core(SR);
        let (l, r, f) = run(&mut c, &sources[..], &sched, sizes, total);
        for (n, ((a, b), (x, y))) in rl.iter().zip(&rr).zip(l.iter().zip(&r)).enumerate() {
            assert_eq!((a.to_bits(), b.to_bits()), (x.to_bits(), y.to_bits()), "blocks {sizes:?}, frame {n}");
        }
        assert_eq!(f, rf, "blocks {sizes:?}: the state fingerprints differ");
        assert_eq!(c.diagnostics(), d, "blocks {sizes:?}");
    }
}

/// The fingerprint sees the state: a different schedule (one note fewer) ends elsewhere.
#[test]
fn the_fingerprint_tells_states_apart() {
    let (mono, left, right) = source_data();
    let sources = [SliceSource::mono(&mono, 44_100.0), SliceSource::stereo(&left, &right, 48_000.0)];
    let sched = schedule();
    let mut a = rich_core(SR);
    let (.., fa) = run(&mut a, &sources[..], &sched, &[64], 1000);
    let fewer: Vec<_> = sched.iter().copied().filter(|e| !matches!(e.1, EventKind::NoteOn { id: 1, .. })).collect();
    assert_eq!(fewer.len() + 1, sched.len());
    let mut b = rich_core(SR);
    let (.., fb) = run(&mut b, &sources[..], &fewer, &[64], 1000);
    assert_ne!(fa, fb);
}
