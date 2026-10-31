//! Inserts that read the track's note stream (#242, ADR 0038) through a realtime engine: an insert
//! whose `reads_track_notes` is true hears exactly what the track's instrument accepted, after the
//! MIDI effect slot, at the same frames; one that does not hears nothing; a skipped reader is told
//! to let go every block. Then the Filter itself on a Macro Oscillator track: the same output bit
//! for bit in 64- and 512-frame blocks with notes at odd frames, and no allocation on the audio
//! thread.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use transit_dsp::Plugin;
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_mix::{
    Adoption, Arrival, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, Message,
    MixEngine, NewInstance, NoteId, PlanCompiler, PlanGeneration, PlanLimits, PluginEvent, RenderFrame, RenderTarget, SlotGen, Source,
    mark_audio_thread, render_range,
};
use transit_model::{ChannelFormat, Clip, Insert, MidiEffectSlot, Session, TrackId, TrackKind};
use transit_rt::testing::{CountingAlloc, measure};
use transit_time::Range;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;
const INSTRUMENT: &str = "clap:test.rec";
const READER: &str = "clap:test.notes";
const DEAF: &str = "clap:test.deaf";

/// Who heard what: (plug-in id, absolute frame, event; `None` for an `all_notes_off`).
type Heard = Arc<Mutex<Vec<(&'static str, u64, Option<PluginEvent>)>>>;

/// Records every event at its absolute frame (the block's first frame plus the offset). As the
/// instrument it renders silence; as an insert it passes the audio through.
struct Spy {
    id: &'static str,
    reads: bool,
    heard: Heard,
    pending: Vec<(u32, Option<PluginEvent>)>,
    rendered: u64,
}

impl Plugin for Spy {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        &transit_dsp::plugins::utility::GAIN_INFO
    }
    fn prepare(&mut self, _: f32, _: usize, _: usize) {}
    /// Releases survive a reset, as in real hosts.
    fn reset(&mut self) {
        self.pending.retain(|(_, e)| e.is_none_or(|e| e.message.survives_reset()));
    }
    fn set_param(&mut self, _: &str, _: f32) -> bool {
        false
    }
    fn param(&self, _: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mut heard = self.heard.lock().unwrap();
        self.pending.sort_by_key(|(o, _)| *o);
        for (o, e) in self.pending.drain(..) {
            heard.push((self.id, self.rendered + u64::from(o), e));
        }
        self.rendered += frames as u64;
        if self.id == INSTRUMENT {
            for c in io.iter_mut() {
                c.iter_mut().take(frames).for_each(|x| *x = 0.0);
            }
        }
    }
    fn event(&mut self, offset: u32, ev: &PluginEvent) -> bool {
        self.pending.push((offset, Some(*ev)));
        true
    }
    fn all_notes_off(&mut self) {
        if self.id == INSTRUMENT {
            self.pending.push((0, None));
        } else {
            self.heard.lock().unwrap().push((self.id, self.rendered, None));
        }
    }
    fn reads_track_notes(&self) -> bool {
        self.reads
    }
}

fn target(max_block: usize) -> RenderTarget {
    RenderTarget { device: DeviceGeneration(3), sample_rate: SR, max_block: max_block as u32, out_channels: 0, in_channels: 0 }
}

struct Rig {
    mix: MixEngine,
    comp: PlanCompiler,
    heard: Heard,
    generation: PlanGeneration,
    pos: i64,
    out: Vec<Vec<f32>>,
    track: TrackId,
}

impl Rig {
    fn new(s: &Session, track: TrackId) -> Rig {
        let mut rig = Rig {
            mix: MixEngine::realtime_with_events(target(BLOCK), EngineEpoch::FIRST, EventLimits::default()),
            comp: PlanCompiler::new(target(BLOCK), PlanLimits::default()),
            heard: Arc::default(),
            generation: PlanGeneration::EMPTY,
            pos: 0,
            out: vec![vec![0.0; BLOCK]; 2],
            track,
        };
        rig.publish(s);
        rig
    }

    fn publish(&mut self, s: &Session) {
        self.generation = self.generation.checked_next().unwrap();
        let heard = Arc::clone(&self.heard);
        let mut make = move |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
            let id = [INSTRUMENT, READER, DEAF].into_iter().find(|id| *id == spec.id)?;
            let plugin = Box::new(Spy { id, reads: id == READER, heard: Arc::clone(&heard), pending: Vec::with_capacity(4096), rendered: 0 });
            Some(NewInstance { plugin, refused_state: false })
        };
        let mut env = self.comp.compile(Arc::new(s.clone()), self.comp.active(), self.generation, &mut make).unwrap();
        assert_eq!(self.mix.adopt_plan(&mut env), Adoption::Adopted);
        self.comp.reclaim(env);
    }

    fn render(&mut self, blocks: usize) {
        for _ in 0..blocks {
            self.mix.render_active(self.pos, BLOCK, &mut self.out);
            self.pos += BLOCK as i64;
        }
    }

    fn send(&mut self, at: u64, message: Message) -> EventOutcome {
        let target = self.comp.active_bindings().into_iter().find(|b| b.0 == self.track && b.1 == INSTRUMENT_SLOT).unwrap();
        self.mix.queue_event(&InstrumentEvent {
            engine: EngineEpoch::FIRST,
            target,
            due: Due::Frame(RenderFrame(at)),
            transport: None,
            source: Source { port: 1, channel: 0, generation: 1 },
            message,
            arrival: Arrival { host_ns: 0, sequence: 0 },
        })
    }

    fn heard_by(&self, id: &str) -> Vec<(u64, Option<PluginEvent>)> {
        self.heard.lock().unwrap().iter().filter(|(i, ..)| *i == id).map(|(_, f, e)| (*f, *e)).collect()
    }
}

