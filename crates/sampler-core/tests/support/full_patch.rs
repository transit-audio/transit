//! Shared full-load fixture: two 8 by 8 groups, sixteen slice regions and 112 loose zones.
#![allow(dead_code)]

use transit_instrument_core::matrix::{Matrix, Row, dst, src};
use transit_instrument_core::shaper::{LfoSettings, Settings};
use transit_sampler_core::event::{Context, Control, Event, EventKind};
use transit_sampler_core::patch::{Fades, GroupMode, Membership, Pad, PlayMode, Playback, Zone};
use transit_sampler_core::playhead::{LoopMode, Region};
use transit_sampler_core::reader::SliceSource;
use transit_sampler_core::{GROUPS, PADS, SamplerCore, ZONES};

pub const RATE: f32 = 48_000.0;
pub const FRAMES: usize = 8192;
pub const SEED: u32 = 0x1357_2468;

pub fn unity() -> Playback {
    Playback { attack_ms: 0.0, decay_ms: 0.0, release_ms: 32.0 * 1000.0 / RATE, velocity_sensitivity: 1.0, key_tracking: false, ..Playback::zone() }
}

pub fn noise(seed: u32) -> Vec<f32> {
    let mut x = seed.max(1);
    (0..4096)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 16) as f32 / 65536.0 - 0.5
        })
        .collect()
}

pub fn data() -> Vec<Vec<f32>> {
    (0..ZONES).map(|z| noise(z as u32 + 1)).collect()
}

pub fn zones(shaped: bool) -> Vec<Option<Zone>> {
    (0..ZONES)
        .map(|z| {
            let mut zone = Zone { playback: unity(), ..Zone::new(Region::whole(z as u16, 4096)) };
            if z < 128 {
                let group = (z / 64) as u8;
                zone.keys = (group * 8, group * 8 + 7);
                zone.membership = Some(Membership { group, layer: (z % 64 / 8) as u8, alternative: (z % 8) as u8 });
                zone.fades = Fades { key_lo: 2, key_hi: 2, vel_lo: 8, vel_hi: 8 };
            } else if z < 144 {
                let slice = (z - 128) as u64;
                zone.region = Region { start: slice * 256, end: (slice + 1) * 256, ..Region::whole(128, 4096) };
                zone.playback.mode = if slice < 2 { PlayMode::Gate } else { PlayMode::OneShot };
                if slice < 2 {
                    zone.region = zone.region.looped(if slice == 0 { LoopMode::Forward } else { LoopMode::PingPong }, slice * 256, (slice + 1) * 256);
                }
            } else {
                let key = 64 + ((z - 144) / 2) as u8;
                zone.keys = (key, key);
                zone.velocities = if z % 2 == 0 { (1, 100) } else { (28, 127) };
                zone.fades = Fades { key_lo: 0, key_hi: 0, vel_lo: 8, vel_hi: 8 };
            }
            if !(128..144).contains(&z) {
                zone.region = zone.region.looped(if z % 3 == 0 { LoopMode::PingPong } else { LoopMode::Forward }, 64, 448);
                if z % 3 != 0 {
                    zone.region = zone.region.crossfaded(64);
                }
            }
            zone.playback.reverse = z % 5 == 0;
            if shaped && z < 4 {
                zone.shaper = Settings {
                    on: true,
                    cutoff_log2: 10.0,
                    resonance: 0.25,
                    sr: RATE,
                    lfo: [LfoSettings::new(0, 3.0, false, 4, true, 0.0, 0.5, 4.0, (0, 0.0), RATE); 2],
                    ..Settings::default()
                };
                zone.matrix = Matrix::new((0..16).map(|i| Row {
                    source: [src::LFO1, src::VELOCITY, src::MOD_WHEEL, src::RANDOM][i % 4],
                    dest: [dst::PITCH, dst::CUTOFF, dst::PAN, dst::LEVEL][i % 4],
                    via: src::OFF,
                    curve: i % 5,
                    amount: 0.025,
                }));
            }
            Some(zone)
        })
        .collect()
}

