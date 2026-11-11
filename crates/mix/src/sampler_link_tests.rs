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
    ins.sampler_patch = patch.map(|p| SamplerPatch { state: PatchState::Ready(Box::new(p)) });
    tr.instrument = Some(ins);
    (s, t)
}

/// Pad A1 on note 60 (pad start note 60, stated), bound to zone 1 over source `source`.
fn kit(source: u64) -> PatchV1 {
    let mut p = PatchV1 { sources: vec![SourceRef { source: SourceId(source), ..SourceRef::default() }], pad_start_note: 60, ..PatchV1::default() };
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
    assert!(last(&out) > 0.4, "the patch plays source 1: {}", last(&out));
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
    assert_eq!(rig.comp.samplers.len(), 0, "a Sampler without a patch has no core link");
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
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made + 3, "removing the patch is a fresh instance off the core path");
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
    // Negative control: a Sampler without a patch is silent and publishes no pads.
    let (none, lt) = session(None);
    let mut rig = Rig::new();
    rig.publish(&none);
    let hub = rig.comp.scope_hub();
    assert!(hub.set_enabled(lt, crate::INSTRUMENT_UID, true));
    let out = rig.render(4);
    assert!(out.iter().all(|x| *x == 0.0), "nothing plays");
    assert_eq!(hub.pad_activity(lt, crate::INSTRUMENT_UID), Some([0; transit_dsp::PAD_SLOTS]));
}

#[test]
fn the_instruments_tap_carries_its_voices_positions_and_reading_them_changes_no_audio() {
    // The clip's note 60 holds pad 1's Gate voice from frame 0 over a 48 kHz source at 48 kHz
    // (the Slices view's playheads, #470).
    let (s, t) = session(Some(kit(1)));
    let (mut read, mut unread) = (Rig::new(), Rig::new());
    read.publish(&s);
    unread.publish(&s);
    let hub = read.comp.scope_hub();
    assert!(hub.set_enabled(t, crate::INSTRUMENT_UID, true), "the instrument has a tap");
    assert!(!unread.comp.scope_hub().get(t, crate::INSTRUMENT_UID).unwrap().enabled(), "the other tap is off: nothing reads it");
    let mut placed = Vec::new();
    for b in 0..6 {
        let (x, y) = (read.render(1), unread.render(1));
        assert!(x.iter().zip(&y).all(|(p, q)| p.to_bits() == q.to_bits()), "block {b}: the audio is the same bit for bit");
        let words = hub.voice_positions(t, crate::INSTRUMENT_UID).expect("the tap is alive");
        placed.push(words.iter().filter_map(|w| transit_dsp::VoicePos(*w).get()).collect::<Vec<_>>());
    }
    let expected: Vec<Vec<(u16, u64)>> = (1..=6).map(|b| vec![(0, (b * BLOCK) as u64)]).collect();
    assert_eq!(placed, expected, "zone 1 (slot 0) at the frames rendered, the source's own");
    // Negative control: the tap that is off holds no positions.
    let off = unread.comp.scope_hub().voice_positions(t, crate::INSTRUMENT_UID).unwrap();
    assert_eq!(off, [0; transit_dsp::VOICE_SLOTS]);
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
    let mut p = PatchV1 { sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }], pad_start_note: 60, ..PatchV1::default() };
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

#[test]
fn the_publication_carries_the_used_fades() {
    // Zone 1 is bound to pad 1 and stores widths it never plays; zone 2 (keys 60..62) stores key
    // widths too wide for its span of 2: the core receives the used widths.
    let mut p = kit(1);
    p.zones[0].fades = transit_model::sampler_fades::Fades::of([1, 2, 3, 4]);
    let mut z = ZoneEntry::whole(1, 0, 48_000);
    z.keys = [60, 62];
    z.fades = transit_model::sampler_fades::Fades::of([5, 5, 0, 9]);
    p.zones.push(z);
    let (s, _) = session(Some(p.clone()));
    let (spec, silent) = crate::sampler_link::spec_of(&p, &s, &mut std::collections::HashMap::new());
    assert!(silent.is_empty());
    let fades = |i: usize| spec.zones[i].unwrap().fades;
    assert_eq!(fades(0), transit_dsp::plugins::sampler::Fades::NONE, "a pad-bound zone plays no fades");
    assert_eq!(fades(1), transit_dsp::plugins::sampler::Fades { key_lo: 2, key_hi: 0, vel_lo: 0, vel_hi: 9 });
}

