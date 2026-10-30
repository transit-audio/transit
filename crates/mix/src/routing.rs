use super::*;

/// Where [`route_into`] sends a strip: the busses, the main mix and its format, and the pan law.
pub(crate) struct MixOut<'a> {
    pub(crate) busses: &'a mut BusTable,
    pub(crate) main: &'a mut [Vec<f32>],
    pub(crate) main_fmt: ChannelFormat,
    /// The output paths the plan renders into.
    pub(crate) sinks: &'a mut [Sink],
    pub(crate) law: PanLaw,
    /// The slowest path into the main mix this block ([`pdc_visit`]).
    pub(crate) main_arrival: usize,
}

pub(crate) struct PdcWalk<'a> {
    pub(crate) busses: &'a mut BusTable,
    pub(crate) sinks: &'a mut [Sink],
    pub(crate) main_arrival: &'a mut usize,
    pub(crate) pdc: bool,
}

/// Delay compensation, forward pass (F05): visits one live strip at render position `at`, after
/// every strip before it. The strip's input arrives with the latency of the bus it reads; its
/// output adds its own latency (when `pdc`); every bus and the main mix it feeds then waits for
/// the slowest of its sources. A strip writing a bus already read this block (a routing loop's
/// feedback, from an older session) does not count: [`route_into`] drops that edge. An inactive
/// aux (`live` false) still marks the bus it reads, so the edges dropped are exactly the route
/// graph's [`RouteScratch::dropped`] (render order is the graph's, inactive tracks included).
pub(crate) fn pdc_visit(t: &Track, st: &mut Strip, at: usize, live: bool, walk: PdcWalk<'_>) {
    let PdcWalk { busses, sinks, main_arrival, pdc } = walk;
    let mut arrive = 0usize;
    if is_aux(t)
        && let Route::Bus(b) = &t.mixer.input
        && let Some(x) = busses.bus_mut(*b)
    {
        x.reader = x.reader.min(at);
        arrive = x.arrival;
    }
    if !live {
        return;
    }
    let out = arrive.saturating_add(if pdc { st.latency } else { 0 });
    st.pdc_out = out;
    st.pdc_pos = at;
    let mut feed = |edge: usize, r: &Route| match r {
        Route::Main => *main_arrival = (*main_arrival).max(out),
        Route::Bus(b) => {
            if let Some(x) = busses.bus_mut(*b).filter(|x| at < x.reader) {
                x.arrival = x.arrival.max(out);
            }
        }
        // An output path is a sink with no readers: it waits for its slowest source too.
        Route::Hardware(_) => {
            if let Some(x) = st.sink_routes.get(edge).copied().flatten().and_then(|k| sinks.get_mut(k)) {
                x.arrival = x.arrival.max(out);
            }
        }
        _ => {}
    };
    feed(0, &t.mixer.output);
    for (i, snd) in t.mixer.sends.iter().enumerate() {
        let Some(snd) = snd else { continue };
        if matches!(snd.target, Route::Bus(_) | Route::Hardware(_)) {
            feed(i + 1, &snd.target);
        }
    }
}

