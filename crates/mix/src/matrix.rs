use super::*;

/// Per-block routing gains: `m[src][dst]`.
type Matrix = [[f32; MAX_CHANNELS]; MAX_CHANNELS];

/// How a source is panned: the stereo pan pair (mono uses `[0]`) and the optional surround pan.
pub(crate) struct PanSpec<'a> {
    pub(crate) stereo: [f32; 2],
    pub(crate) surround: Option<&'a SurroundPan>,
}

pub(crate) fn identity_matrix(n: usize) -> Matrix {
    let mut m = [[0.0; MAX_CHANNELS]; MAX_CHANNELS];
    for (i, row) in m.iter_mut().enumerate().take(n) {
        if let Some(g) = row.get_mut(i) {
            *g = 1.0;
        }
    }
    m
}

/// ITU-R BS.775 stereo fold-down gains (to L, to R) for one speaker: C and surrounds at -3 dB,
/// the centre surround at -6 dB per side, LFE dropped.
pub fn itu_stereo_fold(sp: Speaker) -> (f32, f32) {
    use Speaker::*;
    let h = std::f32::consts::FRAC_1_SQRT_2;
    match sp {
        L | Lc | Lw => (1.0, 0.0),
        R | Rc | Rw => (0.0, 1.0),
        C => (h, h),
        Lfe => (0.0, 0.0),
        Cs => (0.5, 0.5),
        Ls | Lss | Lrs | Ltf | Ltm | Ltr => (h, 0.0),
        Rs | Rss | Rrs | Rtf | Rtm | Rtr => (0.0, h),
    }
}

/// Speaker directions of a format (for the DSP panner). Returns the count written.
fn layout_of(fmt: ChannelFormat, out: &mut [SpeakerPos; MAX_CHANNELS]) -> usize {
    let sp = fmt.speakers();
    for (o, s) in out.iter_mut().zip(sp.iter()) {
        *o = SpeakerPos { az: s.azimuth(), el: s.elevation(), lfe: s.is_lfe() };
    }
    sp.len().min(MAX_CHANNELS)
}

fn surround_params(sp: &SurroundPan, dx: f32) -> SurroundParams {
    SurroundParams { x: (sp.x + dx).clamp(-1.0, 1.0), y: sp.y, z: sp.z, divergence: sp.divergence, center: sp.center / 100.0 }
}