/// The zone fields a publication carries that existed before the crossfade (the golden below), as
/// one FNV-1a hash.
fn published_bits(spec: &transit_dsp::plugins::sampler::PatchSpec) -> u64 {
    let mut text = String::new();
    for (i, z) in spec.zones.iter().enumerate() {
        if let Some(z) = z {
            let r = &z.region;
            text += &format!(
                "{i} {} {} {} {:?} {} {} {:?} {:?} {:?} {:?} {:?};",
                r.source, r.start, r.end, r.loop_mode, r.loop_start, r.loop_end, z.keys, z.velocities, z.playback, z.membership, z.fades
            );
        }
    }
    text += &format!("{:?}", spec.pads);
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A session saved before the crossfade: plain, looping, reverse ping-pong and one-shot looping
/// zones, a key-map group, and a pad-bound slice zone whose stored fields agree with its slice. Its
/// pad start note is 60, what such a session reads (it was saved without the field).
fn old_patch() -> PatchV1 {
    use transit_model::sampler_patch::{LoopMode, Marker, Markers, PlayMode, SliceBinding, SliceLoop, SliceMode, SliceSet};
    let mut p = PatchV1 {
        sources: vec![SourceRef { source: SourceId(1), ..SourceRef::default() }, SourceRef { source: SourceId(2), ..SourceRef::default() }],
        pad_start_note: 60,
        ..PatchV1::default()
    };
    let marker = |id, frame| Marker { id, frame, ..Marker::default() };
    p.slicing = Some(SliceLoop {
        source: 0,
        start: 0,
        end: 40_000,
        mode: SliceMode::Manual,
        manual: Some(Markers { markers: vec![marker(1, 0), marker(2, 10_000)], next_id: 3, ..Markers::default() }),
        ..SliceLoop::default()
    });
    let mut z = ZoneEntry::whole(0, 0, 48_000);
    (z.loop_mode, z.loop_start, z.loop_end, z.keys) = (LoopMode::Forward, 100, 30_000, [0, 40]);
    p.zones.push(z);
    let mut z = ZoneEntry::whole(1, 1, 48_000);
    (z.loop_mode, z.loop_start, z.loop_end, z.reverse, z.keys, z.transpose) = (LoopMode::PingPong, 500, 9000, true, [41, 50], 3.5);
    p.zones.push(z);
    let mut z = ZoneEntry::whole(2, 0, 20_000);
    (z.mode, z.loop_mode, z.loop_start, z.loop_end, z.keys) = (PlayMode::OneShot, LoopMode::Forward, 10, 12, [51, 52]);
    p.zones.push(z);
    for alternative in 0..2u8 {
        let mut z = ZoneEntry::whole(3 + u16::from(alternative), alternative.into(), 30_000);
        z.keys = [53, 59];
        z.membership = Some(Membership { group: 1, layer: 0, alternative, ..Membership::default() });
        p.zones.push(z);
    }
    let mut z = ZoneEntry::whole(9, 0, 48_000);
    (z.start, z.end, z.loop_start, z.loop_end, z.loop_mode) = (10_000, 40_000, 10_000, 40_000, LoopMode::Forward);
    z.slice = Some(SliceBinding { set: SliceSet::Manual, id: 2, ..SliceBinding::default() });
    p.zones.push(z);
    p.set_pad(PadEntry { index: 4, zone: Some(9), ..PadEntry::default() });
    p.validate().unwrap();
    p
}

#[test]
fn old_sessions_publish_bit_identical_zones() {
    let p = old_patch();
    let (s, _) = session(Some(p.clone()));
    let (spec, silent) = crate::sampler_link::spec_of(&p, &s, &mut std::collections::HashMap::new());
    assert!(silent.is_empty());
    assert_eq!(published_bits(&spec), OLD_PUBLICATION, "a session from before the crossfade publishes other zones");
    assert!(spec.zones.iter().flatten().all(|z| z.region.crossfade == 0), "no zone gains a crossfade");
}

/// [`published_bits`] of [`old_patch`] on the base tree (captured on a fresh copy of it, in its own
/// target).
const OLD_PUBLICATION: u64 = 0x08ed_09e7_cf10_647f;

#[test]
fn the_publication_plays_the_used_loop_and_follows_the_slice() {
    use transit_dsp::plugins::sampler::LoopMode as CoreLoop;
    use transit_model::sampler_patch::LoopMode;
    let mut p = old_patch();
    // Zone 9 (slice 2, 10,000..40,000) stores a crossfade, zone 0 one past half its loop, and the
    // ping-pong zone 1 one it never plays.
    p.zone_mut(9).unwrap().crossfade = 2000;
    p.zone_mut(0).unwrap().crossfade = 20_000;
    p.zone_mut(1).unwrap().crossfade = 50;
    p.zone_mut(2).unwrap().crossfade = 1;
    let (s, _) = session(Some(p.clone()));
    let spec = |p: &PatchV1| crate::sampler_link::spec_of(p, &s, &mut std::collections::HashMap::new()).0;
    let region = |p: &PatchV1, id: usize| spec(p).zones[id].unwrap().region;
    assert_eq!(region(&p, 9).crossfade, 2000);
    assert_eq!(region(&p, 0).crossfade, 14_950, "floor((30,000 - 100) / 2)");
    assert_eq!(region(&p, 1).crossfade, 0, "ping-pong");
    assert_eq!(region(&p, 2).crossfade, 0, "a one-shot");
    // A marker move: the slice is the authority over the stored fields, and the structure changes
    // with it, so a running instance gets a new publication.
    let before = crate::sampler_link::structure(&p, &s);
    let mut q = p.clone();
    q.slicing.as_mut().unwrap().manual.as_mut().unwrap().markers[1].frame = 39_999;
    assert_ne!(crate::sampler_link::structure(&q, &s), before);
    let r = region(&q, 9);
    assert_eq!(
        (r.start, r.end, r.loop_start, r.loop_end, r.loop_mode),
        (39_999, 40_000, 39_999, 40_000, CoreLoop::Off),
        "a one-frame slice plays unlooped"
    );
    assert_eq!(r.crossfade, 0);
    assert_eq!(q.zone(9).unwrap().start, 10_000, "the stored fields stay as they are");
    q.slicing.as_mut().unwrap().manual.as_mut().unwrap().markers[1].frame = 30_000;
    let r = region(&q, 9);
    assert_eq!((r.start, r.end, r.loop_mode, r.crossfade), (30_000, 40_000, CoreLoop::Forward, 2000));
    assert_eq!(q.zone(9).unwrap().loop_mode, LoopMode::Forward);
    crate::sampler_link::check_sampler_patch(&q, &s).unwrap();
}

// The pad start note (ADR 0041 amendment 2026-11-09): the core receives effective notes, a start
// change is a new publication, a key-tracked defaulted pad keeps its pitch, and a patch saved
// before the field publishes what it did.

#[test]
fn the_core_receives_effective_notes_and_a_start_change_is_a_new_publication() {
    let mut p = kit(1);
    p.pad_start_note = 36;
    p.zones[0].root = 36;
    p.set_pad(PadEntry { index: 9, note: Some(100), ..PadEntry::default() });
    let (s, _) = session(Some(p.clone()));
    let (spec, _) = crate::sampler_link::spec_of(&p, &s, &mut std::collections::HashMap::new());
    let notes: Vec<u8> = spec.pads.iter().map(|pad| pad.note).collect();
    let want: Vec<u8> = (0..64u8).map(|i| if i == 9 { 100 } else { 36 + i }).collect();
    assert_eq!(notes, want, "stored notes as stored, the others start + index");
    let before = crate::sampler_link::structure(&p, &s);
    let mut q = p.clone();
    q.pad_start_note = 12;
    assert_ne!(crate::sampler_link::structure(&q, &s), before, "a start change alone republishes");
}

/// A ramp the resampler cannot hide: any pitch change alters the samples.
fn ramp() -> Arc<SourceAudio> {
    Arc::new(SourceAudio::new(AudioBuffer { sample_rate: SR, channels: vec![(0..48_000).map(|i| ((i % 480) as f32 / 480.0) - 0.5).collect()] }))
}

/// A Sampler track whose clip holds one long note `pitch`, playing patch `p` (source 7 is the ramp).
fn pitched_session(p: PatchV1, pitch: u8) -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    s.pool.insert(SourceId(7), ramp());
    let mut seq = Sequence::default();
    seq.notes.push(Note { pitch, velocity: 127, release_velocity: 64, channel: 0, start: 0, length: 40_000 });
    let cid = s.new_clip_id();
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 96_000, seq));
    let mut ins = Insert::new("sampler");
    ins.sampler_patch = Some(SamplerPatch { state: PatchState::Ready(Box::new(p)) });
    tr.instrument = Some(ins);
    s
}