/// An instrument track with the spy instrument, `inserts` and `midi_effects`, and a clip with three
/// notes and a sustain pedal (ticks: 960 per quarter at 120 BPM, 25 frames a tick).
fn session(inserts: &[(&str, bool)], midi_effects: &[(&str, &[(&str, f32)])]) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new(INSTRUMENT));
    for (i, (id, bypass)) in inserts.iter().enumerate() {
        let mut ins = Insert::new(*id);
        ins.bypass = *bypass;
        tr.mixer.inserts[i] = Some(ins);
    }
    for (id, params) in midi_effects {
        let mut fx = MidiEffectSlot::new(*id);
        for (k, v) in *params {
            fx.params.insert((*k).into(), *v);
        }
        tr.midi_effects.push(fx);
    }
    let mut seq = Sequence::default();
    for (pitch, start, length) in [(60, 1, 2), (64, 4, 9), (67, 8, 1)] {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length });
    }
    seq.ctrls.push(CtrlEvent { tick: 3, channel: 0, kind: CtrlKind::Cc(64), value: 127 });
    seq.ctrls.push(CtrlEvent { tick: 11, channel: 0, kind: CtrlKind::Cc(64), value: 0 });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "notes", 0, 48_000, seq));
    (s, t)
}

fn note_stream(e: &PluginEvent) -> bool {
    matches!(e.message, Message::NoteOn { .. } | Message::NoteOff { .. } | Message::Control { cc: 64 | 120 | 123, .. })
}

/// Clip notes, live notes and a controller over three blocks, then a Stop and one more block.
fn play(rig: &mut Rig) {
    rig.render(1);
    let now = rig.mix.next_frame().0;
    assert_eq!(rig.send(now + 30, Message::NoteOn { key: 72, velocity: 90, id: NoteId::NONE }), EventOutcome::Queued);
    assert_eq!(rig.send(now + 31, Message::Control { cc: 1, value: 99 }), EventOutcome::Queued);
    assert_eq!(rig.send(now + 401, Message::NoteOff { key: 72, velocity: 0, id: NoteId::NONE }), EventOutcome::Queued);
    assert_eq!(rig.send(now + 700, Message::NoteOn { key: 74, velocity: 70, id: NoteId::NONE }), EventOutcome::Queued);
    rig.render(3);
    rig.mix.reset();
    rig.render(1);
}

#[test]
fn a_reading_insert_hears_what_the_instrument_accepted_after_the_midi_effects() {
    let (s, t) = session(&[(DEAF, false), (READER, false)], &[("midi_transpose", &[("semitones", 12.0)])]);
    let mut rig = Rig::new(&s, t);
    play(&mut rig);
    let instrument: Vec<(u64, PluginEvent)> =
        rig.heard_by(INSTRUMENT).into_iter().filter_map(|(f, e)| e.map(|e| (f, e))).filter(|(_, e)| note_stream(e)).collect();
    let reader: Vec<(u64, PluginEvent)> =
        rig.heard_by(READER).into_iter().map(|(f, e)| (f, e.expect("a reader that runs is never told to let go"))).collect();
    assert!(instrument.len() >= 10, "clip and live notes, the pedal and the Stop's releases: {instrument:?}");
    assert_eq!(reader, instrument, "the same events at the same frames");
    assert!(
        reader.iter().all(|(_, e)| !matches!(e.message, Message::NoteOn { key, .. } if key < 72)),
        "the insert hears the transposed notes, after the MIDI effect slot"
    );
    assert!(rig.heard_by(DEAF).is_empty(), "an insert that does not read notes gets none");
}

#[test]
fn without_midi_effects_the_reader_hears_the_note_tables_deliveries() {
    let (s, t) = session(&[(READER, false)], &[]);
    let mut rig = Rig::new(&s, t);
    play(&mut rig);
    let instrument: Vec<(u64, PluginEvent)> =
        rig.heard_by(INSTRUMENT).into_iter().filter_map(|(f, e)| e.map(|e| (f, e))).filter(|(_, e)| note_stream(e)).collect();
    let reader: Vec<(u64, PluginEvent)> = rig.heard_by(READER).into_iter().filter_map(|(f, e)| e.map(|e| (f, e))).collect();
    assert!(instrument.iter().any(|(_, e)| matches!(e.message, Message::NoteOn { key: 60, .. })));
    assert_eq!(reader, instrument);
}

