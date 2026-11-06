//! Allocation and free probe for the Sampler's core path (ADR 0041 Gate 6, ADR 0012 G5): after
//! the control side installs the core path and prepares the patches, the audio thread's calls
//! (process, events, parameter changes, the transport, All Notes Off, reset) neither allocate nor
//! free, through the first process (which adopts the first generation), a full 1024-event span,
//! 64 voices with every matrix row active on shaped voices, a publication adopted mid-run with its
//! replaced zones fading, a generation retired after its voices end (and dropped on the control
//! side), and a full retirement queue holding a generation back.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::plugins::sampler::{PatchSpec, Publisher, Sampler};
use transit_dsp::{Plugin, SampleBuffer, SampleSource, Transport};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};
use transit_sampler_core::patch::{PlayMode, Playback, Zone};
use transit_sampler_core::playhead::{LoopMode, Region};
use transit_sampler_core::{PADS, VOICES};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const RATE: f32 = 48_000.0;
const BLOCK: usize = 96;

fn source(i: usize, offset: f32) -> Arc<dyn SampleSource> {
    let len = 2_000 + i * 37;
    let data: Vec<f32> = (0..len).map(|n| ((n * (i + 3)) % 101) as f32 / 101.0 - 0.5 + offset).collect();
    if i.is_multiple_of(3) {
        Arc::new(SampleBuffer { sample_rate: 44_100.0, channels: vec![data.clone(), data] })
    } else {
        Arc::new(SampleBuffer { sample_rate: RATE, channels: vec![data] })
    }
}

/// 64 pads on 64 zones: one-shots, looping Gate zones, reversed ones; `offset` changes the samples
/// of the zones listed in `changed`.
fn spec(sources: &[Arc<dyn SampleSource>]) -> PatchSpec {
    let mut s = PatchSpec { sources: sources.to_vec(), ..PatchSpec::default() };
    for (p, src) in sources.iter().enumerate().take(PADS) {
        let frames = src.frames() as u64;
        let mut region = Region::whole(p as u16, frames);
        let mut playback = Playback { release_ms: 20.0, ..Playback::pad(60 + p as u8) };
        match p % 3 {
            0 => playback.mode = PlayMode::OneShot,
            1 => {
                playback.mode = PlayMode::Gate;
                region = region.looped(if p % 2 == 0 { LoopMode::Forward } else { LoopMode::PingPong }, 100, frames - 100);
            }
            _ => playback.reverse = true,
        }
        s.zones.push(Some(Zone { playback, ..Zone::new(region) }));
        s.pads[p].zone = Some(p as u16);
        s.pads[p].note = p as u8 + 30;
        s.pads[p].choke = (p % 5) as u8;
    }
    s
}

fn ev(key: u8, on: bool) -> PluginEvent {
    let message =
        if on { Message::NoteOn { key, velocity: 100, id: NoteId::NONE } } else { Message::NoteOff { key, velocity: 30, id: NoteId::NONE } };
    PluginEvent { channel: 1, note_id: -1, message }
}

struct Rig {
    s: Sampler,
    p: Publisher,
    io: Vec<Vec<f32>>,
    total: AllocStats,
    frame: i64,
}

impl Rig {
    /// Runs `f` on the counted thread and adds what it allocated and freed.
    fn audio(&mut self, f: impl FnOnce(&mut Sampler, &mut Vec<Vec<f32>>)) {
        let ((), stats) = measure(|| f(&mut self.s, &mut self.io));
        self.total.allocs += stats.allocs;
        self.total.frees += stats.frees;
    }

    fn block(&mut self, playing: bool) {
        let t = Transport {
            playing,
            tempo: 128.0,
            numerator: 4,
            denominator: 4,
            samples: self.frame,
            quarters: self.frame as f64 / 22_500.0,
            ..Transport::default()
        };
        self.frame += BLOCK as i64;
        self.audio(|s, io| {
            s.set_transport(&t);
            s.process(io, BLOCK);
        });
    }
}

