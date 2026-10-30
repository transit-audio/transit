//! Shared by `render_workers.rs` (gates) and `render_headroom.rs` (the #124 measurement), as
//! `common::workers` (the test binaries get `common`'s counting allocator with it): an
//! instrument that feeds `transit_rt`'s continuity instrument from inside a real strip render,
//! with a reentry guard, injected worker stalls and a configurable cost, and a rig that drives a
//! realtime engine with render workers the way playback does (live MIDI through
//! `MixEngine::queue_event`, then `render_active`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, dead_code)]

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use transit_dsp::Plugin;
use transit_mix::{
    Adoption, Arrival, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, Message,
    MixEngine, NewInstance, NoteId, PlanCompiler, PlanGeneration, PlanLimits, PluginEvent, RenderFrame, RenderTarget, SlotBinding, SlotGen, Source,
    create_instance,
};
use transit_model::{ChannelFormat, Clip, Insert, Session, SourceAudio, SourceId, TrackId, TrackKind};
use transit_rt::midi_defer::Event;
use transit_rt::midi_defer::testing::{ContinuityInstrument, ContinuityReport};

pub const SR: u32 = 48_000;
/// The continuity instrument's plug-in id (handed to the factory).
pub const CONTINUITY: &str = "clap:test.continuity";

std::thread_local! {
    /// Set on the thread that plays the audio callback: a stall is injected only on a worker.
    static CALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// Marks the calling thread as the callback for [`ContinuityPlugin`]'s stalls (and as the audio
/// thread for the mix).
pub fn become_callback() {
    CALLBACK.set(true);
    transit_mix::mark_audio_thread();
}

/// One continuity instrument's shared state.
pub struct Shared {
    pub inst: Mutex<ContinuityInstrument>,
    inside: AtomicBool,
    /// Renders that found another render of the same instance under way (must stay 0).
    pub reentries: AtomicU64,
    /// Nanoseconds the next render on a worker sleeps (taken by that render).
    pub stall_ns: AtomicU64,
    /// While set, a render on a worker waits inside `process` until the test clears it (#213): a
    /// plug-in that does not return, held for as long as the test needs, whatever the host's speed.
    pub hold: AtomicBool,
    /// Set by a worker while it waits inside `process` on `hold`, cleared when it leaves: the
    /// completion signal a test waits on before it asserts that a worker holds this strip.
    pub holding: AtomicBool,
    /// Nanoseconds the next render sleeps on any thread, the callback included (taken by that
    /// render): a plug-in's one-time load on its first real render.
    pub load_ns: AtomicU64,
    /// Renders on a worker, and on the callback.
    pub on_worker: AtomicU64,
    pub on_callback: AtomicU64,
    /// Busy work per render, in sine evaluations per sample (a plug-in's cost).
    pub cost: AtomicU64,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared {
            inst: Mutex::new(ContinuityInstrument::new(1 << 16, 1 << 14)),
            inside: AtomicBool::new(false),
            reentries: AtomicU64::new(0),
            stall_ns: AtomicU64::new(0),
            hold: AtomicBool::new(false),
            holding: AtomicBool::new(false),
            load_ns: AtomicU64::new(0),
            on_worker: AtomicU64::new(0),
            on_callback: AtomicU64::new(0),
            cost: AtomicU64::new(0),
        })
    }

    pub fn report(&self) -> ContinuityReport {
        self.inst.lock().unwrap().report()
    }
}

/// An instrument that hands every block (transport position, length, events) to a
/// [`ContinuityInstrument`] and plays a quiet tone.
pub struct ContinuityPlugin {
    inner: Box<dyn Plugin>,
    shared: Arc<Shared>,
    transport: u64,
    events: Vec<Event>,
    phase: f32,
}

impl ContinuityPlugin {
    pub fn new(shared: Arc<Shared>, ch: usize, block: usize) -> ContinuityPlugin {
        let mut inner = transit_dsp::create("gain").unwrap();
        inner.prepare(SR as f32, block, ch);
        ContinuityPlugin { inner, shared, transport: 0, events: Vec::with_capacity(4096), phase: 0.0 }
    }
}

