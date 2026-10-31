//! Plan contract tests that need the crate's internals: forged (malformed) plans for every
//! reject reason, atomic rejection, injected commit faults, identity and limits.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan::{Attach, CellKind, CellMove};
use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::ThreadId;
use transit_audio_io::AudioBuffer;
use transit_model::{Insert, SourceId};

/// Counts heap operations on threads a test measures (all other threads are only forwarded).
#[global_allocator]
static ALLOC: transit_rt::testing::CountingAlloc = transit_rt::testing::CountingAlloc;

/// Adopts on this thread, measured: the audio side allocates and frees nothing.
pub(super) fn adopt_measured(eng: &mut MixEngine, env: &mut PlanEnvelope) -> Adoption {
    let (a, stats) = transit_rt::testing::measure(|| eng.adopt_plan(env));
    assert!(stats.is_zero(), "adopt_plan allocated or freed: {stats:?}");
    a
}

pub(super) const SR: u32 = 48_000;
pub(super) const BLOCK: usize = 256;

pub(super) fn target() -> RenderTarget {
    RenderTarget { device: DeviceGeneration(7), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 }
}

/// Where and on which thread test instances were dropped.
#[derive(Default)]
pub(super) struct Log {
    pub(super) drops: Mutex<Vec<ThreadId>>,
    pub(super) made: AtomicUsize,
    /// Latency every `clap:test.latent` instance reports.
    pub(super) latency: AtomicUsize,
    /// `set_sample` unwinds (armed after the instances are made).
    pub(super) panic_on_sample: AtomicBool,
    pub(super) panic_on_drop: AtomicBool,
    pub(super) panic_on_process: AtomicBool,
}

pub(super) type SharedLog = Arc<Log>;

/// A test instance: a built-in (or a gain for `clap:test.*` ids) that records its drop.
struct Probe {
    inner: Box<dyn Plugin>,
    pub(super) log: SharedLog,
    latent: bool,
}

impl Plugin for Probe {
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
        if self.log.panic_on_process.load(Ordering::Relaxed) {
            panic!("plug-in panics in process");
        }
        self.inner.process(io, frames);
    }
    fn latency(&self) -> usize {
        if self.latent { self.log.latency.load(Ordering::Relaxed) } else { self.inner.latency() }
    }
    fn tail_samples(&self) -> usize {
        self.inner.tail_samples()
    }
    fn process_status(&self) -> ProcessStatus {
        self.inner.process_status()
    }
    fn gain_reduction_db(&self) -> f32 {
        self.inner.gain_reduction_db()
    }
    fn set_transport(&mut self, t: &Transport) {
        self.inner.set_transport(t);
    }
    fn note_on(&mut self, o: usize, n: u8, v: u8) {
        self.inner.note_on(o, n, v);
    }
    fn note_off(&mut self, o: usize, n: u8) {
        self.inner.note_off(o, n);
    }
    fn midi(&mut self, o: usize, m: [u8; 3]) {
        self.inner.midi(o, m);
    }
    fn all_notes_off(&mut self) {
        self.inner.all_notes_off();
    }
    fn set_sample(&mut self, s: Option<Arc<dyn SampleSource>>) -> bool {
        if self.log.panic_on_sample.load(Ordering::Relaxed) {
            panic!("test sampler refuses its sample");
        }
        self.inner.set_sample(s)
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.log.drops.lock().unwrap().push(std::thread::current().id());
        if self.log.panic_on_drop.swap(false, Ordering::Relaxed) {
            panic!("plug-in panics in drop");
        }
    }
}

pub(super) fn factory(log: &SharedLog) -> impl FnMut(&InstanceSpec, SlotGen) -> Option<NewInstance> + use<> {
    let log = Arc::clone(log);
    move |spec: &InstanceSpec, _generation: SlotGen| {
        let base = if spec.id.starts_with("clap:test.") { "gain" } else { spec.id.as_str() };
        if spec.id == "clap:test.missing" {
            return None;
        }
        let n = create_instance(base, None, SR as f32, BLOCK, spec.ch)?;
        log.made.fetch_add(1, Ordering::Relaxed);
        let latent = spec.id == "clap:test.latent";
        Some(NewInstance { plugin: Box::new(Probe { inner: n.plugin, log: Arc::clone(&log), latent }), refused_state: false })
    }
}

