//! The core path at the plug-in boundary (ADR 0041 Gate 1 shape: events and transport samples are
//! injected; no sleeps, elapsed-time thresholds or scheduling assertions): Gate 1's frames through
//! the event conversion and the plug-in's buffers, block-size invariance, the handoff's credit and
//! retirement rules, the slot parameter ids, the event queue's reserve and what the transport sends.
//! The allocation and free probe is the integration test `tests/sampler_core_alloc.rs`.

use super::*;
use crate::event::{Message, NoteId};
use crate::{SampleBuffer, SampleSource};
use std::sync::Arc;
use transit_sampler_core::patch::{Pad, PlayMode, Playback, Zone};
use transit_sampler_core::playhead::{LoopMode, Region};

const SR: f32 = 48_000.0;

fn mono(data: Vec<f32>) -> Arc<dyn SampleSource> {
    Arc::new(SampleBuffer { sample_rate: SR, channels: vec![data] })
}

fn constant(value: f32, frames: usize) -> Arc<dyn SampleSource> {
    mono(vec![value; frames])
}

/// Gate 1's controls: attack, decay and declick zero, sustain 1, release `release` frames, key
/// tracking off with the root at `note`.
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

/// A spec binding pad `p` (trigger note `60 + p`) to zone `z` for each `(p, z)`.
fn spec(sources: Vec<Arc<dyn SampleSource>>, zones: Vec<Option<Zone>>, binds: &[(usize, u16)]) -> PatchSpec {
    let mut s = PatchSpec { sources, zones, ..PatchSpec::default() };
    for &(p, z) in binds {
        s.pads[p].zone = Some(z);
    }
    s
}

/// A stereo core-path instance at 48 kHz.
fn instance() -> (Sampler, Publisher) {
    let mut s = Sampler::new();
    s.prepare(SR, 512, 2);
    let p = s.enable_core(7);
    (s, p)
}

fn note_on(s: &mut Sampler, offset: u32, key: u8, id: i32) {
    let ev = PluginEvent { channel: 0, note_id: id, message: Message::NoteOn { key, velocity: 127, id: NoteId::NONE } };
    assert!(s.event(offset, &ev));
}

fn note_off(s: &mut Sampler, offset: u32, key: u8, id: i32) {
    let ev = PluginEvent { channel: 0, note_id: id, message: Message::NoteOff { key, velocity: 64, id: NoteId::NONE } };
    assert!(s.event(offset, &ev));
}

/// One block of `frames` into NaN-filled buffers (the plug-in must replace them); both channels.
fn block(s: &mut Sampler, frames: usize) -> [Vec<f32>; 2] {
    let mut io = vec![vec![f32::NAN; frames]; 2];
    s.process(&mut io, frames);
    let [l, r]: [Vec<f32>; 2] = io.try_into().unwrap_or_default();
    [l, r]
}

fn assert_frames(out: &[f32], expect: impl Fn(usize) -> f32) {
    for (n, y) in out.iter().enumerate() {
        assert_eq!(y.to_bits(), expect(n).to_bits(), "frame {n}: {y} != {}", expect(n));
    }
}

fn fade(n: usize, at: usize) -> f32 {
    if n < at {
        1.0
    } else if n < at + 32 {
        (31 - (n - at)) as f32 / 32.0
    } else {
        0.0
    }
}

#[test]
fn gate1_one_shot_and_gate_release_frames_at_the_plug_in_boundary() {
    for (mode, release) in [(PlayMode::OneShot, 0), (PlayMode::Gate, 8)] {
        let (mut s, mut p) = instance();
        p.publish(spec(vec![constant(1.0, 128)], vec![Some(zone(0, 128, unity(mode, 60, release)))], &[(0, 0)])).unwrap();
        // The publication is adopted at the next block boundary.
        assert_frames(&block(&mut s, 4)[0], |_| 0.0);
        note_on(&mut s, 0, 60, -1);
        note_off(&mut s, 16, 60, -1);
        let [l, r] = block(&mut s, 200);
        let tail = [0.875, 0.75, 0.625, 0.5, 0.375, 0.25, 0.125, 0.0];
        let expect = |n: usize| match mode {
            PlayMode::OneShot => f32::from(u8::from(n < 128)),
            PlayMode::Gate => match n {
                0..16 => 1.0,
                16..24 => tail[n - 16],
                _ => 0.0,
            },
        };
        assert_frames(&l, expect);
        assert_frames(&r, expect);
        assert_eq!(s.core.as_deref().unwrap().core().active_voices(), 0);
    }
}

