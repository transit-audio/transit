//! The task graph of a plan with sidechain keys through the render workers (issue #174, ADR 0029
//! amendment section 8), on the live engine path (`render_active` on a thread marked as the audio
//! thread, blocks rendered back to back: the device clock is the test's):
//!
//! - equivalence: serial and graph renders are bit-exact for the keyed sessions of
//!   `sidechain.rs` (track keys pre and post fader, either declaration order, a bus key onto an
//!   aux, a chain of two keys, latency on either side of a key) and for the corpus sessions, as
//!   saved and with a key edge added; through two workers, and through none (every strip
//!   rendered inline by the callback);
//! - sidechain impulse alignment: an early key, a late key and latency on either side give the
//!   duck's onset on the same sample under the graph as serially;
//! - the allocation-armed callback over a keyed plan with worker stalls, upstream-missed skips and
//!   inline renders;
//! - continuity: zero stuck notes and every hole in an instrument's time a counted overrun or
//!   upstream-missed skip, across skips; and, deterministically (a key source held on the one
//!   worker, released by the test), a skipped dependent's MIDI held and delivered at its next
//!   processed block.
//!
//! The graph's switch and the serial switch are process-wide, so the tests take turns. A far
//! strips' deadline (60 s) makes the equivalence renders wait for every worker: their outcome
//! never depends on scheduling (E49).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
mod common;

use common::workers::{Player, Rig, Rng, become_callback};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use transit_audio_io::AudioBuffer;
use transit_dsp::Plugin;
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, StripFaults,
    create_instance,
};
use transit_model::{Bus, BusId, ChannelFormat, Clip, Insert, KeyFrom, KeySource, Route, Session, SourceAudio, SourceId, Tap, TrackId, TrackKind};
use transit_rt::testing::measure;

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const SR: u32 = 48_000;
const BLOCK: usize = 256;
const LEN: usize = 24_000;
const KICKS: [usize; 3] = [3_000, 10_000, 17_000];
const BURST: usize = 600;
const BASS: f32 = 0.05;
/// A strips' deadline no render reaches: `finish` waits for every worker.
const FAR: Duration = Duration::from_secs(60);

// ---- Sessions (as in `sidechain.rs`) ----

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

/// Four quiet tracks, so phase 1 is wide enough for the render workers (to `output`).
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

/// Kick keys Bass, and Bass keys Pad: a chain of two edges.
fn chain() -> Session {
    let mut s = kick_to_bass(false, Tap::Post);
    let bass = s.tracks.iter().find(|t| t.name == "Bass").unwrap().id;
    let pad = audio(&mut s, "Pad", bass_signal(), Route::Main);
    s.track_mut(pad).unwrap().mixer.inserts[0] = Some(ducker(key_from(bass, Tap::Pre)));
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
        if self.n == 0 {
            return;
        }
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
}

fn delay(n: usize) -> Option<Insert> {
    let mut i = Insert::new("gain");
    i.preset = format!("delay:{n}");
    Some(i)
}

/// A kick outside the mix (`kick` samples of latency) keying a compressor on a steady bass:
/// `before` samples of latency ahead of the compressor, `after` behind it; with fillers outside
/// the mix (so the bass alone is heard).
fn latent_kick_to_bass(kick: usize, before: usize, after: usize) -> Session {
    let mut s = session();
    fillers(&mut s, Route::None);
    let k = audio(&mut s, "Kick", kick_signal(), Route::None);
    let b = audio(&mut s, "Bass", vec![BASS; LEN], Route::Main);
    if kick > 0 {
        s.track_mut(k).unwrap().mixer.inserts[0] = delay(kick);
    }
    let bt = s.track_mut(b).unwrap();
    if before > 0 {
        bt.mixer.inserts[0] = delay(before);
    }
    bt.mixer.inserts[1] = Some(ducker(key_from(k, Tap::Post)));
    if after > 0 {
        bt.mixer.inserts[2] = delay(after);
    }
    s
}

fn make(s: &Session, spec: &InstanceSpec) -> Option<NewInstance> {
    let n = s.track(spec.track).and_then(|t| t.mixer.inserts.get(spec.slot)?.as_ref()).and_then(|i| i.preset.strip_prefix("delay:")?.parse().ok());
    match n {
        Some(n) => {
            let mut d = Delay { n, ring: Vec::new(), w: 0 };
            d.prepare(SR as f32, BLOCK, spec.ch);
            Some(NewInstance { plugin: Box::new(d), refused_state: false })
        }
        None => create_instance(&spec.id, spec.state.as_deref(), SR as f32, BLOCK, spec.ch)
            .map(|n| NewInstance { plugin: n.plugin, refused_state: false }),
    }
}