/// A realtime engine with its compiler, fed the way playback feeds them (N = 1).
pub(super) struct Rig {
    pub(super) eng: MixEngine,
    pub(super) comp: PlanCompiler,
    pub(super) log: SharedLog,
    pub(super) pos: i64,
    pub(super) out: Vec<Vec<f32>>,
    pub(super) issued: u64,
}

impl Rig {
    pub(super) fn new() -> Rig {
        Rig {
            eng: MixEngine::realtime(target()),
            comp: PlanCompiler::new(target(), PlanLimits::default()),
            log: SharedLog::default(),
            pos: 0,
            out: vec![vec![0.0; BLOCK]; 2],
            issued: 0,
        }
    }

    pub(super) fn compile(&mut self, s: &Session) -> PlanEnvelope {
        self.issued += 1;
        let mut make = factory(&self.log);
        self.comp.compile(Arc::new(s.clone()), self.comp.active(), PlanGeneration::from_raw(self.issued), &mut make).unwrap()
    }

    pub(super) fn publish(&mut self, s: &Session) -> Adoption {
        let mut env = self.compile(s);
        let a = self.eng.adopt_plan(&mut env);
        self.comp.reclaim(env);
        a
    }

    /// Renders `blocks` blocks; returns the left channel.
    pub(super) fn render(&mut self, blocks: usize) -> Vec<f32> {
        let mut left = Vec::new();
        for _ in 0..blocks {
            self.eng.render_active(self.pos, BLOCK, &mut self.out);
            left.extend_from_slice(&self.out[0]);
            self.pos += BLOCK as i64;
        }
        left
    }

    pub(super) fn drops(&self) -> usize {
        self.log.drops.lock().unwrap().len()
    }
}

/// Everything observable about a realtime engine's plan, pointers included.
fn digest(eng: &MixEngine) -> Vec<u64> {
    let mut d = Vec::new();
    let p = eng.plan.as_deref().unwrap();
    d.push(p.id.generation.get());
    d.push(p.id.base.get());
    d.push(Arc::as_ptr(&p.session) as usize as u64);
    d.push(u64::from(p.faulted));
    d.push(p.tables.cells.len() as u64);
    for st in &p.tables.strips {
        d.extend([st.generation.get(), st.id.0, st.buf.as_ptr() as usize as u64, st.latency as u64]);
        d.extend([st.tap.len as u64, st.tap.idx as u64, st.tap.bufs.as_ptr() as usize as u64, u64::from(st.tap.live)]);
        let slots = st.plugins.iter().chain(std::iter::once(&st.instrument)).chain(st.clip_fx.0.iter().flat_map(|(_, fx)| [&fx.eq, &fx.comp]));
        for s in slots {
            match s {
                Some(s) => d.extend([
                    s.generation.get(),
                    &*s.plugin as *const dyn Plugin as *const () as usize as u64,
                    s.applied.heap.as_ptr() as usize as u64,
                    s.applied.heap.len() as u64,
                    s.sample_audio.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize as u64),
                    s.events.as_deref().map_or(0, |e| e as *const _ as usize as u64),
                ]),
                None => d.push(u64::MAX),
            }
        }
    }
    d
}

/// What an envelope owns: its Session, every instance in its tables, its moves and attachments.
pub(super) fn holdings(env: &PlanEnvelope) -> (usize, usize, usize, usize, usize) {
    let p = &env.plan;
    (
        Arc::as_ptr(&p.session) as usize,
        p.tables.as_ref().map_or(0, |t| t.plugins().count()),
        p.tables.as_ref().map_or(0, |t| t.strips.len()),
        p.moves.len(),
        p.attach.len(),
    )
}