#[test]
fn an_instance_off_the_core_path_takes_no_slot_ids_and_plays_silence() {
    let mut off = Sampler::new();
    off.prepare(SR, 512, 2);
    assert!(!off.uses_core());
    assert!(!off.set_param("zone001_level", -6.0), "slot ids exist only on the core path");
    off.note_on(0, 60, 127);
    let mut io = vec![vec![1.0; 256]; 2];
    off.process(&mut io, 256);
    assert!(io.iter().flatten().all(|x| *x == 0.0), "nothing to play");
    assert_eq!(off.tail_samples(), 0);
}

/// The block-size schedule: absolute frames of every event.
#[derive(Clone, Copy)]
enum Ev {
    On(u8, i32),
    Off(u8, i32),
    Msg(Message),
}

const SCHEDULE: [(usize, Ev); 12] = [
    (0, Ev::On(60, 1)),
    (33, Ev::On(40, 2)),
    (33, Ev::On(45, 3)),
    (100, Ev::Msg(Message::Control { cc: 1, value: 90 })),
    (250, Ev::Msg(Message::Control { cc: 64, value: 127 })),
    (400, Ev::Off(40, 2)),
    (700, Ev::On(60, 4)),
    (701, Ev::Msg(Message::PitchBend { value: 12_000 })),
    (900, Ev::Msg(Message::Control { cc: 64, value: 0 })),
    (1_313, Ev::On(61, 5)),
    (1_500, Ev::Off(45, 3)),
    (2_222, Ev::On(41, 6)),
];

/// A shaped, modulated patch: a pad one-shot and a stereo looped Gate zone under the keys, with a
/// free-running LFO (`sync`: tempo-synced, free-running) on the matrix.
fn shaped_instance(sync: bool) -> Sampler {
    let (mut s, mut p) = instance();
    let sine: Vec<f32> = (0..6_000).map(|i| (i as f32 * 0.031).sin() * 0.5).collect();
    let saw: Vec<f32> = (0..5_000).map(|i| (i % 211) as f32 / 211.0 - 0.5).collect();
    let stereo: Arc<dyn SampleSource> =
        Arc::new(SampleBuffer { sample_rate: 44_100.0, channels: vec![saw.clone(), saw.iter().map(|x| -x).collect()] });
    let looped = Zone {
        keys: (0, 59),
        playback: Playback { release_ms: 30.0, attack_ms: 2.0, ..Playback::zone() },
        ..Zone::new(Region::whole(1, 5_000).looped(LoopMode::Forward, 1_000, 4_000))
    };
    p.publish(spec(vec![mono(sine), stereo], vec![Some(zone(0, 6_000, Playback { declick_ms: 1.0, ..Playback::pad(60) })), Some(looped)], &[(0, 0)]))
        .unwrap();
    let edits: &[(&str, f32)] = &[
        ("zone001_pan", -30.0),
        ("zone002_shaper", 1.0),
        ("zone002_cutoff", 1_200.0),
        ("zone002_resonance", 40.0),
        ("zone002_lfo1_rate", 3.0),
        ("zone002_lfo1_retrigger", 0.0),
        ("zone002_lfo1_sync", if sync { 1.0 } else { 0.0 }),
        ("zone002_lfo1_division", 2.0),
        ("zone002_mod1_source", 1.0),
        ("zone002_mod1_dest", 7.0),
        ("zone002_mod1_amount", 50.0),
        ("zone002_mod2_source", 10.0),
        ("zone002_mod2_dest", 1.0),
        ("zone002_mod2_amount", 20.0),
    ];
    for &(id, v) in edits {
        assert!(s.set_param(id, v), "{id}");
    }
    s
}

