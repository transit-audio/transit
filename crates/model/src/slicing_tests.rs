use super::*;
use serde_json::json;

/// `round(k * num / den)` with ties upward, in integers: the hand rule for a rational step.
fn rational(k: u64, num: u64, den: u64) -> u64 {
    (2 * k * num + den) / (2 * den)
}

fn grid(start: u64, end: u64, rate: u32, bpm: f64, division: Division, meter: (u32, u32), bars: Option<u32>) -> GridResult {
    grid_boundaries(start, end, rate, bpm, division, meter, bars).unwrap()
}

#[test]
fn grid_boundaries_are_rational_not_accumulated() {
    // 117 BPM, 1/16: the step is 44100 * 240 / (117 * 16) = 73500/13 = 5653.846... frames at
    // 44.1 kHz and 48000 * 240 / 1872 = 80000/13 = 6153.846... at 48 kHz. Adding the rounded
    // step (5654, 6154) drifts about 0.154 frames a step: ten frames by step 63.
    for (rate, num, rounded_step, hand_63) in [(44_100u32, 73_500u64, 5654u64, 356_192u64), (48_000, 80_000, 6154, 387_692)] {
        let start = 1000;
        let len = rational(64, num, 13); // 64 whole steps: k = 64 lands on the end.
        let g = grid(start, start + len, rate, 117.0, Division::Sixteenth, (4, 4), None);
        assert_eq!(g.bpm, 117.0);
        assert_eq!(g.boundaries.len(), 65, "{rate}: 64 starts and the end");
        for (k, b) in g.boundaries.iter().take(64).enumerate() {
            assert_eq!(*b, start + rational(k as u64, num, 13), "{rate}: boundary {k}");
        }
        assert_eq!(g.boundaries[63], start + hand_63, "{rate}: hand-computed boundary 63");
        assert_eq!(g.boundaries[64], start + len);
        // The fixture separates the two rules: the accumulated boundary 63 is off by frames.
        let accumulated = start + 63 * rounded_step;
        assert!(accumulated.abs_diff(start + hand_63) >= 1, "{rate}: the fixture must discriminate");
    }
}

#[test]
fn grid_bars_override_in_4_4_and_3_4() {
    // 4/4, two bars over 96,000 frames at 48 kHz: B = 8 quarters, BPM = 60 * 8 * 48000 / 96000
    // = 240; 1/16 is 3000 frames, 32 slices.
    let g = grid(0, 96_000, 48_000, 999.0, Division::Sixteenth, (4, 4), Some(2));
    assert_eq!(g.bpm, 240.0);
    let want: Vec<u64> = (0..=32).map(|k| k * 3000).collect();
    assert_eq!(g.boundaries, want);
    // 3/4, one bar over 90,000 frames: B = 3, BPM = 60 * 3 * 48000 / 90000 = 96; 1/8 is 15000.
    let g = grid(0, 90_000, 48_000, 999.0, Division::Eighth, (3, 4), Some(1));
    assert_eq!(g.bpm, 96.0);
    assert_eq!(g.boundaries, vec![0, 15_000, 30_000, 45_000, 60_000, 75_000, 90_000]);
    // 3/4, one bar over 100,000 frames at 1/16: the step is 100000 * 4 / 48 = 8333.33...,
    // BPM = 60 * 3 * 48000 / 100000 = 86.4.
    let g = grid(500, 100_500, 48_000, 999.0, Division::Sixteenth, (3, 4), Some(1));
    assert!((g.bpm - 86.4).abs() < 1e-9, "{}", g.bpm);
    let hand = [0u64, 8333, 16_667, 25_000, 33_333, 41_667, 50_000, 58_333, 66_667, 75_000, 83_333, 91_667, 100_000];
    assert_eq!(g.boundaries, hand.iter().map(|o| 500 + o).collect::<Vec<_>>());
}

