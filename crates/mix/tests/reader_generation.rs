//! #275: retained readers release the old instrument while preserving their envelope tails.
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
use transit_model::{ChannelFormat, Insert, MidiEffectSlot, Session, TrackId, TrackKind};
use transit_rt::testing::{CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;
const BLOCK: usize = 64;
const A: &str = "clap:test.source_a";
const B: &str = "clap:test.source_b";
const READER: &str = "clap:test.filter";

#[derive(Default)]
struct Trace {
    releases: AtomicUsize,
    resets: AtomicUsize,
    ons: AtomicUsize,
    off: AtomicUsize,
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
    trace: Arc<Trace>,
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
            _ => {}
        }
        self.filter.event(offset, ev)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.filter.process(io, frames);
    }
}

struct Rig {
    session: Session,
    track: TrackId,
    mix: MixEngine,
    compiler: PlanCompiler,
    trace: Arc<Trace>,
    latency: usize,
    clock: i64,
    out: Vec<Vec<f32>>,
}

impl Rig {
    fn new(latency: usize, kind: TrackKind) -> Self {
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

    fn publish(&mut self) {
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

    fn generation(&self, slot: usize) -> SlotGen {
        self.compiler.active_bindings().iter().find(|b| b.0 == self.track && b.1 == slot).unwrap().2
    }

    fn send(&mut self, message: Message) {
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

    fn on(&mut self) {
        self.send(Message::NoteOn { key: 60, velocity: 127, id: NoteId::NONE });
    }
    fn off(&mut self) {
        self.send(Message::NoteOff { key: 60, velocity: 0, id: NoteId::NONE });
    }

    fn block(&mut self) {
        let ((), stats) = measure(|| self.mix.render_active(self.clock, BLOCK, &mut self.out));
        assert!(stats.is_zero(), "render: {stats:?}");
        self.clock += BLOCK as i64;
    }

    fn sounding(&self) {
        assert!(self.out[0][BLOCK - 1].abs() > 0.1, "held amp envelope must sound");
    }
    fn silent(&self) {
        assert!(self.out.iter().flatten().all(|x| *x == 0.0), "amp envelope must finish releasing");
    }

    fn released_tail(&mut self) {
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

#[test]
fn removing_the_instrument_releases_a_held_filter_without_allocating_or_cutting_its_tail() {
    let mut rig = Rig::new(0, TrackKind::Instrument);
    rig.on();
    rig.block();
    rig.sounding();
    let reader = rig.generation(1);
    rig.session.track_mut(rig.track).unwrap().instrument = None;
    rig.publish();
    assert_eq!(rig.generation(1), reader);
    rig.released_tail();
    assert_eq!(rig.trace.releases.load(Relaxed), 1);
}

#[test]
fn replacing_with_another_kind_or_a_fresh_same_kind_releases_then_reopens() {
    for replacement in [B, A] {
        let mut rig = Rig::new(0, TrackKind::Instrument);
        rig.on();
        rig.block();
        rig.sounding();
        let old = rig.generation(INSTRUMENT_SLOT);
        let reader = rig.generation(1);
        rig.session.track_mut(rig.track).unwrap().instrument = Some(Insert::new(replacement));
        rig.publish();
        assert_ne!(rig.generation(INSTRUMENT_SLOT), old, "fresh instance, even of the same kind");
        assert_eq!(rig.generation(1), reader);
        rig.released_tail();
        rig.on();
        rig.block();
        rig.sounding();
        rig.off();
        rig.released_tail();
        assert_eq!(rig.trace.releases.load(Relaxed), 1);
        assert_eq!(rig.trace.ons.load(Relaxed), 2);
    }
}

#[test]
fn the_new_instruments_first_block_can_start_a_note_after_releasing_the_old_source() {
    let mut rig = Rig::new(0, TrackKind::Instrument);
    rig.on();
    rig.block();
    rig.session.track_mut(rig.track).unwrap().instrument = Some(Insert::new(B));
    rig.publish();
    rig.on();
    rig.block();
    rig.sounding();
    assert_eq!(rig.trace.releases.load(Relaxed), 1);
    rig.off();
    rig.released_tail();
}

#[test]
fn removal_and_replacement_discard_pending_starts_without_counting_an_overflow() {
    for replacement in [None, Some(B), Some(A)] {
        let mut rig = Rig::new(769, TrackKind::Instrument);
        rig.on();
        rig.block();
        assert_eq!(rig.trace.ons.load(Relaxed), 0, "the start is still in the reader's carry");
        rig.session.track_mut(rig.track).unwrap().instrument = replacement.map(Insert::new);
        rig.publish();
        for _ in 0..32 {
            rig.block();
            rig.silent();
        }
        assert_eq!(rig.trace.ons.load(Relaxed), 0, "the retired source must never open the reader");
        assert_eq!(rig.trace.releases.load(Relaxed), 1);
        assert_eq!(rig.mix.event_status().midi_fx.note_view_dropped, 0);
    }
}

#[test]
fn an_unrelated_insert_edit_preserves_the_held_note_and_pending_carry_bit_for_bit() {
    for latency in [0, 769] {
        let mut control = Rig::new(latency, TrackKind::Instrument);
        let mut edited = Rig::new(latency, TrackKind::Instrument);
        control.on();
        edited.on();
        control.block();
        edited.block();
        let instrument = edited.generation(INSTRUMENT_SLOT);
        let reader = edited.generation(1);
        edited.session.track_mut(edited.track).unwrap().mixer.inserts[3] = Some(Insert::new("gain"));
        edited.publish();
        assert_eq!(edited.generation(INSTRUMENT_SLOT), instrument);
        assert_eq!(edited.generation(1), reader);
        for _ in 0..32 {
            control.block();
            edited.block();
            assert_eq!(edited.out, control.out);
        }
        edited.sounding();
        assert_eq!(edited.trace.ons.load(Relaxed), 1);
        assert_eq!(edited.trace.releases.load(Relaxed), 0);
        assert_eq!(edited.trace.resets.load(Relaxed), 0);
    }
}

#[test]
fn replacing_a_sustained_source_lifts_its_pedal_for_the_new_sources_note_off() {
    let mut rig = Rig::new(0, TrackKind::Instrument);
    rig.on();
    rig.send(Message::Control { cc: 64, value: 127 });
    rig.block();
    rig.off();
    rig.block();
    rig.sounding();
    rig.session.track_mut(rig.track).unwrap().instrument = Some(Insert::new(B));
    rig.publish();
    rig.released_tail();
    rig.on();
    rig.block();
    rig.sounding();
    rig.off();
    rig.released_tail();
}

#[test]
fn midi_effect_edits_keep_the_instrument_generation_and_do_not_reset_the_reader() {
    let mut rig = Rig::new(0, TrackKind::Instrument);
    rig.on();
    rig.block();
    let instrument = rig.generation(INSTRUMENT_SLOT);
    rig.session.track_mut(rig.track).unwrap().midi_effects.push(MidiEffectSlot::new("midi_transpose"));
    rig.publish();
    rig.block();
    assert_eq!(rig.generation(INSTRUMENT_SLOT), instrument);
    assert_eq!(rig.trace.releases.load(Relaxed), 0);
    assert_eq!(rig.trace.resets.load(Relaxed), 0);
}

#[test]
fn a_reader_on_an_aux_has_no_source_generation_to_release() {
    let mut rig = Rig::new(0, TrackKind::Aux);
    rig.block();
    rig.session.track_mut(rig.track).unwrap().mixer.inserts[3] = Some(Insert::new("gain"));
    rig.publish();
    rig.block();
    assert_eq!(rig.trace.releases.load(Relaxed), 0);
    assert_eq!(rig.trace.resets.load(Relaxed), 0);
}
