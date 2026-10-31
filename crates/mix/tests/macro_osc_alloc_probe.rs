//! Allocation probe for the Macro Oscillator on a strip (#195 slice 3, ADR 0012 G5): a realtime
//! engine playing a MIDI clip into the instrument (overlapping notes for legato, the mod wheel,
//! pitch bend, the sustain pedal), live notes and controllers, a parameter edit through a new plan
//! (model, tune, levels), all notes off and a reset allocate and free nothing on the audio
//! thread, in host blocks that are not multiples of the voice's 12-frame block.
//!
//! With #196 the default is eight voices; a second test plays sixteen voices with chords wider
//! than the voice count (steals of held and releasing voices), the note-to-macro mapping, the panel's
//! voice activity tap switched on, and a plan edit down to three voices.
//!
//! The binary installs `transit_rt::testing::CountingAlloc` and counts the test thread only.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use transit_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use transit_mix::{
    Adoption, Arrival, DeviceGeneration, Due, EngineEpoch, EventLimits, EventOutcome, INSTRUMENT_SLOT, InstanceSpec, InstrumentEvent, Message,
    MixEngine, NoteId, PlanCompiler, PlanGeneration, PlanLimits, RenderFrame, RenderTarget, SlotGen, Source, mark_audio_thread,
};
use transit_model::{ChannelFormat, Clip, Insert, Session, TrackId, TrackKind};
use transit_rt::testing::{AllocStats, CountingAlloc, measure};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

const SR: u32 = 48_000;
/// Not a multiple of 12: the instrument carries rendered frames across host blocks.
const BLOCK: usize = 250;
const ID: &str = "macro_oscillator";

fn session(params: &[(&str, f32)]) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut ins = Insert::new(ID);
    for (id, v) in params {
        ins.params.insert((*id).to_string(), *v);
    }
    s.track_mut(t).unwrap().instrument = Some(ins);
    let mut seq = Sequence::default();
    // Overlapping notes (legato), a gap (a new trigger), a long held note under the pedal.
    for (pitch, start, length) in [(48, 0, 600), (55, 480, 600), (60, 960, 300), (64, 1500, 900), (67, 2600, 400)] {
        seq.notes.push(Note { pitch, velocity: 100, release_velocity: 0, channel: 0, start, length });
    }
    for tick in (0..3000).step_by(40) {
        seq.ctrls.push(CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(1), value: ((tick / 40) % 128) as i32 });
        seq.ctrls.push(CtrlEvent { tick: tick + 20, channel: 0, kind: CtrlKind::PitchBend, value: (tick as i32 * 5) % 8000 - 4000 });
    }
    for (tick, value) in [(1400, 127), (2500, 0)] {
        seq.ctrls.push(CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(64), value });
    }
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "phrase", 0, 96_000, seq));
    (s, t)
}

