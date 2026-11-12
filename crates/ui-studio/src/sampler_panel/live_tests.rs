//! The pads' live state from what the instrument publishes (`PadWord`s through the tap): the hit
//! fill at the velocity falling over 250 ms, the cut dropping it at once and inverting the badge for
//! 200 ms, all at the frame clock's fixed 1/60 s; and ADR 0041's rule driven end to end through
//! the published state: a muted pad, or a pad a solo excludes, neither sounds nor cuts.

use super::test_rig::T;
use std::sync::Arc;
use transit_dsp::plugins::sampler::{PatchSpec, PlayMode, Playback, Region, Sampler, Zone};
use transit_dsp::{PAD_SLOTS, PadWord, Plugin, SampleBuffer, SampleSource};

fn push(tap: &transit_mix::ScopeTap, words: &[(usize, PadWord)]) {
    let mut all = tap.pad_activity();
    for (i, w) in words {
        all[*i] = w.0;
    }
    tap.push_pads(&all);
}

#[test]
fn a_published_hit_fills_the_pad_at_its_velocity_and_falls_over_250_ms() {
    let (mut t, tr) = T::kit();
    let tap = t.tap(tr);
    // Counts that were there before the panel looked do not flash.
    push(&tap, &[(0, PadWord(0).hit(127))]);
    t.frames(1);
    assert!(t.panel(tr).pad(0).unwrap().pad.hit.is_none(), "negative control: a trigger the panel did not see happen");
    // A new trigger at velocity 127: the full pad, then nothing 15 frames (250 ms) later.
    push(&tap, &[(0, PadWord(tap.pad_activity()[0]).hit(127))]);
    t.frames(1);
    let p = t.panel(tr);
    let pad = &p.pad(0).unwrap().pad;
    let full = pad.hit.expect("the hit shows");
    // The whole inside of the selected pad (its 2 px border).
    assert!((full[3] - full[1] - (56.0 - 2.0 * pad.border_width)).abs() < 0.02, "{full:?}");
    let mut heights = vec![full[3] - full[1]];
    for _ in 0..15 {
        t.frames(1);
        heights.push(t.panel(tr).pad(0).unwrap().pad.hit.map_or(0.0, |h| h[3] - h[1]));
    }
    assert!(heights.windows(2).all(|w| w[1] < w[0] || w[1] == 0.0), "{heights:?}");
    assert_eq!(*heights.last().unwrap(), 0.0, "gone after 250 ms: {heights:?}");
    // Velocity 64 fills half the pad.
    push(&tap, &[(1, PadWord(tap.pad_activity()[1]).hit(64))]);
    t.frames(1);
    let h = t.panel(tr).pad(1).unwrap().pad.hit.unwrap();
    assert!((h[3] - h[1] - 54.0 * 64.0 / 127.0).abs() < 0.02);
    assert!(t.panel(tr).pad(0).unwrap().pad.hit.is_none(), "the other pad is at rest");
}

#[test]
fn a_published_cut_drops_the_fill_and_inverts_the_badge_for_200_ms() {
    let (mut t, tr) = T::kit();
    t.run("mix.sampler_pad_set", serde_json::json!({"track": tr.0, "pad": 2, "choke": 1}));
    let tap = t.tap(tr);
    push(&tap, &[]);
    t.frames(1);
    push(&tap, &[(1, PadWord(0).hit(127))]);
    t.frames(2);
    assert!(t.panel(tr).pad(1).unwrap().pad.hit.is_some());
    push(&tap, &[(1, PadWord(tap.pad_activity()[1]).choked())]);
    t.frames(1);
    let pad = t.panel(tr).pad(1).unwrap().pad.clone();
    assert!(pad.hit.is_none(), "the fill drops at once");
    assert!(pad.choked && pad.badge_fill == crate::theme::TOKENS.text_muted, "the badge inverts");
    let mut shown = vec![true];
    for _ in 0..12 {
        t.frames(1);
        shown.push(t.panel(tr).pad(1).unwrap().pad.choked);
    }
    // Inverted on frames 0 to 11 after the cut (under 200 ms), back on frame 12 (200 ms).
    assert!(shown[..12].iter().all(|x| *x) && !shown[12], "{shown:?}");
    // Negative control: the pad that was not cut never inverted.
    assert!(!t.panel(tr).pad(0).unwrap().pad.choked);
}