#[test]
fn a_key_tracked_defaulted_pad_plays_the_same_samples_after_a_start_change() {
    // Pad A1 (start 60: note 60) key-tracked with a user root a fifth below its note.
    let mut p = kit(7);
    let z = &mut p.zones[0];
    z.key_tracking = true;
    z.root = 53;
    let render = |p: &PatchV1, pitch: u8| {
        let s = pitched_session(p.clone(), pitch);
        let mut rig = Rig::new();
        assert_eq!(rig.publish(&s), Adoption::Adopted);
        rig.render(8)
    };
    let before = render(&p, 60);
    assert!(before.iter().any(|x| *x != 0.0), "the pad plays");
    p.set_pad_start(36).unwrap();
    // The clip is transposed by the notice's -24 to reach the moved pad.
    let after = render(&p, 36);
    assert!(after == before, "the same pitch, sample for sample");
    assert_eq!((p.pad_note(0), p.zones[0].root), (36, 29));
}

/// The zones and pads a publication carries, as one FNV-1a hash of their debug text.
fn publication_digest(spec: &transit_dsp::plugins::sampler::PatchSpec, silent: &[u16]) -> u64 {
    let zones: Vec<_> = spec.zones.iter().enumerate().filter(|(_, z)| z.is_some()).collect();
    let text = format!("{zones:?}|{:?}|{silent:?}", spec.pads);
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

#[test]
fn a_patch_saved_before_the_pad_start_note_publishes_the_same_zones_and_pads() {
    let raw: serde_json::Value = serde_json::from_str(OLD_SAVED_PATCH).unwrap();
    let p = SamplerPatch::from_value(raw);
    let p = p.ready().unwrap();
    assert_eq!(p.pad_start_note, 60, "read without the field");
    let (s, _) = session(None);
    let (spec, silent) = crate::sampler_link::spec_of(p, &s, &mut std::collections::HashMap::new());
    let notes: Vec<u8> = spec.pads.iter().map(|pad| pad.note).collect();
    let want: Vec<u8> = (0..64u8).map(|i| if i == 5 { 20 } else { 60 + i }).collect();
    assert_eq!(notes, want);
    assert_eq!(publication_digest(&spec, &silent), OLD_SAVED_PUBLICATION, "the base published these zones and pads");
}

/// A patch as a build before the pad start note saved it (no `pad_start_note`).
const OLD_SAVED_PATCH: &str = r#"{"schema":1,"seed":7,"sources":[{"source":1},{"source":2}],"zones":[{"id":0,"source":0,"end":48000,"loop_end":48000,"mode":"one_shot","key_tracking":false,"root":60},{"id":1,"source":1,"start":100,"end":30000,"loop_mode":"forward","loop_start":200,"loop_end":20000,"keys":[40,72],"root":64,"transpose":2.0},{"id":2,"source":0,"end":1000,"root":61,"key_tracking":false,"mode":"one_shot"},{"id":3,"source":1,"end":2000,"keys":[0,50]}],"pads":[{"index":0,"zone":0},{"index":5,"zone":2,"note":20,"choke":3},{"index":9,"mute":true}],"loop":{"source":1,"end":40000,"root":62,"manual":{"markers":[{"id":1,"frame":0},{"id":2,"frame":20000}]}}}"#;

/// [`publication_digest`] of [`OLD_SAVED_PATCH`] on the base tree (captured on a fresh copy of it,
/// in its own target).
const OLD_SAVED_PUBLICATION: u64 = 0x3373_9f8f_5fdf_cb64;

// Group, fade and crossfade edits through the plan compiler (ADR 0041 amendment 2026-11-08, items
// 2, 3, 4 and 10): structural patch edits published to the running instance, which is kept, and a
// republish never restarts the round-robin counters or random state.

/// Item 4's draws for group `group` from patch seed `seed` over `count` alternatives, written out
/// here rather than taken from the core.
fn draws(seed: u32, group: u32, count: u32, n: usize) -> Vec<usize> {
    let mut x = seed.max(1) ^ (group + 1).wrapping_mul(0x9E37_79B9);
    if x == 0 {
        x = 1;
    }
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x % count) as usize
        })
        .collect()
}

