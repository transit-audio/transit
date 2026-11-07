//! The plan compiler's ownership of core-mode Sampler instances (ADR 0041, job C2): a patch makes
//! a fresh core instance, edits to a running one are publications and control-lane values (the
//! instance is kept), a mode or seed change is a fresh instance, and a retired generation is
//! dropped on the control thread at the service tick.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::plan_tests::*;
use super::*;
use std::sync::Weak;
use std::sync::atomic::Ordering;
use transit_audio_io::AudioBuffer;
use transit_midi::{Note, Sequence};
use transit_model::sampler_patch::{PadEntry, PatchState, PatchV1, SourceRef, ZoneEntry};
use transit_model::{Insert, SamplerPatch, SourceId};

fn audio(v: f32) -> Arc<SourceAudio> {
    Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![vec![v; 48_000]] }))
}

/// A Sampler track holding a long note 60, and pad 1 (note 60) bound to zone 1 over source 1.
fn session(patch: Option<PatchV1>) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.pool.insert(SourceId(1), audio(0.5));
    s.pool.insert(SourceId(2), audio(0.25));
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch: 60, velocity: 127, release_velocity: 64, channel: 0, start: 0, length: 40_000 });
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 96_000, seq));
    let mut ins = Insert::new("sampler");
    ins.sample = Some(SourceId(2));
    ins.sampler_patch = patch.map(|p| SamplerPatch { state: PatchState::Ready(Box::new(p)) });
    tr.instrument = Some(ins);
    (s, t)
}

fn kit(source: u64) -> PatchV1 {
    let mut p = PatchV1 { sources: vec![SourceRef { source: SourceId(source), ..SourceRef::default() }], ..PatchV1::default() };
    let mut z = ZoneEntry::whole(0, 0, 48_000);
    z.pad_defaults(60);
    z.mode = transit_model::sampler_patch::PlayMode::Gate;
    z.release_ms = 0.0;
    z.attack_ms = 0.0;
    p.zones.push(z);
    p.set_pad(PadEntry { index: 0, zone: Some(0), ..PadEntry::default() });
    p
}

fn set_patch(s: &mut Session, t: TrackId, p: Option<PatchV1>) {
    s.track_mut(t).unwrap().instrument.as_mut().unwrap().sampler_patch = p.map(|p| SamplerPatch { state: PatchState::Ready(Box::new(p)) });
}

fn last(x: &[f32]) -> f32 {
    *x.last().unwrap()
}

#[test]
fn a_patch_makes_a_core_instance_and_edits_reach_the_running_one() {
    let (mut s, t) = session(Some(kit(1)));
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(rig.comp.samplers.len(), 1, "the compiler holds the instance's publisher");
    let made = rig.log.made.load(Ordering::Relaxed);
    let out = rig.render(8);
    assert!(last(&out) > 0.4, "the patch plays source 1, not the one-sample field: {}", last(&out));
    // A control edit: the instance is kept, the value arrives through the lane and glides in.
    let mut p = kit(1);
    p.set_control("zone001_level", -48.0, 0.0);
    set_patch(&mut s, t, Some(p.clone()));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let out = rig.render(4);
    assert!(last(&out) < 0.01, "the level reached the running instance: {}", last(&out));
    // A structural edit: the zone moves to source 2, a publication to the same instance.
    p.sources[0].source = SourceId(2);
    set_patch(&mut s, t, Some(p));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made, "edits keep the instance");
    assert_eq!(rig.drops(), 0);
}

#[test]
fn a_patch_made_or_removed_or_a_new_seed_is_a_fresh_instance() {
    let (mut s, t) = session(None);
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(4);
    assert_eq!(rig.comp.samplers.len(), 0, "a one-sample Sampler stays on its legacy renderer");
    let made = rig.log.made.load(Ordering::Relaxed);
    set_patch(&mut s, t, Some(kit(1)));
    rig.publish(&s);
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made + 1, "a new patch switches through a fresh instance");
    assert_eq!(rig.comp.samplers.len(), 1);
    let mut p = kit(1);
    p.seed = 77;
    set_patch(&mut s, t, Some(p));
    rig.publish(&s);
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made + 2, "a new seed is a fresh instance");
    assert_eq!(rig.comp.samplers.len(), 1, "the replaced instance's link is dropped");
    set_patch(&mut s, t, None);
    rig.publish(&s);
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made + 3, "removing the patch goes back to the legacy renderer");
    assert_eq!(rig.comp.samplers.len(), 0);
}

#[test]
fn a_retired_generation_is_dropped_on_the_control_thread_at_the_service_tick() {
    let (mut s, t) = session(Some(kit(1)));
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(2);
    let first: Weak<SourceAudio> = Arc::downgrade(s.pool.get(SourceId(1)).unwrap());
    // The zone moves to source 2 and source 1 leaves the pool: only the old generation holds it.
    set_patch(&mut s, t, Some(kit(2)));
    s.pool.remove(SourceId(1));
    rig.publish(&s);
    rig.render(8);
    assert!(first.upgrade().is_some(), "retired by the audio side, not dropped there");
    rig.comp.service_samplers();
    assert!(first.upgrade().is_none(), "dropped at the service tick, on this thread");
}

#[test]
fn the_instruments_tap_carries_the_pads_its_instance_published() {
    // Pad 1 (note 60) holds the clip's note from frame 0 (#326, the Pads view's live state).
    let (s, t) = session(Some(kit(1)));
    let mut rig = Rig::new();
    rig.publish(&s);
    let hub = rig.comp.scope_hub();
    assert!(hub.set_enabled(t, crate::INSTRUMENT_UID, true), "the instrument has a tap");
    rig.render(4);
    let words = hub.pad_activity(t, crate::INSTRUMENT_UID).expect("the tap is alive");
    let w = transit_dsp::PadWord(words[0]);
    assert_eq!((w.hits(), w.velocity(), w.chokes()), (1, 127, 0));
    assert!(words[1..].iter().all(|w| *w == 0), "no other pad did anything");
    // Negative control: the one-sample Sampler on its legacy renderer publishes no pads.
    let (legacy, lt) = session(None);
    let mut rig = Rig::new();
    rig.publish(&legacy);
    let hub = rig.comp.scope_hub();
    assert!(hub.set_enabled(lt, crate::INSTRUMENT_UID, true));
    let out = rig.render(4);
    assert!(out.iter().any(|x| *x != 0.0), "the legacy renderer plays the note");
    assert_eq!(hub.pad_activity(lt, crate::INSTRUMENT_UID), Some([0; transit_dsp::PAD_SLOTS]));
}