/// Renders the schedule over `total` frames in blocks cycling through `sizes`; with `playing`, the
/// transport plays at 120 BPM from quarter 3. Returns both channels and the end-state fingerprint.
fn render(sync: bool, sizes: &[usize], playing: bool, total: usize) -> ([Vec<f32>; 2], u64) {
    let mut s = shaped_instance(sync);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    let (mut done, mut k) = (0, 0);
    while done < total {
        let n = sizes[k % sizes.len()].min(total - done);
        k += 1;
        if playing {
            let quarters = 3.0 + done as f64 * 120.0 / 60.0 / f64::from(SR);
            let bar = (quarters / 4.0).floor();
            s.set_transport(&Transport {
                playing: true,
                tempo: 120.0,
                numerator: 4,
                denominator: 4,
                samples: done as i64,
                quarters,
                bar: bar as i64,
                bar_start: bar * 4.0,
                ..Transport::default()
            });
        }
        for &(at, ev) in SCHEDULE.iter().filter(|(at, _)| (done..done + n).contains(at)) {
            let o = (at - done) as u32;
            match ev {
                Ev::On(key, id) => note_on(&mut s, o, key, id),
                Ev::Off(key, id) => note_off(&mut s, o, key, id),
                Ev::Msg(message) => assert!(s.event(o, &PluginEvent { channel: 0, note_id: -1, message })),
            }
        }
        let [a, b] = block(&mut s, n);
        l.extend(a);
        r.extend(b);
        done += n;
    }
    #[cfg_attr(not(feature = "testing"), allow(unused_mut))]
    let mut h = std::collections::hash_map::DefaultHasher::new();
    #[cfg(feature = "testing")]
    s.fingerprint(&mut h);
    (([l, r]), std::hash::Hasher::finish(&h))
}

fn same_bits(a: &[Vec<f32>; 2], b: &[Vec<f32>; 2]) -> Option<usize> {
    let ab = |x: &[Vec<f32>; 2]| x.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>();
    ab(a).iter().zip(ab(b)).position(|(x, y)| *x != y).or_else(|| (a[0].len() != b[0].len()).then_some(0))
}

const IRREGULAR: [usize; 6] = [1, 7, 127, 64, 512, 3];

#[test]
fn block_sizes_do_not_change_the_bits_with_the_transport_stopped() {
    let (want, fp) = render(false, &[64], false, 4_000);
    assert!(want[0].iter().any(|x| *x != 0.0) && want[1].iter().all(|x| x.is_finite()));
    for sizes in [&[512][..], &IRREGULAR] {
        let (got, gfp) = render(false, sizes, false, 4_000);
        assert_eq!(same_bits(&want, &got), None, "blocks {sizes:?}: first differing sample");
        assert_eq!(fp, gfp, "blocks {sizes:?}: end state");
    }
    // A tempo-synced, free-running LFO with the transport stopped runs free: also bit-equal.
    let (want, _) = render(true, &[64], false, 4_000);
    assert_eq!(same_bits(&want, &render(true, &IRREGULAR, false, 4_000).0), None);
}