/// Routing gains from a source (`ns` channels of `src_fmt`) into a destination (`nd` channels of
/// `dst_fmt`). See the crate docs for the rules.
pub(crate) fn build_matrix(src_fmt: ChannelFormat, ns: usize, dst_fmt: ChannelFormat, nd: usize, pan: &PanSpec<'_>, law: PanLaw) -> Matrix {
    let mut m = [[0.0f32; MAX_CHANNELS]; MAX_CHANNELS];
    let (ns, nd) = (ns.min(MAX_CHANNELS), nd.min(MAX_CHANNELS));
    if ns == 0 || nd == 0 {
        return m;
    }
    let set = |m: &mut Matrix, s: usize, d: usize, g: f32| {
        if let Some(x) = m.get_mut(s).and_then(|r| r.get_mut(d)) {
            *x += g;
        }
    };
    if nd == 1 {
        // Mono destination: the stereo routing summed at -6 dB.
        let st = build_matrix(src_fmt, ns, ChannelFormat::Stereo, 2, pan, law);
        for (row, srow) in m.iter_mut().zip(st.iter()).take(ns) {
            row[0] = 0.5 * (srow[0] + srow[1]);
        }
        return m;
    }
    let src_sp = src_fmt.speakers();
    let src_amb = src_fmt.is_ambisonic() && ns >= 4;
    if nd == 2 && !dst_fmt.is_ambisonic() {
        match ns {
            1 => {
                let (gl, gr) = gains(pan.stereo[0], law);
                m[0][0] = gl;
                m[0][1] = gr;
            }
            2 if !src_amb => {
                let (a_l, a_r) = gains(pan.stereo[0], PanLaw::Zero);
                let (b_l, b_r) = gains(pan.stereo[1], PanLaw::Zero);
                m[0] = [0.0; MAX_CHANNELS];
                m[0][0] = a_l;
                m[0][1] = a_r;
                m[1][0] = b_l;
                m[1][1] = b_r;
            }
            _ if src_amb => {
                for (d, az) in [(0usize, -30.0f32), (1, 30.0)] {
                    let g = transit_dsp::pan::ambisonic_decode(az, 0.0, 2);
                    for (c, gc) in g.iter().enumerate() {
                        set(&mut m, c, d, *gc);
                    }
                }
            }
            _ if src_sp.len() == ns => {
                for (s, sp) in src_sp.iter().enumerate() {
                    let (l, r) = itu_stereo_fold(*sp);
                    set(&mut m, s, 0, l);
                    set(&mut m, s, 1, r);
                }
            }
            _ => {
                // Unknown layout: first two channels to L/R, the rest to the centre.
                let h = std::f32::consts::FRAC_1_SQRT_2;
                m[0][0] = 1.0;
                m[1][1] = 1.0;
                for row in m.iter_mut().take(ns).skip(2) {
                    row[0] = h;
                    row[1] = h;
                }
            }
        }
        return m;
    }
    if ns > 2 && src_fmt == dst_fmt && ns == nd {
        return identity_matrix(ns);
    }
    // Multichannel destination.
    let mut layout = [SpeakerPos::default(); MAX_CHANNELS];
    let nl = layout_of(dst_fmt, &mut layout);
    let layout = layout.get(..nl).unwrap_or(&[]);
    let dst_sp = dst_fmt.speakers();
    let dst_amb = dst_fmt.is_ambisonic();
    let find = |sp: Speaker| dst_sp.iter().position(|x| *x == sp).filter(|i| *i < nd);
    let mut g = [0.0f32; MAX_CHANNELS];
    if dst_amb {
        if src_amb || (src_fmt.is_ambisonic() && ns > 2) {
            return identity_matrix(ns.min(nd));
        }
        let mut enc = |m: &mut Matrix, s: usize, az: f32, el: f32, spread: f32| {
            transit_dsp::pan::ambisonic_encode(az, el, spread, &mut g);
            for (d, gd) in g.iter().enumerate().take(nd) {
                set(m, s, d, *gd);
            }
        };
        match (ns, pan.surround) {
            (1 | 2, Some(sp)) => {
                for s in 0..ns {
                    let dx = if ns == 2 { if s == 0 { -1.0 } else { 1.0 } } else { 0.0 };
                    let p = surround_params(sp, dx);
                    enc(&mut m, s, transit_dsp::pan::puck_azimuth(&[], p.x, p.y), p.z * 90.0, p.divergence);
                }
            }
            (1, None) => enc(&mut m, 0, pan.stereo[0].clamp(-1.0, 1.0) * 30.0, 0.0, 0.0),
            (2, None) => {
                enc(&mut m, 0, -30.0, 0.0, 0.0);
                enc(&mut m, 1, 30.0, 0.0, 0.0);
            }
            _ => {
                for (s, sp) in src_sp.iter().enumerate().take(ns) {
                    if !sp.is_lfe() {
                        enc(&mut m, s, sp.azimuth(), sp.elevation(), 0.0);
                    }
                }
            }
        }
        return m;
    }
    if dst_sp.len() != nd {
        // No layout to pan on: channel for channel.
        return identity_matrix(ns.min(nd));
    }
    let (li, ri) = (find(Speaker::L), find(Speaker::R));
    match (ns, pan.surround) {
        (1 | 2, Some(sp)) if !src_amb => {
            for s in 0..ns {
                let dx = if ns == 2 { if s == 0 { -1.0 } else { 1.0 } } else { 0.0 };
                transit_dsp::pan::surround_gains(layout, &surround_params(sp, dx), &mut g);
                for (d, gd) in g.iter().enumerate().take(nd) {
                    set(&mut m, s, d, *gd);
                }
                if let Some(lfe) = find(Speaker::Lfe)
                    && sp.lfe_db > -143.9
                {
                    set(&mut m, s, lfe, db_to_gain(sp.lfe_db.min(12.0)) / ns as f32);
                }
            }
        }
        (1, None) => {
            let (gl, gr) = gains(pan.stereo[0], law);
            if let (Some(l), Some(r)) = (li, ri) {
                set(&mut m, 0, l, gl);
                set(&mut m, 0, r, gr);
            }
        }
        (2, None) if !src_amb => {
            let (a_l, a_r) = gains(pan.stereo[0], PanLaw::Zero);
            let (b_l, b_r) = gains(pan.stereo[1], PanLaw::Zero);
            if let (Some(l), Some(r)) = (li, ri) {
                set(&mut m, 0, l, a_l);
                set(&mut m, 0, r, a_r);
                set(&mut m, 1, l, b_l);
                set(&mut m, 1, r, b_r);
            }
        }
        _ if src_amb => {
            let n_spk = dst_sp.iter().filter(|s| !s.is_lfe()).count();
            for (d, sp) in dst_sp.iter().enumerate().take(nd) {
                if sp.is_lfe() {
                    continue;
                }
                let dec = transit_dsp::pan::ambisonic_decode(sp.azimuth(), sp.elevation(), n_spk);
                for (c, gc) in dec.iter().enumerate() {
                    set(&mut m, c, d, *gc);
                }
            }
        }
        _ if src_sp.len() == ns => {
            for (s, sp) in src_sp.iter().enumerate() {
                if let Some(d) = find(*sp) {
                    set(&mut m, s, d, 1.0);
                } else if sp.is_lfe() {
                    // No LFE in the destination: dropped.
                } else {
                    transit_dsp::pan::direction_gains(layout, sp.azimuth(), sp.elevation(), &mut g);
                    for (d, gd) in g.iter().enumerate().take(nd) {
                        set(&mut m, s, d, *gd);
                    }
                }
            }
        }
        _ => return identity_matrix(ns.min(nd)),
    }
    m
}