// ---- The render ----

#[derive(Clone, Copy, Debug)]
enum Path {
    /// The serial path (render workers off).
    Serial,
    /// The task graph through this many workers (0: the callback renders every strip inline).
    Graph(usize),
}

struct Rendered {
    /// Every output channel of every block, in order.
    all: Vec<f32>,
    /// Main's left channel.
    left: Vec<f32>,
    latency: usize,
    faults: Option<StripFaults>,
}

fn target(out: u16) -> RenderTarget {
    RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: out, in_channels: 0 }
}

/// `blocks` blocks of `s` through the live engine path (`render_active` on a thread marked as the
/// audio thread) along `path`.
fn render(s: &Session, path: Path, blocks: usize) -> Rendered {
    let main = transit_mix::main_channels(s).max(2);
    let t = target(0);
    let mut mix = MixEngine::realtime(t);
    let mut compiler = PlanCompiler::new(t, PlanLimits::default());
    let shared = s.clone();
    let mut maker = |spec: &InstanceSpec, _: transit_mix::SlotGen| make(&shared, spec);
    let mut env = compiler.compile(Arc::new(s.clone()), compiler.active(), PlanGeneration::from_raw(1), &mut maker).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    compiler.reclaim(env);
    if let Path::Graph(w) = path {
        assert_eq!(mix.start_render_workers_with(64, w), w);
        if w == 0 {
            // A cost history for every strip, so the callback renders each inline (#168).
            mix.seed_strip_costs(Duration::from_nanos(1));
        }
        mix.set_strip_deadline(Some(FAR));
    }
    MixEngine::force_serial(matches!(path, Path::Serial));
    MixEngine::force_graph(matches!(path, Path::Graph(_)));
    let (mix, all, left) = std::thread::spawn(move || {
        transit_mix::mark_audio_thread();
        let mut out = vec![vec![0.0f32; BLOCK]; main];
        let (mut all, mut left) = (Vec::with_capacity(blocks * BLOCK * main), Vec::with_capacity(blocks * BLOCK));
        for b in 0..blocks {
            mix.render_active((b * BLOCK) as i64, BLOCK, &mut out);
            for c in &out {
                all.extend_from_slice(c);
            }
            left.extend_from_slice(&out[0]);
        }
        transit_mix::unmark_audio_thread();
        (mix, all, left)
    })
    .join()
    .unwrap();
    MixEngine::force_graph(false);
    MixEngine::force_serial(true);
    let faults = mix.strip_faults().map(|r| r.snapshot());
    let latency = mix.latency;
    drop(mix.into_retired());
    Rendered { all, left, latency, faults }
}

fn bit_exact(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

/// Serial against the graph (two workers, and none), bit for bit; the graph path ran with no
/// overrun, skip or protocol fault. `pooled`: phase 1 is wide enough for the pool.
#[track_caller]
fn assert_equivalent(name: &str, s: &Session, blocks: usize, pooled: bool) {
    let serial = render(s, Path::Serial, blocks);
    assert!(serial.faults.is_none());
    for workers in [2, 0] {
        let graph = render(s, Path::Graph(workers), blocks);
        let f = graph.faults.unwrap();
        if pooled {
            assert_eq!(f.blocks, blocks as u64, "{name}: every block through the pool ({workers} workers)");
        }
        assert_eq!((f.overruns(), f.upstream_missed, f.conflicts, f.faulted), (0, 0, 0, 0), "{name}, {workers} workers: {f:?}");
        assert_eq!(graph.latency, serial.latency, "{name}: the device latency");
        assert!(bit_exact(&graph.all, &serial.all), "{name}: the graph ({workers} workers) and serial renders are bit-exact");
    }
}

fn the_key_holds(s: &Session) -> bool {
    s.tracks.iter().any(|t| t.mixer.inserts.iter().flatten().any(|i| i.key.is_some()))
}

/// Gate (equivalence): the keyed sessions of `sidechain.rs`.
#[test]
fn serial_and_graph_renders_of_the_keyed_sessions_are_bit_exact() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let blocks = LEN / BLOCK;
    for (name, s) in [
        ("kick to bass, post", kick_to_bass(false, Tap::Post)),
        ("kick to bass, pre, bass first", kick_to_bass(true, Tap::Pre)),
        ("a chain of two keys", chain()),
        ("a bus key onto an aux", bus_key_onto_an_aux()),
        ("a late key", latent_kick_to_bass(128, 0, 0)),
        ("an early key", latent_kick_to_bass(0, 128, 0)),
        ("latency behind the keyed insert", latent_kick_to_bass(0, 0, 128)),
        ("both latent, the key early", latent_kick_to_bass(64, 128, 0)),
        ("both latent, the key late", latent_kick_to_bass(192, 128, 32)),
    ] {
        assert!(the_key_holds(&s), "{name}");
        assert_equivalent(name, &s, blocks, true);
    }
}

