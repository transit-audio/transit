use super::*;

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
pub(crate) fn render_clip(s: &Session, clip: &Clip, pos: Samples, frames: usize, buf: &mut [Vec<f32>]) {
    let ClipContent::Audio { source, offset } = clip.content else { return };
    let block = Range { start: pos, end: pos.saturating_add(frames as i64) };
    let Some(isect) = clip.range().intersect(&block) else { return };
    let Some(audio) = s.pool.get(source) else { return };
    let nsrc = audio.buffer.channels.len();
    if nsrc == 0 {
        return;
    }
    let stretch = if clip.stretch.is_finite() && clip.stretch > 0.0 { clip.stretch } else { 1.0 };
    let i0 = usize::try_from(isect.start - pos).unwrap_or(0);
    let i1 = usize::try_from(isect.end - pos).unwrap_or(0).min(frames);
    // Gain is evaluated every 32 samples and interpolated (fades and envelopes stay smooth).
    const STEP: usize = 32;
    let ndst = buf.len();
    for (ch, dst) in buf.iter_mut().enumerate() {
        let src = audio.buffer.channels.get(ch % nsrc).map(Vec::as_slice).unwrap_or(&[]);
        let contributors = if nsrc > ndst { (nsrc - 1 - ch) / ndst + 1 } else { 1 };
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
                // A looped clip (ADR 0019) reads its content modulo the loop range.
                let rel = clip.content_pos(pos.saturating_add(k as i64).saturating_sub(clip.start));
                let sample = |src: &[f32]| {
                    if (stretch - 1.0).abs() < 1e-9 {
                        usize::try_from(offset.saturating_add(rel)).ok().and_then(|x| src.get(x)).copied().unwrap_or(0.0)
                    } else {
                        let f = offset as f64 + rel as f64 / stretch;
                        let x0 = f.floor();
                        let fr = (f - x0) as f32;
                        let a = if x0 >= 0.0 { src.get(x0 as usize).copied().unwrap_or(0.0) } else { 0.0 };
                        let b = if x0 + 1.0 >= 0.0 { src.get((x0 as usize).saturating_add(1)).copied().unwrap_or(0.0) } else { 0.0 };
                        a + (b - a) * fr
                    }
                };
                let v = if nsrc > ndst {
                    audio.buffer.channels.iter().skip(ch).step_by(ndst).map(|src| sample(src)).sum::<f32>() / contributors as f32
                } else {
                    sample(src)
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