/// Send and pan a processed (and metered) strip into the busses / main mix.
///
/// Each edge (the output, each send) is delayed by the latency of its summing point minus the
/// strip's own ([`pdc_visit`]), read from the strip's delay lines. An edge into a bus already read
/// this block is a routing loop's feedback and is dropped. Returns the longest delay a plan-mode
/// line had to clamp (0: none).
pub(crate) fn route_into(t: &Track, strip: &mut Strip, pos: Samples, frames: usize, out: MixOut<'_>) -> usize {
    let MixOut { busses, main, main_fmt, sinks, law, main_arrival } = out;
    // Hardware inserts: what the strip sent goes into its send path's sink, where the device plays it.
    for h in &strip.hw {
        h.flush(sinks, frames);
    }
    let Strip { buf, pre, tap, tap_pre, outbuf, muted, pdc_out, pdc_pos, edges, sink_routes, touch, .. } = strip;
    let touch = *touch;
    let (o, at, muted) = (*pdc_out, *pdc_pos, *muted);
    // The delay of an edge into `r`, `None` when it is dropped (a loop's feedback, or nowhere).
    let delay_of = |busses: &BusTable, sinks: &[Sink], edge: usize, r: &Route| match r {
        Route::Main => Some(main_arrival.saturating_sub(o)),
        Route::Bus(b) => busses.bus(*b).filter(|x| at < x.reader).map(|x| x.arrival.saturating_sub(o)),
        Route::Hardware(_) => sink_routes.get(edge).copied().flatten().and_then(|k| sinks.get(k)).map(|x| x.arrival.saturating_sub(o)),
        _ => None,
    };
    // Edge 0 is the output, edge 1 + i send i. Each line is written when any edge asks it for a
    // delay, or still reads (or crossfades from) one.
    let held = |k: usize| edges.get(k).map_or(0, EdgeTap::holds);
    let (mut post, mut pre_max, mut held_post, mut held_pre) = (0usize, 0usize, held(0), 0usize);
    for (i, snd) in t.mixer.sends.iter().enumerate() {
        let Some(snd) = snd else { continue };
        if let (Route::Bus(_) | Route::Hardware(_), Some(d)) = (&snd.target, delay_of(busses, sinks, i + 1, &snd.target)) {
            if snd.pre_fader {
                pre_max = pre_max.max(d);
                held_pre = held_pre.max(held(i + 1));
            } else {
                post = post.max(d);
                held_post = held_post.max(held(i + 1));
            }
        }
    }
    if let Some(d) = delay_of(busses, sinks, 0, &t.mixer.output) {
        post = post.max(d);
    }
    let clamped = tap.fit(post).max(tap_pre.fit(pre_max));
    if post.max(held_post) > 0 {
        tap.write(buf, frames);
    } else {
        tap.idle();
    }
    if pre_max.max(held_pre) > 0 {
        tap_pre.write(pre, frames);
    } else {
        tap_pre.idle();
    }
    // This block's read of edge `k` at delay `d`: `None` for the undelayed signal itself.
    let mut plan = |k: usize, d: usize, pre_fader: bool| {
        let line: &DelayLine = if pre_fader { tap_pre } else { tap };
        let mut spare = EdgeTap::default();
        let e = edges.get_mut(k).unwrap_or(&mut spare);
        let (from, to, done) = e.step(d, pre_fader, line, frames);
        (from != 0 || to != 0).then_some((from, to, done))
    };
    let src_fmt = strip_format(t, buf.len());
    for (i, snd) in t.mixer.sends.iter().enumerate() {
        let Some(snd) = snd else { continue };
        if !matches!(snd.target, Route::Bus(_) | Route::Hardware(_)) {
            continue;
        }
        let Some(d) = delay_of(busses, sinks, i + 1, &snd.target) else { continue };
        let read = plan(i + 1, d, snd.pre_fader);
        if snd.mute || muted {
            continue;
        }
        let lvl = send_level(t, &touch, i, snd.level_db, pos);
        let gain = db_to_gain(lvl);
        if gain <= 0.0 {
            continue;
        }
        let src: &[Vec<f32>] = match (read, snd.pre_fader) {
            (None, true) => pre,
            (None, false) => buf,
            (Some((from, to, done)), pre_fader) => {
                if pre_fader {
                    tap_pre.read_x(from, to, done, outbuf, frames);
                } else {
                    tap.read_x(from, to, done, outbuf, frames);
                }
                outbuf
            }
        };
        let (dst_buf, dst_fmt): (&mut Vec<Vec<f32>>, ChannelFormat) = match &snd.target {
            Route::Bus(b) => match busses.bus_mut(*b) {
                Some(d) => (&mut d.buf, d.fmt),
                None => continue,
            },
            Route::Hardware(_) => match sink_routes.get(i + 1).copied().flatten().and_then(|k| sinks.get_mut(k)) {
                Some(d) => (&mut d.buf, d.fmt),
                None => continue,
            },
            _ => continue,
        };
        // Mono sends use the send's own pan; stereo sends go straight to L/R.
        let stereo = if src.len() == 1 { [snd.pan, 0.0] } else { [-1.0, 1.0] };
        let surround = snd.surround.as_ref().or(if snd.follow_main_pan { t.mixer.surround.as_ref() } else { None });
        let m = build_matrix(src_fmt, src.len(), dst_fmt, dst_buf.len(), &PanSpec { stereo, surround }, law);
        apply_matrix(src, frames, dst_buf, &m, gain);
    }
    let Some(d) = delay_of(busses, sinks, 0, &t.mixer.output) else { return clamped };
    let src: &[Vec<f32>] = match plan(0, d, false) {
        None => buf,
        Some((from, to, done)) => {
            tap.read_x(from, to, done, outbuf, frames);
            outbuf
        }
    };
    let (dst, dst_fmt): (&mut [Vec<f32>], ChannelFormat) = match &t.mixer.output {
        Route::Main => (main, main_fmt),
        Route::Bus(b) => match busses.bus_mut(*b) {
            Some(d) => (&mut d.buf, d.fmt),
            None => return clamped,
        },
        Route::Hardware(_) => match sink_routes.first().copied().flatten().and_then(|k| sinks.get_mut(k)) {
            Some(d) => (&mut d.buf, d.fmt),
            None => return clamped,
        },
        _ => return clamped,
    };
    let mut start = 0;
    while start < frames {
        let (at, end) = pan_segment(t, pos, start, frames);
        let spec = PanSpec { stereo: [pan_at(t, &touch, 0, at, start), pan_at(t, &touch, 1, at, start)], surround: t.mixer.surround.as_ref() };
        let m = build_matrix(src_fmt, src.len(), dst_fmt, dst.len(), &spec, law);
        apply_matrix_span(src, start..end, dst, &m, 1.0);
        start = end;
    }
    clamped
}