#[test]
fn grid_from_tempo_restores_the_bpm_rule() {
    // From tempo at 100 BPM, 1/4: 48000 * 240 / 400 = 28800 frames, a partial last quarter. No
    // bar count gives 100 BPM over this loop (one bar of 4/4 is 120).
    let from_tempo = grid(0, 96_000, 48_000, 100.0, Division::Quarter, (4, 4), None);
    assert_eq!(from_tempo, GridResult { boundaries: vec![0, 28_800, 57_600, 86_400, 96_000], bpm: 100.0 });
    // Two bars override it (240 BPM, 12000-frame quarters) ...
    let bars = grid(0, 96_000, 48_000, 100.0, Division::Quarter, (4, 4), Some(2));
    assert_eq!(bars.bpm, 240.0);
    assert_eq!(bars.boundaries, (0..=8).map(|k| k * 12_000).collect::<Vec<_>>());
    // ... and returning to From tempo gives the tempo rule's grid again.
    assert_eq!(grid(0, 96_000, 48_000, 100.0, Division::Quarter, (4, 4), None), from_tempo);
}

#[test]
fn grid_partial_last_division_kept() {
    let g = grid(0, 60_000, 48_000, 120.0, Division::Quarter, (4, 4), None);
    assert_eq!(g.boundaries, vec![0, 24_000, 48_000, 60_000]);
    // A 7/8 bar at 1/4 is three and a half quarters.
    let g = grid(0, 70_000, 48_000, 0.0, Division::Quarter, (7, 8), Some(1));
    assert_eq!(g.boundaries, vec![0, 20_000, 40_000, 60_000, 70_000]);
}

#[test]
fn grid_over_capacity_refused_not_truncated() {
    // 1/32 at 120 BPM is 3000 frames: 64 starts fit in 192,000 frames, one more frame makes 65.
    let g = grid(0, 192_000, 48_000, 120.0, Division::ThirtySecond, (4, 4), None);
    assert_eq!(g.boundaries.len(), 65);
    assert_eq!(grid_boundaries(0, 192_001, 48_000, 120.0, Division::ThirtySecond, (4, 4), None), Err(GridError::OverCapacity));
    assert_eq!(grid_boundaries(0, 1_000_000, 48_000, 120.0, Division::ThirtySecond, (4, 4), None), Err(GridError::OverCapacity));
    // A step under one frame makes every frame a start: 64 frames fit, 65 do not.
    let g = grid(10, 74, 48_000, 1e9, Division::ThirtySecond, (4, 4), None);
    assert_eq!(g.boundaries, (10..=74).collect::<Vec<_>>());
    assert_eq!(grid_boundaries(10, 75, 48_000, 1e9, Division::ThirtySecond, (4, 4), None), Err(GridError::OverCapacity));
    assert_eq!(GridError::OverCapacity.to_string(), "the grid makes more than 64 slices: choose a coarser division");
}

#[test]
fn grid_rejects_nonfinite_bpm() {
    for bpm in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -0.0, -120.0] {
        assert_eq!(grid_boundaries(0, 96_000, 48_000, bpm, Division::Sixteenth, (4, 4), None), Err(GridError::BadTempo), "{bpm}");
    }
    assert_eq!(grid_boundaries(0, 96_000, 48_000, 120.0, Division::Sixteenth, (4, 4), Some(0)), Err(GridError::BadBars));
    assert_eq!(grid_boundaries(0, 96_000, 48_000, 120.0, Division::Sixteenth, (4, 4), Some(17)), Err(GridError::BadBars));
    assert_eq!(grid_boundaries(0, 96_000, 48_000, 120.0, Division::Sixteenth, (0, 4), Some(1)), Err(GridError::BadMeter));
    assert_eq!(grid_boundaries(0, 96_000, 48_000, 120.0, Division::Sixteenth, (4, 0), Some(1)), Err(GridError::BadMeter));
    assert_eq!(grid_boundaries(5, 5, 48_000, 120.0, Division::Sixteenth, (4, 4), None), Err(GridError::EmptyRegion));
    assert_eq!(grid_boundaries(0, 96_000, 0, 120.0, Division::Sixteenth, (4, 4), None), Err(GridError::ZeroRate));
}

const S: u64 = 1000;
const E: u64 = 2000;

fn ids(m: &Markers) -> Vec<u32> {
    m.markers.iter().map(|m| m.id).collect()
}

fn frames(m: &Markers) -> Vec<u64> {
    m.markers.iter().map(|m| m.frame).collect()
}