fn peak(out: &[Vec<f32>]) -> f32 {
    out.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn the_macro_oscillator_on_a_strip_allocates_nothing_on_the_audio_thread() {
    MixEngine::force_serial(true);
    mark_audio_thread();
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let epoch = EngineEpoch::FIRST;
    let mut mix = MixEngine::realtime_with_events(target, epoch, EventLimits::default());
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| transit_mix::create_instance(&spec.id, None, SR as f32, BLOCK, spec.ch);
    let (s, _) = session(&[]);
    let mut generation = PlanGeneration::EMPTY.checked_next().unwrap();
    let mut env = comp.compile(Arc::new(s), PlanGeneration::EMPTY, generation, &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    let binding = comp.active_bindings().into_iter().find(|b| b.1 == INSTRUMENT_SLOT).unwrap();
    // The edited plan is compiled on the control thread before the measured region.
    let (edited, _) = session(&[("model", 2.0), ("tune", 7.0), ("level", 0.0), ("aux_mix", 50.0), ("mod_wheel", 2.0)]);
    let next = generation.checked_next().unwrap();
    let mut edit = comp.compile(Arc::new(edited), generation, next, &mut make).unwrap();
    generation = next;

    let ev = |at: u64, message: Message| InstrumentEvent {
        engine: epoch,
        target: binding,
        due: Due::Frame(RenderFrame(at)),
        transport: None,
        source: Source { port: 1, channel: 0, generation: 1 },
        message,
        arrival: Arrival { host_ns: 0, sequence: 0 },
    };
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut pos = 0i64;
    let mut loudest = 0.0f32;
    let mut after_edit = 0.0f32;
    let ((), stats) = measure(|| {
        for _ in 0..160 {
            mix.render_active(pos, BLOCK, &mut out);
            loudest = loudest.max(peak(&out));
            pos += BLOCK as i64;
        }
        let now = mix.next_frame().0;
        for (k, key) in [72u8, 76, 79].into_iter().enumerate() {
            let at = now + 37 * k as u64;
            assert_eq!(mix.queue_event(&ev(at, Message::NoteOn { key, velocity: 90, id: NoteId::NONE })), EventOutcome::Queued);
            assert!(mix.queue_event(&ev(at + 9000, Message::NoteOff { key, velocity: 0, id: NoteId::NONE })).accepted());
        }
        mix.queue_event(&ev(now + 5, Message::Control { cc: 1, value: 90 }));
        mix.queue_event(&ev(now + 6, Message::PitchBend { value: 12_000 }));
        for _ in 0..20 {
            mix.render_active(pos, BLOCK, &mut out);
            pos += BLOCK as i64;
        }
        assert_eq!(mix.adopt_plan(&mut edit), Adoption::Adopted);
        // Past the clip's last note (frames 65000 to 75000).
        for _ in 0..140 {
            mix.render_active(pos, BLOCK, &mut out);
            after_edit = after_edit.max(peak(&out));
            pos += BLOCK as i64;
        }
        mix.all_notes_off();
        mix.render_active(pos, BLOCK, &mut out);
        pos += BLOCK as i64;
        mix.reset();
        mix.render_active(pos, BLOCK, &mut out);
    });
    comp.reclaim(edit);
    println!("{stats:?}; peak {loudest}, after the edit {after_edit}; generation {generation:?}");
    assert_eq!(stats, AllocStats::default(), "the Macro Oscillator's callback path");
    assert!(loudest > 0.01, "the clip played the instrument: peak {loudest}");
    assert!(after_edit > 0.01, "the edited instrument plays: peak {after_edit}");
}

#[test]
fn sixteen_voices_with_steals_the_shaper_and_a_full_matrix_allocate_nothing() {
    MixEngine::force_serial(true);
    mark_audio_thread();
    let target = RenderTarget { device: DeviceGeneration(1), sample_rate: SR, max_block: BLOCK as u32, out_channels: 0, in_channels: 0 };
    let epoch = EngineEpoch::FIRST;
    let mut mix = MixEngine::realtime_with_events(target, epoch, EventLimits::default());
    let mut comp = PlanCompiler::new(target, PlanLimits::default());
    let mut make = |spec: &InstanceSpec, _: SlotGen| transit_mix::create_instance(&spec.id, None, SR as f32, BLOCK, spec.ch);
    // The shaper on (both LFOs, one synced) and all sixteen matrix rows (#243).
    let mut owned: Vec<(String, f32)> = [
        ("shaper", 1.0),
        ("filter_slope", 1.0),
        ("cutoff", 900.0),
        ("resonance", 50.0),
        ("filter_env_amount", 50.0),
        ("lfo1_shape", 5.0),
        ("lfo2_shape", 4.0),
        ("lfo2_sync", 1.0),
        ("lfo2_retrigger", 0.0),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), *v))
    .collect();
    for n in 1..=16usize {
        let (source, dest) = (1 + (n * 7) % 20, 1 + (n * 5) % 14);
        owned.extend([
            (format!("mod{n}_source"), source as f32),
            (format!("mod{n}_via"), ((n * 3) % 21) as f32),
            (format!("mod{n}_dest"), dest as f32),
            (format!("mod{n}_amount"), 20.0 + n as f32),
            (format!("mod{n}_curve"), (n % 5) as f32),
        ]);
    }
    let mapping: Vec<(&str, f32)> = owned.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let (s, t) = session(&[&[("voices", 15.0), ("model", 15.0)][..], &mapping[..]].concat());
    let mut generation = PlanGeneration::EMPTY.checked_next().unwrap();
    let mut env = comp.compile(Arc::new(s), PlanGeneration::EMPTY, generation, &mut make).unwrap();
    assert_eq!(mix.adopt_plan(&mut env), Adoption::Adopted);
    comp.reclaim(env);
    let hub = comp.scope_hub();
    assert!(hub.set_enabled(t, transit_mix::INSTRUMENT_UID, true), "the instrument has a tap");
    let binding = comp.active_bindings().into_iter().find(|b| b.1 == INSTRUMENT_SLOT).unwrap();
    let (edited, _) = session(&[&[("voices", 2.0), ("model", 18.0), ("mod16_amount", -50.0)][..], &mapping[..]].concat());
    let next = generation.checked_next().unwrap();
    let mut edit = comp.compile(Arc::new(edited), generation, next, &mut make).unwrap();
    generation = next;

    let ev = |at: u64, message: Message| InstrumentEvent {
        engine: epoch,
        target: binding,
        due: Due::Frame(RenderFrame(at)),
        transport: None,
        source: Source { port: 1, channel: 0, generation: 1 },
        message,
        arrival: Arrival { host_ns: 0, sequence: 0 },
    };
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut pos = 0i64;
    let mut loudest = 0.0f32;
    let mut most_voices = 0u32;
    let ((), stats) = measure(|| {
        for round in 0..6u64 {
            let now = mix.next_frame().0;
            // 20 keys on 16 voices: the last four steal; half are released early (tails to steal).
            for k in 0..20u64 {
                let key = 36 + (k * 3 + round) as u8;
                let at = now + 11 * k;
                assert!(mix.queue_event(&ev(at, Message::NoteOn { key, velocity: 30 + (k * 5) as u8, id: NoteId::NONE })).accepted());
                let off = if k % 2 == 0 { at + 700 } else { at + 6000 };
                assert!(mix.queue_event(&ev(off, Message::NoteOff { key, velocity: 64, id: NoteId::NONE })).accepted());
                assert!(mix.queue_event(&ev(at + 50, Message::PolyPressure { key, value: 90, id: NoteId::NONE })).accepted());
            }
            for (k, value) in [(0u64, 30u8), (1, 100), (2, 0)] {
                assert!(mix.queue_event(&ev(now + 300 * k, Message::ChannelPressure { value })).accepted());
            }
            for _ in 0..30 {
                mix.render_active(pos, BLOCK, &mut out);
                loudest = loudest.max(peak(&out));
                most_voices = most_voices.max((hub.voice_activity(t, transit_mix::INSTRUMENT_UID) & 0xFFFF).count_ones());
                pos += BLOCK as i64;
            }
            if round == 3 {
                assert_eq!(mix.adopt_plan(&mut edit), Adoption::Adopted);
            }
        }
        mix.all_notes_off();
        mix.render_active(pos, BLOCK, &mut out);
        pos += BLOCK as i64;
        mix.reset();
        mix.render_active(pos, BLOCK, &mut out);
    });
    comp.reclaim(edit);
    println!("{stats:?}; peak {loudest}; most voices sounding {most_voices}; generation {generation:?}");
    assert_eq!(stats, AllocStats::default(), "sixteen voices on the callback path");
    assert!(loudest > 0.01, "the voices play: peak {loudest}");
    assert_eq!(most_voices, 16, "every voice sounded at once, read through the tap");
}