pub(crate) fn strip_channels(t: &Track, main_ch: usize) -> usize {
    match t.kind {
        TrackKind::Master => main_ch,
        TrackKind::Instrument => t.channels().clamp(2, MAX_CHANNELS),
        _ => t.channels().clamp(1, MAX_CHANNELS),
    }
}

/// The format a strip's buffer actually carries (a mono instrument renders in stereo).
pub(crate) fn strip_format(t: &Track, n: usize) -> ChannelFormat {
    if t.format.channels().min(MAX_CHANNELS) == n { t.format } else { ChannelFormat::for_channels(n) }
}

/// Whether this kind reads a bus in the second render phase.
pub(crate) fn is_aux(t: &Track) -> bool {
    match t.kind {
        TrackKind::Aux | TrackKind::Folder => true,
        TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi | TrackKind::Master | TrackKind::Vca => false,
    }
}

/// Whether a strip participates in routing this block (masters process Main separately).
pub(crate) fn live(t: &Track) -> bool {
    !t.inactive
        && match t.kind {
            TrackKind::Master | TrackKind::Vca => false,
            TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi | TrackKind::Aux | TrackKind::Folder => true,
        }
}

/// Writes the processing order into `order`: every track other than an aux or folder in track
/// order, then the auxes and folders in routing order (each after every strip feeding it, at any
/// depth: by the depth of the session's route graph, then track order). Returns the loops the
/// graph had to break (a session from an older file): the render drops their feedback edges.
///
/// `route` is the session's route graph storage: when it was reserved for `s` (and `order` holds
/// the track count), this allocates nothing. Loops a session was loaded with are broken by the
/// graph; the render drops exactly `route.dropped()`. Nothing is logged here (it can run on the
/// audio thread): the engine logs loaded loops when it opens a session, the plan compiler when it
/// compiles one.
pub(crate) fn processing_order(s: &Session, route: &mut RouteScratch, order: &mut Vec<usize>) {
    route.layer(s);
    order.clear();
    order.extend(s.tracks.iter().enumerate().filter(|(_, t)| !is_aux(t)).map(|(i, _)| i));
    // Phase 1 renders in layers: a track keyed from another renders after it (stable: track order
    // within a layer, and no keys means one layer in track order, as before).
    order.sort_by_key(|&i| route.depth(i));
    order.extend(route.order().iter().copied().filter(|&i| s.tracks.get(i).is_some_and(is_aux)));
}

/// Logs the routing loops of a session (control side only).
pub(crate) fn log_loops(s: &Session, loops: &[RouteCycle]) {
    for l in loops {
        log::warn!("mix: routing loop {} in the session; rendering without its feedback edge", l.describe(s));
    }
}

