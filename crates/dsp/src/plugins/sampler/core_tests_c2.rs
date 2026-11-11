//! The core path's C2 behavior at the plug-in boundary: the handoff's source identities (an
//! unchanged zone keeps its id and plays on bit for bit; an id never names two samples while a
//! generation holds it; a sample listed twice takes one id), same-offset event order, the stereo
//! shorter-channel rule, the control lane, the level and pan glide and Reset All Controllers at its
//! frame. Events and buffers are injected; nothing waits on a clock.

use super::*;
use crate::event::{Message, NoteId};
use crate::{SampleBuffer, SampleSource};
use std::sync::Arc;

const SR: f32 = 48_000.0;

fn mono(data: Vec<f32>) -> Arc<dyn SampleSource> {
    Arc::new(SampleBuffer { sample_rate: SR, channels: vec![data] })
}

/// A source whose frame `n` holds `n + 1 + offset` (every position distinct).
fn numbered(frames: usize, offset: f32) -> Arc<dyn SampleSource> {
    mono((0..frames).map(|n| n as f32 + 1.0 + offset).collect())
}

/// Gate 1's controls with release `release` frames, key tracking off, root at `note`.
fn unity(mode: PlayMode, note: u8, release: u32) -> Playback {
    Playback {
        mode,
        key_tracking: false,
        root: note,
        attack_ms: 0.0,
        decay_ms: 0.0,
        sustain: 1.0,
        release_ms: release as f32 * 1000.0 / SR,
        declick_ms: 0.0,
        ..Playback::pad(note)
    }
}

fn zone(source: u16, frames: u64, playback: Playback) -> Zone {
    Zone { playback, ..Zone::new(Region::whole(source, frames)) }
}

/// Pad `p` (note `60 + p`) bound to zone `p` for each zone given.
fn spec(sources: Vec<Arc<dyn SampleSource>>, zones: Vec<Option<Zone>>) -> PatchSpec {
    let mut s = PatchSpec { sources, zones, ..PatchSpec::default() };
    for (p, z) in s.zones.iter().enumerate() {
        if z.is_some() {
            s.pads[p].zone = Some(p as u16);
        }
    }
    s
}

fn instance() -> (Sampler, Publisher) {
    let mut s = Sampler::new();
    s.prepare(SR, 512, 2);
    let p = s.enable_core(7);
    (s, p)
}

fn on(s: &mut Sampler, offset: u32, key: u8) {
    let ev = PluginEvent { channel: 0, note_id: -1, message: Message::NoteOn { key, velocity: 127, id: NoteId::NONE } };
    assert!(s.event(offset, &ev));
}

fn off(s: &mut Sampler, offset: u32, key: u8) {
    let ev = PluginEvent { channel: 0, note_id: -1, message: Message::NoteOff { key, velocity: 64, id: NoteId::NONE } };
    assert!(s.event(offset, &ev));
}

fn block(s: &mut Sampler, frames: usize) -> Vec<f32> {
    let mut io = vec![vec![f32::NAN; frames]; 2];
    s.process(&mut io, frames);
    io.swap_remove(0)
}

fn source_of(s: &Sampler, z: usize) -> u16 {
    s.core.as_deref().and_then(|c| c.core().zone(z)).map(|z| z.region.source).unwrap()
}

