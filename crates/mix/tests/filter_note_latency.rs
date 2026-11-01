//! #247: deterministic impulses, note timing and allocation through the actual strip renderer.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::Relaxed},
};
use transit_dsp::{
    Plugin,
    event::{Message, PluginEvent},
};
use transit_midi::{Note, Sequence};
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, SlotGen,
};
use transit_model::{ChannelFormat, Clip, Insert, Session, TrackKind};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;
const NONE: usize = usize::MAX;

struct Trace {
    input: AtomicUsize,
    output: AtomicUsize,
    on: AtomicUsize,
    off: AtomicUsize,
    ons: AtomicUsize,
    offs: AtomicUsize,
}

impl Trace {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            input: AtomicUsize::new(NONE),
            output: AtomicUsize::new(NONE),
            on: AtomicUsize::new(NONE),
            off: AtomicUsize::new(NONE),
            ons: AtomicUsize::new(0),
            offs: AtomicUsize::new(0),
        })
    }
}

/// A settable audio delay; as an instrument its input is one unit impulse per accepted note.
/// Reading the ring at the current delay also models a lookahead change on the next block.
struct Latent {
    latency: Arc<AtomicUsize>,
    instrument: bool,
    clock: usize,
    ring: Vec<f32>,
    impulses: Vec<usize>,
}

impl Plugin for Latent {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.impulses.clear();
    }
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn latency(&self) -> usize {
        self.latency.load(Relaxed)
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        if matches!(ev.message, Message::NoteOn { .. }) {
            assert!(self.impulses.len() < self.impulses.capacity());
            self.impulses.push(self.clock + offset as usize);
        }
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let delay = self.latency();
        for i in 0..frames {
            let at = self.clock + i;
            let x = if self.instrument { f32::from(self.impulses.contains(&at)) } else { io[0][i] };
            let n = self.ring.len();
            self.ring[at % n] = x;
            let y = self.ring[(at + n - delay) % n];
            for c in io.iter_mut() {
                c[i] = y;
            }
        }
        self.clock += frames;
        self.impulses.clear();
    }
}

/// A real Filter, observing the first input/output impulse and each delivered note.
struct Reader {
    filter: Box<dyn Plugin>,
    trace: Arc<Trace>,
    clock: usize,
}

impl Plugin for Reader {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.filter.info()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        self.filter.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.filter.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.filter.param(id)
    }
    fn reads_track_notes(&self) -> bool {
        true
    }
    fn all_notes_off(&mut self) {
        self.filter.all_notes_off();
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        let at = self.clock + offset as usize;
        match ev.message {
            Message::NoteOn { .. } => {
                self.trace.on.fetch_min(at, Relaxed);
                self.trace.ons.fetch_add(1, Relaxed);
            }
            Message::NoteOff { .. } => {
                self.trace.off.fetch_min(at, Relaxed);
                self.trace.offs.fetch_add(1, Relaxed);
            }
            _ => {}
        }
        self.filter.event(offset, ev)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if let Some(i) = io[0][..frames].iter().position(|x| *x != 0.0) {
            self.trace.input.fetch_min(self.clock + i, Relaxed);
        }
        self.filter.process(io, frames);
        if let Some(i) = io[0][..frames].iter().position(|x| *x != 0.0) {
            self.trace.output.fetch_min(self.clock + i, Relaxed);
        }
        self.clock += frames;
    }
}

fn session(two: bool) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("clap:test.impulse"));
    tr.mixer.inserts[0] = Some(Insert::new("clap:test.reader0"));
    if two {
        tr.mixer.inserts[1] = Some(Insert::new("clap:test.delay"));
        tr.mixer.inserts[2] = Some(Insert::new("clap:test.reader1"));
    }
    let mut seq = Sequence::default();
    // Note at frame 25, off at frame 50. The short release finishes well before a latent impulse
    // arrives on the unfixed renderer, so a first-nonzero assertion detects the original defect.
    seq.notes.push(Note { pitch: 60, velocity: 127, release_velocity: 0, channel: 0, start: 1, length: 1 });
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "impulse", 0, 8192, seq));
    s
}