/// The longest delay compensation can ask of any edge of `s`: the slowest path into any bus or
/// the main mix, with `latency` giving each track's own (instrument and inserts). Control thread
/// (it allocates): plans size their delay lines with it.
pub(crate) fn path_need(s: &Session, latency: &dyn Fn(&Track) -> usize) -> usize {
    let mut route = RouteScratch::new();
    route.reserve(s);
    route.layer(s);
    // Same rule as the render: tracks in the graph's order, a write into a bus already read does
    // not count. The output latency of each track follows the busses' entries.
    let mut bus = vec![(0usize, false); s.busses.len() + s.tracks.len()];
    let nbus = s.busses.len();
    let mut need = 0usize;
    // Every output path is padded to the one device latency Main has, master inserts included
    // (ADR 0015 section 6), so a path's delay can exceed any source's own by that much.
    let mut to_sink = false;
    for &i in route.order() {
        let Some(t) = s.tracks.get(i).filter(|t| !matches!(t.kind, TrackKind::Master | TrackKind::Vca)) else { continue };
        let mut arrive = 0;
        if is_aux(t)
            && let Route::Bus(b) = &t.mixer.input
            && let Some(x) = route.bus_slot(*b).and_then(|k| bus.get_mut(k))
        {
            x.1 = true;
            arrive = x.0;
        }
        // A key that arrives later than the strip's own signal holds it back (`KeyBank::align`):
        // at worst the strip's input waits for the slowest of its keys, whatever sits before the
        // keyed insert.
        for e in route.edges() {
            let (transit_model::EdgeKind::Key { .. }, transit_model::RouteNode::Track(to)) = (e.kind, e.to) else { continue };
            if to != i {
                continue;
            }
            let from = match e.from {
                transit_model::RouteNode::Track(j) => bus.get(nbus + j).map_or(0, |x| x.0),
                transit_model::RouteNode::Bus(b) => bus.get(b).map_or(0, |x| x.0),
                transit_model::RouteNode::Main => 0,
            };
            arrive = arrive.max(from);
        }
        let out = arrive.saturating_add(latency(t));
        need = need.max(out);
        if let Some(x) = bus.get_mut(nbus + i) {
            x.0 = out;
        }
        let targets = std::iter::once(&t.mixer.output).chain(t.mixer.sends.iter().flatten().map(|x| &x.target));
        for r in targets {
            if let Route::Bus(b) = r
                && let Some(x) = route.bus_slot(*b).and_then(|k| bus.get_mut(k))
                && !x.1
            {
                x.0 = x.0.max(out);
            }
            to_sink |= matches!(r, Route::Hardware(_));
        }
    }
    if to_sink {
        let masters = s.tracks.iter().filter(|t| t.kind == TrackKind::Master && !t.inactive);
        need = need.saturating_add(masters.fold(0usize, |a, t| a.saturating_add(latency(t))));
    }
    need.min(MAX_LATENCY)
}

