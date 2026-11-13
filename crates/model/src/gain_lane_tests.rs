//! The gain-lane evaluator (ADR 0058, decision 2). Every expected value is the record's formula
//! written out (a stored value, or `u + (w - u) * (elapsed / span)` from the points) or an exact
//! dyadic value, never captured from the code under test.

#![allow(clippy::unwrap_used)]

use super::*;
use crate::{AutoParam, AutomationLane, MACRO_SLOT, MIDI_EFFECT_SLOT};

fn lane(param: AutoParam, pts: &[(Samples, f32)]) -> AutomationLane {
    AutomationLane { param, points: pts.iter().map(|&(at, value)| AutomationPoint { at, value }).collect(), enabled: true }
}

/// The record's device-lane example (decision 2): an interior group at 64 holding 0 then 1.
const EXAMPLE: [(Samples, f32); 4] = [(0, 0.5), (64, 0.0), (64, 1.0), (128, 0.5)];

#[test]
fn a_group_gives_its_first_value_at_its_time_and_its_last_after() {
    let l = lane(AutoParam::Volume, &EXAMPLE);
    assert_eq!(l.gain_value_at(64, 0.0), 0.0, "the group's first value at its time");
    assert_eq!(l.gain_value_at(65, 0.0), 1.0 + (0.5 - 1.0) * (1.0 / 64.0), "the segment after starts from the last value");
    assert_eq!(l.gain_value_at(65, 0.0), 0.992_187_5);
    assert_eq!(l.gain_value_at(63, 0.0), 0.5 + (0.0 - 0.5) * (63.0 / 64.0), "the segment arriving ends on the first value");
    assert_eq!(l.gain_value_at(32, 0.0), 0.25);
    for at in [-1_000, -1, 0] {
        assert_eq!(l.gain_value_at(at, 0.0), 0.5, "before and at the first group");
    }
    for at in [128, 129, 1 << 40] {
        assert_eq!(l.gain_value_at(at, 0.0), 0.5, "at and after the last group");
    }
    // Today's evaluator, kept for device lanes, reads the group's last value at an interior time.
    assert_eq!(l.value_at(64, 0.0), 1.0);

    // Groups first, inside and last in the lane.
    let l = lane(AutoParam::Volume, &[(0, 3.0), (0, 4.0), (10, 5.0), (10, 6.0), (20, 7.0), (20, 8.0)]);
    assert_eq!(l.gain_value_at(-1, 0.0), 3.0, "before the first group: its first value");
    assert_eq!(l.gain_value_at(0, 0.0), 3.0);
    assert_eq!(l.gain_value_at(1, 0.0), 4.0 + (5.0 - 4.0) * (1.0 / 10.0));
    assert_eq!(l.gain_value_at(10, 0.0), 5.0);
    assert_eq!(l.gain_value_at(11, 0.0), 6.0 + (7.0 - 6.0) * (1.0 / 10.0));
    assert_eq!(l.gain_value_at(20, 0.0), 7.0, "the last group's first value at its time");
    assert_eq!(l.gain_value_at(21, 0.0), 8.0, "after the last group: its last value");
}

#[test]
fn step_lanes_hold_the_outgoing_value_between_groups() {
    let l = lane(AutoParam::Mute, &[(0, 0.0), (100, 1.0), (100, 0.0), (200, 1.0)]);
    assert_eq!(l.gain_value_at(50, 0.0), 0.0);
    assert_eq!(l.gain_value_at(99, 0.0), 0.0);
    assert_eq!(l.gain_value_at(100, 0.0), 1.0, "the group's first value at its time");
    assert_eq!(l.gain_value_at(101, 0.0), 0.0, "then its last value, held");
    assert_eq!(l.gain_value_at(199, 0.0), 0.0);
    assert_eq!(l.gain_value_at(200, 0.0), 1.0);
    let l = lane(AutoParam::SendMute(2), &[(0, 0.0), (10, 1.0)]);
    assert_eq!(l.gain_value_at(9, 0.0), 0.0);
    // The same points on a ramping lane interpolate.
    let l = lane(AutoParam::SendLevel(2), &[(0, 0.0), (10, 1.0)]);
    assert_eq!(l.gain_value_at(5, 0.0), 0.5);
}