/// Runs a refused edit and checks the set is bit-identical afterward.
#[track_caller]
fn refused<T: std::fmt::Debug>(m: &mut Markers, want: MarkerError, edit: impl FnOnce(&mut Markers) -> Result<T, MarkerError>) {
    let before = m.clone();
    assert_eq!(edit(m).unwrap_err(), want);
    assert_eq!(*m, before, "a refused edit left the set changed");
}

#[test]
fn marker_ops_refuse_each_bound() {
    let mut m = Markers::start_only(S);
    assert_eq!((ids(&m), frames(&m)), (vec![1], vec![S]));
    // S is fixed: a move or remove of it changes nothing.
    refused(&mut m, MarkerError::Fixed, |m| m.move_to(S, E, 1, 1500));
    refused(&mut m, MarkerError::Fixed, |m| m.remove(S, E, 1));
    assert_eq!(MarkerError::Fixed.to_string(), "The loop start cannot move.");
    // Adds at S, at E and outside (S, E) are refused, never moved inside.
    refused(&mut m, MarkerError::OnMarker, |m| m.add(S, E, S));
    refused(&mut m, MarkerError::OutsideLoop, |m| m.add(S, E, E));
    refused(&mut m, MarkerError::OutsideLoop, |m| m.add(S, E, E + 500));
    refused(&mut m, MarkerError::OutsideLoop, |m| m.add(S, E, S - 1));
    // An add on a marker is refused, never merged: a zero gap.
    assert_eq!(m.add(S, E, 1500), Ok(2));
    refused(&mut m, MarkerError::OnMarker, |m| m.add(S, E, 1500));
    // A one-frame gap is legal next to S, next to a marker and next to E.
    assert_eq!(m.add(S, E, S + 1), Ok(3));
    assert_eq!(m.add(S, E, 1499), Ok(4));
    assert_eq!(m.add(S, E, E - 1), Ok(5));
    assert_eq!(frames(&m), vec![S, S + 1, 1499, 1500, E - 1]);
    assert_eq!(m.boundaries(S, E), vec![S, S + 1, 1499, 1500, E - 1, E]);
    // A move past a neighbor clamps one frame inside it, and says which neighbor.
    assert_eq!(m.legal_move(S, E, 2), Ok((1500, 1998)));
    assert_eq!(m.move_to(S, E, 2, 5000), Ok(Moved { frame: E - 2, clamped: Some(ClampedAt::Marker { index: 5, id: 5 }) }));
    assert_eq!(m.move_to(S, E, 2, 0), Ok(Moved { frame: 1500, clamped: Some(ClampedAt::Marker { index: 3, id: 4 }) }));
    assert_eq!(m.move_to(S, E, 2, 1700), Ok(Moved { frame: 1700, clamped: None }));
    assert_eq!(m.move_to(S, E, 5, E), Ok(Moved { frame: E - 1, clamped: Some(ClampedAt::End) }));
    assert_eq!(ClampedAt::Marker { index: 4, id: 9 }.to_string(), "clamped at marker 4");
    assert_eq!(ids(&m), vec![1, 3, 4, 2, 5], "moves keep ids and order");
    // Ids are not reused: removing the highest and adding again takes a new one.
    assert_eq!(m.remove(S, E, 5), Ok(Removed { id: 5, merged_into: 2 }));
    assert_eq!(m.add(S, E, 1900), Ok(6));
    // The 65th slice is refused with the exact status text.
    let mut full = Markers::start_only(0);
    for k in 1..64 {
        full.add(0, 10_000, k * 10).unwrap();
    }
    assert_eq!(full.markers.len(), 64);
    refused(&mut full, MarkerError::Full, |m| m.add(0, 10_000, 5000));
    assert_eq!(MarkerError::Full.to_string(), "Cannot add slice: 64 slices per loop");
    // An empty set is S only: an add at S or before it is refused there too.
    let mut empty = Markers::default();
    refused(&mut empty, MarkerError::OnMarker, |m| m.add(S, E, S));
    refused(&mut empty, MarkerError::OutsideLoop, |m| m.add(S, E, S - 1));
    refused(&mut empty, MarkerError::OutsideLoop, |m| m.add(S, E, E));
    // Unknown ids, long labels and a set that does not start at S are refused too.
    refused(&mut m, MarkerError::NoMarker(77), |m| m.move_to(S, E, 77, 1200));
    refused(&mut m, MarkerError::LabelTooLong, |m| m.relabel(2, Some("x".repeat(MAX_LABEL_BYTES + 1))));
    let mut off = Markers { markers: vec![Marker { id: 1, frame: S + 1, ..Marker::default() }], ..Markers::default() };
    refused(&mut off, MarkerError::Malformed, |m| m.add(S, E, 1500));
}

