//! The whole clip path's cost (ADR 0056 decision 8.8): a recorded wall-clock bench, not a CI gate (like
//! transit-resample's and transit-math's benches). Takes no arguments and prints its settings.
//!
//! Part 1, decision 8.8's measurement: one stereo in-memory clip at 48 kHz into a stereo strip through
//! `render_clip` (the read, the clip gain, both fades curved, the sum into the strip buffer), one
//! second of output in 512-frame blocks, at q = 0.5, 1.0595, 2, 4, 16.47 and 32. Each figure is the
//! median of five runs as a percentage of one core, next to the budget max(1, q/2) percent.
//!
//! Part 2, the common case before and after #604: unstretched in-memory clips (mono, stereo and six
//! channels into a stereo strip, and one looped stereo clip), the current `render_clip` against a copy
//! of the inner loop it replaced (`old_render_clip` below, from before #604), same settings.
//!
//! Run in release on a quiet machine: `cargo run --release --locked -p transit-mix --example clip_path_bench`.

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;
use transit_audio_io::AudioBuffer;
use transit_model::{ChannelFormat, Clip, ClipContent, ClipId, ClipLoop, Fade, FadeShape, Session, SourceAudio, SourceId, TrackKind};
use transit_time::{Range, Samples};

const RATE: usize = 48_000;
const BLOCK: usize = 512;
const RUNS: usize = 5;
const OFFSET: Samples = 2_048;

fn noise(channels: usize, frames: usize) -> Vec<Vec<f32>> {
    let mut s: u32 = 0x1234_5678;
    (0..channels)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((s >> 8) as f32) / (1u32 << 23) as f32 - 1.0
                })
                .collect()
        })
        .collect()
}

/// A session with one stereo audio track and `source` in its pool, and a one-second clip of it.
fn session(source: Vec<Vec<f32>>) -> (Session, Clip) {
    let mut s = Session::default();
    s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: RATE as u32, channels: source })));
    let clip = Clip::audio(ClipId(1), "c", SourceId(1), OFFSET, 0, RATE as Samples);
    (s, clip)
}

/// The median over [`RUNS`] of the seconds `render` takes for one second of output in blocks.
fn median(mut render: impl FnMut(Samples, usize, &mut [Vec<f32>])) -> f64 {
    let mut buf = vec![vec![0.0f32; BLOCK]; 2];
    let mut runs = [0.0f64; RUNS];
    for run in &mut runs {
        let mut sum = 0.0f32;
        let start = Instant::now();
        let mut pos = 0;
        while pos < RATE {
            let n = BLOCK.min(RATE - pos);
            buf.iter_mut().for_each(|c| c.fill(0.0));
            render(pos as Samples, n, &mut buf);
            sum += buf[0][0] + buf[1][n - 1];
            pos += n;
        }
        *run = start.elapsed().as_secs_f64();
        black_box(sum);
    }
    runs.sort_by(f64::total_cmp);
    runs[RUNS / 2]
}

fn main() {
    const RATES: [f64; 6] = [0.5, 1.059_463_094_359_295_3, 2.0, 4.0, 16.47, 32.0];
    transit_model::disk::prepare_reader();
    println!("transit-mix clip path bench (ADR 0056 decision 8.8), release build");
    println!(
        "settings: {RATE} Hz, stereo strip, {BLOCK}-frame blocks, one second ({RATE} frames) per run, median of {RUNS} runs, in-memory sources, clip offset {OFFSET}"
    );
    println!("part 1: one stereo clip, gain -3 dB, S-curve fade-in and exponential fade-out of half a second each, stretch 1/q, rates q {RATES:?}");
    println!("{:<10} {:>10} {:>10} {:>10}", "q", "ns/frame", "% core", "budget %");
    let (s, base) = session(noise(2, 32 * RATE + 4 * 1_024 + OFFSET as usize));
    for q in RATES {
        let mut clip = base.clone();
        clip.stretch = 1.0 / q;
        clip.gain_db = -3.0;
        clip.fade_in = Fade { len: RATE as Samples / 2, shape: FadeShape::SCurve, curvature: 0.0 };
        clip.fade_out = Fade { len: RATE as Samples / 2, shape: FadeShape::Exponential, curvature: 0.0 };
        let secs = median(|pos, n, buf| transit_mix::render_clip(&s, &clip, pos, n, buf));
        println!("{q:<10.4} {:>10.1} {:>10.3} {:>10.2}", secs * 1e9 / RATE as f64, secs * 100.0, (q / 2.0).max(1.0));
    }
    println!("two transposed clips crossfading: not measured (clips have no transpose before the rest of slice 7a)");

    println!("part 2: unstretched clips, gain 0 dB, no fades: the current path against the pre-#604 inner loop");
    println!("{:<22} {:>12} {:>12} {:>8}", "case", "new % core", "old % core", "new/old");
    let cases: [(&str, usize, Option<ClipLoop>); 4] = [
        ("mono into stereo", 1, None),
        ("stereo into stereo", 2, None),
        ("6 channels into stereo", 6, None),
        ("stereo, looped", 2, Some(ClipLoop { start: 1_000, end: 13_000 })),
    ];
    for (name, channels, looping) in cases {
        let (s, mut clip) = session(noise(channels, RATE + 4 * 1_024 + OFFSET as usize));
        clip.looping = looping;
        let src = s.pool.get(SourceId(1)).map(|a| a.buffer.channels.clone()).unwrap_or_default();
        let new = median(|pos, n, buf| transit_mix::render_clip(&s, &clip, pos, n, buf));
        let old = median(|pos, n, buf| old_render_clip(&clip, pos, n, buf, &src));
        println!("{name:<22} {:>12.3} {:>12.3} {:>8.3}", new * 100.0, old * 100.0, new / old);
    }
}

