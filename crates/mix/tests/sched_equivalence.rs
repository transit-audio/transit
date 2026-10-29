//! DSP equivalence of the task graph under any scheduling condition (issue #210, ADR 0029
//! amendment section 8): seeded stall schedules, skips as the only difference, state continuity
//! across the holes.
//!
//! **The property.** For every keyed topology of `sidechain.rs` and `render_graph.rs` (and the
//! corpus sessions with a key edge added) and the continuity session, for each seed: the stall
//! schedule of the seed ([`Schedule::seeded`]: which strip is held in which block, for how many
//! blocks) is replayed on the graph path through two render workers and through none, and
//!
//! 1. the strips that missed a block are exactly the ones the graph counted (busy, late,
//!    unstarted, upstream-missed, by strip and block, from the per-strip figures): every silent
//!    strip has exactly one counted cause that block, and every counted skip is silent; a
//!    strip's own overrun falls only on a strip the schedule holds that block, and every held
//!    strip misses it;
//! 2. the graph render equals, bit for bit on every output sample of every block, the serial
//!    render with exactly those blocks muted ([`MixEngine::mute_next_block`]: a busy, unstarted or
//!    upstream-missed strip skipped, its keys not filled, its lines neither written nor idled, its
//!    MIDI held; a late strip rendered and its output dropped, as its worker's late render moves
//!    its state on), and the tail skips of the auxes keyed from a missed strip are the same;
//! 3. at every block boundary, every strip's state fingerprint ([`StripPrint`]: the plug-ins'
//!    state (the compressor's detector envelope, sidechain filters and lookahead lines, the test
//!    delays' and the continuity instruments' state), the key alignment and delay compensation
//!    lines (contents and write positions), and the instrument's delivered and pending MIDI) is
//!    the same in both renders, for every strip that is home (a strip a worker holds has no
//!    readable state until it is back).
//!
//! And across a graph change: a key edge added or removed while a worker holds an old task, the
//! plan refused until the task is home, its result retired unread, and the new graph's output and
//! state equal to the muted serial render's from its first block.
//!
//! **Realizing a schedule (E49).** Nothing here is a clock or a thread-timing outcome: the
//! schedule is the test's input. Every phase-1 strip carries a hold insert (unity gain) that,
//! armed, keeps a render worker inside the strip's render until the test releases it; a held
//! strip's cost history is an hour, so the callback never renders it inline; and the deterministic
//! deadline ([`MixEngine::hold_until_alone`]) is reached only once the held strips are the only
//! ones unresolved and each is claimed by a worker. Through two workers a held strip is then
//! `Late` in its first block and `Busy` after; through none it is `Unstarted`; its dependents are
//! upstream-missed. At most one strip is held at a time with two workers (the other worker stays
//! free for the rest), any number with none.
//!
//! **Seeds.** `TRANSIT_SCHED_SEEDS` (default 32) seeds per topology and worker count; the corpus
//! sessions, whose state is only partly readable, run an eighth of them (at least 4) over a longer
//! render. `TRANSIT_SCHED_SEED=<n>` runs that one seed. A failing seed is shrunk
//! ([`Schedule::shrink`]) and printed as a Rust literal with the first divergence; the minimal
//! schedule lands as a named regression test ([`regression`], see the template at the end).
//!
//! **Where state cannot be read** (the corpus sessions' equalizers, saturator, chorus, reverb,
//! maximizer and synths have no fingerprint): the holds end `WINDOW` blocks before the end of the
//! render and every corpus case states its post-fault window, the longest tail and latency of its
//! plug-ins plus the plan's latency, rounded up to whole blocks; the render runs that long after
//! the last hold, and every sample of it is compared.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use common::workers::{Player, Rig};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_audio_io::AudioBuffer;
use transit_dsp::Plugin;
use transit_mix::{Adoption, InstanceSpec, MixEngine, Mute, NewInstance, PlanGeneration, RejectReason, SlotGen, StripPrint, create_instance};
use transit_model::{Bus, BusId, ChannelFormat, Clip, Insert, KeyFrom, KeySource, Route, Session, SourceAudio, SourceId, Tap, TrackId, TrackKind};
use transit_rt::worker_set::figures::StripFigures;
use transit_rt::worker_set::schedule::{Schedule, Shape};
use transit_rt::worker_set::{Outcome, OverrunCause};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const SR: u32 = common::workers::SR;
const BLOCK: usize = 256;
/// Blocks per render of the built-in topologies.
const BLOCKS: usize = 48;
const LEN: usize = BLOCK * 400;
const KICKS: [usize; 4] = [1_000, 4_200, 7_900, 10_300];
const BURST: usize = 600;
const BASS: f32 = 0.05;
/// Blocks after the last hold. Every plug-in of the built-in topologies has a fingerprint; this
/// is still longer than their longest state (the compressor's 10 ms release, 2 blocks; the
/// longest delay or key alignment line, 320 samples).
const WINDOW: usize = 8;
/// A strips' deadline no render reaches: a strip outside the schedule is always rendered.
const FAR: Duration = Duration::from_secs(60);
/// A held strip's cost history: the callback never renders it inline.
const HOUR: Duration = Duration::from_secs(3600);
/// Everyone else's: the callback may render any of them inline.
const TINY: Duration = Duration::from_nanos(1);
/// A stall bound, not a latency claim: only turns a hang into a failure.
const STALL: Duration = Duration::from_secs(60);

// ---- Sessions (as in `sidechain.rs` and `render_graph.rs`) ----

fn session() -> Session {
    let mut s = Session::default();
    s.edit.delay_compensation = true;
    s.edit.click = false;
    s
}