#[test]
fn an_empty_set_is_s_only() {
    let mut m = Markers::default();
    assert_eq!(m.boundaries(S, E), vec![S, E]);
    assert_eq!(m.add(S, E, 1500), Ok(2));
    assert_eq!((ids(&m), frames(&m)), (vec![1, 2], vec![S, 1500]));
    let mut m = Markers::default();
    assert_eq!(m.clear_to_start(S, E), Ok(vec![]));
    assert_eq!(frames(&m), vec![S]);
}

#[test]
fn remove_merges_into_preceding_and_reports_the_id() {
    let mut m = Markers::start_only(0);
    for f in [250, 500, 750] {
        m.add(0, 1000, f).unwrap();
    }
    assert_eq!(ids(&m), vec![1, 2, 3, 4]);
    // Removing the marker at 500 (id 3) makes slice 2 run from 250 to 750.
    assert_eq!(m.remove(0, 1000, 3), Ok(Removed { id: 3, merged_into: 2 }));
    assert_eq!(m.boundaries(0, 1000), vec![0, 250, 750, 1000]);
    assert_eq!(ids(&m), vec![1, 2, 4]);
    // The last marker merges into its predecessor too; Clear keeps S and reports the rest.
    m.relabel(1, Some("start".into())).unwrap();
    assert_eq!(m.clear_to_start(0, 1000), Ok(vec![2, 4]));
    assert_eq!(ids(&m), vec![1]);
    assert_eq!(m.markers[0].label.as_deref(), Some("start"));
}

#[test]
fn copy_from_creates_new_ids_keeps_labels() {
    let label = |s: &str| Some(s.to_string());
    let auto = Markers {
        markers: vec![
            Marker { id: 1, frame: 0, label: label("kick"), ..Marker::default() },
            Marker { id: 2, frame: 400, ..Marker::default() },
            Marker { id: 3, frame: 800, label: label("snare"), ..Marker::default() },
        ],
        ..Markers::default()
    };
    let mut manual = Markers::start_only(0);
    manual.add(0, 1000, 300).unwrap();
    manual.add(0, 1000, 600).unwrap();
    manual.remove(0, 1000, 3).unwrap();
    let before_auto = auto.clone();
    // Every previous Manual id is reported removed; the copies get ids never used in Manual.
    assert_eq!(manual.copy_from(0, 1000, &auto), Ok(vec![1, 2]));
    assert_eq!(ids(&manual), vec![4, 5, 6]);
    assert_eq!(frames(&manual), vec![0, 400, 800]);
    let labels: Vec<Option<&str>> = manual.markers.iter().map(|m| m.label.as_deref()).collect();
    assert_eq!(labels, vec![Some("kick"), None, Some("snare")]);
    assert_eq!(auto, before_auto, "the source set is untouched");
    // A source over 64 starts is refused and the target stays as it was.
    let big =
        Markers { markers: (0..65).map(|k| Marker { id: k + 1, frame: u64::from(k) * 10, ..Marker::default() }).collect(), ..Markers::default() };
    refused(&mut manual, MarkerError::Full, |m| m.copy_from(0, 1000, &big));
}

/// A set as an older file or a migrated single set loads it: ids 1, 2 and 3, no `next_id`.
fn loaded() -> Markers {
    let m = |id, frame| Marker { id, frame, ..Marker::default() };
    Markers { markers: vec![m(1, 0), m(2, 300), m(3, 600)], next_id: 0, ..Markers::default() }
}