pub fn pads() -> [Pad; PADS] {
    core::array::from_fn(|p| Pad {
        note: 36 + p as u8,
        zone: (p < 16).then_some(128 + p as u16),
        choke: if (2..6).contains(&p) { 1 } else { 0 },
        ..Pad::new(p)
    })
}

pub fn groups() -> [GroupMode; GROUPS] {
    let mut g = [GroupMode::Sequential; GROUPS];
    g[1] = GroupMode::Random;
    g
}

pub fn core(shaped: bool) -> SamplerCore {
    let mut c = SamplerCore::new();
    c.prepare(RATE, SEED);
    c.set_patch(&zones(shaped), &pads()).unwrap();
    c.set_group_modes(&groups());
    c
}

pub fn schedule() -> Vec<Event> {
    let mut events = Vec::new();
    // Eight overlapping triggers of eight layers fill all 64 slots before further steals.
    for i in 0..32u32 {
        events.push(Event::on(i * 17, (i % 16) as u8, 64 + (i % 64) as u8));
    }
    for i in 0..112u32 {
        events.push(Event::on(600 + i * 13, 64 + (i % 56) as u8, if i < 56 { 35 } else { 95 }));
    }
    for i in 0..48u32 {
        events.push(Event::on(2200 + i * 19, 36 + (i % 16) as u8, 127));
    }
    events.extend([
        Event::new(79, EventKind::Control(Control::ModWheel(0.5))),
        Event::new(233, EventKind::Control(Control::Sustain(true))),
        Event::off(267, 3),
        Event::new(1103, EventKind::Control(Control::Sustain(false))),
        Event::new(3301, EventKind::AllNotesOff),
        Event::on(3407, 0, 127),
        Event::on(3503, 8, 127),
        Event::new(3701, EventKind::Stop),
        Event::on(4100, 1, 127),
        Event::on(4117, 9, 127),
        Event::new(4901, EventKind::AllSoundOff),
    ]);
    // Continue both groups after the transport transitions and parameter change.
    for i in 0..32u32 {
        events.push(Event::on(5200 + i * 31, (i % 16) as u8, 100));
    }
    events.sort_by_key(|e| e.frame);
    events
}

/// Transport changes and a pan glide occur at fixed absolute frames in every partition.
pub fn render(c: &mut SamplerCore, pcm: &[Vec<f32>], sizes: &[usize]) -> (Vec<u32>, usize) {
    let sources: Vec<_> = pcm.iter().map(|p| SliceSource::mono(p, RATE)).collect();
    let schedule = schedule();
    let mut out = vec![vec![0.0; FRAMES]; 2];
    let mut at = 0;
    let mut block = 0;
    let mut peak = 0;
    while at < FRAMES {
        let boundary = [4096, 5120, FRAMES].into_iter().find(|b| *b > at).unwrap();
        let n = sizes[block % sizes.len()].min(boundary - at);
        if at == 5120 {
            let mut z = *c.zone(0).unwrap();
            z.playback.pan = 0.5;
            c.set_zone_glide(0, z, at as u64, 32).unwrap();
        }
        let events: Vec<_> =
            schedule.iter().filter(|e| (at..at + n).contains(&(e.frame as usize))).map(|e| Event::new(e.frame - at as u32, e.kind)).collect();
        let ctx = Context { frame: at as u64, tempo: 120.0, quarters: (!(4096..5120).contains(&at)).then_some(at as f64 / 24000.0) };
        let (l, r) = out.split_at_mut(1);
        c.process(&ctx, &sources[..], &events, &mut [&mut l[0][at..at + n], &mut r[0][at..at + n]]);
        peak = peak.max(c.active_voices());
        at += n;
        block += 1;
    }
    assert!(out.iter().flatten().all(|x| x.is_finite()));
    (out.iter().flatten().map(|x| x.to_bits()).collect(), peak)
}

pub fn hash(bits: &[u32]) -> u64 {
    bits.iter().flat_map(|x| x.to_le_bytes()).fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}
