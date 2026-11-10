//! Edge fades on unbound zones (ADR 0041, "Playback, controls and modulation", and its amendment of
//! 2026-11-08, item 3).
//!
//! A zone's [`Fades`] are four widths in steps of its own units. A width n at the low edge fades the
//! n values `lo` to `lo + n - 1`: the value k steps from the edge (k = 0 at `lo`) has the factor
//! `(k + 1)/(n + 1)`, and `lo + n` is at full level; the high edge mirrors downward from `hi`
//! (ADR 0034's factors). Widths too wide for the range are kept and clamped when used
//! ([`Fades::used`]): the low width to the span `hi - lo`, then the high width to what remains.
//!
//! The factors apply after selection, which is by the original key and velocity, so a fade never
//! moves a note between layers and never extends a zone. They scale the velocity of the selected
//! zone's voice, never its gain: with `f_key` and `f_vel` the two factors (1 outside a fade), the
//! voice plays `max(1, round(v * f_key * f_vel))`, computed exactly as a ratio of integers and
//! rounded once, ties upward. Velocity sensitivity and the velocity modulation source see that
//! value. A pad-bound zone plays no fades whatever it stores.

/// A zone's edge-fade widths (0 to 127 each; 0 is no fade).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Fades {
    pub key_lo: u8,
    pub key_hi: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
}

/// One axis's widths clamped to the inclusive range `(lo, hi)`: the low one to the span, then the
/// high one to what remains.
fn used_pair(range: (u8, u8), lo_w: u8, hi_w: u8) -> (u8, u8) {
    let span = range.1.saturating_sub(range.0);
    let lo = lo_w.min(span);
    (lo, hi_w.min(span - lo))
}

/// The factor of `x` on one axis as `(numerator, denominator)`: `(k + 1, n + 1)` inside a fade of
/// the (clamped) widths `used`, `(1, 1)` elsewhere (`x` is inside the range: it was selected).
fn factor(x: u8, range: (u8, u8), used: (u8, u8)) -> (u32, u32) {
    let (x, lo, hi) = (u32::from(x), u32::from(range.0), u32::from(range.1));
    let (nl, nh) = (u32::from(used.0), u32::from(used.1));
    if x < lo + nl {
        (x.saturating_sub(lo) + 1, nl + 1)
    } else if x + nh > hi {
        (hi.saturating_sub(x) + 1, nh + 1)
    } else {
        (1, 1)
    }
}

impl Fades {
    /// No fades.
    pub const NONE: Fades = Fades { key_lo: 0, key_hi: 0, vel_lo: 0, vel_hi: 0 };

    /// The widths a zone with these ranges plays with (each axis clamped, the high width first).
    #[must_use]
    pub fn used(self, keys: (u8, u8), velocities: (u8, u8)) -> Fades {
        let (key_lo, key_hi) = used_pair(keys, self.key_lo, self.key_hi);
        let (vel_lo, vel_hi) = used_pair(velocities, self.vel_lo, self.vel_hi);
        Fades { key_lo, key_hi, vel_lo, vel_hi }
    }
}

/// The velocity the voice of a zone with ranges `keys` and `velocities` and fades `fades` plays for
/// a note of `key` and `velocity` that selected it: `max(1, round(v * f_key * f_vel))`, exact,
/// rounded once, ties upward.
#[must_use]
pub(crate) fn scaled_velocity(fades: Fades, keys: (u8, u8), velocities: (u8, u8), key: u8, velocity: u8) -> u8 {
    let used = fades.used(keys, velocities);
    let (a, b) = factor(key, keys, (used.key_lo, used.key_hi));
    let (c, d) = factor(velocity, velocities, (used.vel_lo, used.vel_hi));
    // At most 2 * 127 * 128 * 128 + 128 * 128: far inside u32.
    let (num, den) = (u32::from(velocity) * a * c, b * d);
    let rounded = (2 * num + den) / (2 * den);
    rounded.clamp(1, 127) as u8
}