pub(super) fn audio_session() -> (Session, TrackId, TrackId) {
    let mut s = Session::default();
    let a = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let b = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
    let len = 48_000usize;
    // A burst, then silence: inserts ring into the tail.
    let signal: Vec<f32> = (0..len).map(|i| if i < 4_000 { (i as f32 * 0.03).sin() * 0.5 } else { 0.0 }).collect();
    s.pool.insert(SourceId(1), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![signal.clone(), signal] })));
    let id = s.new_clip_id();
    s.track_mut(a).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "x", SourceId(1), 0, 0, len as i64));
    (s, a, b)
}

pub(super) fn builtin(id: &str) -> Insert {
    let mut ins = Insert::new(id);
    for p in transit_dsp::plugin_info(id).unwrap().params {
        ins.params.insert(p.id.to_string(), p.default);
    }
    ins
}

/// An active plan with two reused inserts on track `a`, and a structural plan that keeps both
/// and adds a fresh third (so it has slot moves, a strip move, and a fresh slot).
pub(super) fn rig_with_pending() -> (Rig, Session, PlanEnvelope) {
    let (mut s, a, _) = audio_session();
    let ins = &mut s.track_mut(a).unwrap().mixer.inserts;
    ins[0] = Some(builtin("room_reverb"));
    ins[1] = Some(builtin("compressor"));
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.render(4);
    let mut s2 = s.clone();
    let ins = &mut s2.track_mut(a).unwrap().mixer.inserts;
    ins[2] = Some(builtin("compressor"));
    // The reverb's parameter map grows past the inline storage: an `Attach::Applied`.
    for i in 0..140 {
        ins[0].as_mut().unwrap().params.insert(format!("extra{i:03}"), 0.5);
    }
    let env = rig.compile(&s2);
    assert_eq!(env.plan.attach.len(), 1);
    (rig, s2, env)
}

pub(super) fn slot_moves(env: &PlanEnvelope) -> Vec<usize> {
    env.plan.moves.iter().enumerate().filter(|(_, m)| m.kind == CellKind::Insert).map(|(i, _)| i).collect()
}

/// Adopting `env` is refused for `reason`; the engine is exactly as before, and the envelope
/// still holds its entire input. Then a twin that never saw the plan renders the same.
pub(super) fn assert_rejected(mut rig: Rig, mut env: PlanEnvelope, reason: RejectReason) {
    let before = digest(&rig.eng);
    let held = holdings(&env);
    let drops = rig.drops();
    assert_eq!(adopt_measured(&mut rig.eng, &mut env), Adoption::Rejected(reason));
    assert_eq!(env.state(), EnvelopeState::Rejected { reason, active: rig.eng.active().generation });
    assert_eq!(digest(&rig.eng), before, "the engine is unchanged");
    assert_eq!(holdings(&env), held, "the envelope holds its entire input");
    assert_eq!(rig.drops(), drops, "nothing dropped while refusing");
    assert_eq!(rig.eng.adopt_plan(&mut env), Adoption::AlreadyServiced);
    assert_eq!(digest(&rig.eng), before);
    rig.comp.reclaim(env);
    assert_eq!(rig.comp.active(), rig.eng.active().generation, "the compiler's shadow stays at the active generation");
}

/// The rejected engine and an engine that never saw the plan render the same afterwards.
fn twin_renders_match(corrupt: impl Fn(&mut PlanEnvelope)) {
    let (mut rig, _, mut env) = rig_with_pending();
    let (mut twin, _, twin_env) = rig_with_pending();
    corrupt(&mut env);
    assert!(matches!(rig.eng.adopt_plan(&mut env), Adoption::Rejected(_)));
    rig.comp.reclaim(env);
    twin.comp.reclaim(twin_env);
    assert_eq!(rig.render(8), twin.render(8), "bit-identical to an engine that never saw the plan");
}