impl Plugin for ContinuityPlugin {
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
    fn set_transport(&mut self, transport: &transit_dsp::Transport) {
        self.transport = u64::try_from(transport.samples.max(0)).unwrap_or(0);
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        let ch = ev.channel & 0x0F;
        let e = match ev.message {
            Message::NoteOn { key, velocity, .. } => Event::note_on(ch, key, velocity, offset),
            Message::NoteOff { key, velocity, .. } => Event::note_off(ch, key, velocity, offset),
            Message::Control { cc, value } => Event::control(ch, cc, value, offset),
            _ => return true,
        };
        if self.events.len() < self.events.capacity() {
            self.events.push(e);
        }
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let sh = &self.shared;
        if sh.inside.swap(true, Ordering::AcqRel) {
            sh.reentries.fetch_add(1, Ordering::Relaxed);
        }
        let load = sh.load_ns.swap(0, Ordering::Relaxed);
        if load > 0 {
            std::thread::sleep(Duration::from_nanos(load));
        }
        if CALLBACK.get() {
            sh.on_callback.fetch_add(1, Ordering::Relaxed);
        } else {
            sh.on_worker.fetch_add(1, Ordering::Relaxed);
            let ns = sh.stall_ns.swap(0, Ordering::Relaxed);
            if ns > 0 {
                std::thread::sleep(Duration::from_nanos(ns));
            }
            // Bounded (60 s of polls) like a gate plug-in: a test that failed before releasing it
            // cannot leave the worker inside `process` for the rest of the binary.
            let mut polls = 0u32;
            while sh.hold.load(Ordering::Acquire) {
                sh.holding.store(true, Ordering::Release);
                polls += 1;
                assert!(polls < 60_000, "the hold was never released");
                // timing: sleep-ok paces a wait for a release flag, measures nothing
                std::thread::sleep(Duration::from_millis(1));
            }
            sh.holding.store(false, Ordering::Release);
        }
        // The mix delivers a block's events in offset order; the instrument checks it.
        sh.inst.lock().unwrap().process(self.transport, u32::try_from(frames).unwrap(), &self.events);
        self.events.clear();
        let cost = sh.cost.load(Ordering::Relaxed);
        for c in io.iter_mut() {
            for x in c.iter_mut().take(frames) {
                let mut y = self.phase;
                for _ in 0..cost {
                    y = std::hint::black_box((y * 1.000_1 + 0.000_1).sin());
                }
                *x = 0.05 * (self.phase.sin() + 0.0 * y);
                self.phase += 0.03;
            }
        }
        sh.inside.store(false, Ordering::Release);
    }
    /// The tone's phase, the transport and the continuity instrument's whole record (its report
    /// with the notes and pedals held now, its holes, every event with the sample it landed on).
    fn fingerprint(&self, h: &mut dyn std::hash::Hasher) {
        h.write_u32(self.phase.to_bits());
        h.write_u64(self.transport);
        h.write_usize(self.events.len());
        let inst = self.shared.inst.lock().unwrap();
        h.write(format!("{:?}{:?}", inst.report(), inst.holes()).as_bytes());
        for l in inst.trace() {
            h.write_u64(l.sample);
            h.write(&[l.event.status, l.event.data1, l.event.data2]);
            h.write_u32(l.event.offset);
        }
    }
}

/// A realtime engine with `instruments` continuity instruments (and `audio` audio strips with a
/// compressor or a reverb), adopted from a compiled plan.
pub struct Rig {
    pub mix: MixEngine,
    pub comp: PlanCompiler,
    pub session: Session,
    pub shared: Vec<Arc<Shared>>,
    pub bindings: Vec<SlotBinding>,
    pub frames: usize,
    pub pos: i64,
    pub out: Vec<Vec<f32>>,
    pub epoch: EngineEpoch,
    seq: u64,
    /// Blocks after which `MixEngine::block_overruns` was not zero (the take journal's marks).
    pub marked: u64,
}

pub fn target(frames: usize) -> RenderTarget {
    RenderTarget { device: DeviceGeneration(5), sample_rate: SR, max_block: u32::try_from(frames).unwrap(), out_channels: 0, in_channels: 0 }
}