fn audio(s: &mut Session, name: &str, sig: Vec<f32>, output: Route) -> TrackId {
    let src = SourceId(s.next_id + 10_000);
    s.pool.insert(src, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![sig.clone(), sig] })));
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some(name));
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(cid, "c", src, 0, 0, LEN as i64));
    tr.mixer.pan = vec![-1.0, 1.0];
    tr.mixer.output = output;
    t
}

fn kick_signal() -> Vec<f32> {
    let mut v = vec![0.0; LEN];
    for k in KICKS {
        v[k..k + BURST].fill(0.9);
    }
    v
}

fn bass_signal() -> Vec<f32> {
    (0..LEN).map(|i| BASS * (0.2 + 0.8 * (i as f32 * 0.0131).sin().abs())).collect()
}

fn ducker(key: Option<KeySource>) -> Insert {
    let mut c = Insert::new("compressor");
    for p in transit_dsp::plugin_info("compressor").unwrap().params {
        c.params.insert(p.id.to_string(), p.default);
    }
    for (k, v) in [("threshold", -20.0), ("ratio", 20.0), ("attack", 0.5), ("release", 10.0), ("knee", 0.0)] {
        c.params.insert(k.into(), v);
    }
    c.key = key;
    c
}

fn key_from(t: TrackId, tap: Tap) -> Option<KeySource> {
    Some(KeySource { from: KeyFrom::Track(t), tap })
}

fn bus(s: &mut Session, name: &str) -> BusId {
    let id = BusId(s.alloc());
    s.busses.push(Bus { id, name: name.into(), format: ChannelFormat::Stereo });
    id
}

/// Four quiet tracks, so phase 1 is wide enough for the render workers.
fn fillers(s: &mut Session, output: Route) {
    for i in 0..4 {
        let sig: Vec<f32> = (0..LEN).map(|k| 0.01 * ((k + 97 * i) as f32 * 0.003).sin()).collect();
        audio(s, &format!("Filler {i}"), sig, output.clone());
    }
}

fn kick_to_bass(bass_first: bool, tap: Tap) -> Session {
    let mut s = session();
    fillers(&mut s, Route::Main);
    let (bass, kick);
    if bass_first {
        bass = audio(&mut s, "Bass", bass_signal(), Route::Main);
        kick = audio(&mut s, "Kick", kick_signal(), Route::None);
    } else {
        kick = audio(&mut s, "Kick", kick_signal(), Route::None);
        bass = audio(&mut s, "Bass", bass_signal(), Route::Main);
    }
    s.track_mut(bass).unwrap().mixer.inserts[0] = Some(ducker(key_from(kick, tap)));
    s
}

fn track(s: &Session, name: &str) -> TrackId {
    s.tracks.iter().find(|t| t.name == name).unwrap().id
}

/// Kick keys Bass, and Bass keys Pad (`pad_keyed`; else Pad's compressor detects on its own
/// input): a chain of two edges.
fn chain_with(pad_keyed: bool) -> Session {
    let mut s = kick_to_bass(false, Tap::Post);
    let bass = track(&s, "Bass");
    let pad = audio(&mut s, "Pad", bass_signal(), Route::Main);
    s.track_mut(pad).unwrap().mixer.inserts[0] = Some(ducker(if pad_keyed { key_from(bass, Tap::Pre) } else { None }));
    s
}

fn bus_key_onto_an_aux() -> Session {
    let mut s = session();
    fillers(&mut s, Route::Main);
    let (kbus, bbus) = (bus(&mut s, "Kick Bus"), bus(&mut s, "Bass Bus"));
    audio(&mut s, "Kick", kick_signal(), Route::Bus(kbus));
    audio(&mut s, "Bass", bass_signal(), Route::Bus(bbus));
    let a = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Bass Aux"));
    let t = s.track_mut(a).unwrap();
    t.mixer.input = Route::Bus(bbus);
    t.mixer.output = Route::Main;
    t.mixer.pan = vec![-1.0, 1.0];
    t.mixer.inserts[0] = Some(ducker(Some(KeySource { from: KeyFrom::Bus(kbus), tap: Tap::Post })));
    s
}

fn delay(n: usize) -> Option<Insert> {
    let mut i = Insert::new("gain");
    i.preset = format!("delay:{n}");
    Some(i)
}

/// A kick outside the mix (`kick` samples of latency, at `kick_db`) keying a compressor on a
/// bass: `before` samples of latency ahead of the compressor, `after` behind it.
fn latent_kick_to_bass(kick: usize, before: usize, after: usize, tap: Tap, kick_db: f32) -> Session {
    let mut s = session();
    fillers(&mut s, Route::None);
    let k = audio(&mut s, "Kick", kick_signal(), Route::None);
    let b = audio(&mut s, "Bass", bass_signal(), Route::Main);
    let kt = s.track_mut(k).unwrap();
    kt.mixer.volume_db = kick_db;
    if kick > 0 {
        kt.mixer.inserts[0] = delay(kick);
    }
    let bt = s.track_mut(b).unwrap();
    if before > 0 {
        bt.mixer.inserts[0] = delay(before);
    }
    bt.mixer.inserts[1] = Some(ducker(key_from(k, tap)));
    if after > 0 {
        bt.mixer.inserts[2] = delay(after);
    }
    s
}

