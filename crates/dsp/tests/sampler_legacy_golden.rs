//! The legacy Sampler's golden hashes (ADR 0041 Gate 1: legacy instances stay bit-exact while the
//! portable core arrives beside them). Captured from the unsplit renderer at c08a3bcb, whose
//! `plugins/sampler.rs` is the v0.6.2 release code (the only change since is the pure move of the
//! ADSR to the instrument primitives, which the full oracle run checked). Every scenario goes
//! through the public plug-in API, in host blocks of uneven sizes: defaults, both loop modes,
//! off-rate PCM (22.05 and 96 kHz), parameter automation between blocks, voice steals past 32
//! voices, a stereo one-shot from a start point, channel and voice-id events, and All Notes Off.
//!
//! The rendered path calls libm (`powf`, `expf`, `lnf`), whose last bit differs between MSVC's CRT,
//! glibc and Apple's libm, so the capture holds only on Windows (as `lfo_sync_golden.rs`); the
//! Windows CI job enforces it. Never regenerate these from a new renderer.
#![cfg(windows)]
#![allow(clippy::unwrap_used)]

use std::f32::consts::TAU;
use std::sync::Arc;

use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::{Plugin, SampleBuffer};

const SR: f32 = 48_000.0;
/// Host block sizes, cycled: uneven on purpose.
const BLOCKS: [usize; 5] = [512, 37, 128, 1, 300];

fn sine(freq: f32, sr: f32, len: usize) -> Vec<f32> {
    (0..len).map(|i| 0.5 * (TAU * freq * i as f32 / sr).sin()).collect()
}

fn sampler(sample: SampleBuffer, params: &[(&str, f32)]) -> Box<dyn Plugin> {
    let mut p = transit_dsp::create("sampler").unwrap();
    for &(id, v) in params {
        assert!(p.set_param(id, v), "{id}");
    }
    p.prepare(SR, 512, 2);
    assert!(p.set_sample(Some(Arc::new(sample))));
    p
}

/// FNV-1a over the output's bits, block after block.
struct Hash(u64);

