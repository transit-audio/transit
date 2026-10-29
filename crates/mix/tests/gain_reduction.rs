//! The gain reduction of a sidechained compressor over time, at the signal level, serially and
//! through the task graph (issue #179, a prerequisite of the keyed default flip, #174).
//!
//! A key made of bursts at known levels and times (silence between) drives the built-in
//! compressor (threshold -20 dB, ratio 4, hard knee, attack 1 ms, release 20 ms; without and with
//! lookahead) on a steady carrier, so the output's amplitude is the gain. The expected trace comes
//! from the detector's own equations and constants (`crates/dsp/src/plugins/dynamics.rs`):
//!
//! - level: the stereo-linked peak `max |key[c][n]|`, in dB `20 log10(level)` floored at -144;
//! - static curve (hard knee): `target = max(0, level_db - threshold) * (1 - 1 / ratio)`;
//! - ballistics: `gr = target + (gr - target) * c`, `c = attack` when `target > gr`, else
//!   `release`, each `exp(-1 / (ms * 0.001 * sr))`;
//! - gain: `10^(-gr / 20)`; with lookahead `la` the gain computed on key sample `n` is applied to
//!   the input sample `n - la` (the plug-in reports `la` as its latency).
//!
//! The measured trace is `-20 log10(out / carrier)` on main's left channel, aligned by the latency
//! the plan reports, and is asserted sample by sample within [`TOL_DB`] on every keyed topology of
//! `sidechain.rs` and `render_graph.rs` plus the cases #179 lists, in Serial, Graph(2) and
//! Graph(0), the three renders bit-exact. Each case also shows it can fail: the same trace against
//! the oracle fed the key one block early and one block late (a stale, previous block) misses by
//! more than [`DISCRIMINATES_DB`], and one sample either way by more than [`ONE_SAMPLE_DB`].
//!
//! The graph's switch and the serial switch are process-wide, so the tests take turns; renders
//! use the test's own clock (blocks back to back) and a far strips' deadline, so no outcome
//! depends on scheduling (E49).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_audio_io::AudioBuffer;
use transit_dsp::Plugin;
use transit_mix::{
    Adoption, DeviceGeneration, InstanceSpec, MixEngine, NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderTarget, StripFaults,
    create_instance,
};
use transit_model::{
    Bus, BusId, ChannelFormat, Clip, HardwareLatency, Insert, IoMap, KeyFrom, KeySource, Route, Session, SourceAudio, SourceId, Tap, TrackId,
    TrackKind,
};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

const SR: u32 = 48_000;
const BLOCK: usize = 256;
/// The measured span of the timeline (samples).
const LEN: usize = 18_000;
/// Rendered past `LEN` so the largest latency here (a 700-frame hardware round trip) fits.
const BLOCKS: usize = (LEN + 1_024).div_ceil(BLOCK);
/// The key: (start, length, level). Starts off the block grid; a burst shorter than the attack;
/// one long enough to settle; one under the threshold (-21.9 dB), which must not move the gain.
const BURSTS: [(usize, usize, f32); 6] =
    [(1_700, 700, 0.9), (4_100, 300, 0.3), (6_000, 1_500, 0.6), (9_000, 40, 1.0), (10_500, 800, 0.08), (13_000, 500, 0.9)];
/// The steady carrier on the compressed strip.
const CARRIER: f32 = 0.25;
const THRESHOLD: f32 = -20.0;
const RATIO: f32 = 4.0;
const ATTACK_MS: f32 = 1.0;
const RELEASE_MS: f32 = 20.0;
/// Sample-by-sample tolerance. The oracle runs the detector's recursion in f64 against the
/// plug-in's f32, and the measurement divides two f32 samples: the gap measured on every case is
/// 0.000025 dB, forty times under this. A one-sample shift of the key moves the trace by about
/// 0.3 dB on an attack edge (and ~0.015 dB per sample in the release): see [`ONE_SAMPLE_DB`].
const TOL_DB: f64 = 0.001;
/// A key one block early or late misses the trace by at least this much somewhere.
const DISCRIMINATES_DB: f64 = 1.0;
/// A key one sample early or late misses the trace by at least this much somewhere (five times
/// the tolerance; the chain of two keys, where the second detector smooths the first's output,
/// is the least sensitive case at 0.0095 dB).
const ONE_SAMPLE_DB: f64 = 0.005;
/// A strips' deadline no render reaches: `finish` waits for every worker.
const FAR: Duration = Duration::from_secs(60);