struct Rig {
    mix: MixEngine,
    _compiler: PlanCompiler,
    out: Vec<Vec<f32>>,
    clock: usize,
    traces: [Arc<Trace>; 2],
    latency: Arc<AtomicUsize>,
}

impl Rig {
    fn new(block: usize, delay: usize, middle: Option<usize>) -> Self {
        MixEngine::force_serial(true);
        let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: block as u32, in_channels: 0, out_channels: 0 };
        let mut mix = MixEngine::realtime(target);
        let mut compiler = PlanCompiler::new(target, PlanLimits::default());
        let traces = [Trace::new(), Trace::new()];
        let latency = Arc::new(AtomicUsize::new(delay));
        let mut make = |spec: &InstanceSpec, _: SlotGen| {
            let plugin: Box<dyn Plugin> = if spec.id.contains("reader") {
                let index = usize::from(spec.id.ends_with('1'));
                let mut filter = transit_dsp::create("filter").unwrap();
                for (k, v) in [("amp_on", 1.0), ("amp_attack", 0.5), ("amp_release", 1.0), ("cutoff", 20_000.0)] {
                    assert!(filter.set_param(k, v));
                }
                filter.prepare(48_000.0, block, 2);
                Box::new(Reader { filter, trace: traces[index].clone(), clock: 0 })
            } else {
                let instrument = spec.id.ends_with("impulse");
                Box::new(Latent {
                    latency: if instrument { latency.clone() } else { Arc::new(AtomicUsize::new(middle.unwrap())) },
                    instrument,
                    clock: 0,
                    ring: vec![0.0; 131_072],
                    impulses: Vec::with_capacity(512),
                })
            };
            Some(NewInstance { plugin, refused_state: false })
        };
        let mut env = compiler
            .compile(Arc::new(session(middle.is_some())), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make)
            .unwrap();
        assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
        compiler.reclaim(env);
        Self { mix, _compiler: compiler, out: vec![vec![0.0; block]; 2], clock: 0, traces, latency }
    }
    fn block(&mut self) {
        let n = self.out[0].len();
        self.mix.render_active(self.clock as i64, n, &mut self.out);
        self.clock += n;
    }
    fn finish(&mut self) {
        while self.clock < 8192 {
            self.block();
        }
    }
}

#[test]
fn latent_instrument_and_filter_start_on_the_same_frame() {
    for block in [64, 512] {
        for delay in [0, 17, 769, 2049] {
            let mut rig = Rig::new(block, delay, None);
            rig.finish();
            let t = &rig.traces[0];
            let actual = (t.input.load(Relaxed), t.on.load(Relaxed), t.output.load(Relaxed));
            println!("block {block}, latency {delay}: input/note/output {actual:?}");
            assert_eq!(actual, (25 + delay, 25 + delay, 25 + delay));
            assert_eq!(t.off.load(Relaxed), 50 + delay);
            assert_eq!((t.ons.load(Relaxed), t.offs.load(Relaxed)), (1, 1));
        }
    }
}

#[test]
fn two_filters_each_follow_the_latency_of_their_own_input() {
    for block in [64, 512] {
        let mut rig = Rig::new(block, 769, Some(1031));
        rig.finish();
        for (i, expected) in [794, 1825].into_iter().enumerate() {
            let t = &rig.traces[i];
            assert_eq!((t.input.load(Relaxed), t.on.load(Relaxed), t.output.load(Relaxed)), (expected, expected, expected));
        }
    }
}

#[test]
fn a_latency_change_retimes_pending_notes_at_the_next_block_without_loss() {
    for (initial, changed, expected) in [(769, 1537, 1562), (1537, 769, 794), (769, 0, 64)] {
        let mut rig = Rig::new(64, initial, None);
        rig.block();
        rig.latency.store(changed, Relaxed);
        rig.block();
        rig.finish();
        let t = &rig.traces[0];
        assert_eq!(t.on.load(Relaxed), expected);
        assert_eq!(t.off.load(Relaxed), (50 + changed).max(expected + 1));
        assert_eq!((t.ons.load(Relaxed), t.offs.load(Relaxed)), (1, 1));
        if changed > 0 {
            assert_eq!(t.input.load(Relaxed), expected);
        }
        assert_eq!(rig.mix.event_status().midi_fx.note_view_dropped, 0);
    }
}