#[test]
fn a_plan_with_moves_adopts_and_returns_what_it_displaced() {
    let (mut rig, _, mut env) = rig_with_pending();
    assert_eq!(slot_moves(&env).len(), 2, "both inserts are reused");
    assert!(env.plan.moves.iter().any(|m| m.kind == CellKind::Strip), "the strip's state moves");
    assert_eq!(adopt_measured(&mut rig.eng, &mut env), Adoption::Adopted);
    assert_eq!(env.state(), EnvelopeState::Returned { replaced: PlanGeneration::from_raw(1) });
    assert_eq!(rig.eng.adopt_plan(&mut env), Adoption::AlreadyServiced);
    // The returned tables hold no instance: both moved, none was replaced.
    assert_eq!(holdings(&env).1, 0);
    rig.comp.reclaim(env);
    assert_eq!(rig.comp.active(), rig.eng.active().generation);
    assert_eq!(rig.eng.plan.as_deref().unwrap().tables.plugins().count(), 3);
}

#[test]
fn rejects_a_stale_base() {
    let (rig, _, mut env) = rig_with_pending();
    env.plan.id.base = PlanGeneration::EMPTY;
    assert_rejected(rig, env, RejectReason::BaseMismatch);
}

#[test]
fn rejects_a_generation_that_does_not_advance() {
    let (rig, _, mut env) = rig_with_pending();
    env.plan.id.generation = env.plan.id.base;
    assert_rejected(rig, env, RejectReason::BaseMismatch);
}

#[test]
fn rejects_each_target_field() {
    for f in 0..3 {
        let (rig, _, mut env) = rig_with_pending();
        let t = &mut env.plan.id.target;
        match f {
            0 => t.device = DeviceGeneration(8),
            1 => t.sample_rate = 44_100,
            _ => t.max_block = 512,
        }
        assert_rejected(rig, env, RejectReason::TargetMismatch);
    }
}

#[test]
fn rejects_a_move_out_of_range() {
    let (rig, _, mut env) = rig_with_pending();
    let i = slot_moves(&env)[0];
    env.plan.moves[i].from = 9_999;
    assert_rejected(rig, env, RejectReason::MoveOutOfRange);
    let (rig, _, mut env) = rig_with_pending();
    env.plan.moves[i].to = 9_999;
    assert_rejected(rig, env, RejectReason::MoveOutOfRange);
}

#[test]
fn rejects_duplicate_sources() {
    let (rig, _, mut env) = rig_with_pending();
    let [a, b] = slot_moves(&env)[..] else { panic!() };
    let mut moves = env.plan.moves.to_vec();
    moves[b].from = moves[a].from;
    env.plan.moves = moves.into_boxed_slice();
    assert_rejected(rig, env, RejectReason::DuplicateSource);
}

#[test]
fn rejects_two_moves_into_one_vacant_destination() {
    let (rig, _, mut env) = rig_with_pending();
    let [a, b] = slot_moves(&env)[..] else { panic!() };
    let to = env.plan.moves[a].to;
    env.plan.moves[b].to = to;
    assert_rejected(rig, env, RejectReason::DuplicateDestination);
    twin_renders_match(|env| {
        let [a, b] = slot_moves(env)[..] else { panic!() };
        env.plan.moves[b].to = env.plan.moves[a].to;
    });
}

#[test]
fn rejects_a_source_that_holds_nothing() {
    let (mut s, a, _) = audio_session();
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(Insert::new("clap:test.missing"));
    s.track_mut(a).unwrap().mixer.inserts[1] = Some(builtin("compressor"));
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let mut s2 = s.clone();
    s2.track_mut(a).unwrap().mixer.inserts[2] = Some(builtin("gain"));
    let mut env = rig.compile(&s2);
    // The missing instance's cell is in the active plan, empty: point the compressor's move at it.
    let old = &rig.eng.plan.as_deref().unwrap().tables;
    let empty = old.cells.iter().position(|c| c.shape.kind == CellKind::Insert && old.slot(c).is_some_and(Option::is_none)).unwrap();
    let i = slot_moves(&env)[0];
    env.plan.moves[i].from = empty as u16;
    assert_rejected(rig, env, RejectReason::SourceNotLive);
}

