//! Racks (ADR 0034, #197) in the mix: the rack's MIDI reaches its members through their zones,
//! scaled by the fades; note-offs follow the note-ons that were delivered; controllers reach every
//! member; the rack sums its members exactly as a group does; distribution allocates nothing on
//! the audio thread; a macro lane drives the mapped parameter with the mapping's range and curve.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::float_cmp)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use transit_dsp::Plugin;
use transit_mix::{
    Adoption, Arrival, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, InstanceSpec, InstrumentEvent, Message, MixEngine, NewInstance,
    NoteId, PlanCompiler, PlanGeneration, PlanLimits, PluginEvent, RenderFrame, RenderTarget, SlotGen, Source, mark_audio_thread, rack_binding,
    render_range,
};
use transit_model::{
    AutoParam, BusId, ChannelFormat, Clip, Insert, KeyFrom, KeySource, MACRO_SLOT, MacroMapping, ParamRange, Rack, RackMember, Route, Session, Tap,
    TrackId, TrackKind, Zone, macro_param,
};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};
use transit_time::Range;

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;

/// Member A plays keys 0..=64, fading out over 61..=64; member B plays 60..=127, fading in over
/// 60..=62 (full from 63). Key 62 reaches both: A at 3/5, B at 3/4 of its velocity.
fn zones() -> [RackMember; 2] {
    [
        RackMember { track: TrackId(0), key_zone: Zone { low: 0, fade_low: 0, fade_high: 60, high: 64 }, vel_zone: Zone::FULL },
        RackMember { track: TrackId(0), key_zone: Zone { low: 60, fade_low: 63, fade_high: 127, high: 127 }, vel_zone: Zone::FULL },
    ]
}

/// A rack (a routing folder with its bus) of two instrument tracks playing `instrument`; `rack`
/// false leaves the same routing as a plain group.
fn rack_session(instrument: &str, rack: bool) -> (Session, TrackId, [TrackId; 2], BusId) {
    let mut s = Session::default();
    let bus = s.add_bus("Rack Bus", ChannelFormat::Stereo);
    let folder = s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some("Rack"));
    s.track_mut(folder).unwrap().mixer.input = Route::Bus(bus);
    let mut ids = [TrackId(0); 2];
    let mut members = zones();
    for (k, m) in members.iter_mut().enumerate() {
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(["A", "B"][k]));
        let tr = s.track_mut(t).unwrap();
        tr.instrument = Some(Insert::new(instrument));
        tr.folder = Some(folder);
        tr.mixer.output = Route::Bus(bus);
        m.track = t;
        ids[k] = t;
    }
    if rack {
        s.track_mut(folder).unwrap().rack = Some(Rack { members: members.to_vec(), ..Rack::default() });
    }
    (s, folder, ids, bus)
}

fn note(pitch: u8, velocity: u8, start: i64) -> transit_midi::Note {
    transit_midi::Note { pitch, velocity, release_velocity: 0, channel: 0, start, length: 480 }
}

fn put_clip(s: &mut Session, t: TrackId, notes: &[transit_midi::Note]) {
    let mut seq = transit_midi::Sequence::default();
    seq.notes.extend_from_slice(notes);
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "notes", 0, 96_000, seq));
}

/// The rack's notes: 48 (A only), 62 (both, in the fades), 66 and 72 (B only).
fn rack_notes() -> Vec<transit_midi::Note> {
    vec![note(48, 100, 0), note(62, 100, 240), note(66, 100, 480), note(72, 100, 960)]
}

