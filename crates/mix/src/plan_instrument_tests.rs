#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan::{Attach, CellKind};
use super::plan_tests::*;
use super::*;
use std::sync::Weak;
use std::sync::atomic::Ordering;
use transit_audio_io::AudioBuffer;
use transit_model::{Insert, SourceId};

#[test]
fn a_sampler_sample_swap_keeps_both_owners_until_reclaim() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let audio = |v: f32| Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![vec![v; 4_800]] }));
    s.pool.insert(SourceId(1), audio(0.5));
    s.pool.insert(SourceId(2), audio(0.25));
    let mut ins = Insert::new("sampler");
    ins.sample = Some(SourceId(1));
    s.track_mut(t).unwrap().instrument = Some(ins);
    let first: Weak<SourceAudio> = Arc::downgrade(s.pool.get(SourceId(1)).unwrap());
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    rig.render(2);
    // Only the sample changes, and source 1 leaves the pool.
    s.track_mut(t).unwrap().instrument.as_mut().unwrap().sample = Some(SourceId(2));
    s.pool = transit_model::SourcePool::default();
    s.pool.insert(SourceId(2), audio(0.25));
    let mut env = rig.compile(&s);
    assert!(env.plan.attach.iter().any(|a| matches!(a, Attach::Sample { .. })), "the sampler is reused with a new sample");
    let made = rig.log.made.load(Ordering::Relaxed);
    assert_eq!(adopt_measured(&mut rig.eng, &mut env), Adoption::Adopted);
    assert!(first.upgrade().is_some(), "the old audio is still owned, by the envelope");
    rig.comp.reclaim(env);
    assert!(first.upgrade().is_none(), "dropped at reclaim, on the control thread");
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made, "the instance was kept");
    assert_eq!(rig.drops(), 0);
}

// Coverage for the review's mutants.

fn synth_track(s: &mut Session) -> TrackId {
    use transit_midi::{Note, Sequence};
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let mut seq = Sequence::default();
    for (i, p) in [60u8, 64, 67].into_iter().enumerate() {
        seq.notes.push(Note { pitch: p, velocity: 100, release_velocity: 64, channel: 0, start: i as i64 * 10, length: 40_000 });
    }
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 96_000, seq));
    tr.instrument = Some(builtin("macro_oscillator"));
    t
}

#[test]
fn a_sample_attachment_on_an_instrument_that_is_not_the_sampler_is_rejected() {
    let mut s = Session::default();
    let t = synth_track(&mut s);
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    let mut env = rig.compile(&s);
    let (i, m) = env.plan.moves.iter().enumerate().find(|(_, m)| m.kind == CellKind::Instrument).unwrap();
    let (to, from_move) = (m.to, Some(i as u16));
    env.plan.attach = vec![Attach::Sample { cell: to, from_move, audio: None, wrapper: None }].into_boxed_slice();
    assert_rejected(rig, env, RejectReason::AttachInvalid);
}

#[test]
fn held_notes_move_with_the_instrument_and_note_chasing_with_the_strip() {
    let mut s = Session::default();
    let t = synth_track(&mut s);
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(4);
    let strip = |rig: &Rig| {
        let p = rig.eng.plan.as_deref().unwrap();
        let st = &p.tables.strips[p.tables.strip_of(t).unwrap()];
        let keys: Vec<u8> = st.instrument.as_ref().unwrap().events.as_deref().unwrap().sounding().map(|(_, key, _)| key).collect();
        (keys, st.chase)
    };
    let before = strip(&rig);
    assert_eq!(before, (vec![60, 64, 67], false), "three notes sound; the controllers were chased");
    s.track_mut(t).unwrap().mixer.inserts[0] = Some(builtin("gain"));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(strip(&rig), before, "a structural plan keeps the held notes and the chase state");
}