// ---- Sessions ----

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

fn key_signal() -> Vec<f32> {
    let mut v = vec![0.0; LEN];
    for (at, len, level) in BURSTS {
        v[at..at + len].fill(level);
    }
    v
}

fn bus(s: &mut Session, name: &str) -> BusId {
    let id = BusId(s.alloc());
    s.busses.push(Bus { id, name: name.into(), format: ChannelFormat::Stereo });
    id
}

fn aux(s: &mut Session, name: &str, input: BusId, output: Route) -> TrackId {
    let a = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some(name));
    let t = s.track_mut(a).unwrap();
    t.mixer.input = Route::Bus(input);
    t.mixer.output = output;
    t.mixer.pan = vec![-1.0, 1.0];
    a
}

/// Four quiet tracks outside the mix, so phase 1 is wide enough for the render workers.
fn fillers(s: &mut Session) {
    for i in 0..4 {
        let sig: Vec<f32> = (0..LEN).map(|k| 0.01 * ((k + 97 * i) as f32 * 0.003).sin()).collect();
        audio(s, &format!("Filler {i}"), sig, Route::None);
    }
}

fn compressor(params: &[(&str, f32)]) -> Insert {
    let mut c = Insert::new("compressor");
    for p in transit_dsp::plugin_info("compressor").unwrap().params {
        c.params.insert(p.id.to_string(), p.default);
    }
    for (k, v) in params {
        c.params.insert((*k).into(), *v);
    }
    c
}

/// The compressor under test, keyed by `key`, with `lookahead_ms` of lookahead.
fn ducker(key: KeySource, lookahead_ms: f32) -> Option<Insert> {
    let mut c = compressor(&[
        ("threshold", THRESHOLD),
        ("ratio", RATIO),
        ("attack", ATTACK_MS),
        ("release", RELEASE_MS),
        ("knee", 0.0),
        ("makeup", 0.0),
        ("mix", 100.0),
        ("lookahead", lookahead_ms),
    ]);
    c.key = Some(key);
    Some(c)
}

fn track_key(t: TrackId, tap: Tap) -> KeySource {
    KeySource { from: KeyFrom::Track(t), tap }
}

/// A lookahead effect that leaves the level alone: the built-in compressor at ratio 1 (no gain
/// reduction at any level), `ms` of lookahead, and `wet` percent of mix (both legs through its
/// lookahead delay; it reports the lookahead as its latency).
fn lookahead_effect(ms: f32, wet: f32) -> Option<Insert> {
    Some(compressor(&[("ratio", 1.0), ("lookahead", ms), ("mix", wet)]))
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

// ---- The render (the live engine path, as in `render_graph.rs`) ----

#[derive(Clone, Copy, Debug)]
enum Path {
    Serial,
    /// The task graph through this many workers (0: the callback renders every strip inline).
    Graph(usize),
}

struct Rendered {
    all: Vec<f32>,
    left: Vec<f32>,
    latency: usize,
    faults: Option<StripFaults>,
}

/// A fake duplex device: its sinks play back on its input this many frames later.
struct Duplex {
    io: IoMap,
    round_trip: usize,
}

/// `BLOCKS` blocks of `s` through `render_active` on a thread marked as the audio thread, along
/// `path`; through the `duplex` device when given.
fn render(s: &Session, path: Path, duplex: Option<&Duplex>) -> Rendered {
    let main = transit_mix::main_channels(s).max(2);
    let dev_ch = if duplex.is_some() { 4 } else { 0 };
    let t = RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: dev_ch, in_channels: dev_ch };
    let mut mix = MixEngine::realtime(t);
    let mut compiler = PlanCompiler::new(t, PlanLimits::default());
    if let Some(d) = duplex {
        compiler.set_io(Arc::new(d.io.clone()));
    }
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
    let (dest, sink_ch, round_trip) = match duplex {
        Some(d) => {
            let sinks = transit_mix::plan_sinks(s, &d.io, 4);
            (sinks.destinations(), sinks.channels(), d.round_trip)
        }
        None => (Vec::new(), 0, 0),
    };
    let looped = duplex.is_some();
    MixEngine::force_serial(matches!(path, Path::Serial));
    MixEngine::force_graph(matches!(path, Path::Graph(_)));
    let (mix, all, left) = std::thread::spawn(move || {
        let mut mix = mix;
        transit_mix::mark_audio_thread();
        let mut out = vec![vec![0.0f32; BLOCK]; main + sink_ch];
        let mut device: Vec<Vec<f32>> = vec![vec![0.0; BLOCKS * BLOCK]; 4];
        let (mut all, mut left) = (Vec::with_capacity(BLOCKS * BLOCK * main), Vec::with_capacity(BLOCKS * BLOCK));
        for b in 0..BLOCKS {
            let pos = b * BLOCK;
            if looped {
                mix.input = device.iter().map(|c| (0..BLOCK).map(|i| (pos + i).checked_sub(round_trip).map_or(0.0, |k| c[k])).collect()).collect();
            }
            mix.render_active(pos as i64, BLOCK, &mut out);
            for c in &out[..main] {
                all.extend_from_slice(c);
            }
            left.extend_from_slice(&out[0]);
            for (k, &d) in dest.iter().enumerate() {
                for i in 0..BLOCK {
                    device[usize::from(d)][pos + i] += out[main + k][i];
                }
            }
        }
        transit_mix::unmark_audio_thread();
        (mix, all, left)
    })
    .join()
    .unwrap();
    MixEngine::force_graph(false);
    MixEngine::force_serial(true);
    assert_eq!(mix.status().capacity_exceeded, 0, "the plan's delay lines hold every key's delay");
    let faults = mix.strip_faults().map(|r| r.snapshot());
    let latency = mix.latency;
    drop(mix.into_retired());
    Rendered { all, left, latency, faults }
}

