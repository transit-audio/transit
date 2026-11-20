//! The synth's wavetable (`wavetables.rs`) has the upstream length and ends in a wave of zeros.

use crate::wavetables::{UPSTREAM_LEN, WAVES};

#[test]
fn the_table_has_the_upstream_length_and_a_wave_of_zeros() {
    assert_eq!(UPSTREAM_LEN, 49_920);
    assert_eq!(WAVES[0], -20_784);
    assert!(WAVES[UPSTREAM_LEN..].iter().all(|&s| s == 0));
}
