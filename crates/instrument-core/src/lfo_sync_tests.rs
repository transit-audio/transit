//! Pure bar-start rule, including the unchanged absolute-quarter arithmetic.
use super::{DIVISIONS, SyncDivision, division_quarters};

#[test]
fn bar_cycles_keep_the_bar_count_through_meter_changes() {
    // 4/4 -> 3/4 at bar 3; then 7/8 at bar 6. The four-bar cycle is mid-cycle
    // at both changes and must not restart. Positions are independent of a sample clock.
    for (bar, start, length) in [(1, 4.0, 4.0), (2, 8.0, 3.0), (4, 14.0, 3.0), (5, 17.0, 3.5)] {
        for (division, bars) in [(0, 4.0), (1, 2.0), (2, 1.0)] {
            for within in [0.0, 0.25, 1.0, length - 0.125] {
                let expected = (bar as f64 + within / length) / bars;
                let actual = SyncDivision::new(division, length, (bar, start)).phase(start + within);
                assert!((actual - (expected - expected.floor())).abs() < 1e-12);
            }
        }
    }
}

#[test]
fn a_single_meter_keeps_the_old_phase_bits_for_every_division() {
    for length in [0.0625, 4.0, 3.0, 3.5, 256.0] {
        for division in 0..DIVISIONS.len() {
            for quarters in [-0.25_f64, 0.0, 7.123, 17.999, 234.567] {
                let bar = (quarters / length).floor() as i64;
                let old = quarters / division_quarters(division, length);
                let actual = SyncDivision::new(division, length, (bar, bar as f64 * length)).phase(quarters);
                assert_eq!(actual.to_bits(), (old - old.floor()).to_bits());
            }
        }
    }
}

#[test]
fn beat_divisions_ignore_the_meter_origin_bit_for_bit() {
    for division in 3..DIVISIONS.len() {
        for quarters in [8.0, 8.125, 14.0, 18.0, 23.456] {
            let before = SyncDivision::new(division, 4.0, (1, 4.0));
            let after = SyncDivision::new(division, 3.0, (4, 14.0));
            assert_eq!(before.phase(quarters).to_bits(), after.phase(quarters).to_bits());
            assert_eq!(before.quarters.to_bits(), after.quarters.to_bits());
        }
    }
}