/// Implicit solo: an aux is not muted when a soloed track feeds its input bus.
pub(crate) fn receives_solo(s: &Session, t: &Track) -> bool {
    let Route::Bus(b) = &t.mixer.input else { return false };
    s.tracks
        .iter()
        .any(|o| o.mixer.solo && (o.mixer.output == Route::Bus(*b) || o.mixer.sends.iter().flatten().any(|sn| sn.target == Route::Bus(*b))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{PlanRig, render_block, session_with_dc};
    use transit_audio_io::AudioBuffer;
    use transit_model::{Insert, SourceId};

    #[test]
    fn delay_compensation_aligns_latent_tracks() {
        let mut s = Session::default();
        let mut imp = vec![0.0f32; 48_000];
        imp[1000] = 1.0;
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![imp] })));
        let mut ids = Vec::new();
        for (pan, fx) in [(-1.0f32, true), (1.0, false)] {
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
            let cid = s.new_clip_id();
            let tr = s.track_mut(t).unwrap();
            tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "i", SourceId(900), 0, 0, 48_000));
            tr.mixer.pan = vec![pan];
            if fx {
                tr.mixer.inserts[0] = Some(Insert::new("maximizer"));
            }
            ids.push(t);
        }
        let argmax = |c: &[f32]| c.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).unwrap();
        let lat = transit_dsp::create("maximizer").unwrap().latency();
        assert!(lat > 0, "test needs a latent plugin");
        let out = render_range(&s, Range::new(0, 8000), 512);
        assert_eq!(argmax(&out[0]), argmax(&out[1]), "PDC should align L and R");
        s.edit.delay_compensation = false;
        let out = render_range(&s, Range::new(0, 8000), 512);
        assert_ne!(argmax(&out[0]), argmax(&out[1]));
    }

    /// A reset (stop, locate) must not leave the previous position's audio in the alignment delay.
    #[test]
    fn reset_clears_delay_compensation_history() {
        let (mut s, t) = session_with_dc(0.5, 4800);
        s.edit.click = false;
        s.track_mut(t).unwrap().mixer.pan = vec![1.0];
        let latent = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut comp = Insert::new("compressor");
        comp.params.insert("lookahead".into(), 5.0);
        s.track_mut(latent).unwrap().mixer.inserts[0] = Some(comp);
        let mut rig = PlanRig::new(512);
        rig.publish(&s);
        let mut out = Vec::new();
        for b in 0..9 {
            out = rig.render(b * 512);
        }
        assert!(rig.eng.latency > 0 && out[1][500] > 0.3, "the fixture plays through the alignment delay");
        rig.eng.reset();
        let out = rig.render(1_000_000);
        assert!(out.iter().flatten().all(|x| *x == 0.0), "old aligned audio after a reset");
    }

    fn impulse_at(ch: &[f32]) -> Vec<usize> {
        ch.iter().enumerate().filter(|(_, v)| v.abs() > 1e-4).map(|(i, _)| i).collect()
    }

    /// An aux insert's lookahead is part of the mix latency on the first block, not the block after.
    #[test]
    fn delay_compensation_aligns_an_aux_insert() {
        let mut s = Session::default();
        let n = 8192;
        let mut left = vec![0.0f32; n];
        for at in [0usize, 2048, 8000] {
            left[at] = 0.25;
        }
        s.pool.insert(SourceId(901), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![left.clone(), left] })));
        let bus = s.add_bus("Test Bus", ChannelFormat::Stereo);
        let source = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Source"));
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Return"));
        let cid = s.new_clip_id();
        s.track_mut(source).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, "impulses", SourceId(901), 0, 0, n as i64));
        s.track_mut(source).unwrap().mixer.output = Route::Bus(bus);
        s.track_mut(aux).unwrap().mixer.input = Route::Bus(bus);
        let render = |s: &Session, start: i64, end: i64| render_range(s, Range::new(start, end), 1024);
        let control = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&control[0]), vec![0, 2048, 8000]);
        assert_eq!(impulse_at(&control[1]), impulse_at(&control[0]));

        let mut comp = Insert::new("compressor");
        comp.params.insert("lookahead".into(), 5.0);
        comp.params.insert("ratio".into(), 1.0);
        s.track_mut(source).unwrap().mixer.inserts[0] = Some(comp.clone());
        let on_source = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&on_source[0]), impulse_at(&control[0]));

        s.track_mut(source).unwrap().mixer.inserts[0] = None;
        s.track_mut(aux).unwrap().mixer.inserts[0] = Some(comp);
        let on_aux = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&on_aux[0]), vec![0, 2048, 8000], "aux lookahead must stay inside the bounce");
        assert_eq!(impulse_at(&on_aux[1]), impulse_at(&on_aux[0]));
        assert!((on_aux[0][8000] - 0.25).abs() < 1e-3, "{}", on_aux[0][8000]);
        let partial = render(&s, 2048, 3072);
        assert_eq!(impulse_at(&partial[0]), vec![0]);

        s.track_mut(aux).unwrap().mixer.inserts[0].as_mut().unwrap().params.insert("lookahead".into(), 0.0);
        let zero = render(&s, 0, n as i64);
        assert_eq!(impulse_at(&zero[0]), impulse_at(&control[0]));
    }

    #[test]
    fn a_plugins_bogus_latency_is_capped() {
        struct Latent(Box<dyn Plugin>);
        impl Plugin for Latent {
            fn info(&self) -> &'static transit_dsp::PluginInfo {
                self.0.info()
            }
            fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
                self.0.prepare(sr, mb, ch);
            }
            fn reset(&mut self) {
                self.0.reset();
            }
            fn set_param(&mut self, id: &str, v: f32) -> bool {
                self.0.set_param(id, v)
            }
            fn param(&self, id: &str) -> Option<f32> {
                self.0.param(id)
            }
            fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
                self.0.process(io, frames);
            }
            fn latency(&self) -> usize {
                u32::MAX as usize
            }
        }
        let (mut s, t) = session_with_dc(0.5, 48_000);
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Mono, None);
        let mut rig = PlanRig::new(512);
        for track in [t, aux] {
            s.track_mut(track).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.latent"));
            let mut gain = transit_dsp::create("gain").unwrap();
            gain.prepare(48_000.0, 512, 1);
            rig.fakes.push((track, 0, Box::new(Latent(gain))));
        }
        rig.publish(&s);
        render_block(&mut rig, 0);
        // Both strips feed the main mix in parallel: the slowest path is one capped latency.
        assert_eq!(rig.eng.latency, MAX_LATENCY);
    }
}
