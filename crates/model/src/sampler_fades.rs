//! A Sampler zone's edge fades (ADR 0041, amendment 2026-11-08, item 3): four widths in steps of
//! the zone's own units, the widths playback uses, the factors they give and the check an edit that
//! sets them makes.
//!
//! - **Widths.** [`Fades`] holds `key_lo`, `key_hi`, `vel_lo` and `vel_hi`, integers 0 to 127,
//!   default 0; a zone writes them only when one is nonzero. A width n at the low edge fades the n
//!   values `lo` to `lo + n - 1`: the value k steps from the edge (k = 0 at `lo`) has the factor
//!   `(k + 1)/(n + 1)` and `lo + n` is at full level; the high edge mirrors downward from `hi`.
//! - **Checked when set, clamped when used.** The edit that sets the widths refuses
//!   `lo_w + hi_w > hi - lo` on either axis and any nonzero width on a pad-bound zone
//!   ([`check_fades`]). A later range edit keeps them, and playback uses [`fades_used`]: the low
//!   width clamped to the span, then the high width clamped to what remains (the high one shrinks
//!   first); a pad-bound zone's are all 0. The patch validator accepts both on load and on store.
//! - **Playback** scales the velocity of the selected zone's voice after selection by the original
//!   key and velocity, never its gain: `max(1, round(v * f_key * f_vel))`, exact and rounded once,
//!   ties upward (the core does it). These are ADR 0034's rack factors: a rack zone with
//!   `fade_low = low + lo_w` and `fade_high = high - hi_w` gives the same ones.

use crate::sampler_patch::{Extra, PatchV1, ZoneEntry};
use serde::{Deserialize, Serialize};

/// The widest fade width.
pub const MAX_WIDTH: u8 = 127;

/// A zone's four edge-fade widths, in keys and in velocities.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Fades {
    pub key_lo: u8,
    pub key_hi: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Fades {
    /// The four widths as `[key_lo, key_hi, vel_lo, vel_hi]`.
    #[must_use]
    pub fn widths(&self) -> [u8; 4] {
        [self.key_lo, self.key_hi, self.vel_lo, self.vel_hi]
    }

    /// Fades with these widths (`[key_lo, key_hi, vel_lo, vel_hi]`).
    #[must_use]
    pub fn of(w: [u8; 4]) -> Fades {
        Fades { key_lo: w[0], key_hi: w[1], vel_lo: w[2], vel_hi: w[3], extra: Extra::new() }
    }

    /// Whether nothing is set (then the field is not written).
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.widths() == [0; 4] && self.extra.is_empty()
    }
}

/// The widths one axis plays with over the inclusive range `[lo, hi]`: the low width clamped to
/// the span `hi - lo`, then the high width clamped to what remains.
#[must_use]
pub fn used_pair(range: [u8; 2], lo_w: u8, hi_w: u8) -> [u8; 2] {
    let span = range[1].saturating_sub(range[0]);
    let lo = lo_w.min(span);
    [lo, hi_w.min(span - lo)]
}

/// The widths zone `z` plays with, `[key_lo, key_hi, vel_lo, vel_hi]`: each axis clamped as
/// [`used_pair`] says, and all 0 when the zone is bound to a pad (`pad_bound`), whatever it stores.
#[must_use]
pub fn fades_used(z: &ZoneEntry, pad_bound: bool) -> [u8; 4] {
    if pad_bound {
        return [0; 4];
    }
    let f = &z.fades;
    let [kl, kh] = used_pair(z.keys, f.key_lo, f.key_hi);
    let [vl, vh] = used_pair(z.velocities, f.vel_lo, f.vel_hi);
    [kl, kh, vl, vh]
}

impl PatchV1 {
    /// The widths zone `id` plays with ([`fades_used`], pad-bound when a pad binds it).
    #[must_use]
    pub fn fades_used(&self, id: u16) -> Option<[u8; 4]> {
        self.zone(id).map(|z| fades_used(z, self.pad_of(id).is_some()))
    }
}

/// The fade factor of value `x` on one axis as a ratio `(numerator, denominator)`: over the range
/// `[lo, hi]` with the widths `used` (already clamped, [`used_pair`]), `(k + 1)/(n + 1)` inside a
/// fade, `(1, 1)` elsewhere in the range, and `(0, 1)` outside it.
#[must_use]
pub fn factor(x: u8, range: [u8; 2], used: [u8; 2]) -> (u8, u8) {
    let ([lo, hi], [nl, nh]) = (range, used);
    if x < lo || x > hi {
        (0, 1)
    } else if u16::from(x) < u16::from(lo) + u16::from(nl) {
        (x - lo + 1, nl + 1)
    } else if u16::from(x) + u16::from(nh) > u16::from(hi) {
        (hi - x + 1, nh + 1)
    } else {
        (1, 1)
    }
}

/// Which axis a fade refusal names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Keys,
    Velocities,
}

/// Why an edit that sets fade widths is refused (the document stays as it was).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeRefusal {
    /// A width is past [`MAX_WIDTH`].
    Range,
    /// The two widths of `axis` together exceed its span `max` (`hi - lo`).
    TooWide { axis: Axis, max: u8 },
    /// A nonzero width on a zone bound to a pad (one key, one layer).
    PadBound,
}

impl std::fmt::Display for FadeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FadeRefusal::Range => write!(f, "fade widths are integers from 0 to {MAX_WIDTH}"),
            FadeRefusal::TooWide { axis: Axis::Keys, max } => write!(f, "Key fades are at most {max} keys together for this zone."),
            FadeRefusal::TooWide { axis: Axis::Velocities, max } => {
                write!(f, "Velocity fades are at most {max} velocity steps together for this zone.")
            }
            FadeRefusal::PadBound => write!(f, "A pad's zone has no fades: it plays one key and one velocity layer."),
        }
    }
}

/// The check an edit that sets `fades` on a zone with these ranges makes (slice K's commands call
/// it): every width 0 to 127, no nonzero width on a pad-bound zone, and on each axis
/// `lo_w + hi_w <= hi - lo`. The validator does not make it (stored values are clamped at use).
pub fn check_fades(keys: [u8; 2], velocities: [u8; 2], fades: &Fades, pad_bound: bool) -> Result<(), FadeRefusal> {
    let w = fades.widths();
    if w.iter().any(|&x| x > MAX_WIDTH) {
        return Err(FadeRefusal::Range);
    }
    if pad_bound && w != [0; 4] {
        return Err(FadeRefusal::PadBound);
    }
    for (axis, range, lo_w, hi_w) in [(Axis::Keys, keys, w[0], w[1]), (Axis::Velocities, velocities, w[2], w[3])] {
        let max = range[1].saturating_sub(range[0]);
        if u16::from(lo_w) + u16::from(hi_w) > u16::from(max) {
            return Err(FadeRefusal::TooWide { axis, max });
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "sampler_fades_tests.rs"]
mod tests;
