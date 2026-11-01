//! The firmware's two sample tables, joined from their generated chunks (`tools/gen_resources.py`):
//! `smp_sample_data`, the strike exciter's nine 16-bit samples back to back (bounded by
//! [`crate::resources::SMP_BOUNDARIES`]), and `smp_noise_sample`, the blow exciter's noise. 128013
//! and 40963 entries (256 026 and 81 926 bytes); compiled in as the firmware keeps them in flash.

#![forbid(unsafe_code)]

use crate::noise_1::NOISE_1;
use crate::noise_2::NOISE_2;
use crate::samples_1::SAMPLES_1;
use crate::samples_2::SAMPLES_2;
use crate::samples_3::SAMPLES_3;
use crate::samples_4::SAMPLES_4;
use crate::samples_5::SAMPLES_5;

/// `SMP_SAMPLE_DATA_SIZE`.
pub(crate) const SAMPLE_DATA_LEN: usize = 128_013;

/// `SMP_NOISE_SAMPLE_SIZE`.
pub(crate) const NOISE_LEN: usize = 40_963;

/// Copies `parts` back to back into one array of `N` entries (`N` is their total length).
const fn join<const N: usize>(parts: &[&[i16]]) -> [i16; N] {
    let mut out = [0_i16; N];
    let mut at = 0;
    let mut p = 0;
    while p < parts.len() {
        let part = parts[p];
        let mut i = 0;
        while i < part.len() {
            out[at] = part[i];
            at += 1;
            i += 1;
        }
        p += 1;
    }
    assert!(at == N, "the chunks must fill the table exactly");
    out
}

/// `smp_sample_data`.
pub(crate) static SMP_SAMPLE_DATA: [i16; SAMPLE_DATA_LEN] = join(&[&SAMPLES_1, &SAMPLES_2, &SAMPLES_3, &SAMPLES_4, &SAMPLES_5]);

/// `smp_noise_sample`.
pub(crate) static SMP_NOISE_SAMPLE: [i16; NOISE_LEN] = join(&[&NOISE_1, &NOISE_2]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tables_have_the_firmware_lengths_and_ends() {
        assert_eq!(SMP_SAMPLE_DATA[..4], [-964, 18802, 14172, 4105]);
        assert_eq!(*crate::resources::SMP_BOUNDARIES.last().unwrap(), SAMPLE_DATA_LEN);
        assert_eq!(SMP_NOISE_SAMPLE.len(), NOISE_LEN);
    }
}