/// The corpus sessions with a key edge added: a compressor on the last audio or instrument track,
/// keyed from the first.
fn with_a_key(mut s: Session) -> Session {
    let live: Vec<TrackId> =
        s.tracks.iter().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) && !t.inactive).map(|t| t.id).collect();
    if let (Some(&from), Some(&to)) = (live.first(), live.last())
        && from != to
        && let Some(slot) = s.track_mut(to).unwrap().mixer.inserts.iter_mut().find(|x| x.is_none())
    {
        *slot = Some(ducker(key_from(from, Tap::Post)));
    }
    s
}

/// Gate (equivalence): the corpus sessions, as saved (no key: the plain pool path) and with a key
/// edge added (the graph path).
#[test]
fn serial_and_graph_renders_of_the_corpus_sessions_are_bit_exact() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/corpus");
    let mut files: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "transit")).collect();
    files.sort();
    assert!(files.len() >= 4, "{files:?}");
    let mut keyed = 0;
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let s = Session::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let wide = s.tracks.iter().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument) && !t.inactive).count() >= 4;
        assert_equivalent(&name, &s, 200, wide);
        let k = with_a_key(s);
        if the_key_holds(&k) {
            keyed += 1;
            assert_equivalent(&format!("{name} with a key"), &k, 200, wide);
        }
    }
    assert!(keyed >= 4, "a key edge in every released session");
}

/// The duck's onset: the first sample after the latency (plus a margin) where the bass fell
/// below half.
fn onset(out: &[f32], latency: usize) -> usize {
    let from = latency + 1_000;
    out[from..].iter().position(|v| v.abs() < BASS * 0.5).expect("the bass ducks") + from - latency
}

/// Gate (sidechain impulse alignment): a late key, an early key, and latency on either side of
/// the keyed insert give the duck on the kick's own sample under the graph, as serially.
#[test]
fn the_key_meets_its_signal_on_the_same_sample_under_the_graph() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let blocks = LEN / BLOCK;
    let reference = render(&latent_kick_to_bass(0, 0, 0), Path::Graph(2), blocks);
    let want = onset(&reference.left, reference.latency);
    assert!((KICKS[0]..KICKS[0] + 200).contains(&want), "the reference ducks right at the kick: {want}");
    for (kick, before, after, device) in [(128, 0, 0, 128), (0, 128, 0, 128), (0, 0, 128, 128), (64, 128, 0, 128), (192, 128, 32, 224)] {
        let s = latent_kick_to_bass(kick, before, after);
        let serial = render(&s, Path::Serial, blocks);
        let graph = render(&s, Path::Graph(2), blocks);
        assert_eq!((graph.latency, serial.latency), (device, device), "{kick}/{before}/{after}: the device latency");
        assert_eq!(onset(&graph.left, graph.latency), want, "{kick}/{before}/{after}: the duck starts on the kick's own sample");
        assert_eq!(onset(&serial.left, serial.latency), want);
        assert!(bit_exact(&graph.left, &serial.left), "{kick}/{before}/{after}");
    }
}

// ---- Through the continuity instruments ----

const FRAMES: usize = 128;
const INSTRUMENTS: usize = 6;