fn bit_exact(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

// ---- The oracle: the detector's equations ----

/// The one-pole coefficient as the plug-in computes it (`time_coef`, in f32).
fn coef(ms: f32) -> f64 {
    let n = ms * 0.001 * SR as f32;
    f64::from((-1.0f32 / n).exp())
}

/// The gain reduction (dB) at each timeline sample of `0..LEN` for the detector keyed by `key`
/// (the key as it reaches the compressor, on the timeline) with `la` samples of lookahead.
fn detector(key: &[f64], la: usize) -> Vec<f64> {
    let (att, rel) = (coef(ATTACK_MS), coef(RELEASE_MS));
    let slope = 1.0 - 1.0 / f64::from(RATIO);
    let mut gr = 0.0f64;
    let mut trace = vec![0.0; LEN];
    for n in 0..LEN + la {
        let level = key.get(n).copied().unwrap_or(0.0).abs();
        let level_db = if level > 0.0 { (20.0 * level.log10()).max(-144.0) } else { -144.0 };
        let target = (level_db - f64::from(THRESHOLD)).max(0.0) * slope;
        let c = if target > gr { att } else { rel };
        gr = target + (gr - target) * c;
        if n >= la {
            trace[n - la] = gr;
        }
    }
    trace
}

/// The key on the timeline, `shift` samples late (negative: early), scaled by `gain`.
fn key_at(gain: f64, shift: isize) -> Vec<f64> {
    let k = key_signal();
    (0..LEN as isize).map(|p| usize::try_from(p - shift).ok().and_then(|i| k.get(i)).map_or(0.0, |&v| f64::from(v) * gain)).collect()
}

/// What a case's compressor should do.
#[derive(Clone, Copy)]
enum Expect {
    /// One compressor keyed by the bursts at `gain`, with `la` samples of lookahead.
    One { gain: f64, la: usize },
    /// Two keys in a chain: the bursts duck a steady `level` (the second key), which drives the
    /// second compressor.
    Chain { level: f64 },
}

fn expected(e: Expect, shift: isize) -> Vec<f64> {
    match e {
        Expect::One { gain, la } => detector(&key_at(gain, shift), la),
        Expect::Chain { level } => {
            let first = detector(&key_at(1.0, shift), 0);
            let second: Vec<f64> = first.iter().map(|gr| level * 10f64.powf(-gr / 20.0)).collect();
            detector(&second, 0)
        }
    }
}

/// The measured gain reduction (dB) at each timeline sample: main's left channel aligned by the
/// device latency, against the carrier.
fn measured(left: &[f32], latency: usize) -> Vec<f64> {
    left[latency..latency + LEN].iter().map(|&v| -20.0 * (f64::from(v) / f64::from(CARRIER)).log10()).collect()
}

/// The largest error and where.
fn worst(got: &[f64], want: &[f64]) -> (f64, usize) {
    got.iter().zip(want).enumerate().map(|(i, (g, w))| ((g - w).abs(), i)).fold((0.0, 0), |a, b| if b.0 > a.0 || b.0.is_nan() { b } else { a })
}

/// The shift (samples, positive: the trace is late) that brings `got` closest to `want`.
fn best_shift(got: &[f64], want: &[f64]) -> (isize, f64) {
    (-600..=600isize)
        .map(|d| {
            let e = (1_000..LEN - 1_000).map(|p| (got[(p as isize + d) as usize] - want[p]).abs()).fold(0.0, f64::max);
            (d, e)
        })
        .fold((0, f64::INFINITY), |a, b| if b.1 < a.1 { b } else { a })
}

fn around(x: &[f64], at: usize) -> String {
    let lo = at.saturating_sub(6);
    let hi = (at + 7).min(x.len());
    x[lo..hi].iter().map(|v| format!("{v:.4}")).collect::<Vec<_>>().join(" ")
}

// ---- The check ----

struct Case {
    name: &'static str,
    s: Session,
    duplex: Option<Duplex>,
    /// The device latency the plan must report.
    device: usize,
    expect: Expect,
}

/// The case in Serial, Graph(2) and Graph(0): the device latency; the gain-reduction trace
/// sample by sample within `TOL_DB` of the detector's; the three renders bit-exact; and the trace
/// misses a key one block early and one block late by at least `DISCRIMINATES_DB`.
#[track_caller]
fn check(case: &Case) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let name = case.name;
    assert!(case.s.tracks.iter().any(|t| t.mixer.inserts.iter().flatten().any(|i| i.key.is_some())), "{name}: keyed");
    let want = expected(case.expect, 0);
    assert!(want.iter().fold(0.0f64, |m, &v| m.max(v)) > 6.0, "{name}: the key moves the gain");
    let serial = render(&case.s, Path::Serial, case.duplex.as_ref());
    let graphs: Vec<(usize, Rendered)> = [2, 0].into_iter().map(|w| (w, render(&case.s, Path::Graph(w), case.duplex.as_ref()))).collect();
    // The traces first, each aligned by the latency its own render reports: a misaligned key
    // fails here, with the evidence, before the latency and equivalence checks below.
    let mut traces = vec![("serial", measured(&serial.left, serial.latency))];
    for (w, graph) in &graphs {
        traces.push((if *w == 2 { "graph(2)" } else { "graph(0)" }, measured(&graph.left, graph.latency)));
    }
    for (mode, got) in &traces {
        let (err, at) = worst(got, &want);
        println!("{name}: {mode}: worst {err:.6} dB at {at}");
        if err > TOL_DB || err.is_nan() {
            let (shift, residual) = best_shift(got, &want);
            let others: Vec<String> = traces.iter().map(|(m, t)| format!("  {m}: {}", around(t, at))).collect();
            panic!(
                "{name}: {mode} misses the expected gain reduction by {err:.4} dB at timeline sample {at} (best fit: the trace {shift} \
                 samples late, residual {residual:.4} dB)\n  expected: {}\n{}",
                around(&want, at),
                others.join("\n")
            );
        }
    }
    assert!(serial.faults.is_none());
    assert_eq!(serial.latency, case.device, "{name}: the device latency");
    for (w, graph) in &graphs {
        let f = graph.faults.unwrap();
        assert_eq!((f.overruns(), f.upstream_missed, f.conflicts, f.faulted), (0, 0, 0, 0), "{name}, {w} workers: {f:?}");
        assert_eq!(graph.latency, case.device, "{name}, {w} workers: the device latency");
        assert!(bit_exact(&graph.all, &serial.all), "{name}: the graph ({w} workers) and serial renders are bit-exact");
    }
    let got = &traces[0].1;
    for (what, shift, by) in [
        ("one block early", -(BLOCK as isize), DISCRIMINATES_DB),
        ("one block late (the previous block)", BLOCK as isize, DISCRIMINATES_DB),
        ("one sample early", -1, ONE_SAMPLE_DB),
        ("one sample late", 1, ONE_SAMPLE_DB),
    ] {
        let (err, _) = worst(got, &expected(case.expect, shift));
        assert!(err > by, "{name}: a key {what} would pass ({err:.4} dB)");
    }
}

