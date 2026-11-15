use super::*;
use transit_model::disk::{Memory, StreamMap};

/// Render and sum one clip, reusing the prepared FX buffer in bounded chunks.
fn mix_clip_fx(
    frames: usize,
    out: &mut [Vec<f32>],
    clipbuf: &mut [Vec<f32>],
    fx: &mut ClipFx,
    mut render: impl FnMut(usize, usize, &mut [Vec<f32>]),
) {
    let block = clipbuf.iter().map(Vec::len).min().unwrap_or(0);
    if block == 0 || clipbuf.len() != out.len() {
        return;
    }
    let mut done = 0;
    while done < frames {
        let n = (frames - done).min(block);
        for c in clipbuf.iter_mut() {
            c.iter_mut().take(n).for_each(|x| *x = 0.0);
        }
        render(done, n, clipbuf);
        fx.process(clipbuf, n);
        for (dst, src) in out.iter_mut().zip(clipbuf.iter()) {
            for (x, y) in dst.iter_mut().skip(done).take(n).zip(src) {
                *x += *y;
            }
        }
        done += n;
    }
}

pub(crate) fn render_audio_clips(s: &Session, t: &Track, strip: &mut Strip, pos: Samples, frames: usize) {
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    if on_audio_thread() {
        cue_disk_clips(s, t, block);
    }
    for clip in t.clips() {
        if clip.muted || !clip.is_audio() || !clip.range().overlaps(&block) {
            continue;
        }
        let mut gain_only = ClipFx::new(s.sample_rate.as_f64() as f32, strip.buf.len(), false);
        let fx = clip_fx_params(s, clip.id.0).map(|params| {
            // Modules are prepared by the compiler; absent modules never instantiate here.
            let fx = strip.clip_fx.get_mut(clip.id.0).unwrap_or(&mut gain_only);
            fx.configure(params);
            fx
        });
        match fx {
            None => render_clip(s, clip, pos, frames, &mut strip.buf),
            Some(fx) => mix_clip_fx(frames, &mut strip.buf, &mut strip.clipbuf, fx, |done, n, buf| {
                render_clip(s, clip, pos.saturating_add(done as i64), n, buf);
            }),
        }
    }
}

/// Audio thread: ask the streamer for what the playhead reaches next (ADR 0035), lock-free: each
/// disk-backed clip starting within half a ring after this block, and when the block ends within
/// half a ring before the cycle's end, each clip under the cycle's start. A clip that is
/// playing asks for itself when its ring runs short ([`render_clip`]).
fn cue_disk_clips(s: &Session, t: &Track, block: Range) {
    if !s.pool.has_disk() {
        return;
    }
    let cycle = s.edit.cycle_range();
    let looping = s.edit.loop_playback && !cycle.is_empty() && block.end <= cycle.end;
    // No ring is longer than two seconds at the session rate, or its floor: half of that bounds the
    // lead, so only clips that close look their source up.
    let horizon = i64::from(s.sample_rate.hz()).max(1 << 13);
    let near_cycle = looping && cycle.end - block.end <= horizon;
    for clip in t.clips() {
        let ahead = clip.start >= block.end && clip.start < block.end.saturating_add(horizon);
        if clip.muted || !(ahead || near_cycle && clip.range().overlaps(&Range::new(cycle.start, cycle.start.saturating_add(horizon)))) {
            continue;
        }
        let Some(d) = clip.source().and_then(|id| s.pool.get(id)).and_then(|a| a.disk.as_deref()) else { continue };
        let Some(map) = StreamMap::of(clip) else { continue };
        let lead = (d.window() / 2) as i64;
        if clip.start >= block.end && clip.start < block.end.saturating_add(lead) {
            d.cue(&map, 0);
        }
        if looping && cycle.end - block.end <= lead && clip.range().overlaps(&Range::new(cycle.start, cycle.start.saturating_add(lead))) {
            d.cue(&map, cycle.start.saturating_sub(clip.start).max(0));
        }
    }
}

