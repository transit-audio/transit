//! Gate 6 allocation probe (ADR 0041, ADR 0012 G5): after `prepare`, the core's processing neither
//! allocates nor frees, through the first block, a maximal 1024-event span, 64 voices with every
//! matrix row active on shaped stereo and mono voices, steals, chokes, loops, control changes, All
//! Notes Off, Stop, All Sound Off, the Reset event and `reset()`.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use transit_instrument_core::adsr::AdsrRates;
use transit_instrument_core::matrix::{Matrix, ROWS, Row, dst, src};
use transit_instrument_core::shaper::{LfoSettings, Settings, filter_mode};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};
use transit_sampler_core::event::{Context, Control, Event, EventKind};
use transit_sampler_core::patch::{Pad, PlayMode, Playback, Zone};
use transit_sampler_core::playhead::{LoopMode, Region};
use transit_sampler_core::reader::SliceSource;
use transit_sampler_core::{PADS, SamplerCore, VOICES, ZONES};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const RATE: f32 = 48_000.0;
/// The adapter's event capacity (ADR 0041: 1024 per instance).
const MAX_EVENTS: usize = 1024;

fn full_matrix() -> Matrix {
    let pairs = [
        (src::LFO1, dst::PITCH),
        (src::LFO2, dst::CUTOFF),
        (src::AMP_ENV, dst::LEVEL),
        (src::FILTER_ENV, dst::RESONANCE),
        (src::VELOCITY, dst::ENV_AMOUNT),
        (src::KEY, dst::PAN),
        (src::RELEASE_VELOCITY, dst::CUTOFF),
        (src::AFTERTOUCH, dst::LFO1_RATE),
        (src::POLY_AFTERTOUCH, dst::LFO2_RATE),
        (src::MOD_WHEEL, dst::LFO1_AMOUNT),
        (src::PITCH_BEND, dst::PITCH),
        (src::RANDOM, dst::PAN),
        (src::MACRO1, dst::LFO2_AMOUNT),
        (src::MACRO1 + 7, dst::LEVEL),
        (src::LFO1, dst::CUTOFF),
        (src::KEY, dst::RESONANCE),
    ];
    let m = Matrix::new(pairs.iter().enumerate().map(|(i, &(source, dest))| Row { source, dest, via: src::MOD_WHEEL, curve: i % 5, amount: 0.2 }));
    assert_eq!(m.rows().len(), ROWS, "every row active");
    m
}

fn shaped() -> Settings {
    Settings {
        on: true,
        filter_env: AdsrRates::new(2.0, 30.0, 0.4, 40.0, RATE),
        mode: filter_mode(1),
        slope24: true,
        cutoff_log2: 1500f32.log2(),
        resonance: 0.5,
        lfo: [
            LfoSettings::new(3, 5.0, false, 4, true, 10.0, 0.7, 4.0, (0, 0.0), RATE),
            LfoSettings::new(4, 2.0, false, 4, false, 0.0, 0.5, 4.0, (0, 0.0), RATE),
        ],
        sr: RATE,
        ..Settings::default()
    }
}

fn patch() -> SamplerCore {
    let mut c = SamplerCore::new();
    c.prepare(RATE, 3);
    for p in 0..PADS {
        let region =
            Region::whole((p % 2) as u16, 20_000).looped(if p.is_multiple_of(2) { LoopMode::Forward } else { LoopMode::PingPong }, 1000, 9000);
        let playback = Playback {
            mode: if p.is_multiple_of(4) { PlayMode::OneShot } else { PlayMode::Gate },
            reverse: p.is_multiple_of(3),
            declick_ms: 1.0,
            transpose: (p % 5) as f32,
            ..Playback::pad(60 + p as u8)
        };
        c.set_zone(p, Some(Zone { playback, shaper: shaped(), matrix: full_matrix(), ..Zone::new(region) })).unwrap();
        c.set_pad(p, Pad { zone: Some(p as u16), choke: (p % 3) as u8, ..Pad::new(p) }).unwrap();
    }
    // Unbound zones fill the rest of the zone table (keys 0..=59): settings for all 256 exist.
    for z in PADS..ZONES {
        let zone = Zone { keys: (0, 59), shaper: shaped(), matrix: full_matrix(), ..Zone::new(Region::whole((z % 2) as u16, 20_000)) };
        c.set_zone(z, Some(zone)).unwrap();
    }
    c
}