impl Hash {
    fn add(&mut self, x: f32) {
        for b in x.to_bits().to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Renders `frames` frames; `at(p, block_start, block_len)` queues the block's events and edits.
fn render(p: &mut dyn Plugin, frames: usize, mut at: impl FnMut(&mut dyn Plugin, usize, usize)) -> u64 {
    let mut h = Hash(0xcbf2_9ce4_8422_2325);
    let mut io = vec![vec![0.0f32; 512]; 2];
    let (mut done, mut k, mut peak) = (0, 0, 0.0f32);
    while done < frames {
        let n = BLOCKS[k % BLOCKS.len()].min(frames - done);
        k += 1;
        at(p, done, n);
        for c in &mut io {
            c.fill(0.25);
        }
        p.process(&mut io, n);
        for (l, r) in io[0].iter().zip(&io[1]).take(n) {
            h.add(*l);
            h.add(*r);
            peak = peak.max(l.abs()).max(r.abs());
        }
        done += n;
    }
    assert!(peak > 0.01, "a golden of silence proves nothing");
    h.0
}

/// Calls `f(offset)` for each frame of `frames` that falls in the block at `start`, `len` long.
fn within(start: usize, len: usize, frames: &[usize], mut f: impl FnMut(usize, usize)) {
    for (i, &fr) in frames.iter().enumerate() {
        if (start..start + len).contains(&fr) {
            f(i, fr - start);
        }
    }
}

fn defaults() -> u64 {
    let mut p = sampler(SampleBuffer { sample_rate: SR, channels: vec![sine(440.0, SR, 24_000)] }, &[]);
    let ons = [0, 700, 701, 5_000, 9_999];
    let offs = [3_000, 4_000, 12_000];
    render(p.as_mut(), 30_000, |p, s, n| {
        within(s, n, &ons, |i, o| p.note_on(o, [60, 64, 67, 55, 72][i], [127, 90, 40, 1, 100][i]));
        within(s, n, &offs, |i, o| p.note_off(o, [60, 64, 67][i]));
    })
}

fn forward_loop() -> u64 {
    let params = [("loop_mode", 1.0), ("loop_start", 30.0), ("loop_end", 80.0), ("release", 40.0), ("attack", 5.0)];
    let mut p = sampler(SampleBuffer { sample_rate: SR, channels: vec![sine(330.0, SR, 9_001)] }, &params);
    render(p.as_mut(), 40_000, |p, s, n| {
        within(s, n, &[10, 3_000], |i, o| p.note_on(o, [60, 63][i], 110));
        within(s, n, &[30_000, 31_000], |i, o| p.note_off(o, [60, 63][i]));
    })
}

fn ping_pong() -> u64 {
    let ramp: Vec<f32> = (0..2_000).map(|i| (i as f32 / 2_000.0) - 0.5).collect();
    let params = [("loop_mode", 2.0), ("loop_start", 10.0), ("loop_end", 90.0), ("transpose", 7.0), ("fine_tune", -33.0)];
    let mut p = sampler(SampleBuffer { sample_rate: SR, channels: vec![ramp] }, &params);
    render(p.as_mut(), 20_000, |p, s, n| {
        within(s, n, &[0, 4_321], |i, o| p.note_on(o, [48, 61][i], 127));
        within(s, n, &[15_000], |_, o| p.note_off(o, 48));
    })
}

fn off_rate() -> u64 {
    let mut a = sampler(SampleBuffer { sample_rate: 22_050.0, channels: vec![sine(300.0, 22_050.0, 11_025)] }, &[("root_note", 57.0)]);
    let ha = render(a.as_mut(), 30_000, |p, s, n| {
        within(s, n, &[0, 2_000, 2_001], |i, o| p.note_on(o, [57, 69, 45][i], 100));
    });
    let mut b = sampler(SampleBuffer { sample_rate: 96_000.0, channels: vec![sine(1_000.0, 96_000.0, 48_000)] }, &[("key_track", 0.0)]);
    let hb = render(b.as_mut(), 30_000, |p, s, n| {
        within(s, n, &[3, 9_000], |i, o| p.note_on(o, [30, 90][i], 64));
        within(s, n, &[20_000], |_, o| p.note_off(o, 30));
    });
    ha ^ hb.rotate_left(1)
}

fn automation() -> u64 {
    let mut p = sampler(SampleBuffer { sample_rate: SR, channels: vec![sine(220.0, SR, 48_000)] }, &[]);
    let edits: [(usize, &str, f32); 8] = [
        (1_000, "level", -12.0),
        (2_000, "transpose", 5.0),
        (3_000, "attack", 200.0),
        (4_000, "level", 6.0),
        (5_000, "sustain", 30.0),
        (6_000, "decay", 50.0),
        (7_000, "velocity", 0.0),
        (8_000, "play_mode", 1.0),
    ];
    render(p.as_mut(), 24_000, |p, s, n| {
        for &(fr, id, v) in &edits {
            if (s..s + n).contains(&fr) {
                assert!(p.set_param(id, v));
            }
        }
        within(s, n, &[0, 2_500, 3_500, 8_500, 9_000], |i, o| p.note_on(o, [60, 62, 64, 65, 67][i], 100));
        within(s, n, &[6_500, 7_500, 12_000], |i, o| p.note_off(o, [60, 62, 64][i]));
    })
}

fn steals() -> u64 {
    let mut p = sampler(SampleBuffer { sample_rate: SR, channels: vec![sine(110.0, SR, 48_000)] }, &[("level", -18.0), ("release", 300.0)]);
    let ons: Vec<usize> = (0..48).map(|i| i * 97).collect();
    render(p.as_mut(), 16_000, |p, s, n| {
        within(s, n, &ons, |i, o| p.note_on(o, 30 + i as u8, 20 + i as u8 * 2));
        within(s, n, &[2_000, 2_100, 2_200], |i, o| p.note_off(o, 31 + i as u8));
    })
}

fn stereo_one_shot() -> u64 {
    let sample = SampleBuffer { sample_rate: 44_100.0, channels: vec![sine(500.0, 44_100.0, 8_000), sine(750.0, 44_100.0, 7_000)] };
    let mut p = sampler(sample, &[("play_mode", 1.0), ("start", 25.0), ("velocity", 100.0), ("key_low", 40.0), ("key_high", 80.0)]);
    render(p.as_mut(), 12_000, |p, s, n| {
        within(s, n, &[0, 100, 200, 300], |i, o| p.note_on(o, [60, 39, 81, 72][i], 80));
        within(s, n, &[150], |_, o| p.note_off(o, 60));
    })
}

fn events_and_all_off() -> u64 {
    let mut p = sampler(SampleBuffer { sample_rate: SR, channels: vec![sine(261.0, SR, 30_000)] }, &[("release", 10.0)]);
    let ev = |channel: u8, note_id: i32, message| PluginEvent { channel, note_id, message };
    render(p.as_mut(), 20_000, |p, s, n| {
        within(s, n, &[0, 10, 20, 4_000], |i, o| {
            let (ch, id, key) = [(0, 1, 60), (3, 2, 60), (3, -1, 64), (5, 9, 67)][i];
            assert!(p.event(o as u32, &ev(ch, id, Message::NoteOn { key, velocity: 100, id: NoteId::NONE })));
        });
        within(s, n, &[2_000, 2_500], |i, o| {
            let (ch, id, key) = [(3, 2, 60), (3, -1, 64)][i];
            assert!(p.event(o as u32, &ev(ch, id, Message::NoteOff { key, velocity: 0, id: NoteId::NONE })));
        });
        if (s..s + n).contains(&9_000) {
            p.all_notes_off();
        }
    })
}

#[test]
fn the_legacy_renderer_keeps_its_release_bits() {
    let got = [defaults(), forward_loop(), ping_pong(), off_rate(), automation(), steals(), stereo_one_shot(), events_and_all_off()];
    let want: [u64; 8] = [
        0x9d1b_8bc9_a3e8_a211,
        0x3487_dfa9_d754_c2c5,
        0x70c0_1120_79eb_f7c9,
        0x88d7_b269_33b2_432b,
        0xb8f6_3cfe_219f_911d,
        0x6d77_7307_c1d4_4599,
        0x0466_a0fa_ed80_4157,
        0x3aa1_ee17_b45e_c64d,
    ];
    assert_eq!(got, want, "legacy sampler goldens (got {got:#018x?})");
}
