//! ADR 0064 slice HM2 through the engine with an in-process hosted note plug-in in the MIDI effect
//! slot (made by the test's factory for the stage's slot): on the audio thread the stage allocates
//! nothing, through note input, its output, a Stop (whose releases wait between blocks), a state
//! load (a new generation, a barrier) and a failing block; and the stage adds nothing to the track's
//! audio, though the plug-in writes a loud signal into its own buffers.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use transit_dsp::Plugin;
use transit_dsp::event::{Message, NoteId, PluginEvent};
use transit_dsp::hosted_out::{CallRecord, CallReport, HostedOut, HostedOutBuffer, Lifecycle, NoteOutputInfo, OutForm, OutKind, OutValues};
use transit_mix::{
    Adoption, Arrival, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, MixEngine,
    NewInstance, PlanCompiler, PlanGeneration, PlanLimits, RenderFrame, RenderTarget, SlotGen, Source, mark_audio_thread, midi_fx_of_slot,
};
use transit_model::{ChannelFormat, Insert, MidiEffectSlot, Session, TrackKind};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;

/// What the test changes from outside: the generation (a state load), a failing next block.
#[derive(Default)]
struct Control {
    generation: AtomicU64,
    fail_next: AtomicBool,
}

/// A note effect that echoes its input 12 semitones up, holds a note of its own, writes `loud`
/// into its audio buffers, and allocates nothing while it processes (its queue and output are
/// fixed storage made before `prepare`).
struct Echo {
    ctl: Arc<Control>,
    buf: HostedOutBuffer,
    queue: [(u32, PluginEvent); 64],
    queued: usize,
    reset_pending: bool,
    frame: u64,
    loud: f32,
}

impl Echo {
    fn new(ctl: Arc<Control>, loud: f32) -> Echo {
        let empty = PluginEvent { channel: 0, note_id: -1, message: Message::Control { cc: 0, value: 0 } };
        Echo { ctl, buf: HostedOutBuffer::empty(), queue: [(0, empty); 64], queued: 0, reset_pending: false, frame: 0, loud }
    }
}

fn note(kind: OutKind, key: u8, id: i32, v: f64) -> HostedOut {
    HostedOut {
        offset: 0,
        seq: 0,
        kind,
        port: 0,
        channel: 0,
        key: i16::from(key),
        note_id: id,
        form: OutForm::Native,
        values: OutValues::Velocity(v),
    }
}

impl Plugin for Echo {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    fn reset(&mut self) {
        self.reset_pending = true;
    }
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        let Some(slot) = self.queue.get_mut(self.queued) else { return false };
        *slot = (offset, *ev);
        self.queued += 1;
        true
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = self.loud);
        }
        self.buf.begin_block();
        let n = frames as u32;
        for k in 0..self.queued {
            let (o, ev) = self.queue[k];
            let e = match ev.message {
                Message::NoteOn { key, velocity, .. } => note(OutKind::NoteOn, key + 12, ev.note_id, f64::from(velocity) / 127.0),
                Message::NoteOff { key, .. } => note(OutKind::NoteOff, key + 12, ev.note_id, 0.0),
                _ => continue,
            };
            self.buf.push(i64::from(o), 0, n, e);
        }
        self.queued = 0;
        // A note of its own every 1,024 frames, held 300 frames.
        let (from, to) = (self.frame, self.frame + frames as u64);
        for f in from..to {
            if f % 1024 == 100 {
                self.buf.push((f - from) as i64, 0, n, note(OutKind::NoteOn, 90, 77, 1.0));
            } else if f % 1024 == 400 {
                self.buf.push((f - from) as i64, 0, n, note(OutKind::NoteOff, 90, 77, 0.0));
            }
        }
        let error = self.ctl.fail_next.swap(false, Ordering::Relaxed);
        self.buf.record_call(CallRecord {
            generation: self.ctl.generation.load(Ordering::Relaxed),
            reset_before: std::mem::take(&mut self.reset_pending),
            error,
        });
        self.frame = to;
    }
    fn note_output(&self) -> Option<NoteOutputInfo> {
        Some(NoteOutputInfo { read: 0, declared: 1 })
    }
    fn take_note_output(&mut self, out: &mut dyn FnMut(HostedOut) -> bool) -> CallReport {
        self.buf.take(out)
    }
    fn lifecycle(&self) -> Lifecycle {
        Lifecycle { generation: self.ctl.generation.load(Ordering::Relaxed), active: true }
    }
    fn restart_processing(&mut self) {
        self.reset_pending = true;
    }
    fn collect_note_output(&mut self, on: bool) {
        if on {
            self.buf.reserve();
        }
    }
}

type Heard = Arc<Mutex<Vec<(u64, PluginEvent)>>>;

/// The instrument: writes `dc` to its outputs and, unless `quiet`, writes down every event (the
/// allocation probe's instrument is quiet).
struct Recorder {
    frame: u64,
    heard: Option<Heard>,
    dc: f32,
}

impl Plugin for Recorder {
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
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = self.dc);
        }
        self.frame += frames as u64;
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        if let Some(h) = &self.heard {
            h.lock().unwrap().push((self.frame + u64::from(offset), *ev));
        }
        true
    }
}

const HOSTED: &str = "clap:test.echo";