/// Kick (64) -> Kick Bus -> Drums aux (32) -> Drum Out bus, which keys the Bass Aux.
fn latent_bus_chain() -> Session {
    let mut s = session();
    fillers(&mut s, Route::Main);
    let (kb, ob, bb) = (bus(&mut s, "Kick Bus"), bus(&mut s, "Drum Out"), bus(&mut s, "Bass Bus"));
    let k = audio(&mut s, "Kick", kick_signal(), Route::Bus(kb));
    audio(&mut s, "Bass", bass_signal(), Route::Bus(bb));
    s.track_mut(k).unwrap().mixer.inserts[0] = delay(64);
    let d = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Drums"));
    let t = s.track_mut(d).unwrap();
    t.mixer.input = Route::Bus(kb);
    t.mixer.output = Route::Bus(ob);
    t.mixer.pan = vec![-1.0, 1.0];
    t.mixer.inserts[0] = delay(32);
    let a = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Bass Aux"));
    let t = s.track_mut(a).unwrap();
    t.mixer.input = Route::Bus(bb);
    t.mixer.output = Route::Main;
    t.mixer.pan = vec![-1.0, 1.0];
    t.mixer.inserts[0] = Some(ducker(Some(KeySource { from: KeyFrom::Bus(ob), tap: Tap::Post })));
    s
}

/// The continuity session of `render_graph.rs`: six continuity instruments, 1 and 2 keyed from
/// 0, 4 keyed from 3, 5 unrelated; live notes and pedals on all of them.
fn continuity() -> (Session, Vec<TrackId>) {
    let (mut s, ids) = common::workers::session(6, 0);
    for (to, from) in [(1, 0), (2, 0), (4, 3)] {
        s.track_mut(ids[to]).unwrap().mixer.inserts[0] = Some(ducker(key_from(ids[from], Tap::Post)));
    }
    (s, ids)
}

/// The corpus sessions with a key edge (as `render_graph.rs` adds it): a compressor on the last
/// live audio or instrument track, keyed from the first.
fn corpus() -> Vec<(String, Session)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/corpus");
    let mut files: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "transit")).collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let mut s = Session::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let live: Vec<TrackId> =
            s.tracks.iter().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) && !t.inactive).map(|t| t.id).collect();
        if live.len() < 4 {
            continue;
        }
        let (from, to) = (live[0], live[live.len() - 1]);
        if let Some(slot) = s.track_mut(to).unwrap().mixer.inserts.iter_mut().find(|x| x.is_none()) {
            *slot = Some(ducker(key_from(from, Tap::Post)));
            out.push((path.file_name().unwrap().to_string_lossy().into_owned(), s));
        }
    }
    out
}

// ---- The test plug-ins ----

/// A strip's hold switch, shared by its hold insert and the test.
#[derive(Default)]
struct Knob {
    hold: AtomicBool,
}

std::thread_local! {
    /// Set on the test's callback thread: a hold applies only on a render worker.
    static ON_CALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A unity gain that, armed, keeps a render worker inside `process` until the test releases it.
struct Hold {
    inner: Box<dyn Plugin>,
    knob: Arc<Knob>,
}

impl Plugin for Hold {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if !ON_CALLBACK.get() {
            let t0 = Instant::now();
            while self.knob.hold.load(Ordering::Acquire) {
                assert!(t0.elapsed() < STALL, "never released");
                std::thread::yield_now();
            }
        }
        self.inner.process(io, frames);
    }
    fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        self.inner.fingerprint(h);
    }
}

/// A pure delay of `n` samples reporting `n` as its latency (as in `sidechain.rs`).
struct Delay {
    n: usize,
    ring: Vec<Vec<f32>>,
    w: usize,
}

impl Plugin for Delay {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        transit_dsp::plugin_info("gain").unwrap()
    }
    fn prepare(&mut self, _sr: f32, _mb: usize, ch: usize) {
        self.ring = vec![vec![0.0; self.n.max(1)]; ch.max(2)];
        self.w = 0;
    }
    fn reset(&mut self) {
        self.ring.iter_mut().for_each(|r| r.fill(0.0));
    }
    fn set_param(&mut self, _id: &str, _v: f32) -> bool {
        true
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let w0 = self.w;
        for (c, ch) in io.iter_mut().enumerate() {
            let Some(r) = self.ring.get_mut(c) else { continue };
            let mut w = w0;
            for x in ch.iter_mut().take(frames) {
                std::mem::swap(&mut r[w], x);
                w = (w + 1) % self.n;
            }
        }
        self.w = (w0 + frames) % self.n;
    }
    fn latency(&self) -> usize {
        self.n
    }
    fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        h.write_usize(self.w);
        for r in &self.ring {
            r.iter().for_each(|v| h.write_u32(v.to_bits()));
        }
    }
}

// ---- Topologies ----

/// A keyed topology with a hold insert on every live audio and instrument track.
#[derive(Clone)]
struct Topology {
    name: String,
    session: Session,
    /// The continuity instruments (empty: an audio topology).
    instruments: Vec<TrackId>,
    knobs: Arc<HashMap<TrackId, Arc<Knob>>>,
    /// Blocks per render.
    blocks: usize,
    /// Blocks after the last hold (see the module docs).
    window: usize,
}

const HOLD: &str = "hold";

