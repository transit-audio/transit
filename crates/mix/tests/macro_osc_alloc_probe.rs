//! Allocation probe for the Macro Oscillator on a strip (#195 slice 3, ADR 0012 G5): a realtime
//! engine playing a MIDI clip into the instrument (overlapping notes for legato, the mod wheel,
//! pitch bend, the sustain pedal), live notes and controllers, a parameter edit through a new plan
//! (model, tuning, levels), all notes off and a reset allocate and free nothing on the audio
//! thread, in host blocks that are not multiples of the voice's 12-frame block.
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
    let (edited, _) = session(&[("model", 2.0), ("hardware_tuning", 1.0), ("level", 0.0), ("aux_mix", 50.0), ("mod_wheel", 2.0)]);
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