struct Rig {
    mix: MixEngine,
    comp: PlanCompiler,
    binding: transit_mix::SlotBinding,
    out: Vec<Vec<f32>>,
    pos: i64,
}

fn rig(chain: bool, ctl: &Arc<Control>, heard: Option<Heard>, loud: f32) -> Rig {
    let target = RenderTarget { device: DeviceGeneration(2), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let mut mix = MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default());
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut s = Session::default();
    s.edit.click = false;
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.track_mut(t).unwrap().instrument = Some(Insert::new("macro_oscillator"));
    if chain {
        s.track_mut(t).unwrap().midi_effects.push(MidiEffectSlot::new(HOSTED));
    }
    let ctl = Arc::clone(ctl);
    let mut make = move |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
        let plugin: Box<dyn Plugin> = if midi_fx_of_slot(spec.slot).is_some() {
            let mut p = Echo::new(Arc::clone(&ctl), loud);
            p.collect_note_output(true);
            p.prepare(SR as f32, BLOCK, spec.ch);
            Box::new(p)
        } else {
            assert_eq!(spec.slot, INSTRUMENT_SLOT);
            Box::new(Recorder { frame: 0, heard: heard.clone(), dc: 0.25 })
        };
        Some(NewInstance { plugin, refused_state: false })
    };
    let mut env = comp.compile(Arc::new(s), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    let binding = comp.active_bindings().into_iter().find(|b| b.1 == INSTRUMENT_SLOT).unwrap();
    Rig { mix, comp, binding, out: vec![vec![0.0; BLOCK]; 2], pos: 0 }
}

impl Rig {
    fn live(&mut self, at: u64, message: Message) {
        let ev = InstrumentEvent {
            engine: EngineEpoch::FIRST,
            target: self.binding,
            due: Due::Frame(RenderFrame(at)),
            transport: None,
            source: Source { port: 1, channel: 0, generation: 1 },
            message,
            arrival: Arrival { host_ns: 0, sequence: 0 },
        };
        assert_eq!(self.mix.queue_event(&ev), EventOutcome::Queued);
    }

    fn block(&mut self) {
        self.mix.render_active(self.pos, BLOCK, &mut self.out);
        self.pos += BLOCK as i64;
    }
}

#[test]
fn a_hosted_stage_allocates_nothing_on_the_audio_thread() {
    MixEngine::force_serial(true);
    mark_audio_thread();
    let ctl = Arc::new(Control::default());
    ctl.generation.store(1, Ordering::Relaxed);
    let mut r = rig(true, &ctl, None, 0.5);
    // Warm up (the installation's barrier and the first reset), not measured.
    for _ in 0..4 {
        r.block();
    }
    assert!(r.comp.active_bindings().iter().any(|b| midi_fx_of_slot(b.1).is_some()), "the stage's instance is in the plan");
    let ((), stats) = measure(|| {
        let now = r.mix.next_frame().0;
        for k in 0..20u8 {
            r.live(now + u64::from(k) * 40, Message::NoteOn { key: 40 + k, velocity: 100, id: NoteId::NONE });
        }
        for _ in 0..8 {
            r.block();
        }
        // Stop with notes held: the chain's releases run between blocks and wait at the stage.
        r.mix.reset();
        r.block();
        // A state load: a new generation, the barrier at the next block, the reset before the call.
        ctl.generation.store(2, Ordering::Relaxed);
        r.block();
        r.block();
        // A failing block: discarded whole, the barrier, silent until Stop; then Stop.
        ctl.fail_next.store(true, Ordering::Relaxed);
        let now = r.mix.next_frame().0;
        r.live(now + 10, Message::NoteOn { key: 70, velocity: 100, id: NoteId::NONE });
        r.block();
        r.block();
        r.mix.reset();
        for _ in 0..4 {
            r.block();
        }
    });
    assert_eq!(stats, AllocStats::default(), "the hosted stage on the audio thread");
    let st = r.mix.event_status().midi_fx;
    assert!(st.hosted_discarded > 0, "the failing block's output was discarded: {st:?}");
    assert!(st.released_on_edit > 0, "barriers ran: {st:?}");
}

#[test]
fn the_stage_adds_nothing_to_the_tracks_audio() {
    MixEngine::force_serial(true);
    let ctl = Arc::new(Control::default());
    ctl.generation.store(1, Ordering::Relaxed);
    let heard: Heard = Arc::default();
    // The plug-in writes 0.5 into its own buffers; the instrument writes 0.25.
    let mut with = rig(true, &ctl, Some(Arc::clone(&heard)), 0.5);
    let mut without = rig(false, &ctl, None, 0.5);
    let mut max_diff = 0.0f32;
    for _ in 0..16 {
        with.block();
        without.block();
        for (a, b) in with.out.iter().zip(&without.out) {
            for (x, y) in a.iter().zip(b) {
                max_diff = max_diff.max((x - y).abs());
            }
        }
    }
    assert!(without.out[0].iter().all(|x| *x != 0.0), "the instrument's own signal is there");
    assert_eq!(max_diff, 0.0, "the track sounds the same with the stage as without it");
    let ons = heard.lock().unwrap().iter().filter(|(_, e)| matches!(e.message, Message::NoteOn { key: 90, .. })).count();
    assert!(ons >= 3, "the stage ran: its notes reached the instrument ({ons})");
}