#[test]
fn a_mode_change_republish_continues_the_random_sequence_and_the_sequential_order() {
    let random = |id: u8| GroupEntry { id, round_robin: RoundRobin::Random, ..GroupEntry::default() };
    // Random group 0 of four (seed 12345): four notes, then group 1's stored mode changes, a new
    // publication to the same instance, and the next four notes continue group 0's draws.
    let mut p = rr_patch(4, None);
    p.seed = 12345;
    p.groups = vec![random(0)];
    let (mut s, t) = rr_session(p.clone(), 8);
    let mut rig = Rig::new();
    rig.publish(&s);
    let made = rig.log.made.load(Ordering::Relaxed);
    let want: Vec<f32> = draws(12345, 0, 4, 8).into_iter().map(|a| 0.5 + ALT[a]).collect();
    assert_eq!(levels(&rig.render(4 * STEP_BLOCKS), 4), want[..4], "the first four draws");
    p.groups.push(random(1));
    set_patch(&mut s, t, Some(p));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(levels(&rig.render(4 * STEP_BLOCKS), 4), want[4..], "a Random group continues its sequence after the republish");
    assert_eq!(rig.log.made.load(Ordering::Relaxed), made, "the same instance");
    // Sequential group 0 of three: 0, 1, 2, 0, then the republish, then 1, 2, 0.
    let mut p = rr_patch(3, None);
    let (mut s, t) = rr_session(p.clone(), 7);
    let mut rig = Rig::new();
    rig.publish(&s);
    assert_eq!(levels(&rig.render(4 * STEP_BLOCKS), 4), expected(0..4, 3, None));
    p.groups = vec![random(1)];
    set_patch(&mut s, t, Some(p));
    assert_eq!(rig.publish(&s), Adoption::Adopted);
    assert_eq!(levels(&rig.render(3 * STEP_BLOCKS), 3), expected(4..7, 3, None), "a Sequential group continues its order after the republish");
}