impl Topology {
    fn new(name: &str, mut session: Session, instruments: Vec<TrackId>) -> Topology {
        let mut knobs = HashMap::new();
        for t in session.tracks.iter_mut().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) && !t.inactive) {
            let last = t.mixer.inserts.len() - 1;
            assert!(t.mixer.inserts[last].is_none(), "{name}: {} has a free last slot", t.name);
            let mut i = Insert::new("gain");
            i.preset = HOLD.into();
            t.mixer.inserts[last] = Some(i);
            knobs.insert(t.id, Arc::new(Knob::default()));
        }
        Topology { name: name.into(), session, instruments, knobs: Arc::new(knobs), blocks: BLOCKS, window: WINDOW }
    }

    /// The test plug-ins of `s` (a hold, a delay), else `None` for the usual factory.
    fn extra(s: &Session, knobs: &HashMap<TrackId, Arc<Knob>>, spec: &InstanceSpec, frames: usize) -> Option<NewInstance> {
        let preset = s.track(spec.track)?.mixer.inserts.get(spec.slot)?.as_ref()?.preset.clone();
        if preset == HOLD {
            let inner = create_instance("gain", None, SR as f32, frames, spec.ch)?.plugin;
            return Some(NewInstance { plugin: Box::new(Hold { inner, knob: Arc::clone(knobs.get(&spec.track)?) }), refused_state: false });
        }
        let n: usize = preset.strip_prefix("delay:")?.parse().ok()?;
        let mut d = Delay { n, ring: Vec::new(), w: 0 };
        d.prepare(SR as f32, frames, spec.ch);
        Some(NewInstance { plugin: Box::new(d), refused_state: false })
    }

    fn rig(&self) -> Rig {
        let (s, knobs) = (self.session.clone(), Arc::clone(&self.knobs));
        Rig::with_session(self.session.clone(), self.instruments.clone(), BLOCK, move |spec| Topology::extra(&s, &knobs, spec, BLOCK))
    }

    /// Compiles `s` (an audio topology's next session) for `rig` and offers it.
    fn offer(&self, rig: &mut Rig, s: &Session, generation: u64) -> Adoption {
        let knobs = Arc::clone(&self.knobs);
        let mut make = |spec: &InstanceSpec, _: SlotGen| {
            Topology::extra(s, &knobs, spec, BLOCK).or_else(|| create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch))
        };
        let mut env = rig.comp.compile(Arc::new(s.clone()), rig.comp.active(), PlanGeneration::from_raw(generation), &mut make).unwrap();
        let adopted = rig.mix.adopt_plan(&mut env);
        rig.comp.reclaim(env);
        adopted
    }

    fn knob(&self, rig: &Rig, strip: usize) -> Arc<Knob> {
        let t = self.session.tracks.iter().find(|t| rig.mix.strip_index(t.id) == Some(strip)).unwrap().id;
        Arc::clone(&self.knobs[&t])
    }
}

/// What a schedule may hold in `top`: the phase-1 strips a worker can render, the key sources and
/// the keyed strips twice as often as the rest.
fn shape(top: &Topology, workers: usize) -> (Shape, HashMap<usize, String>) {
    let rig = top.rig();
    let (phase1, _) = rig.mix.strip_phases();
    let keyed_or_source = |t: &transit_model::Track| {
        t.mixer.inserts.iter().flatten().any(|i| i.key.is_some())
            || top
                .session
                .tracks
                .iter()
                .any(|o| o.mixer.inserts.iter().flatten().any(|i| i.key.as_ref().is_some_and(|k| k.from == KeyFrom::Track(t.id))))
    };
    let mut strips = Vec::new();
    let mut names = HashMap::new();
    for t in &top.session.tracks {
        let Some(i) = rig.mix.strip_index(t.id).filter(|&i| i < phase1 && top.knobs.contains_key(&t.id) && !rig.mix.strip_pinned(i)) else {
            continue;
        };
        names.insert(i, t.name.clone());
        strips.push(i);
        if keyed_or_source(t) {
            strips.push(i);
        }
    }
    strips.sort_unstable();
    let shape =
        Shape { strips, first: 2, last: top.blocks - top.window, longest: 3, concurrent: if workers == 0 { 3 } else { workers - 1 }, rarity: 4 };
    (shape, names)
}

// ---- The renders ----

/// A plan change in the middle of a render: before block `at`, `session` is offered (refused on
/// the graph path while a worker holds a task), then adopted once every strip is home.
#[derive(Clone)]
struct Change {
    at: usize,
    session: Session,
}

/// One render's record, block by block.
#[derive(Default)]
struct Run {
    /// Every output channel of every block.
    out: Vec<f32>,
    channels: usize,
    /// Every strip's fingerprint after each block.
    prints: Vec<Vec<Option<StripPrint>>>,
    /// The same, readable (for a failure report).
    states: Vec<Vec<Option<String>>>,
    /// Graph: the counted skips of each block, as the muted serial render mutes them.
    mutes: Vec<Vec<(usize, Mute)>>,
    /// Graph: the causes counted per block, by strip.
    causes: Vec<Vec<(usize, OverrunCause)>>,
    /// The auxes' tail skips per block.
    tails: Vec<Vec<bool>>,
    late_results: u64,
    conflicts: u64,
}

fn as_callback<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::spawn(move || {
        common::workers::become_callback();
        ON_CALLBACK.set(true);
        let r = f();
        transit_mix::unmark_audio_thread();
        r
    })
    .join()
    .unwrap()
}

/// Waits until no worker holds strip `s` (its late render is done; the next block retires it).
fn wait_home(rig: &Rig, s: usize) {
    let t0 = Instant::now();
    while rig.mix.strip_claimed(s) {
        assert!(t0.elapsed() < STALL, "strip {s}: the worker never returned");
        std::thread::yield_now();
    }
}

/// The phase-1 key edges of `session` as `rig`'s plan lays its strips out: (source, keyed),
/// from the tracks' inserts, not from the plan's tables.
fn key_edges(rig: &Rig, session: &Session) -> Vec<(usize, usize)> {
    let (phase1, _) = rig.mix.strip_phases();
    let index = |t: TrackId| rig.mix.strip_index(t).filter(|&i| i < phase1);
    let mut edges = Vec::new();
    for t in session.tracks.iter().filter(|t| !t.inactive) {
        let Some(to) = index(t.id) else { continue };
        for ins in t.mixer.inserts.iter().flatten().filter(|i| i.active && !i.bypass) {
            if let Some(KeySource { from: KeyFrom::Track(src), .. }) = ins.key
                && let Some(from) = index(src)
            {
                edges.push((from, to));
            }
        }
    }
    edges
}

/// Waits until every strip is home (a plan is adopted only then).
fn settle(rig: &mut Rig) {
    let t0 = Instant::now();
    while !rig.mix.render_workers_settled() {
        assert!(t0.elapsed() < STALL, "a worker never returned");
        std::thread::yield_now();
    }
}