/// A full 1024-event span inside `n` frames: every pad (64 voices), unbound notes that steal,
/// controllers, offs, presses, All Notes Off and the rest, sorted by frame.
fn span(n: u32, round: usize) -> Vec<Event> {
    let mut e = Vec::with_capacity(MAX_EVENTS);
    for p in 0..PADS as u8 {
        e.push(Event::new(0, EventKind::NoteOn { channel: 0, key: 60 + p, velocity: 100, id: i32::from(p) }));
    }
    let kinds = [
        EventKind::Control(Control::ModWheel(0.5)),
        EventKind::Control(Control::PitchBend(-0.25)),
        EventKind::Control(Control::Aftertouch(0.3)),
        EventKind::Control(Control::PolyAftertouch { key: 61, value: 0.8 }),
        EventKind::Control(Control::Macro { index: 7, value: 0.6 }),
        EventKind::Control(Control::Sustain(round.is_multiple_of(2))),
        EventKind::NoteOn { channel: 1, key: (round % 60) as u8, velocity: 90, id: -1 },
        EventKind::NoteOff { channel: 0, key: 61, velocity: 20, id: 1 },
        EventKind::PadPress { pad: 5, velocity: 70 },
        EventKind::PadRelease { pad: 5 },
        EventKind::Control(Control::Sustain(false)),
    ];
    let ends = round % 4 == 2;
    let tail = if ends { 4 } else { 0 };
    let mut i = 0;
    while e.len() < MAX_EVENTS - tail {
        e.push(Event::new(1 + (i as u32 * 7) % (n - 1), kinds[i % kinds.len()]));
        i += 1;
    }
    if ends {
        e.push(Event::new(n - 3, EventKind::AllNotesOff));
        e.push(Event::new(n - 2, EventKind::Stop));
        e.push(Event::new(n - 1, if round.is_multiple_of(3) { EventKind::Reset } else { EventKind::AllSoundOff }));
        e.push(Event::new(n - 1, EventKind::NoteOn { channel: 0, key: 70, velocity: 127, id: 999 }));
    }
    e.sort_by_key(|x| x.frame);
    assert_eq!(e.len(), MAX_EVENTS);
    e
}

#[test]
fn processing_events_controls_and_reset_neither_allocate_nor_free() {
    let mono: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.013).sin()).collect();
    let (left, right): (Vec<f32>, Vec<f32>) = (0..20_000).map(|i| ((i as f32 * 0.007).sin(), (i as f32 * 0.011).cos())).unzip();
    let sources = [SliceSource::mono(&mono, 44_100.0), SliceSource::stereo(&left, &right, 48_000.0)];
    let mut c = patch();
    let sizes = [512usize, 1, 7, 64, 127, 333];
    let spans: Vec<Vec<Event>> =
        (0..12usize).map(|r| if r.is_multiple_of(2) { span(sizes[r % sizes.len()].max(8) as u32, r) } else { Vec::new() }).collect();
    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    let mut total = AllocStats::default();
    let mut frame = 0u64;
    let mut peak = 0;
    for (round, events) in spans.iter().enumerate() {
        let n = sizes[round % sizes.len()].max(if events.is_empty() { 1 } else { 8 });
        let ctx = Context { frame, tempo: 128.0, quarters: if round % 4 == 1 { Some(frame as f64 / 22_500.0) } else { None } };
        let ((), stats) = measure(|| {
            c.process(&ctx, &sources[..], events, &mut [&mut l[..n], &mut r[..n]]);
            peak = peak.max(c.active_voices());
            if round == 7 {
                c.reset();
            }
        });
        total.allocs += stats.allocs;
        total.frees += stats.frees;
        assert!(l.iter().chain(&r).all(|x| x.is_finite()), "round {round}");
        frame += n as u64;
    }
    // The probe reached the paths it claims: all voices busy, steals and chokes happened.
    let d = c.diagnostics();
    assert_eq!(peak, VOICES);
    assert!(d.steals > 0 && d.chokes > 0, "{d:?}");
    assert!(total.is_zero(), "{total:?}");
}
