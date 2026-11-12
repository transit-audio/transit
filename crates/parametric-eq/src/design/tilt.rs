//! Flat tilt (decision 2.4): fifteen first-order shelving pairs at `f_k = 10·2^k` Hz, k = −1 … 13,
//! each `(1 + s/z_k)/(1 + s/p_k)` for a slope of `G/9.966` dB per octave, the whole scaled to 0 dB at
//! the band frequency. Each pair is a first-order section (pole mapped, `b0` and `b1` fitted at DC and
//! Nyquist, both roots positive, so minimum phase); the fitted pairs are multiplied two by two into
//! second-order sections whose σ coefficients are the products of the pairs' (no second fit): seven
//! second-order sections, then the last pair first-order. The prototype's scale (its value at the band
//! frequency) multiplies the first section's numerator.

use super::{Design, fit2, map_real};
use crate::math::TAU;
use crate::prototype::{TILT_PAIRS, tilt_pair, tilt_pairs};
use crate::section::{Fos, Sos};

pub(super) fn flat(f0: f64, gain_db: f64, fs: f64) -> Design {
    let (pairs, scale) = tilt_pairs(gain_db, f0);
    let nyquist = 0.5 * fs;
    let mut fos = [Fos::default(); TILT_PAIRS];
    for (dst, &(z, p)) in fos.iter_mut().zip(pairs.iter()) {
        let d = map_real(TAU * p / fs);
        *dst = Fos { n: fit2(d, 1.0, tilt_pair(nyquist, z, p)), d };
    }
    let mut d = Design::default();
    for pair in fos[..TILT_PAIRS - 1].as_chunks::<2>().0 {
        d.push(Sos { n: product(pair[0].n, pair[1].n), c: product(pair[0].d, pair[1].d) });
    }
    d.fos = Some(fos[TILT_PAIRS - 1]);
    d.scale_first(scale);
    d
}

/// `(a0 + a1σ)(b0 + b1σ)`.
fn product(a: [f64; 2], b: [f64; 2]) -> [f64; 3] {
    [a[0] * b[0], a[0] * b[1] + a[1] * b[0], a[1] * b[1]]
}
