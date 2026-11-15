//! Unstretched in-memory clips render exactly as they did before #604 (issue #604 round 3): the direct
//! path that keeps their cost is pinned bit for bit against a copy of the pre-#604 loop, over every
//! source width (1, 2, 3, 6, 32, 33 and 40 channels) into strips of 1, 2 and 6, offsets before the first
//! frame and past the last, loops, gain, envelope and fades, and blocks at the clip's start, middle and
//! end.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use transit_audio_io::AudioBuffer;
use transit_model::{ClipId, Fade, FadeShape, SourceAudio, SourceId};

/// The in-memory clip render from before #604 (`render_clip`'s memory branch, `mix_clip` and
/// `StreamMap::sample` at the base commit), for a stretch of exactly 1.
fn old_render_clip(clip: &Clip, pos: Samples, frames: usize, buf: &mut [Vec<f32>], src: &[Vec<f32>]) {
    let ClipContent::Audio { offset, .. } = clip.content else { return };
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    let Some(isect) = clip.range().intersect(&block) else { return };
    let i0 = usize::try_from(isect.start - pos).unwrap_or(0);
    let i1 = usize::try_from(isect.end - pos).unwrap_or(0).min(frames);
    let nsrc = src.len();
    if nsrc == 0 {
        return;
    }
    let fetch = |c: usize, k: Samples| {
        let rel = clip.content_pos(k);
        usize::try_from(offset.saturating_add(rel)).ok().and_then(|x| src.get(c).and_then(|ch| ch.get(x)).copied()).unwrap_or(0.0)
    };
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

/// `channels` channels of 3,000 frames of integer noise, with -0.0 and -1.0 among them.
fn source(channels: usize) -> Vec<Vec<f32>> {
    let mut x: u32 = 0x5eed_0604 ^ channels as u32;
    (0..channels)
        .map(|_| {
            (0..3_000)
                .map(|i| {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    match i % 89 {
                        3 => -0.0,
                        7 => -1.0,
                        _ => (x >> 8) as f32 / (1u32 << 23) as f32 - 1.0,
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn an_unstretched_clip_renders_bit_for_bit_as_before_604() {
    // (offset, start, length, loop): plain, before the first frame, past the last, two loops.
    let shapes: [(Samples, Samples, Samples, Option<ClipLoop>); 6] = [
        (0, 0, 2_000, None),
        (37, 500, 2_000, None),
        (-120, 300, 1_500, None),
        (2_600, 200, 1_200, None),
        (100, 400, 3_000, Some(ClipLoop { start: 200, end: 1_000 })),
        (0, 0, 2_500, Some(ClipLoop { start: -50, end: 700 })),
    ];
    let mut blocks_checked = 0;
    for channels in [1usize, 2, 3, 6, 32, 33, 40] {
        let src = source(channels);
        let mut s = Session::default();
        s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: src.clone() })));
        for (offset, start, length, looping) in shapes {
            for shaped in [false, true] {
                let mut c = Clip::audio(ClipId(1), "c", SourceId(1), offset, start, length);
                c.looping = looping;
                if shaped {
                    c.gain_db = -4.5;
                    c.gain_env = vec![(0, -6.0), (300, 0.0), (900, -12.0)];
                    c.fade_in = Fade { len: 400, shape: FadeShape::SCurve, curvature: 0.0 };
                    c.fade_out = Fade { len: 700, shape: FadeShape::Exponential, curvature: 0.3 };
                }
                let end = start + length;
                let positions = [start - 50, start, start + length / 2 - 33, end - 100, end - 4_096 + 20];
                for strip in [1usize, 2, 6] {
                    for frames in [97usize, 512, 4_096] {
                        for pos in positions {
                            // Prefilled, so the sum into the strip is checked too.
                            let mut got: Vec<Vec<f32>> = (0..strip).map(|d| vec![0.25 * d as f32 - 0.1; frames]).collect();
                            let mut want = got.clone();
                            render_clip(&s, &c, pos, frames, &mut got);
                            old_render_clip(&c, pos, frames, &mut want, &src);
                            let bits = |x: &Vec<Vec<f32>>| x.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>();
                            assert_eq!(
                                bits(&got),
                                bits(&want),
                                "{channels} channels into {strip}, offset {offset} start {start} loop {looping:?} shaped {shaped}, block {frames} at {pos}"
                            );
                            blocks_checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(blocks_checked, 7 * 6 * 2 * 3 * 3 * 5);
}

/// Every loop gives the same samples, so only this pins the cost: an unstretched in-memory clip takes
/// the direct slice loop, an unstretched clip from the ring or the file takes the sample-per-fetch
/// loop, and only stretched reads from memory or the file take the reader's frame loop (#604).
#[test]
fn clip_loop_routing_keeps_the_common_case_on_the_cheap_loops() {
    use crate::clips::{Loop, Path, clip_loop};
    assert_eq!(clip_loop(Path::Memory, true), Loop::Direct);
    assert_eq!(clip_loop(Path::Ring, true), Loop::Channels);
    assert_eq!(clip_loop(Path::Offline, true), Loop::Channels);
    assert_eq!(clip_loop(Path::Ring, false), Loop::Channels);
    assert_eq!(clip_loop(Path::Memory, false), Loop::Frames);
    assert_eq!(clip_loop(Path::Offline, false), Loop::Frames);
}