fn record(run: &mut Run, rig: &Rig) {
    run.channels = rig.out.len();
    for c in &rig.out {
        run.out.extend_from_slice(c);
    }
    run.prints.push(rig.mix.strip_prints());
    run.states.push(rig.mix.strip_states());
    run.tails.push(rig.mix.tail_skips());
}

/// `top` through the task graph with `workers` render workers, replaying `schedule`; checks the
/// realization (property 1) as it goes. `Err`: the realization broke the property.
fn graph_run(top: &Topology, workers: usize, schedule: &Schedule, change: Option<&Change>) -> Result<Run, String> {
    let mut rig = top.rig();
    assert_eq!(rig.mix.start_render_workers_with(64, workers), workers);
    rig.mix.seed_strip_costs(TINY);
    rig.mix.set_strip_deadline(Some(FAR));
    let knobs: HashMap<usize, Arc<Knob>> = schedule.holds.iter().map(|h| (h.strip, top.knob(&rig, h.strip))).collect();
    let name = top.name.clone();
    let (top, schedule, change) = (top.clone(), schedule.clone(), change.cloned());
    let (rig, run, failure) = as_callback(move || {
        let mut rig = rig;
        let mut player = (!top.instruments.is_empty()).then(|| Player::new(top.instruments.len(), 210));
        let mut run = Run::default();
        let mut failure = None;
        for b in 0..top.blocks {
            if let Some(c) = change.as_ref().filter(|c| c.at == b) {
                let held = b.checked_sub(1).is_some_and(|p| schedule.active(p).next().is_some());
                let offered = top.offer(&mut rig, &c.session, 2);
                if held && offered != Adoption::Rejected(RejectReason::StripsInFlight) {
                    failure.get_or_insert(format!("block {b}: the change was {offered:?} while a worker held a task"));
                }
            }
            for s in schedule.ending(b) {
                knobs[&s].hold.store(false, Ordering::Release);
                wait_home(&rig, s);
                rig.mix.set_strip_cost(s, TINY);
            }
            if let Some(c) = change.as_ref().filter(|c| c.at == b) {
                settle(&mut rig);
                assert_eq!(top.offer(&mut rig, &c.session, 3), Adoption::Adopted, "the change, once every task is home");
            }
            for s in schedule.starting(b) {
                knobs[&s].hold.store(true, Ordering::Release);
                rig.mix.set_strip_cost(s, HOUR);
            }
            let held: Vec<usize> = schedule.active(b).collect();
            rig.mix.hold_until_alone(&held);
            if let Some(p) = player.as_mut() {
                p.play(&mut rig, b as u64);
            }
            let before = rig.mix.strip_counts();
            rig.block();
            let (after, outcomes, silence) = (rig.mix.strip_counts(), rig.mix.strip_outcomes(), rig.mix.strip_silence());
            let session = change.as_ref().filter(|c| b >= c.at).map_or(&top.session, |c| &c.session);
            let edges = key_edges(&rig, session);
            let mut mutes = Vec::new();
            let mut causes = Vec::new();
            for (i, (a, z)) in after.iter().zip(&before).enumerate() {
                // A strip whose chain changed has its counts reset in its first block (#168).
                let reset = a.overruns_busy < z.overruns_busy
                    || a.overruns_late < z.overruns_late
                    || a.overruns_unstarted < z.overruns_unstarted
                    || a.upstream_missed < z.upstream_missed;
                let z = if reset { &StripFigures::default() } else { z };
                let counted = [
                    (a.overruns_busy - z.overruns_busy, OverrunCause::Busy),
                    (a.overruns_late - z.overruns_late, OverrunCause::Late),
                    (a.overruns_unstarted - z.overruns_unstarted, OverrunCause::Unstarted),
                    (a.upstream_missed - z.upstream_missed, OverrunCause::UpstreamMissed),
                ];
                let n: u64 = counted.iter().map(|c| c.0).sum();
                let cause = counted.iter().find(|c| c.0 > 0).map(|c| c.1);
                let mut why = Vec::new();
                if n > 1 {
                    why.push("more than one cause counted");
                }
                if silence[i] != (n == 1) {
                    why.push("its silence is not one counted skip");
                }
                match cause {
                    Some(cause) => {
                        if outcomes[i] != Some(Outcome::Overrun(cause)) {
                            why.push("its outcome is not the counted cause");
                        }
                        if cause != OverrunCause::UpstreamMissed && !held.contains(&i) {
                            why.push("an overrun of its own, not held by the schedule");
                        }
                        mutes.push((i, if cause == OverrunCause::Late { Mute::Discard } else { Mute::Skip }));
                        causes.push((i, cause));
                    }
                    None if held.contains(&i) => why.push("held by the schedule, but rendered"),
                    None if rig.mix.strip_keys_pending(i) => why.push("rendered, but a key's early delay is still pending"),
                    None => {}
                }
                if !why.is_empty() {
                    failure.get_or_insert(format!(
                        "block {b}, strip {i}: {} (outcome {:?}, silent {}, counted {counted:?})",
                        why.join("; "),
                        outcomes[i],
                        silence[i]
                    ));
                }
            }
            // Independently of the graph's tables: from the session's key edges, a strip keyed
            // from a strip that missed the block misses it as upstream-missed (or, still held by a
            // worker, busy: A2), and an upstream-missed strip has a source that missed.
            let missed = |j: usize| causes.iter().any(|c: &(usize, OverrunCause)| c.0 == j);
            let cause_of = |j: usize| causes.iter().find(|c| c.0 == j).map(|c| c.1);
            for d in 0..after.len() {
                let from_missed = edges.iter().any(|&(src, to)| to == d && missed(src));
                let why = match (from_missed, cause_of(d)) {
                    (true, Some(OverrunCause::UpstreamMissed | OverrunCause::Busy)) | (false, None | Some(_)) => None,
                    (true, other) => Some(format!("keyed from a strip that missed, but {other:?}")),
                };
                let orphan = cause_of(d) == Some(OverrunCause::UpstreamMissed) && !from_missed;
                if let Some(why) = why.or_else(|| orphan.then(|| "upstream-missed with no key source that missed".to_string())) {
                    failure.get_or_insert(format!("block {b}, strip {d}: {why} (key edges {edges:?}, skips {causes:?})"));
                }
            }
            run.mutes.push(mutes);
            run.causes.push(causes);
            record(&mut run, &rig);
        }
        for k in knobs.values() {
            k.hold.store(false, Ordering::Release);
        }
        for &s in knobs.keys() {
            wait_home(&rig, s);
        }
        (rig, run, failure)
    });
    let mut run = run;
    run.late_results = rig.mix.late_results();
    run.conflicts = rig.mix.strip_faults().map_or(0, |r| r.snapshot().conflicts);
    drop(rig.mix.into_retired());
    match failure {
        Some(f) => Err(format!("{name}: property 1 (the counted skips): {f}")),
        None => Ok(run),
    }
}