#[test]
fn removed_ids_never_return_on_a_loaded_set() {
    let old = [1u32, 2, 3];
    // Remove the highest, then add: a new id, and again after removing that one.
    let mut m = loaded();
    assert_eq!(m.remove(0, 1000, 3), Ok(Removed { id: 3, merged_into: 2 }));
    let first = m.add(0, 1000, 800).unwrap();
    assert!(!old.contains(&first), "remove then add reused {first}");
    assert_eq!(first, 4);
    m.remove(0, 1000, first).unwrap();
    assert_eq!(m.add(0, 1000, 900), Ok(5), "a removed new id does not return either");
    // Clear, then add.
    let mut m = loaded();
    assert_eq!(m.clear_to_start(0, 1000), Ok(vec![2, 3]));
    let id = m.add(0, 1000, 500).unwrap();
    assert!(!old.contains(&id), "Clear then add reused {id}");
    assert_eq!(ids(&m), vec![1, 4]);
    // Start from Auto: every copy is a new Manual identity, none equal to a removed id.
    let auto = Markers {
        markers: vec![Marker { id: 1, frame: 0, ..Marker::default() }, Marker { id: 2, frame: 500, ..Marker::default() }],
        ..Markers::default()
    };
    let mut m = loaded();
    assert_eq!(m.copy_from(0, 1000, &auto), Ok(vec![1, 2, 3]));
    assert!(ids(&m).iter().all(|id| !old.contains(id)), "the copy reused an old id: {:?}", ids(&m));
    assert_eq!(ids(&m), vec![4, 5]);
}

#[test]
fn markers_next_id_written_only_when_set() {
    let plain = Markers { markers: vec![Marker { id: 1, frame: 0, ..Marker::default() }], ..Markers::default() };
    assert_eq!(serde_json::to_value(&plain).unwrap(), json!({"markers": [{"id": 1, "frame": 0}]}));
    let mut m = plain.clone();
    m.add(0, 100, 50).unwrap();
    assert_eq!(serde_json::to_value(&m).unwrap(), json!({"markers": [{"id": 1, "frame": 0}, {"id": 2, "frame": 50}], "next_id": 3}));
    let back: Markers = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
    assert_eq!(back, m);
}

/// One channel: `-1` before each listed frame flips the sign, starting at `first`.
fn channel(len: usize, first: f32, flips: &[usize]) -> Vec<f32> {
    let mut v = first;
    (0..len)
        .map(|i| {
            if flips.contains(&i) {
                v = -v;
            }
            v
        })
        .collect()
}

#[test]
fn snap_prefers_nearest_earlier_tie() {
    // Crossings at 490 and 510.
    let c = channel(1000, 1.0, &[490, 510]);
    let ch: [&[f32]; 1] = [&c];
    assert_eq!(snap_to_zero_crossing(&ch, 500, 48_000, 1, 999), Ok(490), "a tie goes to the earlier frame");
    assert_eq!(snap_to_zero_crossing(&ch, 501, 48_000, 1, 999), Ok(510));
    assert_eq!(snap_to_zero_crossing(&ch, 499, 48_000, 1, 999), Ok(490));
    assert_eq!(snap_to_zero_crossing(&ch, 510, 48_000, 1, 999), Ok(510));
}

#[test]
fn snap_checks_each_channel_not_the_sum() {
    // Opposite phase: the sum is zero everywhere and never changes sign; each channel does at 300.
    let l = channel(1000, -0.5, &[300]);
    let r: Vec<f32> = l.iter().map(|x| -x).collect();
    assert!(l.iter().zip(&r).all(|(a, b)| a + b == 0.0));
    let ch: [&[f32]; 2] = [&l, &r];
    assert_eq!(snap_to_zero_crossing(&ch, 350, 48_000, 1, 999), Ok(300));
    // Either channel alone is enough.
    let flat = vec![0.25f32; 1000];
    assert_eq!(snap_to_zero_crossing(&[&flat, &l], 350, 48_000, 1, 999), Ok(300));
    assert_eq!(snap_to_zero_crossing(&[&r, &flat], 350, 48_000, 1, 999), Ok(300));
}