/// A key track (the bursts, outside the mix, `kick` samples of latency) keying the compressor on
/// a steady carrier: `before` samples of latency ahead of it, `after` behind it.
fn latent(kick: usize, before: usize, after: usize, lookahead_ms: f32) -> Session {
    let mut s = session();
    fillers(&mut s);
    let k = audio(&mut s, "Key", key_signal(), Route::None);
    let c = audio(&mut s, "Carrier", vec![CARRIER; LEN], Route::Main);
    if kick > 0 {
        s.track_mut(k).unwrap().mixer.inserts[0] = delay(kick);
    }
    let ct = s.track_mut(c).unwrap();
    if before > 0 {
        ct.mixer.inserts[0] = delay(before);
    }
    ct.mixer.inserts[1] = ducker(track_key(k, Tap::Post), lookahead_ms);
    if after > 0 {
        ct.mixer.inserts[2] = delay(after);
    }
    s
}

fn track(s: &Session, name: &str) -> TrackId {
    s.tracks.iter().find(|t| t.name == name).unwrap().id
}

const PLAIN: Expect = Expect::One { gain: 1.0, la: 0 };

fn case(name: &'static str, s: Session, device: usize, expect: Expect) -> Case {
    Case { name, s, duplex: None, device, expect }
}

// ---- The cases ----

