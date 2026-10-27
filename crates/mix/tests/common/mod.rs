//! F10 (ADR 0012 G5), mix side: sample-accurate, identity-carrying instrument events through
//! [`MixEngine::queue_event`] on a realtime engine fed the way playback feeds it. A recording
//! instrument writes down every event it receives with its absolute render frame.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use transit_dsp::Plugin;
use transit_mix::{
    Adoption, Arrival, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, Message,
    MixEngine, NewInstance, NoteId, PlanCompiler, PlanGeneration, PlanLimits, PluginEvent, RenderFrame, RenderTarget, SlotBinding, SlotGen, Source,
};
use transit_model::{ChannelFormat, Insert, Session, TrackId, TrackKind};
use transit_rt::testing::CountingAlloc;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

pub(super) const SR: u32 = 48_000;
pub(super) const BLOCK: usize = 256;

pub(super) fn target() -> RenderTarget {
    RenderTarget { device: DeviceGeneration(3), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 }
}

/// What a recording instrument received: the instance number, the absolute frame (its block's
/// first frame plus the offset) and the event. `None` marks an `all_notes_off`.
pub(super) type Heard = Arc<Mutex<Vec<(usize, u64, Option<PluginEvent>)>>>;

/// An instrument that records every event at its absolute render frame.
struct Recorder {
    inner: Box<dyn Plugin>,
    heard: Heard,
    instance: usize,
    pending: Vec<(u32, Option<PluginEvent>)>,
    rendered: u64,
    /// While set, the instrument renders NaN (the mix takes it out as failed).
    fail: Arc<AtomicBool>,
    refuse_releases: Arc<AtomicBool>,
    refuse_downs: Arc<AtomicBool>,
}

impl Plugin for Recorder {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    /// As real hosts do: pending non-releases are dropped, releases (and `all_notes_off`) are
    /// kept for the next block, so a mix that relies on a reset to drop a release is caught.
    fn reset(&mut self) {
        self.pending.retain(|(_, e)| e.is_none_or(|e| e.message.survives_reset()));
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mut heard = self.heard.lock().unwrap();
        // Stable by offset, as hosts sort.
        self.pending.sort_by_key(|(o, _)| *o);
        for (o, e) in self.pending.drain(..) {
            heard.push((self.instance, self.rendered + u64::from(o), e));
        }
        self.rendered += frames as u64;
        let y = if self.fail.load(Ordering::Relaxed) { f32::NAN } else { 0.0 };
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = y);
        }
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        if (ev.is_release() && self.refuse_releases.load(Ordering::Relaxed))
            || (matches!(ev.message, Message::Control { cc: 64..=69, value: 64..=127 }) && self.refuse_downs.load(Ordering::Relaxed))
        {
            return false;
        }
        self.pending.push((offset, Some(*ev)));
        true
    }
    fn all_notes_off(&mut self) {
        self.pending.push((0, None));
    }
}

pub(super) struct Rig {
    pub(super) mix: MixEngine,
    pub(super) comp: PlanCompiler,
    pub(super) heard: Heard,
    pub(super) made: Arc<Mutex<usize>>,
    pub(super) generation: PlanGeneration,
    pub(super) pos: i64,
    pub(super) out: Vec<Vec<f32>>,
    pub(super) epoch: EngineEpoch,
    pub(super) track: TrackId,
    pub(super) session: Session,
    pub(super) fail: Arc<AtomicBool>,
    pub(super) refuse_releases: Arc<AtomicBool>,
    pub(super) refuse_downs: Arc<AtomicBool>,
}

pub(super) fn instrument_session(id: &str) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new(id));
    (s, t)
}

impl Rig {
    pub(super) fn new() -> Rig {
        Rig::with_epoch(EngineEpoch::FIRST)
    }

