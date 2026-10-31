//! `beads/dsp/mu_law.h`: the 8-bit buffer's mu-law codec.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::LUT_ULAW;

/// `MuLaw2Lin(u)`: the code's linear value, from `lut_ulaw_f`.
#[inline(always)]
#[must_use]
pub fn mulaw_to_lin(code: u8) -> F {
    F(LUT_ULAW[usize::from(code)])
}

/// `Lin2MuLaw(pcm)`: a 16-bit sample to its mu-law code (the G.711 encoder on 14 bits).
#[inline(always)]
#[must_use]
pub fn lin_to_mulaw(pcm: i16) -> u8 {
    let mut pcm = pcm >> 2;
    let mask: u8 = if pcm < 0 {
        pcm = -pcm;
        0x7f
    } else {
        0xff
    };
    if pcm > 8159 {
        pcm = 8159;
    }
    pcm += 0x84 >> 2;
    let seg: i16 = [0x3f, 0x7f, 0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff, 0x1fff].iter().take_while(|&&top| pcm > top).count().try_into().unwrap_or(8);
    if seg >= 8 {
        0x7f ^ mask
    } else {
        let uval = u8::try_from((seg << 4) | ((pcm >> (seg + 1)) & 0x0f)).unwrap_or(0);
        uval ^ mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_codec_round_trips_within_its_steps() {
        assert_eq!(lin_to_mulaw(0), 0xff);
        assert_eq!(mulaw_to_lin(0xff), F(0.0));
        for pcm in [-32768_i16, -12345, -100, 100, 4000, 32767] {
            let back = mulaw_to_lin(lin_to_mulaw(pcm)).0;
            let x = f32::from(pcm) / 32768.0;
            assert!((back - x).abs() <= x.abs() / 16.0 + 0.002, "{pcm}: {back}");
        }
    }
}