#[test]
fn an_unchanged_zone_keeps_its_source_id_and_plays_on_bit_for_bit() {
    let a = numbered(1024, 0.0);
    let silent = mono(vec![0.0; 1024]);
    let other = mono(vec![0.0; 1024]);
    let zones = || vec![Some(zone(0, 1024, unity(PlayMode::OneShot, 60, 0))), Some(zone(1, 1024, unity(PlayMode::OneShot, 61, 0)))];
    // The reference plays both zones without a publication in between.
    let (mut reference, mut rp) = instance();
    rp.publish(spec(vec![Arc::clone(&a), Arc::clone(&silent)], zones())).unwrap();
    let (mut s, mut p) = instance();
    p.publish(spec(vec![Arc::clone(&a), Arc::clone(&silent)], zones())).unwrap();
    for x in [&mut reference, &mut s] {
        on(x, 0, 60);
        on(x, 0, 61);
    }
    let mut want = block(&mut reference, 64);
    let mut got = block(&mut s, 64);
    let before = source_of(&s, 0);
    // Zone 1 moves to another (silent) sample; zone 0's sample is the same `Arc`.
    p.publish(spec(vec![Arc::clone(&a), Arc::clone(&other)], zones())).unwrap();
    want.extend(block(&mut reference, 128));
    got.extend(block(&mut s, 128));
    assert_eq!(source_of(&s, 0), before, "the unchanged zone keeps its source id");
    assert_ne!(source_of(&s, 1), before);
    for (n, (w, g)) in want.iter().zip(&got).enumerate() {
        assert_eq!(w.to_bits(), g.to_bits(), "frame {n}: the unchanged zone's voice plays on");
    }
    assert!(got.iter().take(192).all(|y| *y != 0.0), "zone 0 sounds throughout");
}

#[test]
fn an_id_never_names_two_samples_while_a_generation_holds_it() {
    let (s0, s1, s2) = (numbered(256, 0.0), numbered(256, 1000.0), numbered(256, 2000.0));
    let z = |src: u16| Some(zone(src, 256, unity(PlayMode::OneShot, 60, 0)));
    let (mut s, mut p) = instance();
    // A {s0}.
    p.publish(spec(vec![Arc::clone(&s0)], vec![z(0)])).unwrap();
    block(&mut s, 8);
    let id0 = source_of(&s, 0);
    // B {s0, s1}: A is retired (no voice reads it) and collected; s0 is still B's.
    p.publish(spec(vec![Arc::clone(&s0), Arc::clone(&s1)], vec![z(0), z(1)])).unwrap();
    block(&mut s, 8);
    assert_eq!(p.collect(), 1, "A is collected");
    assert_eq!(source_of(&s, 0), id0);
    // A new sample while B is live must not take s0's id.
    p.publish(spec(vec![Arc::clone(&s2), Arc::clone(&s1)], vec![z(0), z(1)])).unwrap();
    block(&mut s, 8);
    let id2 = source_of(&s, 0);
    assert_ne!(id2, id0, "s2 took the id s0 still has in B");
    assert_ne!(id2, source_of(&s, 1));
}

#[test]
fn a_sample_listed_twice_takes_one_id_and_is_released_once() {
    let a = numbered(256, 0.0);
    let (mut s, mut p) = instance();
    let zones = vec![Some(zone(0, 256, unity(PlayMode::OneShot, 60, 0))), Some(zone(1, 256, unity(PlayMode::OneShot, 61, 0)))];
    p.publish(spec(vec![Arc::clone(&a), Arc::clone(&a)], zones)).unwrap();
    block(&mut s, 8);
    assert_eq!(source_of(&s, 0), source_of(&s, 1), "one id for the one sample");
    assert_eq!(p.held_ids(), 1);
    p.publish(PatchSpec::default()).unwrap();
    block(&mut s, 8);
    p.collect();
    assert_eq!(p.held_ids(), 0, "the generation released the id it held: nothing leaks");
}

#[test]
fn events_at_one_offset_keep_their_order_at_the_plug_in_boundary() {
    // A Gate pad: an off then an on at frame 16 retriggers (the new voice starts at the region
    // entry and keeps sounding); the other order would release the new voice at once.
    let (mut s, mut p) = instance();
    p.publish(spec(vec![numbered(1024, 0.0)], vec![Some(zone(0, 1024, unity(PlayMode::Gate, 60, 8)))])).unwrap();
    on(&mut s, 0, 60);
    off(&mut s, 16, 60);
    on(&mut s, 16, 60);
    let out = block(&mut s, 48);
    for (n, y) in out.iter().enumerate() {
        let want = if n < 16 { n as f32 + 1.0 } else { (n - 16) as f32 + 1.0 };
        assert_eq!(y.to_bits(), want.to_bits(), "frame {n}");
    }
}