#[test]
fn the_rack_plays_its_clips_through_the_zones_and_sums_like_its_members_alone() {
    let range = Range::new(0, 72_000);
    let (mut rack, folder, _, _) = rack_session("subtractive_synth", true);
    put_clip(&mut rack, folder, &rack_notes());
    let got = render_range(&rack, range, BLOCK);
    assert!(got.iter().flatten().any(|x| x.abs() > 1e-4), "the rack sounds");

    // The same routing without a rack, each member holding exactly the notes its zones admit at
    // the velocities the fades give (62: A 100 x 3/5 = 60, B 100 x 3/4 = 75).
    let a_notes = [note(48, 100, 0), note(62, 60, 240)];
    let b_notes = [note(62, 75, 240), note(66, 100, 480), note(72, 100, 960)];
    let (mut group, _, [a, b], _) = rack_session("subtractive_synth", false);
    put_clip(&mut group, a, &a_notes);
    put_clip(&mut group, b, &b_notes);
    assert_eq!(got, render_range(&group, range, BLOCK), "bit-equal to the members playing the zoned notes themselves");

    // And the sum of each member rendered alone.
    let (mut only_a, _, [a, _], _) = rack_session("subtractive_synth", false);
    put_clip(&mut only_a, a, &a_notes);
    let (mut only_b, _, [_, b], _) = rack_session("subtractive_synth", false);
    put_clip(&mut only_b, b, &b_notes);
    let (ra, rb) = (render_range(&only_a, range, BLOCK), render_range(&only_b, range, BLOCK));
    let summed: Vec<Vec<f32>> = ra.iter().zip(&rb).map(|(x, y)| x.iter().zip(y).map(|(p, q)| p + q).collect()).collect();
    assert_eq!(got, summed, "the rack's output is its members' outputs summed, bit for bit");
}

/// What each member's instrument heard, by track.
type Heard = Arc<Mutex<HashMap<TrackId, Vec<Option<PluginEvent>>>>>;

/// The last value each (track, slot) instance was given, any parameter.
type Params = Arc<Mutex<HashMap<(TrackId, usize), (f32, u32)>>>;

struct Recorder {
    inner: Box<dyn Plugin>,
    heard: Heard,
    params: Params,
    key: (TrackId, usize),
    track: TrackId,
    pending: Vec<Option<PluginEvent>>,
}

impl Plugin for Recorder {
    fn info(&self) -> &'static transit_dsp::PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        if let Ok(mut p) = self.params.try_lock()
            && let Some(x) = p.get_mut(&self.key)
        {
            *x = (v, x.1 + 1);
        }
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        // Recorded in place (no allocation while the probe counts: the vector has room).
        if let Ok(mut h) = self.heard.try_lock()
            && let Some(v) = h.get_mut(&self.track)
        {
            for e in self.pending.drain(..) {
                if v.len() < v.capacity() {
                    v.push(e);
                }
            }
        }
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }
    fn event(&mut self, _offset: u32, ev: &PluginEvent) -> bool {
        if self.pending.len() < self.pending.capacity() {
            self.pending.push(Some(*ev));
        }
        true
    }
    fn all_notes_off(&mut self) {}
}

struct Rig {
    mix: MixEngine,
    comp: PlanCompiler,
    heard: Heard,
    params: Params,
    generation: PlanGeneration,
    pos: i64,
    out: Vec<Vec<f32>>,
    session: Session,
    rack: TrackId,
    members: [TrackId; 2],
}

