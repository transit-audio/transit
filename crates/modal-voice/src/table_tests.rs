//! The firmware's two sample tables (`samples.rs`, `noise.rs`) have the firmware's lengths and ends.

use crate::noise::{NOISE_LEN, SMP_NOISE_SAMPLE};
use crate::samples::{SAMPLE_DATA_LEN, SMP_SAMPLE_DATA};

#[test]
fn the_tables_have_the_firmware_lengths_and_ends() {
    assert_eq!(SMP_SAMPLE_DATA[..4], [-964, 18802, 14172, 4105]);
    assert_eq!(*crate::resources::SMP_BOUNDARIES.last().unwrap(), SAMPLE_DATA_LEN);
    assert_eq!(SMP_NOISE_SAMPLE.len(), NOISE_LEN);
}