/// The in-memory clip render as it was before #604 (`render_clip`'s memory branch, `mix_clip` and
/// `StreamMap::sample` at c3468fe7), kept to compare the common case.
fn old_render_clip(clip: &Clip, pos: Samples, frames: usize, buf: &mut [Vec<f32>], src: &[Vec<f32>]) {
    let ClipContent::Audio { offset, .. } = clip.content else { return };
    let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    let Some(isect) = clip.range().intersect(&block) else { return };
    let i0 = usize::try_from(isect.start - pos).unwrap_or(0);
    let i1 = usize::try_from(isect.end - pos).unwrap_or(0).min(frames);
    let fetch = |c: usize, k: Samples| old_sample(clip, offset, stretch, k, |x| src.get(c).and_then(|ch| ch.get(x)).copied());
    old_mix_clip(clip, pos, i0, i1, buf, src.len(), fetch);
}

/// `StreamMap::sample` before #604, generic over `get` as it was (so it inlines the same way).
#[inline]
fn old_sample(clip: &Clip, offset: Samples, stretch: f64, k: Samples, get: impl Fn(usize) -> Option<f32>) -> f32 {
    let rel = clip.content_pos(k);
    if (stretch - 1.0).abs() < 1e-9 {
        usize::try_from(offset.saturating_add(rel)).ok().and_then(&get).unwrap_or(0.0)
    } else {
        let f = offset as f64 + rel as f64 / stretch;
        let x0 = f.floor();
        let fr = (f - x0) as f32;
        let a = if x0 >= 0.0 { get(x0 as usize).unwrap_or(0.0) } else { 0.0 };
        let b = if x0 + 1.0 >= 0.0 { get((x0 as usize).saturating_add(1)).unwrap_or(0.0) } else { 0.0 };
        a + (b - a) * fr
    }
}

fn old_mix_clip(clip: &Clip, pos: Samples, i0: usize, i1: usize, buf: &mut [Vec<f32>], nsrc: usize, fetch: impl Fn(usize, Samples) -> f32) {
    if nsrc == 0 {
        return;
    }
    const STEP: usize = 32;
    let ndst = buf.len();
    for (ch, dst) in buf.iter_mut().enumerate() {
        let contributors = if nsrc > ndst { (nsrc - 1 - ch) / ndst + 1 } else { 1 };
        let mut i = i0;
        while i < i1 {
            let seg_end = (i + STEP).min(i1);
            let rel0 = pos.saturating_add(i as i64).saturating_sub(clip.start);
            let rel1 = pos.saturating_add(seg_end.saturating_sub(1) as i64).saturating_sub(clip.start);
            let g0 = clip.gain_at(rel0);
            let g1 = clip.gain_at(rel1.min(clip.length));
            let n = (seg_end - i).saturating_sub(1).max(1) as f32;
            for k in i..seg_end {
                let rel = pos.saturating_add(k as i64).saturating_sub(clip.start);
                let v = if nsrc > ndst {
                    (ch..nsrc).step_by(ndst).map(|c| fetch(c, rel)).sum::<f32>() / contributors as f32
                } else {
                    fetch(ch % nsrc, rel)
                };
                let t = (k - i) as f32 / n;
                if let Some(d) = dst.get_mut(k) {
                    *d += v * (g0 + (g1 - g0) * t);
                }
            }
            i = seg_end;
        }
    }
}