pub fn session(instruments: usize, audio: usize) -> (Session, Vec<TrackId>) {
    let mut s = Session::default();
    s.edit.click = false;
    let ids = (0..instruments)
        .map(|_| {
            let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
            s.track_mut(t).unwrap().instrument = Some(Insert::new(CONTINUITY));
            t
        })
        .collect();
    let len = SR as usize * 120;
    for k in 0..audio {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let f = 0.003 + 0.0007 * k as f32;
        let signal: Vec<f32> = (0..len).map(|i| (i as f32 * f).sin() * 0.2).collect();
        let src = SourceId(7000 + k as u64);
        s.pool.insert(src, Arc::new(SourceAudio::new(transit_audio_io::AudioBuffer { sample_rate: SR, channels: vec![signal] })));
        let id = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::audio(id, "x", src, 0, 0, len as i64));
        tr.mixer.inserts[0] = Some(Insert::new(if k % 2 == 0 { "compressor" } else { "room_reverb" }));
    }
    (s, ids)
}

impl Rig {
    pub fn new(instruments: usize, audio: usize, frames: usize) -> Rig {
        let (s, ids) = session(instruments, audio);
        Rig::with_session(s, ids, frames, |_| None)
    }

    /// A rig over `s`, whose continuity instruments are on `ids`; `extra` makes the instances it
    /// knows (by spec) before the usual factory (#174: test plug-ins on inserts).
    pub fn with_session(s: Session, ids: Vec<TrackId>, frames: usize, mut extra: impl FnMut(&InstanceSpec) -> Option<NewInstance> + 'static) -> Rig {
        let epoch = EngineEpoch(9);
        let mut mix = MixEngine::realtime_with_events(target(frames), epoch, EventLimits::default());
        let mut comp = PlanCompiler::new(target(frames), PlanLimits::default());
        let made: Arc<Mutex<Vec<(TrackId, Arc<Shared>)>>> = Arc::default();
        let m = Arc::clone(&made);
        let mut make = move |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
            if spec.id == CONTINUITY {
                let shared = Shared::new();
                m.lock().unwrap().push((spec.track, Arc::clone(&shared)));
                return Some(NewInstance { plugin: Box::new(ContinuityPlugin::new(shared, spec.ch, frames)), refused_state: false });
            }
            if let Some(made) = extra(spec) {
                return Some(made);
            }
            create_instance(&spec.id, spec.state.as_deref(), SR as f32, frames, spec.ch)
        };
        let mut env = comp.compile(Arc::new(s.clone()), comp.active(), PlanGeneration::from_raw(1), &mut make).unwrap();
        assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
        comp.reclaim(env);
        let made = made.lock().unwrap().clone();
        let shared = ids.iter().map(|t| made.iter().find(|(m, _)| m == t).unwrap().1.clone()).collect();
        let active = comp.active_bindings();
        let bindings = ids.iter().map(|t| *active.iter().find(|b| b.0 == *t && b.1 == INSTRUMENT_SLOT).unwrap()).collect();
        Rig { mix, comp, session: s, shared, bindings, frames, pos: 0, out: vec![vec![0.0; frames]; 2], epoch, seq: 0, marked: 0 }
    }

    /// Renders one block (on the calling thread, which should be the callback).
    pub fn block(&mut self) {
        self.mix.render_active(self.pos, self.frames, &mut self.out);
        self.pos += self.frames as i64;
        self.marked += u64::from(self.mix.block_overruns() > 0);
    }

    /// Queues `message` on instrument `k` at `offset` in the next block, as playback does.
    pub fn send(&mut self, k: usize, offset: u32, message: Message) -> EventOutcome {
        self.seq += 1;
        let at = self.mix.next_frame().0 + u64::from(offset);
        let ev = InstrumentEvent {
            engine: self.epoch,
            target: self.bindings[k],
            due: Due::Frame(RenderFrame(at)),
            transport: None,
            source: Source { port: 1, channel: 0, generation: 1 },
            message,
            arrival: Arrival { host_ns: 0, sequence: self.seq },
        };
        self.mix.queue_event(&ev)
    }

    pub fn note_on(&mut self, k: usize, offset: u32, key: u8) -> EventOutcome {
        self.send(k, offset, Message::NoteOn { key, velocity: 100, id: NoteId::NONE })
    }

    pub fn note_off(&mut self, k: usize, offset: u32, key: u8) -> EventOutcome {
        self.send(k, offset, Message::NoteOff { key, velocity: 0, id: NoteId::NONE })
    }

    pub fn pedal(&mut self, k: usize, offset: u32, down: bool) -> EventOutcome {
        self.send(k, offset, Message::Control { cc: 64, value: if down { 127 } else { 0 } })
    }
}