/// A stereo source whose `frames()` reports the longer channel.
struct Uneven(Vec<f32>, Vec<f32>);

impl SampleSource for Uneven {
    fn sample_rate(&self) -> f32 {
        SR
    }
    fn num_channels(&self) -> usize {
        2
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        [&self.0, &self.1].get(ch).map(|c| c.as_slice())
    }
    fn frames(&self) -> usize {
        self.0.len().max(self.1.len())
    }
}

#[test]
fn a_stereo_source_plays_its_shorter_channel_and_a_region_past_it_is_refused() {
    let uneven: Arc<dyn SampleSource> = Arc::new(Uneven(vec![1.0; 100], vec![1.0; 80]));
    let (mut s, mut p) = instance();
    let over = spec(vec![Arc::clone(&uneven)], vec![Some(zone(0, 90, unity(PlayMode::OneShot, 60, 0)))]);
    assert_eq!(check_spec(&over), Err(PublishError::ShortChannel { zone: 0 }));
    assert_eq!(p.publish(over), Err(PublishError::ShortChannel { zone: 0 }));
    let past = spec(vec![Arc::clone(&uneven)], vec![Some(zone(0, 101, unity(PlayMode::OneShot, 60, 0)))]);
    assert_eq!(p.publish(past), Err(PublishError::Region { zone: 0 }));
    p.publish(spec(vec![uneven], vec![Some(zone(0, 80, unity(PlayMode::OneShot, 60, 0)))])).unwrap();
    on(&mut s, 0, 60);
    let out = block(&mut s, 96);
    assert!(out.iter().take(80).all(|y| *y == 1.0) && out.iter().skip(80).all(|y| *y == 0.0));
    assert_eq!(s.core.as_deref().unwrap().core().diagnostics().underruns, 0, "nothing past the shorter channel is read");
}

#[test]
fn control_values_reach_the_instance_through_the_lane_in_order() {
    let (mut s, mut p) = instance();
    p.publish(spec(vec![mono(vec![1.0; 4096])], vec![Some(zone(0, 4096, unity(PlayMode::Gate, 60, 0)))])).unwrap();
    assert!(!p.set_control("level", -6.0), "not a slot id");
    assert!(!p.set_control("zone257_level", -6.0));
    for _ in 0..CONTROL_RING + 10 {
        assert!(p.set_control("zone001_pan", 20.0));
    }
    assert!(p.set_control("zone001_level", -6.0));
    assert_eq!(p.controls_waiting(), 11, "the full ring keeps the rest waiting, in order");
    block(&mut s, 8);
    assert_eq!(s.param("zone001_pan"), Some(20.0));
    assert_eq!(s.param("zone001_level"), Some(0.0), "not yet: it waits behind the others");
    p.collect();
    assert_eq!(p.controls_waiting(), 0);
    block(&mut s, 8);
    assert_eq!(s.param("zone001_level"), Some(-6.0));
}

#[test]
fn a_slot_level_change_glides_linearly_across_the_block() {
    let (mut s, mut p) = instance();
    p.publish(spec(vec![mono(vec![1.0; 4096])], vec![Some(zone(0, 4096, unity(PlayMode::Gate, 60, 0)))])).unwrap();
    on(&mut s, 0, 60);
    let first = block(&mut s, 32);
    assert!(first.iter().all(|y| *y == 1.0));
    assert!(s.set_param("zone001_level", -12.0));
    let out = block(&mut s, 64);
    let to = 10f32.powf(-12.0 / 20.0);
    let step = (1.0 - to) / 64.0;
    let mut prev = 1.0f32;
    for (n, y) in out.iter().enumerate() {
        let want = 1.0 + (to - 1.0) * ((n + 1) as f32 / 64.0);
        assert_eq!(y.to_bits(), want.to_bits(), "frame {n}");
        assert!(prev - y <= step + f32::EPSILON, "frame {n}: a step of {} (the glide's is {step})", prev - y);
        prev = *y;
    }
    let after = block(&mut s, 16);
    assert!(after.iter().all(|y| y.to_bits() == to.to_bits()), "the new level holds");
}