#[test]
fn breakpoints_are_exact_and_an_equal_segment_is_its_value_at_every_sample() {
    let tenth = 0.1f32;
    let l = lane(AutoParam::Volume, &[(0, -144.0), (31, tenth), (1_000, -6.5), (2_000, -6.5)]);
    assert_eq!(l.gain_value_at(31, 0.0), f64::from(tenth), "the stored value itself, not 0.100006103515625");
    assert_eq!(l.gain_value_at(0, 0.0), -144.0);
    for at in 1_000..=2_000 {
        assert_eq!(l.gain_value_at(at, 0.0), -6.5, "sample {at}");
    }
}

#[test]
fn positions_past_two_to_the_24_keep_full_precision() {
    // In f32, 2^24 + 1 is 2^24, so today's division reads 0.5 there; f64 reads the exact dyadic value.
    let span = 1i64 << 25;
    let l = lane(AutoParam::Volume, &[(0, 0.0), (span, 1.0)]);
    let at = (1i64 << 24) + 1;
    assert_eq!(l.gain_value_at(at, 0.0), 0.5 + 1.0 / span as f64);
    // Hours long: the last sample before a breakpoint 2^40 samples away is not its value.
    let l = lane(AutoParam::Volume, &[(0, 0.0), (1 << 40, 1.0)]);
    assert!(l.gain_value_at((1 << 40) - 1, 0.0) < 1.0);
}

/// Lanes whose groups sit first, inside and last, with long and short groups and equal values.
fn mixed_lanes() -> Vec<Vec<(Samples, f32)>> {
    vec![
        EXAMPLE.to_vec(),
        vec![(0, 3.0), (0, 4.0), (0, -2.0), (10, 5.0), (17, 5.0), (17, 6.0), (17, 6.5), (30, -1.0), (30, 8.0)],
        vec![(5, 1.0)],
        vec![(5, 1.0), (5, 2.0)],
        vec![(-20, 0.25), (-3, -0.75), (40, 0.5), (41, 0.0), (41, 0.0), (90, 1.0)],
    ]
}

#[test]
fn the_cursor_agrees_with_random_access_at_every_sample_with_and_without_the_index() {
    for pts in mixed_lanes() {
        let index = build_group_index(&pts).unwrap();
        for step in [false, true] {
            let start = pts.first().unwrap().0 - 5;
            let end = pts.last().unwrap().0 + 5;
            let (mut with, mut without) = (GainCursor::seek(&pts, start), GainCursor::seek(&pts, start));
            for at in start..=end {
                let steps = with.advance(&pts, Some(&index), at);
                assert!(steps <= 1, "one group crossing per sample at most ({steps} at {at})");
                without.advance(&pts, None, at);
                let want = gain_value_at(&pts, at, step);
                assert_eq!(with.value(&pts, at, step), want, "{pts:?} at {at}, step {step}");
                assert_eq!(without.value(&pts, at, step), want, "{pts:?} at {at}, step {step}");
                assert_eq!(GainCursor::seek(&pts, at).value(&pts, at, step), want);
            }
        }
    }
    let empty: [(Samples, f32); 0] = [];
    assert_eq!(gain_value_at(&empty, 7, false), None);
    assert_eq!(lane(AutoParam::Volume, &[]).gain_value_at(7, -3.0), -3.0, "an empty lane gives its default");
}

#[test]
fn a_group_of_100_000_points_is_crossed_in_one_step_through_its_index() {
    let mut pts = vec![(0, 0.0f32)];
    let value = |i: i32| match i {
        0 => 0.25,
        99_999 => 0.75,
        _ => (i % 7) as f32,
    };
    pts.extend((0..100_000).map(|i| (64, value(i))));
    pts.push((128, 1.0));
    let index = build_group_index(&pts).unwrap();
    let short = [(0, 0.0f32), (64, 0.25), (64, 0.75), (128, 1.0)];
    for (ix, crossing) in [(Some(&index[..]), 1), (None, 100_000)] {
        let mut c = GainCursor::seek(&pts, 63);
        assert_eq!(c.advance(&pts, ix, 64), 0);
        assert_eq!(c.value(&pts, 64, false), Some(0.25), "the first point's value at the group's time");
        assert_eq!(c.advance(&pts, ix, 65), crossing, "steps to cross the group: one with the index");
        assert_eq!(c.value(&pts, 65, false), Some(0.75 + (1.0 - 0.75) * (1.0 / 64.0)), "from the last point's value after it");
    }
    for at in -2..=130 {
        assert_eq!(gain_value_at(&pts, at, false), gain_value_at(&short, at, false), "same as the group's first and last alone at {at}");
    }
}