/// `dst[d] += Σ_s src[s] × gain × m[s][d]` for every destination channel (no allocation).
pub(crate) fn apply_matrix(src: &[Vec<f32>], frames: usize, dst: &mut [Vec<f32>], m: &Matrix, gain: f32) {
    apply_matrix_span(src, 0..frames, dst, m, gain);
}

/// [`apply_matrix`] over the frames in `span` only.
pub(crate) fn apply_matrix_span(src: &[Vec<f32>], span: std::ops::Range<usize>, dst: &mut [Vec<f32>], m: &Matrix, gain: f32) {
    let ns = src.len().min(MAX_CHANNELS);
    for (d, out) in dst.iter_mut().enumerate().take(MAX_CHANNELS) {
        let mut idx = [(0usize, 0.0f32); MAX_CHANNELS];
        let mut k = 0;
        for (s, row) in m.iter().enumerate().take(ns) {
            let g = row.get(d).copied().unwrap_or(0.0);
            if g != 0.0
                && let Some(slot) = idx.get_mut(k)
            {
                *slot = (s, g);
                k += 1;
            }
        }
        let taps = idx.get(..k).unwrap_or(&[]);
        if taps.is_empty() {
            continue;
        }
        for (i, o) in out.iter_mut().enumerate().take(span.end).skip(span.start) {
            let mut acc = 0.0f32;
            for &(s, g) in taps {
                acc += src.get(s).and_then(|c| c.get(i)).copied().unwrap_or(0.0) * gain * g;
            }
            *o += acc;
        }
    }
}

