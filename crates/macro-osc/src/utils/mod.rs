//! Utility functions.
//!
//! This module contains ports of functions that were used in several of the original author's
//! devices in common and were made for the STM32 platform.

pub mod atan;
pub mod cosine_oscillator;
pub mod delay_line;
pub mod filter;
pub mod hysteresis_quantizer;
pub mod limiter;
pub mod parameter_interpolator;
pub mod polyblep;
pub mod random;
pub mod rsqrt;
pub mod units;

/// `table[i]`, or the nearest end of the table when `i` is outside it.
///
/// The firmware's interpolators (`stmlib/dsp/dsp.h`) do not clamp: every caller keeps its index
/// in range (the sine lookup reads up to 1.25 of the table on purpose, which is why the table
/// has 640 + 1 entries). Out of range the firmware would read past the table; the port reads
/// the nearest entry instead of panicking. In range the result is the firmware's, bit for bit.
#[inline]
fn at(table: &[f32], i: i32) -> f32 {
    let last = table.len() as i32 - 1;
    table[i.clamp(0, last) as usize]
}

/// `stmlib::Interpolate`: linear interpolation at `index * size`, no clamping.
#[inline]
pub fn interpolate(table: &[f32], mut index: f32, size: f32) -> f32 {
    index *= size;
    let index_integral = index as i32;
    let index_fractional = index - (index_integral as f32);
    let a = at(table, index_integral);
    let b = at(table, index_integral + 1);

    a + (b - a) * index_fractional
}

/// `stmlib::InterpolateHermite` on the table that starts at `table[origin]` (the firmware passes
/// a pointer into its table, such as `lut_fold + 1`, so the sample before index 0 is
/// `table[origin - 1]`). No clamping.
#[inline]
pub fn interpolate_hermite(table: &[f32], origin: usize, mut index: f32, size: f32) -> f32 {
    index *= size;
    let index_integral = index as i32;
    let index_fractional = index - (index_integral as f32);
    let i = origin as i32 + index_integral;
    let xm1 = at(table, i - 1);
    let x0 = at(table, i);
    let x1 = at(table, i + 1);
    let x2 = at(table, i + 2);
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b_neg = w + a;
    let f = index_fractional;

    (((a * f) - b_neg) * f + c) * f + x0
}

/// `stmlib::InterpolateWrap`: the integral part of `index` is dropped (so the index wraps for
/// positive values), then linear interpolation. No clamping.
#[inline]
pub fn interpolate_wrap(table: &[f32], mut index: f32, size: f32) -> f32 {
    index -= (index as i32) as f32;
    index *= size;
    let index_integral = index as i32;
    let index_fractional = index - (index_integral as f32);
    let a = at(table, index_integral);
    let b = at(table, index_integral + 1);

    a + (b - a) * index_fractional
}

/// Flush to zero: a subnormal result becomes a zero of the same sign.
///
/// The firmware runs with flush-to-zero on (the oracle reproduces it with the FPU mode, as the
/// firmware's desktop test harness does). Rust code always runs in the default mode, where
/// subnormals are kept, so the port flushes explicitly, operation by operation, wherever a state
/// can decay into the subnormal range (a decaying filter, envelope or resonator). Elsewhere a
/// subnormal never occurs in the sweep and the flush would be a no-op.
#[inline]
pub fn ftz(x: f32) -> f32 {
    if x.is_subnormal() { 0.0_f32.copysign(x) } else { x }
}

/// `ONE_POLE(out, in, coefficient)`, with each operation flushed to zero as on the firmware.
#[inline]
pub fn one_pole(out: &mut f32, in_: f32, coefficient: f32) {
    *out = ftz(*out + ftz(coefficient * ftz(in_ - *out)));
}

#[inline]
/// `SLOPE(out, in, positive, negative)`: the coefficient picked by the sign of the error
/// multiplies the error either way.
pub fn slope(out: &mut f32, in_: f32, positive: f32, negative: f32) {
    let error = in_ - *out;
    *out += (if error > 0.0 { positive } else { negative }) * error;
}

#[inline]
pub fn slew(out: &mut f32, in_: f32, delta: f32) {
    let mut error = (in_) - *out;
    let d = delta;
    if error > d {
        error = d;
    } else if error < -d {
        error = -d;
    }
    *out += error;
}

#[inline]
pub fn crossfade(a: f32, b: f32, fade: f32) -> f32 {
    a + (b - a) * fade
}

#[inline]
pub fn soft_limit(x: f32) -> f32 {
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

#[inline]
pub fn soft_clip(x: f32) -> f32 {
    if x < -3.0 {
        -1.0
    } else if x > 3.0 {
        1.0
    } else {
        soft_limit(x)
    }
}

#[inline]
pub fn clip_16(x: i32) -> i32 {
    x.clamp(-32768, 32767)
}

#[inline]
pub fn sqrt(x: f32) -> f32 {
    f32::sqrt(x)
}