#[test]
fn the_group_index_holds_each_points_last_group_member() {
    let pts: Vec<(Samples, f32)> = [0, 0, 5, 7, 7, 7, 9].iter().map(|&t| (t, 0.0)).collect();
    assert_eq!(&*build_group_index(&pts).unwrap(), &[1, 1, 2, 5, 5, 5, 6]);
    assert_eq!(&*build_group_index(&[(3i64, 1.0f32)]).unwrap(), &[0]);
    let empty: [(Samples, f32); 0] = [];
    assert!(build_group_index(&empty).unwrap().is_empty());
    // Into caller storage: the front of a longer table, nothing written into a short one.
    let mut table = [9u32; 9];
    assert!(fill_group_index(&pts, &mut table));
    assert_eq!(table, [1, 1, 2, 5, 5, 5, 6, 9, 9]);
    let mut short = [9u32; 6];
    assert!(!fill_group_index(&pts, &mut short));
    assert_eq!(short, [9; 6]);
    assert_eq!(group_index_bytes(0), Some(0));
    assert_eq!(group_index_bytes(100_000), Some(400_000), "the record's example");
    #[cfg(target_pointer_width = "64")]
    {
        let most = u32::MAX as usize + 1;
        assert_eq!(group_index_bytes(most), Some(most * 4), "every index fits a u32");
        assert_eq!(group_index_bytes(most + 1), None, "one more point cannot be indexed");
    }
}

#[test]
fn incoming_and_outgoing_values_at_a_group() {
    let pts = [(0i64, 0.5f32), (64, -1.0), (64, 0.0), (64, 1.0), (128, 1.0)];
    let index = build_group_index(&pts).unwrap();
    for ix in [Some(&index[..]), None] {
        assert_eq!(group_values_at(&pts, 64, ix), Some((-1.0, 1.0)), "first value in, last value out");
        assert_eq!(group_values_at(&pts, 0, ix), Some((0.5, 0.5)), "a single point: both equal");
        assert_eq!(group_values_at(&pts, 63, ix), None, "no breakpoint there");
        assert_eq!(group_values_at(&pts, 200, ix), None);
    }
}

#[test]
fn clip_envelope_pairs_and_lane_points_evaluate_alike() {
    for pts in mixed_lanes() {
        let points: Vec<AutomationPoint> = pts.iter().map(|&(at, value)| AutomationPoint { at, value }).collect();
        for at in -30..100 {
            assert_eq!(gain_value_at(&pts, at, false), gain_value_at(&points, at, false), "{pts:?} at {at}");
        }
    }
}

#[test]
fn the_mixer_gain_predicate_names_exactly_the_records_lanes() {
    let plugin = |slot: u8, param: &str| AutoParam::Plugin { slot, param: param.into() };
    for p in [
        AutoParam::Volume,
        AutoParam::Pan(0),
        AutoParam::Pan(1),
        AutoParam::Mute,
        AutoParam::SendLevel(0),
        AutoParam::SendLevel(7),
        AutoParam::SendPan(3),
        AutoParam::SendMute(2),
        plugin(u8::MAX, "trim"),
    ] {
        assert!(p.is_mixer_gain(), "{p:?} is a mixer gain lane");
    }
    for p in [
        plugin(0, "trim"),
        plugin(u8::MAX, "gain"),
        plugin(u8::MAX, "Trim"),
        plugin(AutoParam::INSTRUMENT_SLOT, "cutoff"),
        plugin(MACRO_SLOT, "macro1"),
        plugin(MIDI_EFFECT_SLOT, "semitones"),
        plugin(2, "threshold"),
    ] {
        assert!(!p.is_mixer_gain(), "{p:?} is a device-parameter lane");
    }
    assert!(AutoParam::Mute.steps() && AutoParam::SendMute(1).steps());
    assert!(!AutoParam::Volume.steps() && !AutoParam::Pan(0).steps() && !plugin(u8::MAX, "trim").steps());
}