/// `top` serially with the blocks of `mutes` muted (and the same `change`).
fn muted_run(top: &Topology, mutes: &[Vec<(usize, Mute)>], change: Option<&Change>) -> Run {
    let rig = top.rig();
    let (top, mutes, change) = (top.clone(), mutes.to_vec(), change.cloned());
    let (rig, run) = as_callback(move || {
        let mut rig = rig;
        let mut player = (!top.instruments.is_empty()).then(|| Player::new(top.instruments.len(), 210));
        let mut run = Run::default();
        for (b, m) in mutes.iter().enumerate().take(top.blocks) {
            if let Some(c) = change.as_ref().filter(|c| c.at == b) {
                assert_eq!(top.offer(&mut rig, &c.session, 3), Adoption::Adopted);
            }
            if let Some(p) = player.as_mut() {
                p.play(&mut rig, b as u64);
            }
            rig.mix.mute_next_block(m);
            rig.block();
            record(&mut run, &rig);
        }
        (rig, run)
    });
    drop(rig.mix.into_retired());
    run
}

/// The first divergence between the graph render and its muted serial reference (properties 2
/// and 3), with both modes' state.
fn compare(top: &Topology, names: &HashMap<usize, String>, g: &Run, m: &Run) -> Result<(), String> {
    let per_block = BLOCK * g.channels;
    let mut first: Option<(usize, String)> = None;
    let mut note = |b: usize, what: String| {
        if first.as_ref().is_none_or(|(fb, _)| b < *fb) {
            first = Some((b, what));
        }
    };
    let sample = g.out.iter().zip(&m.out).position(|(x, y)| x.to_bits() != y.to_bits()).map(|k| {
        let b = k / per_block;
        (b, format!("sample {} of block {b} (channel {}): graph {} serial {}", k % BLOCK, (k % per_block) / BLOCK, g.out[k], m.out[k]))
    });
    if let Some((b, what)) = sample.clone() {
        note(b, what);
    }
    if g.out.len() != m.out.len() {
        note(0, format!("lengths {} and {}", g.out.len(), m.out.len()));
    }
    for (b, (x, y)) in g.tails.iter().zip(&m.tails).enumerate() {
        if x != y {
            note(b, format!("the auxes' tail skips: graph {x:?} serial {y:?}"));
        }
    }
    for (b, (gp, mp)) in g.prints.iter().zip(&m.prints).enumerate() {
        for (i, (x, y)) in gp.iter().zip(mp).enumerate() {
            if let (Some(x), Some(y)) = (x, y)
                && x != y
            {
                let what = [("plugins", x.plugins != y.plugins), ("lines", x.lines != y.lines), ("midi", x.midi != y.midi)]
                    .iter()
                    .filter(|w| w.1)
                    .map(|w| w.0)
                    .collect::<Vec<_>>()
                    .join(", ");
                let name = names.get(&i).cloned().unwrap_or_else(|| format!("strip {i}"));
                let state = |r: &Run| r.states.get(b).and_then(|s| s.get(i).cloned().flatten()).unwrap_or_default();
                note(
                    b,
                    format!(
                        "the state of {name} after block {b} ({what} differ):
    graph:  {}
    serial: {}",
                        state(g),
                        state(m)
                    ),
                );
                break;
            }
        }
    }
    let Some((b, what)) = first else { return Ok(()) };
    let samples = sample.map_or_else(|| "the output never diverges".to_string(), |(_, w)| format!("the output first diverges at {w}"));
    let mut out = format!("{}: diverges at block {b}: {what}\n  {samples}\n  counted skips, blocks {}..={b}:", top.name, b.saturating_sub(3));
    for k in b.saturating_sub(3)..=b {
        let _ = write!(out, "\n    block {k}: {:?}", g.causes.get(k));
    }
    Err(out)
}

/// The property for one schedule (see the module docs).
fn check(top: &Topology, workers: usize, names: &HashMap<usize, String>, schedule: &Schedule, change: Option<&Change>) -> Result<(), String> {
    let g = graph_run(top, workers, schedule, change)?;
    if g.conflicts != 0 {
        return Err(format!("{} protocol conflicts", g.conflicts));
    }
    let m = muted_run(top, &g.mutes, change);
    compare(top, names, &g, &m)
}