#[test]
fn the_plain_key() {
    check(&case("the plain key", latent(0, 0, 0, 0.0), 0, PLAIN));
}

#[test]
fn a_late_key() {
    check(&case("a late key (128 on the key)", latent(128, 0, 0, 0.0), 128, PLAIN));
}

#[test]
fn an_early_key() {
    check(&case("an early key (128 ahead of the compressor)", latent(0, 128, 0, 0.0), 128, PLAIN));
}

#[test]
fn latency_behind_the_keyed_insert() {
    check(&case("128 behind the compressor", latent(0, 0, 128, 0.0), 128, PLAIN));
}

#[test]
fn latency_before_and_after_the_keyed_insert() {
    check(&case("64 on the key, 128 ahead", latent(64, 128, 0, 0.0), 128, PLAIN));
    check(&case("192 on the key, 128 ahead, 32 behind", latent(192, 128, 32, 0.0), 224, PLAIN));
}

/// A device latency: the carrier goes out to an outboard unit and back (a measured 700-frame round
/// trip) in slot 0 ahead of the compressor, whose key has no latency.
#[test]
fn a_hardware_insert_ahead_of_the_keyed_insert() {
    const D: usize = 700;
    let (send, ret) = ("Out 3-4", "In 3-4");
    let mut io = IoMap::for_device("test", "fake duplex", 4, 4);
    io.set_hardware_latency(HardwareLatency { send: send.into(), ret: ret.into(), round_trip_frames: D as u32, rate: SR, measured_at: 1 });
    let mut s = latent(0, 0, 0, 0.0);
    let c = track(&s, "Carrier");
    s.track_mut(c).unwrap().mixer.inserts[0] = Some(Insert::hardware(send, ret));
    check(&Case { name: "a hardware insert (700 round trip)", s, duplex: Some(Duplex { io, round_trip: D }), device: D, expect: PLAIN });
}

/// Key (64) -> Key Bus -> Drums aux (32) -> Drum Out bus, which keys the carrier's aux.
#[test]
fn a_key_through_a_latent_bus_chain() {
    let mut s = session();
    fillers(&mut s);
    let (kb, ob, cb) = (bus(&mut s, "Key Bus"), bus(&mut s, "Drum Out"), bus(&mut s, "Carrier Bus"));
    let k = audio(&mut s, "Key", key_signal(), Route::Bus(kb));
    audio(&mut s, "Carrier", vec![CARRIER; LEN], Route::Bus(cb));
    s.track_mut(k).unwrap().mixer.inserts[0] = delay(64);
    let d = aux(&mut s, "Drums", kb, Route::Bus(ob));
    s.track_mut(d).unwrap().mixer.inserts[0] = delay(32);
    let a = aux(&mut s, "Carrier Aux", cb, Route::Main);
    s.track_mut(a).unwrap().mixer.inserts[0] = ducker(KeySource { from: KeyFrom::Bus(ob), tap: Tap::Post }, 0.0);
    check(&case("a key through a latent bus chain (64 + 32)", s, 96, PLAIN));
}

/// #179's first case: the keyed compressor in slot 1 after a 5 ms lookahead effect in slot 0; the
/// key meets the carrier as it arrives at slot 1.
#[test]
fn a_key_after_a_lookahead_effect() {
    let mut s = latent(0, 0, 0, 0.0);
    let c = track(&s, "Carrier");
    s.track_mut(c).unwrap().mixer.inserts[0] = lookahead_effect(5.0, 100.0);
    check(&case("a key after a 5 ms lookahead effect", s, 240, PLAIN));
}