#[test]
fn rejects_a_stale_slot_generation() {
    let (rig, _, mut env) = rig_with_pending();
    let i = slot_moves(&env)[0];
    env.plan.moves[i].expect = SlotGen(9_999);
    assert_rejected(rig, env, RejectReason::SlotGenMismatch);
    let (rig, _, mut env) = rig_with_pending();
    let i = env.plan.moves.iter().position(|m| m.kind == CellKind::Strip).unwrap();
    env.plan.moves[i].expect = SlotGen(9_999);
    assert_rejected(rig, env, RejectReason::SlotGenMismatch);
}

#[test]
fn rejects_a_destination_that_is_filled() {
    let (rig, _, mut env) = rig_with_pending();
    let new = env.plan.tables.as_ref().unwrap();
    let fresh = new.cells.iter().position(|c| c.shape.kind == CellKind::Insert && new.slot(c).is_some_and(Option::is_some)).unwrap();
    let i = slot_moves(&env)[0];
    env.plan.moves[i].to = fresh as u16;
    assert_rejected(rig, env, RejectReason::DestinationNotVacant);
}

#[test]
fn rejects_a_destination_of_another_kind_channel_count_or_layout() {
    for f in 0..4 {
        let (rig, _, mut env) = rig_with_pending();
        let i = slot_moves(&env)[0];
        let to = usize::from(env.plan.moves[i].to);
        let cells = &mut env.plan.tables.as_mut().unwrap().cells;
        match f {
            0 => env.plan.moves[i].kind = CellKind::Instrument,
            1 => cells[to].shape.kind = CellKind::Instrument,
            2 => cells[to].shape.ch = 1,
            _ => cells[to].shape.layout ^= 0x40,
        }
        assert_rejected(rig, env, RejectReason::ShapeMismatch);
    }
    // A strip shell of another width.
    let (rig, _, mut env) = rig_with_pending();
    let i = env.plan.moves.iter().position(|m| m.kind == CellKind::Strip).unwrap();
    let to = usize::from(env.plan.moves[i].to);
    env.plan.tables.as_mut().unwrap().cells[to].shape.layout ^= 0x40;
    assert_rejected(rig, env, RejectReason::ShapeMismatch);
}

#[test]
fn rejects_invalid_attachments() {
    for f in 0..3 {
        let (rig, _, mut env) = rig_with_pending();
        let to = env.plan.moves[slot_moves(&env)[0]].to;
        let strip_cell = env.plan.moves.iter().find(|m| m.kind == CellKind::Strip).unwrap().to;
        let a = match f {
            0 => Attach::Applied { cell: to, from_move: None, storage: vec![f32::NAN; 8] },
            1 => Attach::Applied { cell: strip_cell, from_move: None, storage: vec![f32::NAN; 80] },
            _ => Attach::Sample { cell: to, from_move: None, audio: None, wrapper: None },
        };
        env.plan.attach = vec![a].into_boxed_slice();
        assert_rejected(rig, env, RejectReason::AttachInvalid);
    }
}

#[test]
fn rejects_claim_scratch_of_the_wrong_size() {
    let (rig, _, mut env) = rig_with_pending();
    env.plan.src_claimed = vec![false; 1].into_boxed_slice();
    assert_rejected(rig, env, RejectReason::MoveOutOfRange);
}