#[test]
fn reset_all_controllers_acts_at_its_own_frame() {
    let (mut s, mut p) = instance();
    p.publish(spec(vec![mono(vec![1.0; 4096])], vec![Some(zone(0, 4096, unity(PlayMode::Gate, 60, 8)))])).unwrap();
    s.midi(0, [0xB0, 64, 127]);
    on(&mut s, 0, 60);
    off(&mut s, 2, 60);
    s.midi(8, [0xB0, 121, 0]);
    let out = block(&mut s, 24);
    for (n, y) in out.iter().enumerate() {
        let want = match n {
            0..8 => 1.0,
            8..16 => 1.0 - (n - 7) as f32 / 8.0,
            _ => 0.0,
        };
        assert_eq!(y.to_bits(), want.to_bits(), "frame {n}: the pedal lifts at frame 8, not at the block start");
    }
}

#[test]
fn a_slot_level_or_pan_edit_holds_after_a_reset() {
    let (mut s, mut p) = instance();
    p.publish(spec(vec![mono(vec![1.0; 4096])], vec![Some(zone(0, 4096, unity(PlayMode::Gate, 60, 0)))])).unwrap();
    // A long run first, so the edit lands far from frame 0.
    for _ in 0..100 {
        block(&mut s, 512);
    }
    assert!(s.set_param("zone001_level", -12.0));
    assert!(s.set_param("zone001_pan", 50.0));
    block(&mut s, 512);
    let to = 10f32.powf(-12.0 / 20.0);
    on(&mut s, 0, 60);
    let before = block(&mut s, 16);
    assert!(before.iter().all(|y| y.to_bits() == (to * 0.5).to_bits()), "the edit holds while playing: {}", before[0]);
    s.reset();
    on(&mut s, 0, 60);
    let mut io = vec![vec![f32::NAN; 64]; 2];
    s.process(&mut io, 64);
    for (n, (l, r)) in io[0].iter().zip(&io[1]).enumerate() {
        assert_eq!(l.to_bits(), (to * 0.5).to_bits(), "left, frame {n} after the reset: the new level and pan");
        assert_eq!(r.to_bits(), to.to_bits(), "right, frame {n} after the reset");
    }
}

/// Each key-map group's round-robin mode travels with the patch: a Random group plays the seeded
/// xorshift32 sequence of the instance's seed (ADR 0041 amendment 2026-11-08, item 4), computed
/// here independently.
#[test]
fn a_random_group_mode_reaches_the_core() {
    let (mut s, mut p) = instance();
    let sources: Vec<Arc<dyn SampleSource>> = (0..4).map(|k| mono(vec![k as f32 + 1.0; 4])).collect();
    let zones = (0..4u8)
        .map(|a| {
            let membership = Some(Membership { group: 3, layer: 0, alternative: a });
            Some(Zone { membership, ..zone(u16::from(a), 4, unity(PlayMode::OneShot, 60, 0)) })
        })
        .collect();
    let mut groups = [GroupMode::Sequential; GROUPS];
    groups[3] = GroupMode::Random;
    p.publish(PatchSpec { sources, zones, groups, ..PatchSpec::default() }).unwrap();
    let mut x = 7u32 ^ 4u32.wrapping_mul(0x9E37_79B9);
    for _ in 0..16 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        on(&mut s, 0, 60);
        let out = block(&mut s, 8);
        assert_eq!(out[0], (x % 4) as f32 + 1.0);
    }
}
