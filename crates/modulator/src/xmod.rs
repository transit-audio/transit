//! The six cross-modulation algorithms (`Modulator::Xmod<...>` and `ProcessXmod` in
//! `warps/dsp/modulator.{h,cc}`), run at 6x the codec rate between two neighbors with a
//! per-sample balance.

#![forbid(unsafe_code)]

use crate::float::F;
use crate::resources::{LUT_BIPOLAR_FOLD, LUT_XFADE_IN, LUT_XFADE_OUT};
use crate::stmlib::{clip16, integral_fractional, interpolate_clamped, soft_limit};

/// `XmodAlgorithm`, in the firmware's order (`NOP` passes the modulator through).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Algorithm {
    Crossfade,
    Fold,
    DiodeRing,
    DigitalRing,
    Xor,
    Comparator,
    Nop,
}

/// `xmod_table_`: the pair of neighbors each integral position morphs between.
pub(crate) const PAIRS: [(Algorithm, Algorithm); 6] = [
    (Algorithm::Crossfade, Algorithm::Fold),
    (Algorithm::Fold, Algorithm::DiodeRing),
    (Algorithm::DiodeRing, Algorithm::DigitalRing),
    (Algorithm::DigitalRing, Algorithm::Xor),
    (Algorithm::Xor, Algorithm::Comparator),
    (Algorithm::Comparator, Algorithm::Nop),
];

/// `Diode(x)`: Julian Parker's diode approximation ("A simple digital model of the diode-based
/// ring-modulator", DAFx-11), as the firmware writes it.
#[inline(always)]
fn diode(x: F) -> F {
    let sign = if x > 0.0 { F::ONE } else { F(-1.0) };
    let mut dead_zone = x.abs() - 0.667;
    dead_zone += dead_zone.abs();
    dead_zone *= dead_zone;
    #[allow(clippy::excessive_precision)] // The firmware's literal, verbatim.
    let k = 0.043_247_658_227_260_63_f32;
    k * dead_zone * sign
}

/// `x * 32768` as the firmware converts it to a short: truncated, then clipped to 16 bits.
#[inline(always)]
fn to_short(x: F) -> i32 {
    clip16((x * 32768.0).trunc_i32())
}

/// `Xmod<algorithm>(x_1, x_2, parameter)`: `x_1` is the modulator, `x_2` the carrier.
#[inline(always)]
pub(crate) fn xmod(algorithm: Algorithm, x_1: F, x_2: F, parameter: F) -> F {
    match algorithm {
        Algorithm::Crossfade => {
            let fade_in = interpolate_clamped(&LUT_XFADE_IN, 0, parameter, 256.0);
            let fade_out = interpolate_clamped(&LUT_XFADE_OUT, 0, parameter, 256.0);
            x_1 * fade_in + x_2 * fade_out
        }
        Algorithm::Fold => {
            let mut sum = F::ZERO;
            sum += x_1;
            sum += x_2;
            sum += x_1 * x_2 * 0.25;
            sum *= 0.02 + parameter;
            let scale = 2048.0_f32 / ((1.0_f32 + 1.0 + 0.25) * 1.02);
            interpolate_clamped(&LUT_BIPOLAR_FOLD, 2048, sum, scale)
        }
        Algorithm::DiodeRing => {
            let (modulator, carrier) = (x_1, x_2 * 2.0);
            let mut ring = diode(modulator + carrier) + diode(modulator - carrier);
            ring *= 4.0 + parameter * 24.0;
            soft_limit(ring)
        }
        Algorithm::DigitalRing => {
            let ring = 4.0 * x_1 * x_2 * (1.0 + parameter * 8.0);
            ring / (1.0 + ring.abs())
        }
        Algorithm::Xor => {
            let mixed = to_short(x_1) ^ to_short(x_2);
            let modulated = F::from_i32(mixed) / 32768.0;
            let sum = (x_1 + x_2) * 0.7;
            sum + (modulated - sum) * parameter
        }
        Algorithm::Comparator => {
            let (modulator, carrier) = (x_1, x_2);
            let (x_integral, x_fractional) = integral_fractional(parameter * 2.995);
            let direct = if modulator < carrier { modulator } else { carrier };
            let window = if modulator.abs() > carrier.abs() { modulator } else { carrier };
            let window_2 = if modulator.abs() > carrier.abs() { modulator.abs() } else { -carrier.abs() };
            let threshold = if carrier > 0.05 { carrier } else { modulator };
            let sequence = [direct, threshold, window, window_2];
            let i = usize::try_from(x_integral).unwrap_or(0).min(2);
            let a = sequence[i];
            let b = sequence[i + 1];
            a + (b - a) * x_fractional
        }
        Algorithm::Nop => x_1,
    }
}

/// `ProcessXmod<algorithm_1, algorithm_2>`: `size` samples of `in_1` (the modulator) and `in_2`
/// (the carrier) into `out`, the balance and the parameter ramped linearly across them.
pub(crate) fn process(pair: (Algorithm, Algorithm), balance: (F, F), parameter: (F, F), in_1: &[F], in_2: &[F], out: &mut [F]) {
    let size = out.len();
    let step = 1.0 / F::from_usize(size);
    let parameter_increment = (parameter.1 - parameter.0) * step;
    let balance_increment = (balance.1 - balance.0) * step;
    let (mut parameter, mut balance) = (parameter.0, balance.0);
    for ((o, x_1), x_2) in out.iter_mut().zip(in_1).zip(in_2) {
        let a = xmod(pair.0, *x_1, *x_2, parameter);
        let b = xmod(pair.1, *x_1, *x_2, parameter);
        *o = a + (b - a) * balance;
        parameter += parameter_increment;
        balance += balance_increment;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crossfade_ends_are_each_input_at_minus_3_db() {
        let (m, c) = (F(0.25), F(-0.5));
        let w = std::f32::consts::FRAC_1_SQRT_2;
        assert!((xmod(Algorithm::Crossfade, m, c, F(0.0)).0 - c.0 * w).abs() < 1e-6);
        assert!((xmod(Algorithm::Crossfade, m, c, F(1.0)).0 - m.0 * w).abs() < 1e-6);
    }

    #[test]
    fn the_comparator_index_stays_in_its_sequence() {
        for p in [0.0_f32, 0.5, 1.0, 1.000_001] {
            let y = xmod(Algorithm::Comparator, F(0.3), F(-0.2), F(p));
            assert!(y.0.is_finite());
        }
    }
}