/// Clip-effect parameters of a clip, or None when it has none or they are bypassed.
pub(crate) fn clip_fx_params(s: &Session, clip: u64) -> Option<impl Iterator<Item = (&str, f32)> + Clone> {
    if s.edit.values.is_empty() || Key::new(format_args!("clip_fx.bypass.{clip}")).is_none_or(|k| s.edit.flag(k.as_str())) {
        return None;
    }
    let pre = Key::new(format_args!("clip_fx.{clip}."))?;
    let mut params = s
        .edit
        .values
        .range::<str, _>((Bound::Included(pre.as_str()), Bound::Unbounded))
        .take_while(move |(k, _)| k.starts_with(pre.as_str()))
        .map(move |(k, v)| (k.get(pre.len..).unwrap_or(""), *v as f32))
        .peekable();
    params.peek()?;
    Some(params)
}

/// Channel-width law (shared by plan and consolidate rendering): when the
/// source is wider, destination j averages source channels j, j + destination_width, ... .
/// Mono therefore gets the mean of ALL channels, (L + R) / 2 for stereo. This is an ordinal,
/// layout-agnostic fold, not a speaker-aware surround matrix: six channels into stereo become
/// (0 + 2 + 4) / 3 and (1 + 3 + 5) / 3, including LFE if present. Nothing is discarded.
/// Equal widths map directly; narrower sources retain cyclic duplication (mono to both sides).
pub fn render_clip(s: &Session, clip: &Clip, pos: Samples, frames: usize, buf: &mut [Vec<f32>]) {
    let ClipContent::Audio { source, .. } = clip.content else { return };
    let Some(map) = StreamMap::of(clip) else { return };
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    let Some(isect) = clip.range().intersect(&block) else { return };
    let Some(audio) = s.pool.get(source) else { return };
    let i0 = usize::try_from(isect.start - pos).unwrap_or(0);
    let i1 = usize::try_from(isect.end - pos).unwrap_or(0).min(frames);
    // Every read of the clip's source goes through `ClipReader::read` (a stretched clip through the
    // band-limited reader, ADR 0056 decision 8; #604), except the ring, which holds its output.
    match audio.disk.as_deref() {
        None => {
            let mut src = Memory(&audio.buffer.channels);
            let nsrc = audio.buffer.channels.len();
            let reader = map.reader();
            mix_clip(clip, pos, i0, i1, buf, nsrc, |k, first, out| reader.read(k, &mut src, first, out));
        }
        // The audio thread (and a render worker for it) only drains the clip's ring (ADR 0035).
        Some(d) if on_audio_thread() => {
            let k0 = isect.start.saturating_sub(clip.start);
            match d.stream(&map, k0, i1.saturating_sub(i0)) {
                Some(ring) => mix_clip(clip, pos, i0, i1, buf, d.channels(), |k, first, out| {
                    for (c, o) in out.iter_mut().enumerate() {
                        *o = ring.get(first + c, k);
                    }
                }),
                None => d.underrun(&map, k0),
            }
        }
        // Offline (bounce, export, consolidate): the same file, read here.
        Some(d) => {
            let reader = map.reader();
            d.with_sync_source(|src| mix_clip(clip, pos, i0, i1, buf, d.channels(), |k, first, out| reader.read(k, src, first, out)));
        }
    }
}

/// Source channels one read of a clip fetches together (the reader then computes each tap's
/// coefficient once for all of them); a wider source reads each channel on its own, which gives the
/// same samples.
const FRAME: usize = 32;

