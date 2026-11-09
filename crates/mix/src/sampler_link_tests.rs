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
use transit_model::sampler_patch::{GroupEntry, Membership, PadEntry, PatchState, PatchV1, RoundRobin, SourceRef, ZoneEntry};
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

// Round robin through the plan compiler (ADR 0041 and its amendment of 2026-11-08, item 4): a
// group member whose media is missing is published as a silent placeholder, so it keeps its turn
// and relinking it does not reorder the others; each group's stored mode reaches the instance.

/// Ticks between the notes (6400 frames at 120 BPM: 25 blocks), each note 40 ticks long.
const STEP: i64 = 256;
const STEP_FRAMES: usize = 6400;
const STEP_BLOCKS: usize = STEP_FRAMES / BLOCK;
/// The value of each alternative's source (source ids 3, 4, 5, 6); the pad's source 1 holds 0.5.
const ALT: [f32; 4] = [0.031_25, 0.0625, 0.125, 0.25];

/// A Sampler track playing `notes` note-60s, one every [`STEP`], with sources 1 and 3 to 6.
fn rr_session(p: PatchV1, notes: usize) -> (Session, TrackId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.pool.insert(SourceId(1), audio(0.5));
    for (i, v) in ALT.iter().enumerate() {
        s.pool.insert(SourceId(3 + i as u64), audio(*v));
    }
    let mut seq = Sequence::default();
    for k in 0..notes as i64 {
        seq.notes.push(Note { pitch: 60, velocity: 127, release_velocity: 64, channel: 0, start: k * STEP, length: 40 });
    }
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, (notes * STEP_FRAMES) as i64, seq));
    let mut ins = Insert::new("sampler");
    ins.sampler_patch = Some(SamplerPatch { state: PatchState::Ready(Box::new(p)) });
    tr.instrument = Some(ins);
    (s, t)
}

fn gate(z: &mut ZoneEntry) {
    z.mode = transit_model::sampler_patch::PlayMode::Gate;
    z.attack_ms = 0.0;
    z.release_ms = 0.0;
    z.key_tracking = false;
    z.root = 60;
}

/// Pad A1 (note 60) on a plain zone over source 1, and group 0's `count` alternatives on sources
/// 3, 4, ...; alternative `missing` names source 99, which is not in the pool.
fn rr_patch(count: u8, missing: Option<u8>) -> PatchV1 {
    let mut p = PatchV1 { sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }], ..PatchV1::default() };
    let mut pad = ZoneEntry::whole(0, 0, 48_000);
    pad.pad_defaults(60);
    gate(&mut pad);
    p.zones.push(pad);
    p.set_pad(PadEntry { index: 0, zone: Some(0), ..PadEntry::default() });
    for a in 0..count {
        let source = if missing == Some(a) { 99 } else { 3 + u64::from(a) };
        p.sources.push(SourceRef { source: SourceId(source), ..SourceRef::default() });
        let mut z = ZoneEntry::whole(1 + u16::from(a), 1 + u16::from(a), 48_000);
        gate(&mut z);
        z.membership = Some(Membership { group: 0, layer: 0, alternative: a, ..Membership::default() });
        p.zones.push(z);
    }
    p
}

/// The level each of `notes` notes played, sampled 500 frames into it; `out` starts at the first
/// note's start.
fn levels(out: &[f32], notes: usize) -> Vec<f32> {
    (0..notes).map(|k| out[k * STEP_FRAMES + 500]).collect()
}

/// What notes `range` should play: the pad's 0.5 plus alternative `k % count`, silent when it is
/// `silent`.
fn expected(range: std::ops::Range<usize>, count: usize, silent: Option<usize>) -> Vec<f32> {
    range.map(|k| 0.5 + if Some(k % count) == silent { 0.0 } else { ALT[k % count] }).collect()
}

/// Alternative `missing` of three has no media: the patch stands, the pad plays beside the group,
/// the gap keeps its turn; after relinking the order is unchanged and the alternative sounds.
fn missing_member_keeps_its_turn(missing: u8) {
    let p = rr_patch(3, Some(missing));
    let (mut s, _) = rr_session(p.clone(), 9);
    assert_eq!(check_sampler_patch(&p, &s), Ok(()), "alternative {missing} missing: the patch stands");
    let mut rig = Rig::new();
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let before = levels(&rig.render(6 * STEP_BLOCKS), 6);
    assert_eq!(before, expected(0..6, 3, Some(usize::from(missing))), "alternative {missing} missing: the pad plays, the gap keeps its turn");
    // Relink: the missing source appears; the next publication reaches the same instance and
    // the counter carries on, so the order is unchanged and the relinked alternative sounds.
    s.pool.insert(SourceId(99), audio(ALT[usize::from(missing)]));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    let after = levels(&rig.render(3 * STEP_BLOCKS), 3);
    assert_eq!(after, expected(6..9, 3, None), "alternative {missing} relinked: the same order");
}

#[test]
fn a_missing_first_alternative_keeps_its_turn_and_relinking_keeps_the_order() {
    missing_member_keeps_its_turn(0);
}

#[test]
fn a_missing_middle_alternative_keeps_its_turn_and_relinking_keeps_the_order() {
    missing_member_keeps_its_turn(1);
}

#[test]
fn a_missing_last_alternative_keeps_its_turn_and_relinking_keeps_the_order() {
    missing_member_keeps_its_turn(2);
}

#[test]
fn a_stored_random_mode_plays_random_and_a_mode_change_republishes() {
    let reference = |seed: u32, n: usize| -> Vec<usize> {
        let mut x = seed ^ 0x9E37_79B9;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x % 4) as usize
            })
            .collect()
    };
    let random = |p: &mut PatchV1| {
        p.groups = vec![GroupEntry { id: 0, round_robin: RoundRobin::Random, ..GroupEntry::default() }];
    };
    let mut p = rr_patch(4, None);
    p.seed = 12345;
    random(&mut p);
    let (s, _) = rr_session(p, 8);
    let mut rig = Rig::new();
    rig.publish(&s);
    let got = levels(&rig.render(8 * STEP_BLOCKS), 8);
    let want: Vec<f32> = reference(12345, 8).into_iter().map(|a| 0.5 + ALT[a]).collect();
    assert_eq!(got, want, "a stored Random group plays the seeded sequence");

    // Sequential first, then the stored mode changes to Random: a new publication to the same
    // instance, which draws from the group's untouched start state.
    let mut p = rr_patch(4, None);
    p.seed = 12345;
    let (mut s, t) = rr_session(p.clone(), 8);
    let mut rig = Rig::new();
    rig.publish(&s);
    let made = rig.log.made.load(Ordering::Relaxed);
    assert_eq!(levels(&rig.render(4 * STEP_BLOCKS), 4), expected(0..4, 4, None));
    random(&mut p);
    set_patch(&mut s, t, Some(p));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made, "the same instance");
    let got = levels(&rig.render(4 * STEP_BLOCKS), 4);
    let want: Vec<f32> = reference(12345, 4).into_iter().map(|a| 0.5 + ALT[a]).collect();
    assert_eq!(got, want, "the mode change reached the instance");
}