/// A small xorshift generator: deterministic per seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// A live player per instrument: notes of one to six blocks on distinct keys, a sustain pedal
/// pressed now and then, every event in offset order within its block.
pub struct Player {
    rng: Rng,
    /// Per instrument: (key, block its off is due).
    open: Vec<Vec<(u8, u64)>>,
    pedal: Vec<Option<u64>>,
    next_key: Vec<u8>,
}

impl Player {
    pub fn new(instruments: usize, seed: u64) -> Player {
        Player {
            rng: Rng::new(seed),
            open: (0..instruments).map(|_| Vec::with_capacity(64)).collect(),
            pedal: vec![None; instruments],
            next_key: vec![36; instruments],
        }
    }

    /// The events of block `b` for every instrument.
    pub fn play(&mut self, rig: &mut Rig, b: u64) {
        let frames = u64::try_from(rig.frames).unwrap();
        for k in 0..self.open.len() {
            let mut evs: [(u32, u8); 8] = [(0, 0); 8];
            let mut n = 0;
            // Offs due now (kind 0), then maybe a new note (1), then the pedal (2 up, 3 down).
            let mut i = 0;
            while i < self.open[k].len() {
                if self.open[k][i].1 <= b && n < 8 {
                    let (key, _) = self.open[k].swap_remove(i);
                    evs[n] = (u32::try_from(self.rng.below(frames)).unwrap(), key);
                    n += 1;
                } else {
                    i += 1;
                }
            }
            let offs = n;
            let mut on = None;
            if self.open[k].len() < 6 && self.rng.below(3) == 0 {
                let key = self.next_key[k];
                self.next_key[k] = if key >= 90 { 36 } else { key + 1 };
                // A key released in this block is not struck again in it (that would be a
                // restrike if the new on landed before the off).
                if !self.open[k].iter().any(|(x, _)| *x == key) && !evs[..offs].iter().any(|(_, x)| *x == key) {
                    on = Some((u32::try_from(self.rng.below(frames)).unwrap(), key));
                    self.open[k].push((key, b + 1 + self.rng.below(6)));
                }
            }
            let mut pedal = None;
            match self.pedal[k] {
                Some(up) if up <= b => {
                    pedal = Some((u32::try_from(self.rng.below(frames)).unwrap(), false));
                    self.pedal[k] = None;
                }
                None if self.rng.below(40) == 0 => {
                    pedal = Some((u32::try_from(self.rng.below(frames)).unwrap(), true));
                    self.pedal[k] = Some(b + 2 + self.rng.below(8));
                }
                _ => {}
            }
            // In offset order, as playback dispatches a block's events.
            let mut all: [(u32, u8, u8); 10] = [(0, 0, 0); 10];
            let mut m = 0;
            for &(o, key) in &evs[..offs] {
                all[m] = (o, 0, key);
                m += 1;
            }
            if let Some((o, key)) = on {
                all[m] = (o, 1, key);
                m += 1;
            }
            if let Some((o, down)) = pedal {
                all[m] = (o, if down { 3 } else { 2 }, 0);
                m += 1;
            }
            all[..m].sort_unstable_by_key(|(o, kind, key)| (*o, *kind, *key));
            for &(o, kind, key) in &all[..m] {
                let outcome = match kind {
                    0 => rig.note_off(k, o, key),
                    1 => rig.note_on(k, o, key),
                    2 => rig.pedal(k, o, false),
                    _ => rig.pedal(k, o, true),
                };
                assert!(outcome.accepted(), "instrument {k} block {b}: {outcome:?}");
            }
        }
    }

    /// Every open note's off and every held pedal's up, in the next block.
    pub fn release_all(&mut self, rig: &mut Rig) {
        for k in 0..self.open.len() {
            for (key, _) in std::mem::take(&mut self.open[k]) {
                assert!(rig.note_off(k, 0, key).accepted());
            }
            if self.pedal[k].take().is_some() {
                assert!(rig.pedal(k, 1, false).accepted());
            }
        }
    }
}