/// Six continuity instruments; 1 and 2 keyed from 0, 4 keyed from 3 (compressors on their first
/// insert); 5 unrelated.
fn keyed_rig(workers: Option<usize>, deadline: Duration) -> Option<Rig> {
    let (mut s, ids) = common::workers::session(INSTRUMENTS, 0);
    for (to, from) in [(1, 0), (2, 0), (4, 3)] {
        s.track_mut(ids[to]).unwrap().mixer.inserts[0] = Some(ducker(key_from(ids[from], Tap::Post)));
    }
    let mut rig = Rig::with_session(s, ids, FRAMES, |_| None);
    let started = match workers {
        Some(w) => rig.mix.start_render_workers_with(64, w),
        None => rig.mix.start_render_workers(64),
    };
    if started == 0 && workers != Some(0) {
        return None;
    }
    rig.mix.set_strip_deadline(Some(deadline));
    Some(rig)
}

fn settle(rig: &mut Rig) {
    let start = Instant::now();
    while !rig.mix.render_workers_settled() {
        assert!(start.elapsed() < Duration::from_secs(10), "a worker never returned");
        // timing: sleep-ok paces the blocks while a stalled worker finishes; measures nothing
        std::thread::sleep(Duration::from_millis(1));
        rig.block();
    }
    for _ in 0..8 {
        rig.block();
    }
}

fn graph_on() {
    MixEngine::force_serial(false);
    MixEngine::force_graph(true);
}

fn graph_off() {
    MixEngine::force_graph(false);
    MixEngine::force_serial(true);
}

/// Gate 4 through the graph: live notes and pedals on six instruments, worker stalls on the key
/// sources and elsewhere. No note or pedal is left stuck; every hole in an instrument's time is a
/// counted busy or unstarted overrun or an upstream-missed skip; the take journal counts only the
/// strips' own overruns.
#[test]
fn continuity_across_upstream_missed_skips() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    graph_on();
    let Some(rig) = keyed_rig(None, Duration::from_micros(1500)) else {
        graph_off();
        return;
    };
    let rig = std::thread::spawn(move || {
        let mut rig = rig;
        become_callback();
        let mut player = Player::new(INSTRUMENTS, 174);
        let mut rng = Rng::new(174 ^ 0x5eed);
        let period = Duration::from_nanos(FRAMES as u64 * 1_000_000_000 / u64::from(common::workers::SR));
        let first = Instant::now();
        for b in 0..1500u64 {
            if let Some(wait) = (first + period * u32::try_from(b).unwrap()).checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            player.play(&mut rig, b);
            if rng.below(25) == 0 {
                // The key sources twice as often as the rest.
                let k = [0, 3, 0, 3, 1, 2, 4, 5][usize::try_from(rng.below(8)).unwrap()];
                rig.shared[k].stall_ns.store(2_000_000 + rng.below(6_000_000), Ordering::Relaxed);
            }
            rig.block();
        }
        for s in &rig.shared {
            s.stall_ns.store(0, Ordering::Relaxed);
        }
        player.release_all(&mut rig);
        settle(&mut rig);
        transit_mix::unmark_audio_thread();
        rig
    })
    .join()
    .unwrap();
    graph_off();
    let f = rig.mix.strip_faults().unwrap().snapshot();
    println!("{f:?}");
    let reports: Vec<_> = rig.shared.iter().map(|s| s.report()).collect();
    for (k, (r, s)) in reports.iter().zip(&rig.shared).enumerate() {
        assert_eq!((r.stuck_notes, r.stuck_pedals), (0, 0), "instrument {k}: {r:?}");
        let x = r.faults;
        assert_eq!((x.orphan_offs, x.overlaps, x.unsorted, x.out_of_block, x.restrikes, x.zero_length), (0, 0, 0, 0, 0, 0), "instrument {k}: {r:?}");
        assert_eq!(s.reentries.load(Ordering::Relaxed), 0, "instrument {k}");
    }
    assert_eq!(f.conflicts, 0, "{f:?}");
    assert!(f.overruns_late > 0 && f.upstream_missed > 0, "the stalls overran and skipped dependents: {f:?}");
    let hole_samples: u64 = reports.iter().map(|r| r.hole_samples).sum();
    assert_eq!(
        hole_samples,
        (f.overruns_busy + f.overruns_unstarted + f.upstream_missed) * FRAMES as u64,
        "every hole is a counted busy or unstarted overrun or an upstream-missed skip: {f:?}"
    );
    assert_eq!(rig.marked, f.overrun_blocks, "the take journal marks the blocks with an overrun of a strip's own");
    let listed: u64 = f.top_strips.iter().flatten().map(|s| s.upstream_missed).sum();
    assert_eq!(listed, f.upstream_missed, "every skip is listed with its strip: {:?}", f.top_strips);
    assert!(
        f.top_strips.iter().flatten().filter(|s| s.upstream_missed > 0).all(|s| [0, 3].contains(&s.upstream_origin)),
        "named with their origin, a key source: {:?}",
        f.top_strips
    );
    let retired = rig.mix.into_retired();
    assert_eq!(retired.instances().len(), INSTRUMENTS + 3, "every strip came home (the instruments and the three compressors)");
}