#[test]
fn delayed_filter_notes_allocate_nothing_including_a_latency_change_and_reset() {
    transit_mix::mark_audio_thread();
    let mut rig = Rig::new(64, 769, Some(1031));
    let ((), stats) = measure(|| {
        rig.block();
        rig.latency.store(1537, Relaxed);
        rig.finish();
        rig.mix.reset();
        rig.block();
    });
    assert!(stats.is_zero(), "{stats:?}");
    assert_eq!(rig.traces[1].on.load(Relaxed), 2593);
}

fn builtin_session() -> Session {
    let mut s = session(false);
    let tr = &mut s.tracks[0];
    tr.instrument = Some(Insert::new("macro_oscillator"));
    let mut compressor = Insert::new("compressor");
    compressor.params.insert("lookahead".into(), 5.0);
    tr.mixer.inserts[0] = Some(compressor);
    let mut filter = Insert::new("filter");
    for (k, v) in [("amp_on", 1.0), ("amp_attack", 0.5), ("amp_release", 1.0)] {
        filter.params.insert(k.into(), v);
    }
    tr.mixer.inserts[1] = Some(filter);
    s
}

#[test]
fn offline_and_live_with_compressor_lookahead_have_identical_audio() {
    MixEngine::force_serial(true);
    let s = builtin_session();
    for block in [64, 512] {
        let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: block as u32, in_channels: 0, out_channels: 0 };
        let mut mix = MixEngine::realtime(target);
        let mut compiler = PlanCompiler::new(target, PlanLimits::default());
        let mut make = |spec: &InstanceSpec, _: SlotGen| transit_mix::create_instance(&spec.id, None, 48_000.0, block, spec.ch);
        let mut env = compiler.compile(Arc::new(s.clone()), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
        assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
        compiler.reclaim(env);
        let mut out = vec![vec![0.0; block]; 2];
        let mut live = [Vec::new(), Vec::new()];
        for pos in (0..8192 + 512).step_by(block) {
            mix.render_active(pos as i64, block, &mut out);
            for (dst, src) in live.iter_mut().zip(&out) {
                dst.extend_from_slice(src);
            }
        }
        let offline = transit_mix::render_range(&s, transit_time::Range::new(0, 8192), block);
        assert!(offline.iter().flatten().any(|v| v.abs() > 0.0001));
        for (a, b) in offline.iter().zip(&live) {
            assert_eq!(a, &b[240..240 + 8192], "offline strips the 240-sample PDC latency, block {block}");
        }
    }
}

#[test]
fn changing_the_real_compressor_lookahead_retimes_the_filters_pending_release() {
    MixEngine::force_serial(true);
    let mut s = builtin_session();
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: 64, in_channels: 0, out_channels: 0 };
    let mut mix = MixEngine::realtime(target);
    let mut compiler = PlanCompiler::new(target, PlanLimits::default());
    let trace = Trace::new();
    let mut make = |spec: &InstanceSpec, _: SlotGen| {
        let mut instance = transit_mix::create_instance(&spec.id, None, 48_000.0, 64, spec.ch)?;
        if spec.id == "filter" {
            instance.plugin = Box::new(Reader { filter: instance.plugin, trace: trace.clone(), clock: 0 });
        }
        Some(instance)
    };
    let generation = PlanGeneration::EMPTY.checked_next().unwrap();
    let mut env = compiler.compile(Arc::new(s.clone()), PlanGeneration::EMPTY, generation, &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    let mut out = vec![vec![0.0; 64]; 2];
    mix.render_active(0, 64, &mut out);
    assert_eq!(trace.ons.load(Relaxed), 0);
    s.tracks[0].mixer.inserts[0].as_mut().unwrap().params.insert("lookahead".into(), 10.0);
    let mut env = compiler.compile(Arc::new(s), generation, generation.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    for pos in (64..8192).step_by(64) {
        mix.render_active(pos, 64, &mut out);
    }
    assert_eq!((trace.on.load(Relaxed), trace.off.load(Relaxed)), (505, 530));
    assert_eq!((trace.ons.load(Relaxed), trace.offs.load(Relaxed)), (1, 1));
    assert!(out.iter().flatten().all(|v| *v == 0.0), "no stuck amp envelope");
}