/// What is promised while the transport plays. Every patch whose LFOs do not follow the song
/// position (free-rate or retriggered LFOs) is bit-equal at any block partition: the song position
/// only anchors tempo-synced, free-running LFOs. Such an LFO follows the song position, which the
/// core interpolates from each block's first frame (`quarters + n * tempo / 60 / rate`), so its
/// phase at a frame may differ between partitions by the rounding of that sum: not promised
/// bit-equal. A partition is always deterministic (the same blocks give the same bits), and the
/// difference stays at rounding size, asserted under 1e-4 of full scale over these 4000 frames (in
/// this patch, at the time of writing, the partitions happen to agree bit for bit).
#[test]
fn block_sizes_while_the_transport_plays_keep_what_is_promised() {
    let (want, _) = render(false, &[64], true, 4_000);
    for sizes in [&[512][..], &IRREGULAR] {
        assert_eq!(same_bits(&want, &render(false, sizes, true, 4_000).0), None, "free-rate LFO, blocks {sizes:?}");
    }
    let (synced, _) = render(true, &[64], true, 4_000);
    assert_eq!(same_bits(&synced, &render(true, &[64], true, 4_000).0), None, "one partition is deterministic");
    let other = render(true, &IRREGULAR, true, 4_000).0;
    let worst = synced.iter().flatten().zip(other.iter().flatten()).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    // The synced LFO does follow the song position: it sounds unlike the free-rate one.
    assert!(same_bits(&synced, &want).is_some(), "the synced LFO is audible in this patch");
    assert!(worst < 1.0e-4, "a synced LFO's song-position rounding stays at rounding size: {worst}");
}

#[test]
fn slot_ids_are_stable_and_the_unprefixed_legacy_ids_are_retired() {
    assert_eq!(ZONE_FIELDS, 113);
    assert_eq!(slot_id(0, FIELD_LEVEL).as_deref(), Some("zone001_level"));
    assert_eq!(slot_id(0, FIELD_PAN).as_deref(), Some("zone001_pan"));
    assert_eq!(slot_id(255, FIELD_LEVEL).as_deref(), Some("zone256_level"));
    assert_eq!(slot_id(0, 2).as_deref(), Some("zone001_shaper"));
    assert_eq!(slot_id(41, ZONE_FIELDS - 1).as_deref(), Some("zone042_mod16_curve"));
    assert_eq!(slot_id(256, 0), None);
    assert_eq!(slot_id(0, ZONE_FIELDS), None);
    // Every id is unique and resolves back to its slot and field.
    let table = params::SlotParams::new();
    let mut seen = std::collections::HashSet::new();
    for z in 0..transit_sampler_core::ZONES {
        for f in 0..ZONE_FIELDS {
            let id = slot_id(z, f).unwrap();
            assert_eq!(table.resolve(&id), Some((z, f)), "{id}");
            assert!(seen.insert(id));
        }
    }
    for bad in ["zone000_level", "zone257_level", "zone01_level", "zone001level", "zone001_", "zone001_nope", "zoneabc_level", "level_zone001"] {
        assert_eq!(table.resolve(bad), None, "{bad}");
    }
    // The table has no parameters: the 19 unprefixed legacy ids are retired (#455).
    let (mut s, mut p) = instance();
    assert!(SAMPLER_INFO.params.is_empty());
    assert!(!s.set_param("level", -6.0));
    assert_eq!(s.param("level"), None);
    assert_eq!(s.param("zone001_level"), Some(0.0));
    // The slot ids map to the zone's fields: level, pan, the matrix rows.
    p.publish(spec(vec![constant(1.0, 512)], vec![Some(zone(0, 512, unity(PlayMode::OneShot, 60, 0)))], &[(0, 0)])).unwrap();
    for (id, v) in
        [("zone001_level", -12.0), ("zone001_pan", 50.0), ("zone001_mod3_source", 9.0), ("zone001_mod3_dest", 1.0), ("zone001_mod3_amount", 25.0)]
    {
        assert!(s.set_param(id, v));
        assert_eq!(s.param(id), Some(v));
    }
    assert!(s.set_param("zone001_pan", 500.0), "clamped into range");
    assert_eq!(s.param("zone001_pan"), Some(100.0));
    block(&mut s, 1);
    let z = s.core.as_deref().unwrap().core().zone(0).copied().unwrap();
    assert_eq!(z.playback.level_db, -12.0);
    assert_eq!(z.playback.pan, 1.0);
    assert_eq!(z.matrix.rows().len(), 1);
    let row = z.matrix.rows()[0];
    assert_eq!((row.source, row.dest, row.amount), (9, 1, 0.25));
}