/// A dsp Sampler with pads 0 and 1 in choke group 1 (one-shots, 4800 frames each), pad 1 muted
/// when `mute`, pad 2 (no group) solo when `solo`.
fn instance(mute: bool, solo: bool) -> Sampler {
    let mut s = Sampler::new();
    s.prepare(48_000.0, 512, 2);
    let mut p = s.enable_core(1);
    let src = |o: f32| Arc::new(SampleBuffer { sample_rate: 48_000.0, channels: vec![vec![0.25 + o; 4800]] }) as Arc<dyn SampleSource>;
    let pb = |n: u8| Playback { mode: PlayMode::OneShot, key_tracking: false, root: n, attack_ms: 0.0, release_ms: 0.0, ..Playback::pad(n) };
    let mut spec = PatchSpec {
        sources: vec![src(0.0), src(0.1), src(0.2)],
        zones: (0..3).map(|i| Some(Zone { playback: pb(60 + i as u8), ..Zone::new(Region::whole(i as u16, 4800)) })).collect(),
        ..PatchSpec::default()
    };
    for (i, pad) in spec.pads[..3].iter_mut().enumerate() {
        pad.zone = Some(i as u16);
    }
    spec.pads[0].choke = 1;
    spec.pads[1].choke = 1;
    spec.pads[1].mute = mute;
    spec.pads[2].solo = solo;
    p.publish(spec).unwrap();
    let mut io = vec![vec![0.0; 64]; 2];
    s.process(&mut io, 64);
    s
}

/// Plays `notes` (one block each) and returns the output peak of the last block and the words.
fn play(s: &mut Sampler, notes: &[u8]) -> (f32, [u32; PAD_SLOTS]) {
    let mut peak = 0.0f32;
    for n in notes {
        s.note_on(0, *n, 100);
        let mut io = vec![vec![0.0; 128]; 2];
        s.process(&mut io, 128);
        peak = io[0].iter().fold(0.0, |m, x| m.max(x.abs()));
    }
    let mut words = [0u32; PAD_SLOTS];
    assert!(s.pad_activity(&mut words));
    (peak, words)
}

#[test]
fn a_muted_or_solo_excluded_pad_neither_sounds_nor_cuts_through_the_published_state() {
    // Pad 1 (A2) is muted; pad 0 (A1) sounds, then A2's note: A2 admits nothing and cuts nothing.
    // The panel's pads carry the same groups, so a cut would show on A1's badge.
    let (mut t, tr) = T::kit();
    t.run("mix.sampler_pad_set", serde_json::json!({"track": tr.0, "pad": 1, "choke": 1}));
    t.run("mix.sampler_pad_set", serde_json::json!({"track": tr.0, "pad": 2, "choke": 1, "mute": true}));
    let tap = t.tap(tr);
    push(&tap, &[]);
    t.frames(1);
    let mut s = instance(true, false);
    let (_, words) = play(&mut s, &[60, 61]);
    tap.push_pads(&words);
    t.frames(1);
    let p = t.panel(tr);
    assert!(p.pad(1).unwrap().pad.hit.is_none(), "the muted pad shows no hit");
    assert!(!p.pad(0).unwrap().pad.choked, "and cut nothing");
    assert!(p.pad(0).unwrap().pad.hit.is_some(), "A1's own hit shows");
    // Pad 2 (A3) is solo: A1's note is excluded, A3's plays.
    let (mut t, tr) = T::kit();
    let tap = t.tap(tr);
    push(&tap, &[]);
    t.frames(1);
    let mut s = instance(false, true);
    let (silent, words) = play(&mut s, &[60]);
    assert_eq!(silent, 0.0, "the excluded pad does not sound");
    tap.push_pads(&words);
    t.frames(1);
    assert!(t.panel(tr).pad(0).unwrap().pad.hit.is_none());
    let (loud, words) = play(&mut s, &[62]);
    assert!(loud > 0.0);
    tap.push_pads(&words);
    t.frames(1);
    assert!(t.panel(tr).pad(2).unwrap().pad.hit.is_some());
    // Negative control: unmuted, A2's note cuts A1 and the panel inverts A1's badge.
    let (mut t, tr) = T::kit();
    t.run("mix.sampler_pad_set", serde_json::json!({"track": tr.0, "pad": 1, "choke": 1}));
    t.run("mix.sampler_pad_set", serde_json::json!({"track": tr.0, "pad": 2, "choke": 1}));
    let tap = t.tap(tr);
    push(&tap, &[]);
    t.frames(1);
    let mut s = instance(false, false);
    let (_, words) = play(&mut s, &[60, 61]);
    tap.push_pads(&words);
    t.frames(1);
    let p = t.panel(tr);
    assert!(p.pad(0).unwrap().pad.choked && p.pad(0).unwrap().pad.hit.is_none());
    assert!(p.pad(1).unwrap().pad.hit.is_some());
}