#[test]
fn a_group_mode_edit_keeps_the_instance_and_prepares_no_more_than_a_zone_edit() {
    let p = rr_patch(3, None);
    let (mut s, t) = rr_session(p.clone(), 4);
    let uid = |s: &Session| s.track(t).unwrap().instrument.as_ref().unwrap().uid.get();
    let first = uid(&s);
    let mut rig = Rig::new();
    rig.publish(&s);
    rig.render(2);
    let counts = |rig: &Rig| (rig.log.made.load(Ordering::Relaxed), rig.log.prepared.load(Ordering::Relaxed));
    let start = counts(&rig);
    let edit = |rig: &mut Rig, s: &mut Session, f: &dyn Fn(&mut PatchV1), q: &mut PatchV1| {
        f(q);
        set_patch(s, t, Some(q.clone()));
        assert_eq!(rig.publish(s), Adoption::Adopted);
        rig.render(2);
        counts(rig)
    };
    let mut q = p.clone();
    // Today's structural edit: a zone's release.
    let zone = edit(&mut rig, &mut s, &|q| q.zones[1].release_ms = 20.0, &mut q);
    // A group's mode, a fade and a crossfade: each a publication to the same instance.
    let group = edit(&mut rig, &mut s, &|q| q.groups = vec![GroupEntry { id: 0, round_robin: RoundRobin::Random, ..GroupEntry::default() }], &mut q);
    let fade = edit(&mut rig, &mut s, &|q| q.zones[2].fades = transit_model::sampler_fades::Fades::of([0, 0, 3, 0]), &mut q);
    let crossfade = edit(
        &mut rig,
        &mut s,
        &|q| {
            let z = &mut q.zones[3];
            (z.loop_mode, z.loop_start, z.loop_end, z.crossfade) = (transit_model::sampler_patch::LoopMode::Forward, 0, 1000, 100);
        },
        &mut q,
    );
    assert_eq!(start.0, zone.0, "a zone edit keeps the instance");
    for (what, c) in [("a group mode edit", group), ("a fade edit", fade), ("a crossfade edit", crossfade)] {
        assert_eq!(c.0, start.0, "{what} keeps the instance");
    }
    let zone_prepares = zone.1 - start.1;
    for (what, before, after) in [("a group mode edit", zone, group), ("a fade edit", group, fade), ("a crossfade edit", fade, crossfade)] {
        assert!(after.1 - before.1 <= zone_prepares, "{what} prepares no more than a zone edit ({} > {zone_prepares})", after.1 - before.1);
    }
    assert_eq!(uid(&s), first, "the same uid");
    assert_eq!(rig.comp.samplers.len(), 1);
    assert_eq!(rig.drops(), 0);
}