/// Gate 3 through the graph: the callback allocates and frees nothing over a keyed plan with
/// stalls, overruns, upstream-missed skips, inline renders and live MIDI.
#[test]
fn the_callback_allocates_nothing_through_the_graph() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    graph_on();
    let Some(rig) = keyed_rig(None, Duration::from_micros(800)) else {
        graph_off();
        return;
    };
    let (stats, f) = std::thread::spawn(move || {
        let mut rig = rig;
        become_callback();
        let mut player = Player::new(INSTRUMENTS, 9);
        let mut rng = Rng::new(99);
        for b in 0..200 {
            player.play(&mut rig, b);
            rig.block();
        }
        let ((), stats) = measure(|| {
            for b in 200..1400u64 {
                player.play(&mut rig, b);
                if rng.below(15) == 0 {
                    let k = [0, 3, 1, 5][usize::try_from(rng.below(4)).unwrap()];
                    rig.shared[k].stall_ns.store(1_000_000 + rng.below(3_000_000), Ordering::Relaxed);
                }
                rig.block();
                if b % 3 == 0 {
                    // timing: sleep-ok gives a stalled worker room; measures nothing
                    std::thread::sleep(Duration::from_micros(500));
                }
            }
        });
        for s in &rig.shared {
            s.stall_ns.store(0, Ordering::Relaxed);
        }
        player.release_all(&mut rig);
        settle(&mut rig);
        transit_mix::unmark_audio_thread();
        let f = rig.mix.strip_faults().unwrap().snapshot();
        drop(rig.mix.into_retired());
        (stats, f)
    })
    .join()
    .unwrap();
    graph_off();
    println!("{f:?}");
    assert!(f.overruns() > 0 && f.upstream_missed > 0 && f.midi_deferred + f.inline_renders > 0, "the paths ran: {f:?}");
    assert!(stats.is_zero(), "the callback allocated or freed: {stats:?}");
}

std::thread_local! {
    /// Set on the test's callback thread: a hold applies only on a render worker.
    static ON_CALLBACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A gain stage that, armed, holds a render worker inside `process` until released; and, as the
/// pinned strip's insert, waits on the callback until the hold is entered (when asked) and the
/// block's deadline has passed.
struct Gate {
    inner: Box<dyn Plugin>,
    holds: bool,
    hold: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    wait: Arc<Mutex<Option<(Instant, bool)>>>,
}

impl Plugin for Gate {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let t0 = Instant::now();
        if self.holds && self.hold.load(Ordering::Acquire) && !ON_CALLBACK.get() {
            self.entered.store(true, Ordering::Release);
            while self.hold.load(Ordering::Acquire) {
                assert!(t0.elapsed() < Duration::from_secs(60), "never released");
                // timing: sleep-ok paces a wait for a release flag, measures nothing
                std::thread::sleep(Duration::from_millis(1));
            }
            self.entered.store(false, Ordering::Release);
        }
        if !self.holds
            && let Some((deadline, need_entered)) = *self.wait.lock().unwrap()
        {
            while (need_entered && !self.entered.load(Ordering::Acquire)) || Instant::now() < deadline {
                assert!(t0.elapsed() < Duration::from_secs(60), "the hold was never entered");
                std::hint::spin_loop();
            }
        }
        self.inner.process(io, frames);
    }
}

