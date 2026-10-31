//! The synth's wavetables (`wav_integrated_waves` in `beads/resources.cc`): 12 banks of 16 waves of
//! 256 samples plus 4 of padding for the Hermite reads, each wave pre-integrated (the voices
//! differentiate what they read), then one wave of zeros.

#![forbid(unsafe_code)]

use crate::wavetables_a::WAVES_A;
use crate::wavetables_b::WAVES_B;

/// Samples per wave, `kTableSize`.
pub(crate) const TABLE_SIZE: usize = 256;

/// Waves per bank, `kNumWavesPerTable`.
pub(crate) const WAVES_PER_BANK: usize = 16;

/// Banks, `kNumBanks`.
pub(crate) const BANKS: usize = 12;

/// The stride between waves: a wave and its 4 samples of padding.
pub(crate) const WAVE_STRIDE: usize = TABLE_SIZE + 4;

/// The upstream table's length, `WAV_INTEGRATED_WAVES_SIZE`.
pub(crate) const UPSTREAM_LEN: usize = BANKS * WAVES_PER_BANK * WAVE_STRIDE;

const LEN: usize = UPSTREAM_LEN + WAVE_STRIDE;

const fn join(a: &[i16; UPSTREAM_LEN / 2], b: &[i16; UPSTREAM_LEN / 2 + WAVE_STRIDE]) -> [i16; LEN] {
    let mut out = [0_i16; LEN];
    let mut i = 0;
    while i < a.len() {
        out[i] = a[i];
        i += 1;
    }
    let mut j = 0;
    while j < b.len() {
        out[a.len() + j] = b[j];
        j += 1;
    }
    out
}

/// The table and one more wave of zeros. A wave position of exactly 1.0 (the last wave of a
/// bank) interpolates toward the wave after it with a zero fraction; for the last bank that wave
/// is past the upstream table, where the firmware reads whatever follows it. The oracle's build
/// appends the same zeros (`patches/all/0006`), so both read the same values.
pub(crate) static WAVES: [i16; LEN] = join(&WAVES_A, &WAVES_B);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_the_upstream_length_and_a_wave_of_zeros() {
        assert_eq!(UPSTREAM_LEN, 49_920);
        assert_eq!(WAVES[0], -20_784);
        assert!(WAVES[UPSTREAM_LEN..].iter().all(|&s| s == 0));
    }
}