#[test]
fn snap_window_edge_inclusive_and_one_past_excluded() {
    // 48 kHz: floor(48000 * 0.005) = 240 frames either side of 1000.
    let at = |flip: usize, lo: u64, hi: u64| {
        let c = channel(2000, 1.0, &[flip]);
        snap_to_zero_crossing(&[&c], 1000, 48_000, lo, hi)
    };
    assert_eq!(at(760, 1, 1999), Ok(760));
    assert_eq!(at(759, 1, 1999), Ok(1000));
    assert_eq!(at(1240, 1, 1999), Ok(1240));
    assert_eq!(at(1241, 1, 1999), Ok(1000));
    // The legal interval clips the window, both ends inclusive.
    assert_eq!(at(900, 900, 1999), Ok(900));
    assert_eq!(at(900, 901, 1999), Ok(1000));
    assert_eq!(at(1100, 1, 1100), Ok(1100));
    assert_eq!(at(1100, 1, 1099), Ok(1000));
    // 44.1 kHz: floor(220.5) = 220.
    let c = channel(2000, 1.0, &[780]);
    assert_eq!(snap_to_zero_crossing(&[&c], 1000, 44_100, 1, 1999), Ok(780));
    let c = channel(2000, 1.0, &[779]);
    assert_eq!(snap_to_zero_crossing(&[&c], 1000, 44_100, 1, 1999), Ok(1000));
}

#[test]
fn snap_zero_samples_count_nonnegative() {
    // -1 for 0..100, 0 for 100..200, +1 after: only -1 to 0 at frame 100 is a crossing.
    let c: Vec<f32> = (0..1000)
        .map(|i| {
            if i < 100 {
                -1.0
            } else if i < 200 {
                0.0
            } else {
                1.0
            }
        })
        .collect();
    assert_eq!(snap_to_zero_crossing(&[&c], 190, 48_000, 1, 999), Ok(100));
    // +1 to 0 and 0 to -0.0 are not crossings; -0.0 to -1 is.
    let c: Vec<f32> = (0..1000)
        .map(|i| {
            if i < 100 {
                1.0
            } else if i < 150 {
                0.0
            } else if i < 300 {
                -0.0
            } else {
                -1.0
            }
        })
        .collect();
    assert_eq!(snap_to_zero_crossing(&[&c], 140, 48_000, 1, 999), Ok(300));
}

#[test]
fn snap_none_in_range_returns_request() {
    let c = vec![1.0f32; 1000];
    assert_eq!(snap_to_zero_crossing(&[&c], 500, 48_000, 1, 999), Ok(500));
    // Missing source data is its own refusal, never a pretend snap.
    assert_eq!(snap_to_zero_crossing(&[], 500, 48_000, 1, 999), Err(SnapError::NoSource));
    let short = vec![1.0f32; 600];
    assert_eq!(snap_to_zero_crossing(&[&c, &short], 500, 48_000, 1, 999), Err(SnapError::NoSource));
    assert_eq!(snap_to_zero_crossing(&[&c], 500, 48_000, 501, 999), Err(SnapError::OutsideInterval));
}

#[test]
fn snap_finds_the_known_loop_crossing() {
    // ADR 0041: 48,000 frames at 48 kHz, -1 for 0..11,999 and +1 for 12,000..47,999.
    let c: Vec<f32> = (0..48_000).map(|i| if i < 12_000 { -1.0 } else { 1.0 }).collect();
    assert_eq!(snap_to_zero_crossing(&[&c], 12_100, 48_000, 1, 47_999), Ok(12_000));
    assert_eq!(snap_to_zero_crossing(&[&c], 12_241, 48_000, 1, 47_999), Ok(12_241), "outside the 240-frame radius");
    assert_eq!(snap_to_zero_crossing(&[&c], 12_240, 48_000, 1, 47_999), Ok(12_000));
}

#[test]
fn round_request_ties_upward() {
    assert_eq!(round_request(2.5), 3);
    assert_eq!(round_request(3.5), 4);
    assert_eq!(round_request(2.4999), 2);
    assert_eq!(round_request(0.499_999_999_999_999_94), 0);
    assert_eq!(round_request(7.0), 7);
    assert_eq!(round_request(12_100.5), 12_101);
    assert_eq!(round_request(-0.7), 0);
    assert_eq!(round_request(f64::NAN), 0);
    assert_eq!(round_request(f64::INFINITY), 0);
}