/// Sum frames `i0..i1` of a block at `pos` of `clip` into `buf`, with its gain, reading source
/// channels `first..first + out.len()` at clip time `k` into `out` through `fetch(k, first, out)`
/// (`nsrc` channels).
fn mix_clip(clip: &Clip, pos: Samples, i0: usize, i1: usize, buf: &mut [Vec<f32>], nsrc: usize, mut fetch: impl FnMut(Samples, usize, &mut [f32])) {
    if nsrc == 0 {
        return;
    }
    // Gain is evaluated every 32 samples and interpolated (fades and envelopes stay smooth).
    const STEP: usize = 32;
    let ndst = buf.len();
    let mut frame = [0.0f32; FRAME];
    let mut i = i0;
    while i < i1 {
        let seg_end = (i + STEP).min(i1);
        let rel0 = pos.saturating_add(i as i64).saturating_sub(clip.start);
        // The last sample of this step, not the first sample of the next one, so a breakpoint
        // on the boundary does not ramp the audio before it.
        let rel1 = pos.saturating_add(seg_end.saturating_sub(1) as i64).saturating_sub(clip.start);
        let g0 = clip.gain_at(rel0);
        let g1 = clip.gain_at(rel1.min(clip.length));
        let n = (seg_end - i).saturating_sub(1).max(1) as f32;
        for k in i..seg_end {
            // Clip time; a looped clip (ADR 0019) reads its content modulo the loop range.
            let rel = pos.saturating_add(k as i64).saturating_sub(clip.start);
            let t = (k - i) as f32 / n;
            let gain = g0 + (g1 - g0) * t;
            let whole = nsrc <= FRAME;
            if whole {
                fetch(rel, 0, &mut frame[..nsrc]);
            }
            for (ch, dst) in buf.iter_mut().enumerate() {
                let mut get = |c: usize| {
                    if whole {
                        frame[c]
                    } else {
                        let mut one = [0.0f32];
                        fetch(rel, c, &mut one);
                        one[0]
                    }
                };
                let v = if nsrc > ndst {
                    let contributors = (nsrc - 1 - ch) / ndst + 1;
                    (ch..nsrc).step_by(ndst).map(&mut get).sum::<f32>() / contributors as f32
                } else {
                    get(ch % nsrc)
                };
                if let Some(d) = dst.get_mut(k) {
                    *d += v * gain;
                }
            }
        }
        i = seg_end;
    }
}

/// Render only one track's clips (no mixer processing) — Consolidate.
pub fn render_clips(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let len = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / t.channels().max(1));
    let mut out = vec![vec![0.0f32; len]; t.channels().max(1)];
    let mut clipbuf = vec![vec![0.0; len.min(4096)]; out.len()];
    for c in t.clips().iter().filter(|c| !c.muted && c.range().overlaps(&range)) {
        let mut fx = ClipFx::new(s.sample_rate.as_f64() as f32, out.len(), true);
        let fx = clip_fx_params(s, c.id.0).map(|params| {
            fx.configure(params);
            &mut fx
        });
        match fx {
            None => render_clip(s, c, range.start, len, &mut out),
            Some(fx) => {
                // Render the full range first to preserve its fade-interpolation boundaries.
                let mut raw = vec![vec![0.0; len]; out.len()];
                render_clip(s, c, range.start, len, &mut raw);
                mix_clip_fx(len, &mut out, &mut clipbuf, fx, |done, n, buf| {
                    for (dst, src) in buf.iter_mut().zip(&raw) {
                        if let (Some(dst), Some(src)) = (dst.get_mut(..n), src.get(done..done + n)) {
                            dst.copy_from_slice(src);
                        }
                    }
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::session_with_dc;

    #[test]
    fn clip_effects_gain_and_trim_automation_apply() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        let cid = s.track(t).unwrap().clips()[0].id.0;
        s.edit.values.insert(format!("clip_fx.{cid}.gain"), -6.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        let expect = 0.5 * std::f32::consts::FRAC_1_SQRT_2 * db_to_gain(-6.0);
        assert!((out[0][2000] - expect).abs() < 0.01, "{}", out[0][2000]);
        s.edit.set_flag(&format!("clip_fx.bypass.{cid}"), true);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][2000] - 0.5 * std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        s.track_mut(t).unwrap().lane_mut(&AutoParam::Plugin { slot: u8::MAX, param: "trim".into() }).set_point(0, -12.0);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!(out[0][2000] < 0.2, "{}", out[0][2000]);
    }
}