/// ITU-R BS.775 stereo fold-down of planar audio in `fmt` (C and surrounds at -3 dB, LFE dropped;
/// Ambisonics play their W channel). Mono/stereo input is returned unchanged.
pub fn fold_down_stereo(fmt: ChannelFormat, ch: &[Vec<f32>]) -> Vec<Vec<f32>> {
    if ch.len() <= 2 {
        return ch.to_vec();
    }
    let len = ch.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![vec![0.0f32; len]; 2];
    let sp = fmt.speakers();
    let h = std::f32::consts::FRAC_1_SQRT_2;
    for (c, src) in ch.iter().enumerate() {
        let (gl, gr) = match sp.get(c) {
            Some(x) if sp.len() == ch.len() => itu_stereo_fold(*x),
            _ if c == 0 => (h, h),
            _ => (0.0, 0.0),
        };
        for (g, dst) in [(gl, 0usize), (gr, 1)] {
            if g == 0.0 {
                continue;
            }
            if let Some(d) = out.get_mut(dst) {
                for (o, x) in d.iter_mut().zip(src.iter()) {
                    *o += x * g;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{PlanRig, session_with_dc};
    use transit_audio_io::AudioBuffer;
    use transit_model::SourceId;

    /// A session with one track of `fmt` playing a distinct DC level per channel (0.1, 0.2, …).
    fn multichannel_session(fmt: ChannelFormat, main: ChannelFormat, frames: usize) -> (Session, TrackId) {
        let mut s = Session::default();
        s.outputs[0].format = main;
        let t = s.add_track(TrackKind::Audio, fmt, None);
        let chans: Vec<Vec<f32>> = (0..fmt.channels()).map(|c| (0..frames).map(|i| 0.1 * (c + 1) as f32 + (i % 7) as f32 * 1e-3).collect()).collect();
        s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: chans })));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "mc", SourceId(901), 0, 0, frames as i64));
        (s, t)
    }

    #[test]
    fn surround_track_passes_through_bit_exact() {
        let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround51, 4800);
        let src = s.pool.get(SourceId(901)).unwrap().buffer.channels.clone();
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert_eq!(out.len(), 6);
        for (o, x) in out.iter().zip(src.iter()) {
            assert_eq!(o, x, "5.1 → 5.1 must be bit-exact");
        }
    }

    #[test]
    fn surround_to_stereo_folds_down_itu() {
        let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Stereo, 4800);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert_eq!(out.len(), 2);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let x = |c: usize| 0.1 * (c + 1) as f32 + (1000 % 7) as f32 * 1e-3;
        // L R C LFE Ls Rs.
        let l = x(0) + h * x(2) + h * x(4);
        let r = x(1) + h * x(2) + h * x(5);
        assert!((out[0][1000] - l).abs() < 1e-5, "{} vs {l}", out[0][1000]);
        assert!((out[1][1000] - r).abs() < 1e-5, "{} vs {r}", out[1][1000]);
    }

    #[test]
    fn mono_track_on_surround_main_pans() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.outputs[0].format = ChannelFormat::Surround51;
        // No surround pan: stereo behaviour on L/R.
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert_eq!(out.len(), 6);
        let h = 0.5 * gains(0.0, PanLaw::Minus3).0;
        assert!((out[0][100] - h).abs() < 1e-6 && (out[1][100] - h).abs() < 1e-6);
        assert!(out[2..].iter().all(|c| c[100].abs() < 1e-7));
        // Front centre: the centre speaker only.
        s.track_mut(t).unwrap().mixer.surround = Some(SurroundPan::default());
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[2][100] - 0.5).abs() < 1e-4, "{}", out[2][100]);
        assert!(out.iter().enumerate().all(|(c, x)| c == 2 || x[100].abs() < 1e-4));
        // Rear right with LFE: Rs plus the LFE send.
        s.track_mut(t).unwrap().mixer.surround = Some(SurroundPan { x: 1.0, y: -1.0, lfe_db: -6.0, ..SurroundPan::default() });
        let mut rig = PlanRig::new(512);
        rig.publish(&s);
        let buf = rig.render(0);
        assert_eq!(buf.len(), 6);
        assert!((buf[5][100] - 0.5).abs() < 1e-4 && (buf[3][100] - 0.5 * db_to_gain(-6.0)).abs() < 1e-4);
        assert_eq!(rig.eng.active_main_meter().peaks.len(), 6);
        assert_eq!(rig.eng.meters().find(|(id, _)| *id == t).map(|(_, m)| m.peaks.len()), Some(1));
        // A stereo device buffer gets the first two channels.
        let mut two = vec![vec![1.0; 512]; 2];
        rig.eng.render_active(512, 512, &mut two);
        assert!(two[0][10].abs() < 1e-6);
    }

    #[test]
    fn surround_busses_and_master_fader() {
        let (mut s, t) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround51, 4800);
        let bus = s.add_bus("Stem", ChannelFormat::Surround51);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Surround51, Some("Stem"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(t).unwrap().mixer.output = Route::Bus(bus);
        let m = s.add_track(TrackKind::Master, ChannelFormat::Surround51, None);
        s.track_mut(m).unwrap().mixer.volume_db = -6.0;
        let out = render_range(&s, Range::new(0, 4800), 512);
        let g = db_to_gain(-6.0);
        for (c, ch) in out.iter().enumerate() {
            let x = 0.1 * (c + 1) as f32 + (1000 % 7) as f32 * 1e-3;
            assert!((ch[1000] - x * g).abs() < 1e-5, "channel {c}: {} vs {}", ch[1000], x * g);
        }
        // A mono bus sums a stereo source like the old stereo bus did.
        let (mut s, t) = session_with_dc(0.5, 4800);
        let bus = s.add_bus("Mono", ChannelFormat::Mono);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, Some("M"));
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        s.track_mut(aux).unwrap().mixer.pan = vec![-1.0];
        s.track_mut(t).unwrap().mixer.output = Route::Bus(bus);
        let out = render_range(&s, Range::new(0, 4800), 512);
        assert!((out[0][1000] - 0.5 * gains(0.0, PanLaw::Minus3).0).abs() < 1e-6, "{}", out[0][1000]);
    }

    #[test]
    fn surround_upmix_and_downmix_between_layouts() {
        // 5.1 → 7.1: shared speakers map 1:1; 5.x surrounds pan between the 7.1 sides and rears.
        let (s, _) = multichannel_session(ChannelFormat::Surround51, ChannelFormat::Surround71, 2000);
        let out = render_range(&s, Range::new(0, 2000), 512);
        assert_eq!(out.len(), 8);
        let x = |c: usize| 0.1 * (c + 1) as f32 + (100 % 7) as f32 * 1e-3;
        for (c, ch) in out.iter().enumerate().take(4) {
            assert!((ch[100] - x(c)).abs() < 1e-6);
        }
        let e_ls = out[4][100].powi(2) + out[6][100].powi(2);
        assert!((e_ls - x(4).powi(2)).abs() < 1e-5, "Ls energy is preserved");
    }
}