impl Rig {
    fn new() -> Rig {
        let target = RenderTarget { device: DeviceGeneration(7), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
        let (session, rack, members, _) = rack_session("clap:test.rack", true);
        let heard: Heard = Arc::default();
        let params: Params = Arc::default();
        for m in members {
            heard.lock().unwrap().insert(m, Vec::with_capacity(4096));
            params.lock().unwrap().insert((m, 0), (f32::NAN, 0));
        }
        let mut rig = Rig {
            mix: MixEngine::realtime_with_events(target, EngineEpoch::FIRST, EventLimits::default()),
            comp: PlanCompiler::new(target, PlanLimits::default()),
            heard,
            params,
            generation: PlanGeneration::EMPTY,
            pos: 0,
            out: vec![vec![0.0; BLOCK]; 2],
            session: session.clone(),
            rack,
            members,
        };
        rig.publish(&session);
        rig.render(1);
        rig.clear();
        rig
    }

    fn publish(&mut self, s: &Session) {
        self.generation = self.generation.checked_next().unwrap();
        let (heard, params) = (Arc::clone(&self.heard), Arc::clone(&self.params));
        let mut make = move |spec: &InstanceSpec, _: SlotGen| -> Option<NewInstance> {
            let mut inner = transit_dsp::create("gain")?;
            inner.prepare(SR as f32, BLOCK, spec.ch);
            let plugin = Box::new(Recorder {
                inner,
                heard: Arc::clone(&heard),
                params: Arc::clone(&params),
                key: (spec.track, spec.slot),
                track: spec.track,
                pending: Vec::with_capacity(4096),
            });
            Some(NewInstance { plugin, refused_state: false })
        };
        let mut env = self.comp.compile(Arc::new(s.clone()), self.comp.active(), self.generation, &mut make).unwrap();
        assert_eq!(self.mix.adopt_plan(&mut env), Adoption::Adopted);
        self.comp.reclaim(env);
        self.session = s.clone();
    }

    fn render(&mut self, blocks: usize) {
        for _ in 0..blocks {
            self.mix.render_active(self.pos, BLOCK, &mut self.out);
            self.pos += BLOCK as i64;
        }
    }

    fn clear(&self) {
        for v in self.heard.lock().unwrap().values_mut() {
            v.clear();
        }
    }

    fn event(&self, message: Message) -> InstrumentEvent {
        InstrumentEvent {
            engine: EngineEpoch::FIRST,
            target: rack_binding(self.rack),
            due: Due::Frame(RenderFrame(self.mix.next_frame().0)),
            transport: None,
            source: Source { port: 2, channel: 0, generation: 1 },
            message,
            arrival: Arrival { host_ns: 0, sequence: 0 },
        }
    }

    fn send(&mut self, message: Message) -> EventOutcome {
        let ev = self.event(message);
        self.mix.queue_event(&ev)
    }

    /// The note and controller messages member `k` heard, in order.
    fn heard(&self, k: usize) -> Vec<Message> {
        self.heard.lock().unwrap()[&self.members[k]].iter().flatten().map(|e| e.message).collect()
    }
}

fn on(key: u8, velocity: u8) -> Message {
    Message::NoteOn { key, velocity, id: NoteId::NONE }
}

fn off(key: u8) -> Message {
    Message::NoteOff { key, velocity: 0, id: NoteId::NONE }
}

fn ons(m: &[Message]) -> Vec<(u8, u8)> {
    m.iter().filter_map(|m| if let Message::NoteOn { key, velocity, .. } = m { Some((*key, *velocity)) } else { None }).collect()
}

fn offs(m: &[Message]) -> Vec<u8> {
    m.iter().filter_map(|m| if let Message::NoteOff { key, .. } = m { Some(*key) } else { None }).collect()
}

#[test]
fn live_notes_reach_the_members_whose_zones_contain_them_scaled_by_the_fades() {
    MixEngine::force_serial(true);
    let mut rig = Rig::new();
    for (key, member_count) in [(48, 1), (62, 2), (72, 1)] {
        assert!(rig.send(on(key, 100)).accepted(), "{key} is queued on {member_count} member(s)");
    }
    assert_eq!(rig.send(Message::PitchBend { value: 9000 }), EventOutcome::Queued, "a bend reaches every member");
    rig.render(1);
    let (a, b) = (rig.heard(0), rig.heard(1));
    assert_eq!(ons(&a), [(48, 100), (62, 60)], "A: 48 whole, 62 at 3/5; never 72");
    assert_eq!(ons(&b), [(62, 75), (72, 100)], "B: 62 at 3/4, 72 whole; never 48");
    for m in [&a, &b] {
        assert!(m.contains(&Message::PitchBend { value: 9000 }));
    }
    // Velocity zones fade the velocity too, and the two factors multiply.
    let mut s = rig.session.clone();
    s.track_mut(rig.rack).unwrap().rack.as_mut().unwrap().members[1].vel_zone = Zone { low: 1, fade_low: 4, fade_high: 100, high: 101 };
    rig.publish(&s);
    rig.clear();
    assert!(rig.send(on(61, 101)).accepted());
    rig.render(1);
    assert_eq!(ons(&rig.heard(1)), [(61, 25)], "B at 61: key 2/4 x velocity 1/2 of 101 = 25.25");
}

#[test]
fn a_note_outside_every_zone_reaches_no_member_and_its_off_none_either() {
    MixEngine::force_serial(true);
    let mut rig = Rig::new();
    let mut s = rig.session.clone();
    s.track_mut(rig.rack).unwrap().rack.as_mut().unwrap().members[1].key_zone = Zone::new(100, 127);
    rig.publish(&s);
    assert_eq!(rig.send(on(80, 100)), EventOutcome::Unmatched, "no member's zone holds 80");
    assert_eq!(rig.send(off(80)), EventOutcome::Unmatched);
    rig.render(1);
    assert!(rig.heard(0).is_empty() && rig.heard(1).is_empty());
}

#[test]
fn note_offs_follow_the_note_ons_that_were_delivered() {
    MixEngine::force_serial(true);
    let mut rig = Rig::new();
    assert!(rig.send(on(48, 100)).accepted());
    rig.render(1);
    // A zone edit while 48 sounds: A no longer contains it, B now does.
    let mut s = rig.session.clone();
    {
        let r = s.track_mut(rig.rack).unwrap().rack.as_mut().unwrap();
        r.members[0].key_zone = Zone::new(70, 127);
        r.members[1].key_zone = Zone::new(0, 127);
    }
    rig.publish(&s);
    assert!(rig.send(off(48)).accepted(), "A still holds 48");
    rig.render(1);
    assert_eq!(offs(&rig.heard(0)), [48], "the off reaches the member that got the on");
    assert!(offs(&rig.heard(1)).is_empty(), "and not the one that did not");

    // The rack's clips: the same rule through the clip table.
    let mut s = rig.session.clone();
    {
        let r = s.track_mut(rig.rack).unwrap().rack.as_mut().unwrap();
        r.members[0].key_zone = Zone::new(0, 59);
        r.members[1].key_zone = Zone::new(60, 127);
    }
    put_clip(&mut s, rig.rack, &[note(50, 90, 0), note(70, 90, 0)]);
    rig.publish(&s);
    rig.clear();
    rig.pos = 0;
    // A quarter note at 120 bpm is 24000 samples: 100 blocks hold the clip's notes and offs.
    rig.render(100);
    let (a, b) = (rig.heard(0), rig.heard(1));
    assert_eq!((ons(&a), offs(&a)), (vec![(50, 90)], vec![50]));
    assert_eq!((ons(&b), offs(&b)), (vec![(70, 90)], vec![70]));
}

#[test]
fn distributing_the_racks_midi_allocates_nothing() {
    MixEngine::force_serial(true);
    mark_audio_thread();
    let mut rig = Rig::new();
    let mut s = rig.session.clone();
    put_clip(&mut s, rig.rack, &rack_notes());
    rig.publish(&s);
    rig.pos = 0;
    rig.render(2);
    let events: Vec<InstrumentEvent> =
        [on(48, 100), on(62, 100), on(72, 100), Message::Control { cc: 64, value: 127 }, off(48), off(62), off(72), off(30)]
            .map(|m| rig.event(m))
            .to_vec();
    let ((), stats) = measure(|| {
        for e in &events {
            let _ = rig.mix.queue_event(e);
        }
        for _ in 0..8 {
            rig.mix.render_active(rig.pos, BLOCK, &mut rig.out);
            rig.pos += BLOCK as i64;
        }
    });
    assert_eq!(stats, AllocStats::default(), "rack fan-out and the rack's clips allocate nothing");
    assert!(!rig.heard(0).is_empty() && !rig.heard(1).is_empty());
}

#[test]
fn a_macro_lane_drives_the_mapped_parameter_with_range_and_curve() {
    MixEngine::force_serial(true);
    let mut rig = Rig::new();
    let a = rig.members[0];
    // Macro 1 to the level of the plug-in on A's insert A: 0.25..0.75 of its travel, curve 0.5.
    let info = transit_dsp::plugin_info("gain").unwrap();
    let p = info.params.first().unwrap();
    let range = ParamRange { lo: p.min, hi: p.max, log: false, step: false };
    let mapping = MacroMapping { track: a, slot: 0, param: p.id.into(), min: 0.25, max: 0.75, curve: 0.5, range };
    let expect = range.plain(0.25 + 0.5 * 0.25);
    assert_eq!(mapping.plain(0.5), expect, "macro 0.5 -> curve 0.25 -> travel 0.375 of the parameter");
    let mut s = rig.session.clone();
    // A mapped parameter is in the insert's stored values (`rack.macro_map` writes it there).
    let mut ins = Insert::new("clap:test.rack");
    ins.params.insert(p.id.into(), p.default);
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(ins);
    let rack = s.track_mut(rig.rack).unwrap();
    rack.rack.as_mut().unwrap().macros[0].mappings.push(mapping);
    rack.lane_mut(&AutoParam::Plugin { slot: MACRO_SLOT, param: macro_param(0) }).set_point(0, 0.5);
    rig.publish(&s);
    rig.render(2);
    assert_eq!(rig.params.lock().unwrap()[&(a, 0)].0, expect, "the insert got the mapped value from the rack's macro lane");
}

/// A session with a sidechain key walks every insert's parameters before the block
/// (`KeyBank::align`): a macro-ruled parameter gets the same value there, so it is set once and
/// then left alone while the lane holds still.
#[test]
fn a_macro_ruled_parameter_is_set_once_in_a_session_with_a_sidechain_key() {
    MixEngine::force_serial(true);
    let mut rig = Rig::new();
    let (a, b) = (rig.members[0], rig.members[1]);
    let p = transit_dsp::plugin_info("gain").unwrap().params.first().unwrap();
    let range = ParamRange { lo: p.min, hi: p.max, log: false, step: false };
    let mut s = rig.session.clone();
    let mut ins = Insert::new("clap:test.rack");
    ins.params.insert(p.id.into(), p.default);
    s.track_mut(a).unwrap().mixer.inserts[0] = Some(ins);
    // A key: B's compressor listens to an audio track outside the rack.
    let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Kick"));
    let mut comp = Insert::new("compressor");
    comp.key = Some(KeySource { from: KeyFrom::Track(kick), tap: Tap::Post });
    s.track_mut(b).unwrap().mixer.inserts[0] = Some(comp);
    let rack = s.track_mut(rig.rack).unwrap();
    rack.rack.as_mut().unwrap().macros[0].mappings.push(MacroMapping {
        track: a,
        slot: 0,
        param: p.id.into(),
        min: 0.0,
        max: 1.0,
        curve: 0.0,
        range,
    });
    rack.lane_mut(&AutoParam::Plugin { slot: MACRO_SLOT, param: macro_param(0) }).set_point(0, 0.5);
    rig.publish(&s);
    rig.render(3);
    let (value, _) = rig.params.lock().unwrap()[&(a, 0)];
    assert_eq!(value, range.plain(0.5));
    rig.params.lock().unwrap().insert((a, 0), (value, 0));
    rig.render(4);
    assert_eq!(rig.params.lock().unwrap()[&(a, 0)], (value, 0), "no set while the macro lane holds still");
}

/// Review finding 4: a member taken out of the rack while it holds the rack's notes and pedal
/// gets their releases, from live input and from the rack's clips.
#[test]
fn a_member_that_leaves_the_rack_lets_go_of_what_the_rack_held() {
    MixEngine::force_serial(true);
    let mut rig = Rig::new();
    let (a, b) = (rig.members[0], rig.members[1]);
    let mut s = rig.session.clone();
    put_clip(&mut s, rig.rack, &[note(40, 90, 0)]);
    rig.publish(&s);
    rig.pos = 0;
    assert!(rig.send(on(48, 100)).accepted());
    assert!(rig.send(Message::Control { cc: 64, value: 127 }).accepted());
    rig.render(2);
    let held = rig.heard(0);
    assert!(ons(&held).contains(&(48, 100)) && ons(&held).contains(&(40, 90)), "A holds a live and a clip note: {held:?}");
    rig.clear();
    // A leaves the rack's folder (and its member list) while both notes and the pedal are down.
    let mut s = rig.session.clone();
    s.track_mut(a).unwrap().folder = None;
    s.track_mut(a).unwrap().mixer.output = Route::Main;
    s.track_mut(rig.rack).unwrap().rack.as_mut().unwrap().members.retain(|m| m.track != a);
    rig.publish(&s);
    rig.render(1);
    let after = rig.heard(0);
    let mut released = offs(&after);
    released.sort_unstable();
    assert_eq!(released, [40, 48], "both notes are released");
    assert!(after.contains(&Message::Control { cc: 64, value: 0 }), "the pedal goes up: {after:?}");
    assert!(!rig.heard(1).contains(&Message::Control { cc: 64, value: 0 }), "B, still a member, keeps the pedal down");
    let _ = b;
}
