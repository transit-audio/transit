use super::*;

/// Ceiling on one in-memory offline render (samples summed over channels): 2^29 f32 = 2 GiB, about
/// 93 minutes of stereo at 48 kHz. Longer requests are truncated rather than exhausting memory;
/// [`RangeRender`] streams any length.
pub const MAX_RENDER_SAMPLES: usize = 1 << 29;

/// Offline render of the main mix over `range` (planar, [`main_channels`] channels in SMPTE/WAV
/// order). The mix's delay-compensation latency is removed, so the result lines up with the
/// timeline.
pub fn render_range(s: &Session, range: Range, block: usize) -> Vec<Vec<f32>> {
    render_in_memory(s, None, range, block)
}

/// Offline render of one bus over `range` (planar, the bus format's channel count).
/// The compensation delay written onto the bus is removed (track inserts, and the aux returns
/// when an aux returns onto it), so the bus lines up with the timeline. An unknown bus is silent.
pub fn render_bus(s: &Session, bus: BusId, range: Range, block: usize) -> Vec<Vec<f32>> {
    render_in_memory(s, Some(bus), range, block)
}

fn render_in_memory(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> Vec<Vec<f32>> {
    render_in_memory_timed(s, bus, range, block).0
}

/// [`render_in_memory`] and the mix's latency in samples that the render removed.
fn render_in_memory_timed(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> (Vec<Vec<f32>>, usize) {
    let nch = source_channels(s, bus);
    let len = usize::try_from(range.len().max(0)).unwrap_or(0).min(MAX_RENDER_SAMPLES / nch.max(1));
    let mut out: Vec<Vec<f32>> = (0..nch).map(|_| Vec::with_capacity(len)).collect();
    let mut render = RangeRender::new(s, bus, Range::new(range.start, range.start + len as i64), block);
    while let Some((chunk, n)) = render.next_block() {
        for (o, c) in out.iter_mut().zip(chunk) {
            o.extend_from_slice(c.get(..n).unwrap_or_default());
        }
    }
    (out, render.latency)
}

/// Channels of the main mix, or of `bus` (none for an unknown bus).
pub fn source_channels(s: &Session, bus: Option<BusId>) -> usize {
    match bus {
        Some(id) => s.bus(id).map_or(0, |b| b.format.channels()),
        None => main_channels(s),
    }
}

/// [`render_range`] or [`render_bus`] of any length, a block at a time.
///
/// It renders through the plan renderer, the one the live player uses: the session is compiled
/// once by a [`PlanCompiler`] for a headless [`RenderTarget`] (device generation 0), adopted by a
/// [`MixEngine::realtime`] engine and driven with [`MixEngine::render_active`], so tempo and meter
/// maps, automation, delay compensation and plug-in state behave exactly as they do live. The
/// engine works on a snapshot of the session taken in [`RangeRender::try_new`].
pub struct RangeRender {
    pub(crate) eng: MixEngine,
    bus: Option<BusId>,
    tmp: Vec<Vec<f32>>,
    start: Samples,
    done: usize,
    total: usize,
    latency: usize,
    /// The output paths this render fills after Main's channels in `tmp`, in order (empty unless
    /// built by [`RangeRender::try_new_paths`]).
    sinks: Vec<SinkSpec>,
}

impl RangeRender {
    /// Renders the main mix over `range`, or `bus` when one is named. A session the plan compiler
    /// refuses (over its limits) renders nothing; use [`RangeRender::try_new`] to learn why.
    pub fn new(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> Self {
        Self::try_new(s, bus, range, block).unwrap_or_else(|err| {
            log::error!("offline render: the session could not be compiled: {err:?}");
            let target = Self::target(s, block);
            RangeRender {
                eng: MixEngine::realtime(target),
                bus,
                tmp: Vec::new(),
                start: range.start,
                done: 0,
                total: 0,
                latency: 0,
                sinks: Vec::new(),
            }
        })
    }

    /// The headless target an offline render compiles for: the session's rate, no device.
    fn target(s: &Session, block: usize) -> RenderTarget {
        Self::target_for(s, block, 0)
    }

    fn target_for(s: &Session, block: usize, out_channels: u16) -> RenderTarget {
        RenderTarget {
            device: DeviceGeneration(0),
            sample_rate: s.sample_rate.hz(),
            max_block: u32::try_from(block).unwrap_or(u32::MAX),
            out_channels,
            in_channels: 0,
        }
        .normalized()
    }

    /// Plan limits for an offline render: the live defaults, with track and bus counts raised to
    /// the largest the plan tables index (a bounce has no realtime budget to protect).
    fn limits() -> PlanLimits {
        PlanLimits { tracks: u16::MAX, buses: u16::MAX, ..PlanLimits::default() }
    }

    /// [`RangeRender::new`], or why the session could not be compiled into a plan.
    pub fn try_new(s: &Session, bus: Option<BusId>, range: Range, block: usize) -> Result<Self, CompileError> {
        Self::build(s, bus, range, block, None)
    }

    /// A render of Main and every output path of `io` that `s` routes to, through the one plan
    /// renderer, with a headless target sized to `io`'s opened outputs. [`RangeRender::next_block`]
    /// returns Main's channels, then each sink's in [`RangeRender::sinks`] order. Every sink is
    /// delayed to the one device latency Main has (ADR 0015 section 6), and the latency is skipped
    /// from all of them together, so the files line up with each other and with a plain main bounce.
    pub fn try_new_paths(s: &Session, range: Range, block: usize, io: &IoMap) -> Result<Self, CompileError> {
        Self::build(s, None, range, block, Some(io))
    }

    fn build(s: &Session, bus: Option<BusId>, range: Range, block: usize, io: Option<&IoMap>) -> Result<Self, CompileError> {
        let target = Self::target_for(s, block, io.map_or(0, |m| m.outputs));
        let mut eng = MixEngine::realtime(target);
        let mut compiler = PlanCompiler::new(target, Self::limits()).skipping_inactive_instances();
        let sinks = io.map(|m| plan_sinks(s, m, target.out_channels).sinks).unwrap_or_default();
        if let Some(m) = io {
            compiler.set_io(Arc::new(m.clone()));
        }
        let (sr, max_block) = (target.sample_rate as f32, target.block());
        let mut make = |spec: &InstanceSpec, _: SlotGen| create_instance(&spec.id, spec.state.as_deref(), sr, max_block, spec.ch);
        let next = compiler.active().checked_next().ok_or(CompileError::GenerationExhausted)?;
        let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), next, &mut make)?;
        let adoption = eng.adopt_plan(&mut env);
        compiler.reclaim(env);
        if adoption != Adoption::Adopted {
            log::error!("offline render: the engine refused its first plan: {adoption:?}");
        }
        let nch = source_channels(s, bus);
        let width = nch + sinks.iter().map(|k| k.dest.len()).sum::<usize>();
        let tmp = vec![vec![0.0f32; eng.max_block()]; width];
        let len = if nch == 0 || adoption != Adoption::Adopted { 0 } else { usize::try_from(range.len().max(0)).unwrap_or(0) };
        Ok(RangeRender { eng, bus, tmp, start: range.start, done: 0, total: len, latency: 0, sinks })
    }

    /// The output paths after Main's channels in each block, in order (see [`RangeRender::try_new_paths`]).
    pub fn sinks(&self) -> &[SinkSpec] {
        &self.sinks
    }

    fn render(&mut self, pos: Samples, n: usize) {
        match self.bus {
            Some(id) => {
                self.eng.render_active(pos, n, &mut []);
                self.eng.copy_bus_active(id, n, &mut self.tmp);
            }
            None => self.eng.render_active(pos, n, &mut self.tmp),
        }
    }

    /// Renders one more block of the mix and returns it as `(channels, n)`: the next `n` samples
    /// of the range lead each channel (none while the mix's latency is still being skipped).
    /// `None` once the range is done. The per-block work allocates nothing.
    pub fn next_block(&mut self) -> Option<(&[Vec<f32>], usize)> {
        if self.done >= self.total {
            return None;
        }
        let n = (self.total - self.done).min(self.eng.max_block());
        self.render(self.start + self.done as i64, n);
        if self.done == 0 {
            // The first block tells the mix's latency: render that much more and drop the head.
            self.latency = match self.bus {
                Some(b) => self.eng.bus_latency_active(b),
                None => self.eng.latency,
            };
            self.total = self.total.saturating_add(self.latency);
        }
        let skip = self.latency.saturating_sub(self.done).min(n);
        self.done += n;
        if skip > 0 && skip < n {
            for c in &mut self.tmp {
                c.copy_within(skip..n, 0);
            }
        }
        Some((&self.tmp, n - skip))
    }
}

/// Render one track through its inserts (pre-fader) — Commit / Freeze.
pub fn render_track_pre_fader(s: &Session, track: TrackId, range: Range) -> Vec<Vec<f32>> {
    render_track_pre_fader_timed(s, track, range).0
}

/// [`render_track_pre_fader`], and the latency in samples of the track's instrument and inserts
/// (what delay compensation holds the track back by; the render has it removed).
pub fn render_track_pre_fader_timed(s: &Session, track: TrackId, range: Range) -> (Vec<Vec<f32>>, usize) {
    let mut solo = s.clone();
    // Multichannel tracks render into a main mix of their own format (straight through).
    let fmt = s.track(track).map_or(ChannelFormat::Stereo, |t| if t.channels() > 2 { t.format } else { ChannelFormat::Stereo });
    solo.outputs = vec![transit_model::OutputPath { name: "Out".into(), first_channel: 0, format: fmt }];
    // The tracks and busses that feed the track a sidechain key keep running, so a keyed insert
    // detects as it does live; their audio goes nowhere else (outputs and other sends are cut).
    let (feeding_tracks, feeding_busses) = key_feeders(s, track);
    let feeds_bus = |r: &Route| matches!(r, Route::Bus(b) if s.busses.iter().position(|x| x.id == *b).is_some_and(|i| feeding_busses.get(i).copied().unwrap_or(false)));
    for (i, t) in solo.tracks.iter_mut().enumerate() {
        if t.id == track {
            t.mixer.volume_db = 0.0;
            t.mixer.mute = false;
            t.mixer.solo = false;
            t.mixer.output = Route::Main;
            t.mixer.pan = match t.channels() {
                1 => vec![0.0],
                _ => vec![-1.0, 1.0],
            };
            t.automation.retain(|l| matches!(l.param, AutoParam::Plugin { .. }));
            t.mixer.sends.iter_mut().for_each(|x| *x = None);
            t.mixer.surround = None;
        } else if feeding_tracks.get(i).copied().unwrap_or(false) {
            t.mixer.solo = false;
            if !feeds_bus(&t.mixer.output) {
                t.mixer.output = Route::None;
            }
            for send in &mut t.mixer.sends {
                if send.as_ref().is_some_and(|x| !feeds_bus(&x.target)) {
                    *send = None;
                }
            }
        } else {
            t.inactive = true;
        }
    }
    let (mut out, latency) = render_in_memory_timed(&solo, None, range, 1024);
    // Mono tracks: fold back to one channel (undo the centre pan law).
    if s.track(track).is_some_and(|t| t.channels() == 1)
        && let Some(l) = out.first().cloned()
    {
        let g = std::f32::consts::SQRT_2;
        out = vec![l.iter().map(|x| x * g).collect()];
    }
    (out, latency)
}

/// The tracks (by table index) and busses (by index in [`Session::busses`]) that must run for
/// `track` to hear its sidechain keys as it does live: the sources of its keyed inserts and,
/// transitively, whatever feeds those (a bus's writers, a keyed source's own keys). `track`
/// itself is not marked.
fn key_feeders(s: &Session, track: TrackId) -> (Vec<bool>, Vec<bool>) {
    use transit_model::{EdgeKind, RouteNode};
    let graph = s.route_graph();
    let mut tracks = vec![false; s.tracks.len()];
    let mut busses = vec![false; s.busses.len()];
    let Some(start) = s.tracks.iter().position(|t| t.id == track) else { return (tracks, busses) };
    let mut work = vec![RouteNode::Track(start)];
    while let Some(n) = work.pop() {
        for e in graph.edges().iter().filter(|e| e.to == n) {
            // The frozen track's own chain is rendered live; only its keys reach beyond it.
            if n == RouteNode::Track(start) && !matches!(e.kind, EdgeKind::Key { .. }) {
                continue;
            }
            let seen = match e.from {
                RouteNode::Track(i) if i != start => tracks.get_mut(i),
                RouteNode::Bus(i) => busses.get_mut(i),
                _ => None,
            };
            if let Some(seen) = seen
                && !std::mem::replace(seen, true)
            {
                work.push(e.from);
            }
        }
    }
    (tracks, busses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use transit_audio_io::AudioBuffer;
    use transit_model::{Insert, SourceId};

    /// What a keyed track needs running: its key sources and, through them, what feeds those.
    #[test]
    fn key_feeders_follow_keys_through_busses_and_keyed_sources() {
        use transit_model::{ChannelFormat, KeyFrom, KeySource, Tap, TrackKind};
        let mut s = Session::default();
        let add = |s: &mut Session, k, n: &str| s.add_track(k, ChannelFormat::Stereo, Some(n));
        let a = add(&mut s, TrackKind::Audio, "A");
        let b = add(&mut s, TrackKind::Audio, "B");
        let c = add(&mut s, TrackKind::Audio, "C");
        let unrelated = add(&mut s, TrackKind::Audio, "U");
        let bus = s.add_bus("Drums", ChannelFormat::Stereo);
        let target = add(&mut s, TrackKind::Aux, "Target");
        let key = |from| Some(KeySource { from, tap: Tap::Post });
        for (t, slot_key) in [(target, key(KeyFrom::Bus(bus))), (c, key(KeyFrom::Track(unrelated)))] {
            let tr = s.track_mut(t).unwrap();
            let mut i = Insert::new("compressor");
            i.key = slot_key;
            tr.mixer.inserts[0] = Some(i);
        }
        s.track_mut(a).unwrap().mixer.output = Route::Bus(bus);
        s.track_mut(b).unwrap().mixer.sends[0] = Some(transit_model::SendSlot::new(Route::Bus(bus)));
        let ix: Vec<usize> = [a, b, c, unrelated, target].iter().map(|&id| s.track_index(id).unwrap()).collect();
        let idx = |id| ix[[a, b, c, unrelated, target].iter().position(|&x| x == id).unwrap()];
        let (tracks, busses) = key_feeders(&s, target);
        assert!(tracks[idx(a)] && tracks[idx(b)], "both writers of the key bus run");
        assert!(!tracks[idx(c)] && !tracks[idx(unrelated)] && !tracks[idx(target)]);
        assert_eq!(busses, vec![true]);
        // A key from a track whose own insert is keyed keeps that source too.
        let tr = s.track_mut(target).unwrap();
        tr.mixer.inserts[1] = Some({
            let mut i = Insert::new("compressor");
            i.key = key(KeyFrom::Track(c));
            i
        });
        let (tracks, _) = key_feeders(&s, target);
        assert!(tracks[idx(c)] && tracks[idx(unrelated)]);
        assert!(key_feeders(&s, unrelated).0.iter().all(|x| !x), "no keys: nothing else runs");
    }

    #[test]
    fn offline_renders_line_up_with_the_timeline_in_any_block_size() {
        let mut s = Session::default();
        let mut imp = vec![0.0f32; 48_000];
        imp[1000] = 1.0;
        s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![imp] })));
        // A mono strip carrying an instrument (the one-channel buffer is what takes the scratch path).
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let bus = s.add_bus("Stem", ChannelFormat::Stereo);
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "i", SourceId(900), 0, 0, 48_000));
        tr.mixer.inserts[0] = Some(Insert::new("maximizer"));
        let mut send = transit_model::SendSlot::new(Route::Bus(bus));
        send.level_db = 0.0;
        tr.mixer.sends[0] = Some(send);
        let latency = transit_dsp::create("maximizer").unwrap().latency();
        assert!(latency > 16);
        for source in [None, Some(bus)] {
            for block in [16, 100, 512] {
                let mut got = Vec::new();
                let mut skipped = 0;
                let mut render = RangeRender::new(&s, source, Range::new(900, 2900), block);
                while let Some((chunk, n)) = render.next_block() {
                    assert!(n <= block);
                    skipped += usize::from(n == 0);
                    got.extend_from_slice(&chunk[0][..n]);
                }
                assert_eq!(got.len(), 2000, "{source:?} block {block}");
                assert_eq!(skipped, latency / block, "{source:?} block {block}");
                let peak = got.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i);
                assert_eq!(peak, Some(100), "{source:?} block {block}");
            }
        }
    }
}