/// Gate (deterministic late workers, deferred MIDI): instrument 1 is keyed from instrument 0,
/// whose insert holds the one render worker; a monitored audio track (pinned to the callback)
/// waits until the hold is entered and the deadline has passed. A note-on for instrument 1 in the
/// late block and its note-off in the busy block are held (instrument 1 is skipped twice, its
/// time has two holes) and delivered at its next processed block, the on at offset 0 and the off
/// one sample later; nothing is stuck.
#[test]
fn a_skipped_dependent_s_midi_is_held_and_delivered() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (mut s, ids) = common::workers::session(3, 1);
    s.track_mut(ids[0]).unwrap().mixer.inserts[0] = Some({
        let mut g = Insert::new("gain");
        g.preset = "hold".into();
        g
    });
    s.track_mut(ids[1]).unwrap().mixer.inserts[0] = Some(ducker(key_from(ids[0], Tap::Post)));
    let pad = s.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap().id;
    let p = s.track_mut(pad).unwrap();
    p.mixer.input_monitor = true;
    p.mixer.inserts[0] = Some({
        let mut g = Insert::new("gain");
        g.preset = "wait".into();
        g
    });
    let (hold, entered, wait) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
    let (h, e, w) = (Arc::clone(&hold), Arc::clone(&entered), Arc::clone(&wait));
    let gates = s.clone();
    let rig = Rig::with_session(s, ids, FRAMES, move |spec| {
        let preset = gates.track(spec.track)?.mixer.inserts.get(spec.slot)?.as_ref()?.preset.clone();
        let holds = match preset.as_str() {
            "hold" => true,
            "wait" => false,
            _ => return None,
        };
        let inner = create_instance("gain", None, common::workers::SR as f32, FRAMES, spec.ch)?.plugin;
        let gate = Gate { inner, holds, hold: Arc::clone(&h), entered: Arc::clone(&e), wait: Arc::clone(&w) };
        Some(NewInstance { plugin: Box::new(gate), refused_state: false })
    });
    graph_on();
    let mut rig = rig;
    assert_eq!(rig.mix.start_render_workers_with(16, 1), 1);
    let (rig, (on_block, heard)) = std::thread::spawn(move || {
        let mut rig = rig;
        become_callback();
        ON_CALLBACK.set(true);
        rig.mix.set_strip_deadline(Some(FAR));
        for _ in 0..10 {
            rig.block();
        }
        let heard = rig.shared[1].inst.lock().unwrap().trace().len();
        let deadline_block = |rig: &mut Rig, need_entered: bool| {
            let d = Instant::now() + Duration::from_millis(5);
            *wait.lock().unwrap() = Some((d, need_entered));
            rig.mix.set_strip_deadline(None);
            rig.mix.set_block_deadline(Some(d));
            rig.block();
            *wait.lock().unwrap() = None;
        };
        hold.store(true, Ordering::Release);
        assert!(rig.note_on(1, 10, 60).accepted());
        deadline_block(&mut rig, true);
        assert!(rig.note_off(1, 20, 60).accepted());
        deadline_block(&mut rig, false);
        let f = rig.mix.strip_faults().unwrap().snapshot();
        assert_eq!((f.overruns_late, f.overruns_busy, f.upstream_missed), (1, 1, 2), "{f:?}");
        assert_eq!(rig.shared[1].inst.lock().unwrap().trace().len(), heard, "nothing delivered to the skipped instrument yet");
        hold.store(false, Ordering::Release);
        let t0 = Instant::now();
        while !rig.mix.render_workers_settled() {
            assert!(t0.elapsed() < Duration::from_secs(60), "the worker never returned");
            std::thread::yield_now();
        }
        rig.mix.set_block_deadline(None);
        rig.mix.set_strip_deadline(Some(FAR));
        let on_block = u64::try_from(rig.pos).unwrap();
        rig.block();
        transit_mix::unmark_audio_thread();
        (rig, (on_block, heard))
    })
    .join()
    .unwrap();
    graph_off();
    let inst = rig.shared[1].inst.lock().unwrap();
    let trace: Vec<(u64, u8, u8)> = inst.trace()[heard..].iter().map(|l| (l.sample, l.event.status & 0xF0, l.event.data1)).collect();
    assert_eq!(trace, [(on_block, 0x90, 60), (on_block + 1, 0x80, 60)], "held, then delivered at offset 0, the off one sample later");
    let r = inst.report();
    assert_eq!((r.holes, r.hole_samples, r.stuck_notes, r.faults.zero_length), (1, 2 * FRAMES as u64, 0, 0), "{r:?}");
    drop(inst);
    let f = rig.mix.strip_faults().unwrap().snapshot();
    assert_eq!(f.conflicts, 0);
    drop(rig.mix.into_retired());
}