#[test]
fn a_bypassed_reader_hears_no_notes_and_lets_go_each_block() {
    let (s, t) = session(&[(READER, true)], &[]);
    let mut rig = Rig::new(&s, t);
    play(&mut rig);
    let heard = rig.heard_by(READER);
    assert!(heard.len() >= 4, "{heard:?}");
    assert!(heard.iter().all(|(_, e)| e.is_none()), "only all-notes-off: {heard:?}");
}

// ---------------------------------------------------------------------------------------------
// The Filter on a Macro Oscillator track

fn filter_session() -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("macro_oscillator"));
    let mut f = Insert::new("filter");
    for (k, v) in [
        ("slope", 1.0),
        ("cutoff", 300.0),
        ("resonance", 60.0),
        ("env_amount", 70.0),
        ("env_decay", 150.0),
        ("lfo_amount", 15.0),
        ("lfo_sync", 1.0),
        ("lfo_shape", 1.0),
        ("key_track", 40.0),
        ("follow_amount", 10.0),
        ("amp_on", 1.0),
        ("amp_sustain", 60.0),
        ("amp_release", 120.0),
    ] {
        f.params.insert(k.into(), v);
    }
    tr.mixer.inserts[0] = Some(f);
    let mut seq = Sequence::default();
    // Odd frames: 25 frames a tick.
    for (pitch, start, length) in [(48, 1, 300), (55, 157, 301), (60, 523, 99), (43, 901, 777)] {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length });
    }
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "phrase", 0, 96_000, seq));
    s
}

#[test]
fn the_filter_renders_the_same_bits_in_64_and_512_frame_blocks() {
    MixEngine::force_serial(true);
    let s = filter_session();
    let range = Range::new(0, 48_000);
    let a = render_range(&s, range, 512);
    let b = render_range(&s, range, 64);
    assert!(a.iter().flatten().any(|v| v.abs() > 0.01), "the track sounds");
    for (x, y) in a.iter().zip(&b) {
        assert!(x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits()), "64-frame blocks differ from 512-frame blocks");
    }
    // The amp stage plays from the notes: without them the track is silent.
    let mut silent = s.clone();
    silent.tracks[0].playlist_mut().unwrap().clips.clear();
    assert!(render_range(&silent, range, 512).iter().flatten().all(|v| *v == 0.0));
}

#[test]
fn the_filter_on_a_strip_allocates_nothing_on_the_audio_thread() {
    MixEngine::force_serial(true);
    mark_audio_thread();
    let s = filter_session();
    let t = s.tracks[0].id;
    let mut mix = MixEngine::realtime_with_events(target(BLOCK), EngineEpoch::FIRST, EventLimits::default());
    let mut comp = PlanCompiler::new(target(BLOCK), PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| transit_mix::create_instance(&spec.id, None, SR as f32, BLOCK, spec.ch);
    let mut env = comp.compile(Arc::new(s.clone()), PlanGeneration::EMPTY, PlanGeneration::EMPTY.checked_next().unwrap(), &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    let target = comp.active_bindings().into_iter().find(|b| b.0 == t && b.1 == INSTRUMENT_SLOT).unwrap();
    let ev = |at: u64, message: Message| InstrumentEvent {
        engine: EngineEpoch::FIRST,
        target,
        due: Due::Frame(RenderFrame(at)),
        transport: None,
        source: Source { port: 1, channel: 0, generation: 1 },
        message,
        arrival: Arrival { host_ns: 0, sequence: 0 },
    };
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut pos = 0i64;
    let mut loudest = 0.0f32;
    let ((), stats) = measure(|| {
        for _ in 0..120 {
            mix.render_active(pos, BLOCK, &mut out);
            loudest = out.iter().flatten().fold(loudest, |m, v| m.max(v.abs()));
            pos += BLOCK as i64;
        }
        let now = mix.next_frame().0;
        assert!(mix.queue_event(&ev(now + 13, Message::NoteOn { key: 70, velocity: 90, id: NoteId::NONE })).accepted());
        assert!(mix.queue_event(&ev(now + 14, Message::Control { cc: 64, value: 127 })).accepted());
        assert!(mix.queue_event(&ev(now + 3000, Message::NoteOff { key: 70, velocity: 0, id: NoteId::NONE })).accepted());
        assert!(mix.queue_event(&ev(now + 6000, Message::Control { cc: 64, value: 0 })).accepted());
        for _ in 0..60 {
            mix.render_active(pos, BLOCK, &mut out);
            pos += BLOCK as i64;
        }
        mix.all_notes_off();
        mix.render_active(pos, BLOCK, &mut out);
        mix.reset();
        mix.render_active(pos, BLOCK, &mut out);
    });
    assert!(stats.is_zero(), "the Filter's strip allocated or freed: {stats:?}");
    assert!(loudest > 0.01, "it sounds: {loudest}");
}