/// The keyed compressor's own 3 ms (144-sample) lookahead: the gain moves 144 samples ahead of the
/// key on the timeline; alone and with a late key.
#[test]
fn a_keyed_compressor_with_lookahead() {
    check(&case("a 3 ms lookahead keyed compressor", latent(0, 0, 0, 3.0), 144, Expect::One { gain: 1.0, la: 144 }));
    check(&case("a 3 ms lookahead keyed compressor, the key 128 late", latent(128, 0, 0, 3.0), 272, Expect::One { gain: 1.0, la: 144 }));
}

/// #179's second case: a partially wet (50 %) latent effect (2 ms lookahead, both legs delayed
/// inside it) ahead of the keyed compressor, and on the key.
#[test]
fn a_partially_wet_latent_effect_on_the_keyed_path() {
    let mut s = latent(0, 0, 0, 0.0);
    let c = track(&s, "Carrier");
    s.track_mut(c).unwrap().mixer.inserts[0] = lookahead_effect(2.0, 50.0);
    check(&case("a partially wet latent effect on the keyed path", s, 96, PLAIN));
}

#[test]
fn a_partially_wet_latent_effect_on_the_key_path() {
    let mut s = latent(0, 0, 0, 0.0);
    let k = track(&s, "Key");
    s.track_mut(k).unwrap().mixer.inserts[0] = lookahead_effect(2.0, 50.0);
    check(&case("a partially wet latent effect on the key path", s, 96, PLAIN));
}

/// A pre-fader key ignores the key track's fader (all the way down, the key 128 late); a
/// post-fader key follows it (-6 dB).
#[test]
fn pre_and_post_fader_keys() {
    let mut pre = latent(128, 0, 0, 0.0);
    let k = track(&pre, "Key");
    pre.track_mut(k).unwrap().mixer.volume_db = -80.0;
    let c = track(&pre, "Carrier");
    pre.track_mut(c).unwrap().mixer.inserts[1] = ducker(track_key(k, Tap::Pre), 0.0);
    check(&case("a pre-fader key from a silent fader", pre, 128, PLAIN));
    let mut post = latent(0, 0, 0, 0.0);
    let k = track(&post, "Key");
    post.track_mut(k).unwrap().mixer.volume_db = -6.0;
    check(&case("a post-fader key at -6 dB", post, 0, Expect::One { gain: f64::from(transit_dsp::db_to_gain(-6.0)), la: 0 }));
}

/// A key on a bus ducks an aux fed from another bus.
#[test]
fn a_bus_key_onto_an_aux() {
    let mut s = session();
    fillers(&mut s);
    let (kb, cb) = (bus(&mut s, "Key Bus"), bus(&mut s, "Carrier Bus"));
    audio(&mut s, "Key", key_signal(), Route::Bus(kb));
    audio(&mut s, "Carrier", vec![CARRIER; LEN], Route::Bus(cb));
    let a = aux(&mut s, "Carrier Aux", cb, Route::Main);
    s.track_mut(a).unwrap().mixer.inserts[0] = ducker(KeySource { from: KeyFrom::Bus(kb), tap: Tap::Post }, 0.0);
    check(&case("a bus key onto an aux", s, 0, PLAIN));
}

/// A chain of two keys: the bursts duck a steady 0.5 (-6 dB, outside the mix), whose pre-fader
/// signal keys the carrier's compressor (10.5 dB of reduction at rest, released under each burst).
#[test]
fn a_chain_of_two_keys() {
    const LEVEL: f32 = 0.5;
    let mut s = session();
    fillers(&mut s);
    let k = audio(&mut s, "Key", key_signal(), Route::None);
    let m = audio(&mut s, "Middle", vec![LEVEL; LEN], Route::None);
    let c = audio(&mut s, "Carrier", vec![CARRIER; LEN], Route::Main);
    s.track_mut(m).unwrap().mixer.inserts[0] = ducker(track_key(k, Tap::Post), 0.0);
    s.track_mut(c).unwrap().mixer.inserts[0] = ducker(track_key(m, Tap::Pre), 0.0);
    check(&case("a chain of two keys", s, 0, Expect::Chain { level: f64::from(LEVEL) }));
}