fn seeds(corpus: bool) -> Vec<u64> {
    if let Some(one) = std::env::var("TRANSIT_SCHED_SEED").ok().and_then(|v| v.parse().ok()) {
        return vec![one];
    }
    let n: u64 = std::env::var("TRANSIT_SCHED_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(32);
    (0..if corpus { (n / 8).max(4) } else { n }).collect()
}

/// Runs the property for `seeds` on `top` through `workers`; on a failure, shrinks the schedule
/// and panics with the minimal schedule as a Rust literal and the first divergence.
fn property(top: &Topology, workers: usize, seeds: &[u64]) -> Result<(), String> {
    let (shape, names) = shape(top, workers);
    let salt = top.name.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    for &seed in seeds {
        let schedule = Schedule::seeded(seed ^ salt, &shape);
        let Err(why) = check(top, workers, &names, &schedule, None) else { continue };
        let min = schedule.shrink(&mut |c: &Schedule| check(top, workers, &names, c, None).is_err());
        let detail = check(top, workers, &names, &min, None).err().unwrap_or_default();
        return Err(format!(
            "{} through {workers} workers, seed {seed} (TRANSIT_SCHED_SEED={seed}): {why}

minimal schedule ({} holds):
{}

{detail}",
            top.name,
            min.holds.len(),
            min.to_rust(&|s| names.get(&s).cloned().unwrap_or_default())
        ));
    }
    Ok(())
}

/// Runs the property on every topology through two workers and through none, and fails once
/// with every failing case (a passing case is listed as passing: one defect does not hide the
/// rest).
fn every(tops: &[Topology], seeds: &[u64]) {
    let t0 = Instant::now();
    let mut failed = Vec::new();
    for top in tops {
        for workers in [2, 0] {
            let t = Instant::now();
            match property(top, workers, seeds) {
                Ok(()) => println!("pass: {} through {workers} workers, {} seeds, {:?}", top.name, seeds.len(), t.elapsed()),
                Err(e) => {
                    println!("FAIL: {} through {workers} workers", top.name);
                    failed.push(e);
                }
            }
        }
    }
    println!("{} topologies x 2 worker counts x {} seeds in {:?}", tops.len(), seeds.len(), t0.elapsed());
    assert!(
        failed.is_empty(),
        "{} of {} cases failed:

{}",
        failed.len(),
        tops.len() * 2,
        failed.join(
            "

"
        )
    );
}

/// The switches for the graph path; serial renders here have no workers, so they are serial
/// whatever the switches say. Restored after.
struct GraphOn<'a>(#[allow(dead_code)] std::sync::MutexGuard<'a, ()>);

fn graph_on() -> GraphOn<'static> {
    let one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    MixEngine::force_serial(false);
    MixEngine::force_graph(true);
    GraphOn(one)
}

impl Drop for GraphOn<'_> {
    fn drop(&mut self) {
        MixEngine::force_graph(false);
        MixEngine::force_serial(true);
    }
}

fn builtin() -> Vec<Topology> {
    vec![
        Topology::new("kick to bass, post", kick_to_bass(false, Tap::Post), vec![]),
        Topology::new("kick to bass, pre, bass first", kick_to_bass(true, Tap::Pre), vec![]),
        Topology::new("a chain of two keys", chain_with(true), vec![]),
        Topology::new("a bus key onto an aux", bus_key_onto_an_aux(), vec![]),
        Topology::new("a late key", latent_kick_to_bass(128, 0, 0, Tap::Post, 0.0), vec![]),
        Topology::new("an early key", latent_kick_to_bass(0, 128, 0, Tap::Post, 0.0), vec![]),
        Topology::new("latency behind the keyed insert", latent_kick_to_bass(0, 0, 128, Tap::Post, 0.0), vec![]),
        Topology::new("both latent, the key early", latent_kick_to_bass(64, 128, 0, Tap::Post, 0.0), vec![]),
        Topology::new("both latent, the key late", latent_kick_to_bass(192, 128, 32, Tap::Post, 0.0), vec![]),
        Topology::new("a pre-fader key from a silent fader, early", latent_kick_to_bass(0, 128, 0, Tap::Pre, -80.0), vec![]),
        Topology::new("a key through a latent bus chain", latent_bus_chain(), vec![]),
    ]
}

/// Gate (#210): the built-in keyed topologies, through two workers and through none.
#[test]
fn the_graph_equals_the_muted_serial_render_under_seeded_stall_schedules() {
    let _on = graph_on();
    every(&builtin(), &seeds(false));
}

/// Gate (#210): the continuity session (live notes and pedals; held notes, pending and deferred
/// MIDI in the fingerprint), through two workers and through none.
#[test]
fn the_continuity_session_equals_its_muted_serial_render_under_seeded_stall_schedules() {
    let _on = graph_on();
    let (s, ids) = continuity();
    every(&[Topology::new("the continuity session", s, ids)], &seeds(false));
}

/// The longest state of `s` no fingerprint reads: every insert's and instrument's tail and
/// latency, plus the plan's own latency, in whole blocks (at least `WINDOW`).
fn post_fault_window(s: &Session, latency: usize) -> usize {
    let mut longest = 0;
    for t in &s.tracks {
        for ins in t.mixer.inserts.iter().flatten().chain(t.instrument.iter()) {
            if let Some(n) = create_instance(&ins.plugin, None, SR as f32, BLOCK, 2) {
                let mut p = n.plugin;
                for (k, v) in &ins.params {
                    p.set_param(k, *v);
                }
                longest = longest.max(p.tail_samples() + p.latency());
            }
        }
    }
    (longest + latency).div_ceil(BLOCK).max(WINDOW)
}