#[test]
fn the_core_path_allocates_and_frees_nothing_on_the_audio_thread() {
    let sources: Vec<Arc<dyn SampleSource>> = (0..PADS).map(|i| source(i, 0.0)).collect();
    let mut s = Sampler::new();
    s.prepare(RATE, BLOCK, 2);
    let p = s.enable_core(11);
    let mut rig = Rig { s, p, io: vec![vec![0.0; BLOCK]; 2], total: AllocStats::default(), frame: 0 };
    rig.p.publish(spec(&sources)).unwrap();
    // Every zone shaped, all 16 matrix rows active (set on the audio thread, measured).
    rig.audio(|s, _| {
        for z in 1..=PADS {
            for (field, v) in
                [("shaper", 1.0), ("cutoff", 2_000.0), ("resonance", 30.0), ("lfo1_rate", 4.0), ("lfo2_sync", 1.0), ("lfo2_retrigger", 0.0)]
            {
                let mut id = [0u8; 32];
                assert!(s.set_param(fmt(&mut id, z, field), v));
            }
            for row in 1..=16 {
                for (field, v) in [("source", (row % 12 + 1) as f32), ("dest", (row % 8 + 6) as f32), ("amount", 10.0)] {
                    let mut name = [0u8; 16];
                    let mut id = [0u8; 32];
                    assert!(s.set_param(fmt(&mut id, z, row_field(&mut name, row, field)), v));
                }
            }
        }
    });
    // The first process adopts the generation.
    rig.block(true);
    // A full span: 767 note-ons (every pad, 64 voices sounding) and 257 releases in one block.
    rig.audio(|s, _| {
        for i in 0..767u32 {
            assert!(s.event(i % BLOCK as u32, &ev(30 + (i % PADS as u32) as u8, true)));
        }
        assert!(!s.event(0, &ev(30, true)), "the reserve refuses the 768th note-on");
        for i in 0..257u32 {
            assert!(s.event(BLOCK as u32 - 1, &ev(30 + (i % PADS as u32) as u8, false)));
        }
    });
    rig.block(true);
    rig.audio(|s, _| {
        for k in 0..PADS as u8 {
            assert!(s.event(u32::from(k), &ev(30 + k, true)));
        }
    });
    for _ in 0..4 {
        rig.block(true);
    }
    // A publication replacing ten zones' samples: adopted at a block boundary, the replaced zones'
    // voices fade reading the retained generation, which retires once they end.
    let mut next = sources.clone();
    for (i, s) in next.iter_mut().enumerate().take(10) {
        *s = source(i, 0.25);
    }
    rig.p.publish(spec(&next)).unwrap();
    let mut retired = 0;
    for b in 0..40 {
        rig.audio(|s, _| {
            let mut id = [0u8; 32];
            assert!(s.set_param(fmt(&mut id, 1 + b % PADS, "level"), -(b as f32)));
            assert!(s.set_param(fmt(&mut id, 2, "cutoff"), 500.0 + 40.0 * b as f32));
            assert!(s.event(3, &PluginEvent { channel: 0, note_id: -1, message: Message::PitchBend { value: 4_000 + 300 * b as u16 } }));
        });
        rig.block(true);
        retired += rig.p.collect();
    }
    // Backpressure: without collecting, two more generations fill the retirement queue and a third
    // stays retained; the audio thread keeps playing and frees nothing.
    let mut published = Vec::new();
    for round in 0..3 {
        let mut generation = next.clone();
        generation[round] = source(round, 0.5 + round as f32);
        published.push(rig.p.publish(spec(&generation)));
        rig.audio(|s, _| assert!(s.event(0, &ev(30 + round as u8, true))));
        for _ in 0..8 {
            rig.block(true);
        }
    }
    // Transport stop, Reset All Controllers, All Notes Off, a reset.
    rig.audio(|s, _| {
        assert!(s.event(0, &PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: 121, value: 0 } }));
        s.all_notes_off();
    });
    rig.block(false);
    rig.audio(|s, _| s.reset());
    rig.block(false);
    let held_back = rig.p.collect();
    rig.block(false);
    assert_eq!(rig.total.allocs, 0, "allocations on the audio thread: {:?}", rig.total);
    assert_eq!(rig.total.frees, 0, "frees on the audio thread: {:?}", rig.total);
    assert!(published.iter().all(Result::is_ok), "{published:?}");
    assert_eq!(retired, 1, "the old generation retired and dropped on the control side");
    assert!(held_back >= 2, "the held generations come back to the control side");
    assert!(rig.io[0].iter().chain(&rig.io[1]).all(|x| x.is_finite()));
    assert_eq!(VOICES, 64);
}

/// Writes `zoneNNN_<field>` into `buf` without allocating.
fn fmt<'a>(buf: &'a mut [u8; 32], zone: usize, field: &str) -> &'a str {
    let head = [b'z', b'o', b'n', b'e', b'0' + (zone / 100) as u8, b'0' + (zone / 10 % 10) as u8, b'0' + (zone % 10) as u8, b'_'];
    buf[..8].copy_from_slice(&head);
    buf[8..8 + field.len()].copy_from_slice(field.as_bytes());
    std::str::from_utf8(&buf[..8 + field.len()]).unwrap()
}

/// Writes `mod<row>_<field>` into `buf` without allocating.
fn row_field<'a>(buf: &'a mut [u8; 16], row: usize, field: &str) -> &'a str {
    let mut n = 0;
    for b in b"mod" {
        buf[n] = *b;
        n += 1;
    }
    if row >= 10 {
        buf[n] = b'0' + (row / 10) as u8;
        n += 1;
    }
    buf[n] = b'0' + (row % 10) as u8;
    buf[n + 1] = b'_';
    n += 2;
    buf[n..n + field.len()].copy_from_slice(field.as_bytes());
    std::str::from_utf8(&buf[..n + field.len()]).unwrap()
}