#[test]
fn the_event_queue_keeps_its_release_reserve_and_refuses_with_false() {
    let (mut s, _p) = instance();
    let ev = |message| PluginEvent { channel: 2, note_id: -1, message };
    let mut ons = 0;
    for i in 0..1_024u32 {
        if s.event(0, &ev(Message::NoteOn { key: (i % 128) as u8, velocity: 100, id: NoteId::NONE })) {
            ons += 1;
        }
    }
    assert_eq!(ons, 1_024 - 257, "non-releases stop at the reserve");
    for i in 0..257u32 {
        assert!(s.event(1, &ev(Message::NoteOff { key: (i % 128) as u8, velocity: 0, id: NoteId::NONE })), "release {i}");
    }
    assert_eq!(s.core.as_deref().unwrap().queued(), 1_024);
    assert!(!s.event(2, &ev(Message::NoteOff { key: 1, velocity: 0, id: NoteId::NONE })), "a full queue refuses even a release");
    block(&mut s, 64);
    assert_eq!(s.core.as_deref().unwrap().queued(), 0);
    assert!(s.event(0, &ev(Message::NoteOn { key: 1, velocity: 100, id: NoteId::NONE })), "the queue empties each block");
}

#[test]
fn stopping_the_transport_fades_every_voice_and_a_locate_sends_nothing() {
    let (mut s, mut p) = instance();
    p.publish(spec(vec![constant(1.0, 4_800)], vec![Some(zone(0, 4_800, unity(PlayMode::OneShot, 60, 0)))], &[(0, 0)])).unwrap();
    let t = |samples: i64, playing: bool| Transport {
        playing,
        tempo: 120.0,
        numerator: 4,
        denominator: 4,
        samples,
        quarters: samples as f64 / 24_000.0,
        ..Transport::default()
    };
    s.set_transport(&t(0, true));
    note_on(&mut s, 0, 60, -1);
    assert_frames(&block(&mut s, 64)[0], |_| 1.0);
    // A jump while playing (a locate, a loop wrap) leaves the voice alone.
    s.set_transport(&t(96_000, true));
    assert_frames(&block(&mut s, 64)[0], |_| 1.0);
    s.set_transport(&t(96_064, false));
    assert_frames(&block(&mut s, 64)[0], |n| fade(n, 0));
    assert_eq!(s.core.as_deref().unwrap().core().active_voices(), 0);
}