/// Gate (#210): the corpus sessions with a key edge added (their equalizers, saturator, chorus,
/// reverb, maximizer and synths have no fingerprint: each case states its post-fault window).
#[test]
fn the_corpus_sessions_with_a_key_equal_their_muted_serial_render_under_seeded_stall_schedules() {
    let _on = graph_on();
    let seeds = seeds(true);
    let cases = corpus();
    assert!(cases.len() >= 2, "the released sessions with four or more live tracks");
    let mut tops = Vec::new();
    for (name, s) in cases {
        let mut top = Topology::new(&name, s, vec![]);
        let latency = {
            let mut rig = top.rig();
            rig.block();
            rig.mix.latency
        };
        top.window = post_fault_window(&top.session, latency);
        top.blocks = 40 + top.window;
        println!("{name}: holds in blocks 2..40, post-fault window {} blocks ({} samples)", top.window, top.window * BLOCK);
        tops.push(top);
    }
    every(&tops, &seeds);
}

/// Gate (#210): a key edge removed, and one added, while a worker holds an old task: the plan is
/// refused until the task is home, the old result is retired unread, and the new graph's output
/// and state equal the muted serial render's from its first block.
#[test]
fn a_graph_change_while_a_worker_holds_an_old_task_matches_the_muted_serial_render() {
    let _on = graph_on();
    for (name, from, to) in [("a key edge removed", true, false), ("a key edge added", false, true)] {
        let top = Topology::new(name, chain_with(from), vec![]);
        let next = Topology::new(name, chain_with(to), vec![]);
        let (shape, names) = shape(&top, 2);
        let bass = {
            let rig = top.rig();
            rig.mix.strip_index(track(&top.session, "Bass")).unwrap()
        };
        let change = Change { at: 20, session: next.session.clone() };
        for seed in seeds(false) {
            // The seed's schedule before the change, then Bass (a key source of the edge that
            // changes) held across it.
            let mut schedule = Schedule::seeded(seed, &Shape { last: 16, ..shape.clone() });
            schedule.holds.push(transit_rt::worker_set::schedule::Hold { block: 19, strip: bass, blocks: 1 });
            let g = graph_run(&top, 2, &schedule, Some(&change)).unwrap_or_else(|e| panic!("{name}, seed {seed}: {e}"));
            assert_eq!(g.conflicts, 0, "{name}, seed {seed}: the new graph's results carry its identity");
            assert!(g.late_results >= 1, "{name}, seed {seed}: the old task's result retired unread");
            assert_eq!(g.causes[19].iter().find(|c| c.0 == bass).map(|c| c.1), Some(OverrunCause::Late), "{name}, seed {seed}");
            let m = muted_run(&top, &g.mutes, Some(&change));
            if let Err(e) = compare(&top, &names, &g, &m) {
                panic!("{name}, seed {seed}:\n{}\n{e}", schedule.to_rust(&|s| names.get(&s).cloned().unwrap_or_default()));
            }
        }
    }
}

// ---- Regressions ----

/// A minimal schedule from a failing seed, replayed as a named regression: `topology` through
/// `workers` render workers must equal its muted serial render under `schedule`.
#[track_caller]
fn regression(top: &Topology, workers: usize, schedule: &Schedule) {
    let (_, names) = shape(top, workers);
    if let Err(e) = check(top, workers, &names, schedule, None) {
        panic!("{}\n{e}", schedule.to_rust(&|s| names.get(&s).cloned().unwrap_or_default()));
    }
}

/// The template a minimal schedule lands as (copy, name it after what it found, paste the
/// printed literal): here the key source of a chain held two blocks through two workers, then
/// the middle strip, then both ends through none.
#[test]
fn regression_a_chain_held_at_each_link() {
    let _on = graph_on();
    let top = Topology::new("a chain of two keys", chain_with(true), vec![]);
    let rig = top.rig();
    let [kick, bass, pad] = ["Kick", "Bass", "Pad"].map(|n| rig.mix.strip_index(track(&top.session, n)).unwrap());
    drop(rig);
    regression(&top, 2, &Schedule::new(&[(kick, 5, 2), (bass, 12, 1), (pad, 20, 3)]));
    regression(&top, 0, &Schedule::new(&[(kick, 5, 2), (bass, 6, 3), (pad, 20, 1)]));
}

/// Found by this gate (#174): an early key's strip held one block with no workers is taken back
/// unrendered at the deadline; its key entry's line must not have run for that block (it did:
/// the key was filled, early-key delay included, before the publish, so from then on the key ran
/// one block ahead of the strip's own signal). The keyed strip is Bass.
#[test]
fn regression_an_early_key_held_with_no_worker() {
    let _on = graph_on();
    let top = Topology::new("an early key", latent_kick_to_bass(0, 128, 0, Tap::Post, 0.0), vec![]);
    let bass = bass_index(&top);
    regression(&top, 0, &Schedule::new(&[(bass, 33, 1)]));
}

/// The same defect with both sides latent and the key early.
#[test]
fn regression_both_latent_the_key_early_held_with_no_worker() {
    let _on = graph_on();
    let top = Topology::new("both latent, the key early", latent_kick_to_bass(64, 128, 0, Tap::Post, 0.0), vec![]);
    let bass = bass_index(&top);
    regression(&top, 0, &Schedule::new(&[(bass, 38, 1)]));
}

/// The same defect with a pre-fader key from a silent fader, early.
#[test]
fn regression_a_pre_fader_key_from_a_silent_fader_early_held_with_no_worker() {
    let _on = graph_on();
    let top = Topology::new("a pre-fader key from a silent fader, early", latent_kick_to_bass(0, 128, 0, Tap::Pre, -80.0), vec![]);
    let bass = bass_index(&top);
    regression(&top, 0, &Schedule::new(&[(bass, 30, 1)]));
}

/// Bass's strip index in `top` (the printed schedules name it 5).
fn bass_index(top: &Topology) -> usize {
    let rig = top.rig();
    let bass = rig.mix.strip_index(track(&top.session, "Bass")).unwrap();
    assert_eq!(bass, 5, "the strip the minimal schedules hold");
    bass
}