    pub(super) fn with_epoch(epoch: EngineEpoch) -> Rig {
        let (session, track) = instrument_session("clap:test.rec");
        let mut rig = Rig {
            mix: MixEngine::realtime_with_events(target(), epoch, EventLimits::default()),
            comp: PlanCompiler::new(target(), PlanLimits::default()),
            heard: Arc::default(),
            made: Arc::default(),
            generation: PlanGeneration::EMPTY,
            pos: 0,
            out: vec![vec![0.0; BLOCK]; 2],
            epoch,
            track,
            session,
            fail: Arc::default(),
            refuse_releases: Arc::default(),
            refuse_downs: Arc::default(),
        };
        let s = rig.session.clone();
        rig.publish(&s);
        // The first block chases the controllers (reset all controllers): not under test.
        rig.render(1);
        rig.heard.lock().unwrap().clear();
        rig
    }

    pub(super) fn publish(&mut self, s: &Session) {
        self.publish_with_mode(s, transit_mix::CompileMode::Update);
    }

    pub(super) fn publish_with_mode(&mut self, s: &Session, mode: transit_mix::CompileMode) {
        self.generation = self.generation.checked_next().unwrap();
        let (heard, made, fail) = (Arc::clone(&self.heard), Arc::clone(&self.made), Arc::clone(&self.fail));
        let (refuse_releases, refuse_downs) = (Arc::clone(&self.refuse_releases), Arc::clone(&self.refuse_downs));
        let mut make = move |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
            let mut inner = transit_dsp::create("gain")?;
            inner.prepare(SR as f32, BLOCK, spec.ch);
            let mut n = made.lock().unwrap();
            *n += 1;
            let plugin = Box::new(Recorder {
                inner,
                heard: Arc::clone(&heard),
                instance: *n,
                pending: Vec::with_capacity(4096),
                rendered: 0,
                fail: Arc::clone(&fail),
                refuse_releases: Arc::clone(&refuse_releases),
                refuse_downs: Arc::clone(&refuse_downs),
            });
            Some(NewInstance { plugin, refused_state: false })
        };
        let mut env = self.comp.compile_with_mode(Arc::new(s.clone()), self.comp.active(), self.generation, mode, &mut make).unwrap();
        assert_eq!(self.mix.adopt_plan(&mut env), Adoption::Adopted);
        self.comp.reclaim(env);
    }

    pub(super) fn binding(&self) -> SlotBinding {
        self.comp.active_bindings().into_iter().find(|b| b.0 == self.track && b.1 == INSTRUMENT_SLOT).unwrap()
    }

    pub(super) fn render(&mut self, blocks: usize) {
        for _ in 0..blocks {
            self.mix.render_active(self.pos, BLOCK, &mut self.out);
            self.pos += BLOCK as i64;
        }
    }

    pub(super) fn event(&self, source: Source, due: Due, message: Message) -> InstrumentEvent {
        InstrumentEvent {
            engine: self.epoch,
            target: self.binding(),
            due,
            transport: None,
            source,
            message,
            arrival: Arrival { host_ns: 0, sequence: 0 },
        }
    }

    pub(super) fn send(&mut self, source: Source, at: u64, message: Message) -> EventOutcome {
        let ev = self.event(source, Due::Frame(RenderFrame(at)), message);
        self.mix.queue_event(&ev)
    }

    /// Every event instance `instance` heard: (frame, event).
    pub(super) fn heard_by(&self, instance: usize) -> Vec<(u64, PluginEvent)> {
        self.heard.lock().unwrap().iter().filter(|(i, _, _)| *i == instance).filter_map(|(_, f, e)| e.map(|e| (*f, e))).collect()
    }
}

pub(super) fn port(port: u16, channel: u8) -> Source {
    Source { port, channel, generation: 1 }
}

pub(super) fn on(key: u8) -> Message {
    Message::NoteOn { key, velocity: 100, id: NoteId::NONE }
}

pub(super) fn off(key: u8) -> Message {
    Message::NoteOff { key, velocity: 0, id: NoteId::NONE }
}

pub(super) fn cc(cc: u8, value: u8) -> Message {
    Message::Control { cc, value }
}