/// The handoff's rules, in 8-frame blocks so a replaced zone's 32-frame fade spans several blocks.
#[test]
fn the_handoff_keeps_credits_retires_after_the_voices_and_holds_on_a_full_queue() {
    const B: usize = 8;
    let (mut s, mut p) = instance();
    // An unbound zone: every note takes its own voice (a pad would retrigger its one voice).
    let one = |value: f32| spec(vec![constant(value, 4_800)], vec![Some(zone(0, 4_800, unity(PlayMode::OneShot, 60, 0)))], &[]);
    // Generation A (source 3.0) is adopted; a note sounds from it.
    p.publish(one(3.0)).unwrap();
    block(&mut s, B);
    note_on(&mut s, 0, 60, -1);
    assert_frames(&block(&mut s, B)[0], |_| 3.0);
    // B replaces zone 0's sample: at its adoption the old voice fades, still reading A's 3.0.
    p.publish(one(5.0)).unwrap();
    assert_frames(&block(&mut s, B)[0], |n| 3.0 * fade(n, 0));
    assert_eq!(s.core.as_deref().unwrap().held(), (true, 1), "A is retained while its voice fades");
    assert_eq!(p.collect(), 0, "A is not retired while a voice reads it");
    // A note on B, then C replaces the sample again: B's voice fades too; both are retained.
    note_on(&mut s, 0, 60, -1);
    block(&mut s, B);
    p.publish(one(7.0)).unwrap();
    let out = block(&mut s, B);
    assert_frames(&out[0], |n| 3.0 * fade(n + 2 * B, 0) + 5.0 * fade(n, 0));
    assert_eq!(s.core.as_deref().unwrap().held(), (true, 2));
    // D waits: both retained slots are full, so the old patch (C) keeps playing.
    p.publish(one(9.0)).unwrap();
    assert_eq!(p.credits_in_use(), 4);
    assert!(p.pending());
    assert_eq!(p.publish(one(11.0)), Err(PublishError::NoCredit), "a fifth preparation waits");
    note_on(&mut s, 0, 60, -1);
    let out = block(&mut s, B);
    assert!(p.pending(), "not adopted while the retained slots are full");
    assert_eq!(out[0][0].to_bits(), (3.0 * fade(3 * B, 0) + 5.0 * fade(B, 0) + 7.0).to_bits(), "C plays");
    // A's voice ends 32 frames after B's adoption: A retires at the end of that block.
    let mut retired = 0;
    for _ in 0..8 {
        block(&mut s, B);
        retired += p.collect();
        if retired == 2 {
            break;
        }
    }
    assert_eq!(retired, 2, "A and then B retire once their voices are done, dropped on the control side");
    assert!(!p.pending(), "D was adopted once a retained slot was free");
    // A full retirement queue keeps the old generation retained: no collect from here on.
    let mut held = s.core.as_deref().unwrap().held();
    for value in [13.0, 15.0] {
        p.publish(one(value)).unwrap();
        for _ in 0..8 {
            block(&mut s, B);
        }
        held = s.core.as_deref().unwrap().held();
    }
    assert_eq!(held, (true, 1), "two generations sit in the full queue; the third stays retained");
    assert_eq!(p.collect(), 2);
    block(&mut s, B);
    assert_eq!(s.core.as_deref().unwrap().held(), (true, 0));
    assert_eq!(p.collect(), 1);
    assert_eq!(p.credits_in_use(), 1);
}

#[test]
fn a_publication_is_checked_whole_and_a_refused_one_changes_nothing() {
    let (mut s, mut p) = instance();
    let ok = zone(0, 100, unity(PlayMode::OneShot, 60, 0));
    assert_eq!(
        p.publish(spec(vec![constant(1.0, 100)], vec![Some(Zone { region: Region::whole(3, 100), ..ok })], &[])),
        Err(PublishError::Source { zone: 0 })
    );
    assert_eq!(p.publish(spec(vec![constant(1.0, 99)], vec![Some(ok)], &[])), Err(PublishError::Region { zone: 0 }));
    let three: Arc<dyn SampleSource> = Arc::new(SampleBuffer { sample_rate: SR, channels: vec![vec![1.0; 100]; 3] });
    assert_eq!(p.publish(spec(vec![three], vec![Some(ok)], &[])), Err(PublishError::Channels { zone: 0 }));
    let mut dup = spec(vec![constant(1.0, 100)], vec![Some(ok), Some(ok)], &[(0, 0), (1, 1)]);
    dup.pads[1].note = dup.pads[0].note;
    assert_eq!(p.publish(dup), Err(PublishError::Patch(transit_sampler_core::patch::PatchError::DuplicateNote)));
    assert_eq!(p.credits_in_use(), 0);
    assert!(!p.pending());
    block(&mut s, 8);
    assert_eq!(s.core.as_deref().unwrap().held(), (false, 0));
    // Pads swapping trigger notes are one valid patch.
    let mut swap = spec(vec![constant(1.0, 100)], vec![Some(ok), Some(ok)], &[(0, 0), (1, 1)]);
    (swap.pads[0].note, swap.pads[1].note) = (61, 60);
    p.publish(swap).unwrap();
    block(&mut s, 8);
    assert_eq!(s.core.as_deref().unwrap().core().pad(0).map(|p| p.note), Some(61));
    let _: Pad = *s.core.as_deref().unwrap().core().pad(1).unwrap();
}
