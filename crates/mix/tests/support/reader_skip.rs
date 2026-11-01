//! Real Filter and constant audio for the #276 skip regressions.
#![allow(dead_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering::Relaxed},
};
use transit_dsp::{Plugin, event::PluginEvent};
use transit_mix::{
    Adoption, Arrival, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, Message,
    MixEngine, NewInstance, NoteId, PlanCompiler, PlanLimits, RenderFrame, RenderTarget, SlotGen, Source,
};
use transit_model::{ChannelFormat, Insert, Session, TrackId, TrackKind};
use transit_rt::testing::measure;

pub const BLOCK: usize = 64;
const A: &str = "clap:test.source_a";
const B: &str = "clap:test.source_b";
const READER: &str = "clap:test.filter";

#[derive(Default)]
pub struct Trace {
    pub pedal_ups: AtomicUsize,
    pub channels: AtomicUsize,
    pub releases: AtomicUsize,
    pub resets: AtomicUsize,
    pub ons: AtomicUsize,
    pub off: AtomicUsize,
}

/// Constant input remains after source removal: silence must come from the real Filter's amp
/// envelope, never from a missing instrument. The instrument fixture only accepts note events.
struct Feed {
    latency: usize,
}

impl Plugin for Feed {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn latency(&self) -> usize {
        self.latency
    }
    fn event(&mut self, _: u32, _: &PluginEvent) -> bool {
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for c in io {
            c[..frames].fill(0.25);
        }
    }
}

struct Reader {
    filter: Box<dyn Plugin>,
    pub trace: Arc<Trace>,
}

impl Plugin for Reader {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.filter.info()
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        self.trace.resets.fetch_add(1, Relaxed);
        self.filter.reset();
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.filter.set_param(id, value)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.filter.param(id)
    }
    fn reads_track_notes(&self) -> bool {
        true
    }
    fn all_notes_off(&mut self) {
        self.trace.releases.fetch_add(1, Relaxed);
        self.filter.all_notes_off();
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        match ev.message {
            Message::NoteOn { .. } => {
                self.trace.ons.fetch_add(1, Relaxed);
            }
            Message::NoteOff { .. } => {
                self.trace.off.fetch_add(1, Relaxed);
            }
            Message::Control { cc: 64, value: 0 } => {
                self.trace.pedal_ups.fetch_add(1, Relaxed);
                self.trace.channels.fetch_or(1 << ev.channel, Relaxed);
                assert_eq!(offset, 0, "skip releases precede resumed events");
                assert!(self.trace.releases.load(Relaxed) > 0, "all_notes_off precedes pedal ups");
            }
            _ => {}
        }
        self.filter.event(offset, ev)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.filter.process(io, frames);
    }
}

pub struct Rig {
    pub session: Session,
    pub track: TrackId,
    pub mix: MixEngine,
    compiler: PlanCompiler,
    pub trace: Arc<Trace>,
    latency: usize,
    clock: i64,
    pub out: Vec<Vec<f32>>,
}

impl Rig {
    pub fn new(latency: usize, kind: TrackKind) -> Self {
        MixEngine::force_serial(true);
        transit_mix::mark_audio_thread();
        let mut session = Session::default();
        let track = session.add_track(kind, ChannelFormat::Stereo, None);
        let t = session.track_mut(track).unwrap();
        t.instrument = (kind == TrackKind::Instrument).then(|| Insert::new(A));
        t.mixer.inserts[0] = Some(Insert::new("clap:test.feed"));
        t.mixer.inserts[1] = Some(Insert::new(READER));
        let target = RenderTarget { device: DeviceGeneration(1), sample_rate: 48_000, max_block: BLOCK as u32, in_channels: 0, out_channels: 0 };
        let mut rig = Self {
            session,
            track,
            mix: MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default()),
            compiler: PlanCompiler::new(target, PlanLimits::default()),
            trace: Arc::default(),
            latency,
            clock: 0,
            out: vec![vec![0.0; BLOCK]; 2],
        };
        rig.publish();
        rig
    }

    pub fn publish(&mut self) {
        let trace = self.trace.clone();
        let mut make = |spec: &InstanceSpec, _: SlotGen| {
            let plugin: Box<dyn Plugin> = match spec.id.as_str() {
                READER => {
                    let mut filter = transit_dsp::create("filter").unwrap();
                    for (id, v) in [("amp_on", 1.0), ("amp_attack", 0.5), ("amp_release", 1.0), ("amp_velocity", 0.0), ("cutoff", 20_000.0)] {
                        assert!(filter.set_param(id, v));
                    }
                    filter.prepare(48_000.0, BLOCK, 2);
                    Box::new(Reader { filter, trace: trace.clone() })
                }
                A | B => Box::new(Feed { latency: self.latency }),
                "clap:test.feed" => Box::new(Feed { latency: 0 }),
                _ => return transit_mix::create_instance(&spec.id, None, 48_000.0, BLOCK, spec.ch),
            };
            Some(NewInstance { plugin, refused_state: false })
        };
        let base = self.compiler.active();
        let mut env = self.compiler.compile(Arc::new(self.session.clone()), base, base.checked_next().unwrap(), &mut make).unwrap();
        let (adoption, stats) = measure(|| self.mix.adopt_plan(&mut env));
        assert_eq!(adoption, Adoption::Adopted);
        assert!(stats.is_zero(), "adoption: {stats:?}");
        self.compiler.reclaim(env);
    }

    pub fn bypass(&mut self, bypass: bool) {
        self.session.track_mut(self.track).unwrap().mixer.inserts[1].as_mut().unwrap().bypass = bypass;
        self.publish();
    }

    pub fn generation(&self, slot: usize) -> SlotGen {
        self.compiler.active_bindings().iter().find(|b| b.0 == self.track && b.1 == slot).unwrap().2
    }

    pub fn send(&mut self, message: Message) {
        let target = (self.track, INSTRUMENT_SLOT, self.generation(INSTRUMENT_SLOT));
        assert_eq!(
            self.mix.queue_event(&InstrumentEvent {
                engine: EngineEpoch::FIRST,
                target,
                due: Due::Frame(RenderFrame(self.mix.next_frame().0)),
                transport: None,
                source: Source { port: 1, channel: 0, generation: 1 },
                message,
                arrival: Arrival { host_ns: 0, sequence: 0 },
            }),
            EventOutcome::Queued
        );
    }

    pub fn on(&mut self) {
        self.send(Message::NoteOn { key: 60, velocity: 127, id: NoteId::NONE });
    }
    pub fn off(&mut self) {
        self.send(Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE });
    }

    pub fn block(&mut self) {
        let ((), stats) = measure(|| self.mix.render_active(self.clock, BLOCK, &mut self.out));
        assert!(stats.is_zero(), "render: {stats:?}");
        self.clock += BLOCK as i64;
    }

    pub fn sounding(&self) {
        assert!(self.out[0][BLOCK - 1].abs() > 0.1, "held amp envelope must sound");
    }
    pub fn silent(&self) {
        assert!(self.out.iter().flatten().all(|x| *x == 0.0), "amp envelope must finish releasing");
    }

    pub fn released_tail(&mut self) {
        self.block();
        assert!(self.out[0][0].abs() > 0.1, "a release preserves the tail; a reset would cut it");
        assert!(self.out[0][BLOCK - 1].abs() < 0.001, "below -60 dB within the 1 ms release");
        for _ in 0..3 {
            self.block();
        }
        self.silent();
        assert_eq!(self.trace.resets.load(Relaxed), 0);
    }
}