#[test]
fn limits_are_checked_before_anything_is_allocated() {
    let (mut s, a, _) = audio_session();
    for i in 0..3 {
        s.track_mut(a).unwrap().mixer.inserts[i] = Some(builtin("gain"));
    }
    let s = Arc::new(s);
    let log = SharedLog::default();
    let cases: [(PlanLimits, &str); 5] = [
        (PlanLimits { tracks: 1, ..PlanLimits::default() }, "tracks"),
        (PlanLimits { slots_per_strip: 4, ..PlanLimits::default() }, "slots_per_strip"),
        (PlanLimits { prepared_bytes: 1 << 16, ..PlanLimits::default() }, "prepared_bytes"),
        (PlanLimits { buses: 0, ..PlanLimits::default() }, "buses"),
        (PlanLimits { clip_fx_per_strip: 0, ..PlanLimits::default() }, "clip_fx_per_strip"),
    ];
    for (limits, what) in cases {
        let mut ses = (*s).clone();
        if what == "buses" {
            ses.busses.push(transit_model::Bus { id: BusId(77), name: "b".into(), format: ChannelFormat::Stereo });
        }
        if what == "clip_fx_per_strip" {
            let clip = ses.track(a).unwrap().clips()[0].id.0;
            ses.edit.values.insert(format!("clip_fx.{clip}.eq.gain1"), 1.0);
        }
        let ses = Arc::new(ses);
        let mut comp = PlanCompiler::new(target(), limits);
        let mut make = factory(&log);
        let (r, stats) =
            transit_rt::testing::measure(|| comp.compile(Arc::clone(&ses), PlanGeneration::EMPTY, PlanGeneration::from_raw(1), &mut make));
        match r {
            Err(CompileError::ResourceLimit { what: w, need, limit }) => {
                assert_eq!(w, what);
                assert!(need > limit);
            }
            other => panic!("{what}: {other:?}"),
        }
        assert_eq!(stats.allocs, 0, "{what}: nothing allocated before the check");
        assert_eq!(log.made.load(Ordering::Relaxed), 0, "{what}: no instance made");
    }
    // Multiplication overflow is a limit, not a wrap.
    assert_eq!(
        compile::strip_bytes(16, 16_384, usize::MAX / 8, 4, 0),
        Err(CompileError::ResourceLimit { what: "prepared_bytes", need: usize::MAX, limit: usize::MAX })
    );
}

#[test]
fn a_volume_only_edit_publishes_no_tables() {
    let (s, a, _) = audio_session();
    let mut rig = Rig::new();
    rig.publish(&s);
    let mut s2 = s.clone();
    s2.track_mut(a).unwrap().mixer.volume_db = -6.0;
    let env = rig.compile(&s2);
    assert!(!env.plan().is_structural());
    rig.comp.reclaim(env);
}

#[test]
fn envelopes_are_send() {
    fn send<T: Send>() {}
    send::<PlanEnvelope>();
    send::<MixEngine>();
    send::<Retired>();
}

/// A move list the compiler made is accepted only for the active plan it was made against.
#[test]
fn moves_name_cells_of_the_active_plan() {
    let (rig, _, env) = rig_with_pending();
    let old = &rig.eng.plan.as_deref().unwrap().tables;
    for CellMove { kind, from, .. } in env.plan.moves.iter() {
        assert_eq!(old.cells[usize::from(*from)].shape.kind, *kind);
    }
    assert_eq!(env.plan.src_claimed.len(), old.cells.len());
}

// Review repros (ADR 0009 refutation review of feat/prepared-plan).

#[test]
fn empty_and_shrinking_sessions_and_the_track_limit() {
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&Session::default()), Adoption::Adopted);
    assert_eq!(rig.render(2), vec![0.0; 2 * BLOCK]);
    let (s, _, _) = audio_session();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let mut one = s.clone();
    one.tracks.truncate(1);
    assert_eq!(rig.publish(&one), Adoption::Adopted);
    rig.render(2);
    for n in [2u16, 1] {
        let mut comp = PlanCompiler::new(target(), PlanLimits { tracks: n, ..PlanLimits::default() });
        let mut make = factory(&rig.log);
        let r = comp.compile(Arc::new(s.clone()), PlanGeneration::EMPTY, PlanGeneration::from_raw(1), &mut make);
        if n == 2 { assert!(r.is_ok()) } else { assert!(matches!(r, Err(CompileError::ResourceLimit { what: "tracks", .. }))) }
    }
}
