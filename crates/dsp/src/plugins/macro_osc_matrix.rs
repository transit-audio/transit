//! The Macro Oscillator's modulation matrix (#243): transit's addition, not part of the original
//! module. Sixteen rows, each a source, an optional "via" source that scales the amount, a
//! destination, a bipolar amount and a curve; every row is five parameters (`mod<n>_source`,
//! `mod<n>_via`, `mod<n>_dest`, `mod<n>_amount`, `mod<n>_curve`, n = 1..16) and so an automation
//! target with a stable id.
//!
//! - **Evaluation.** Per voice, once per 12-frame block, from the sources' values at the block's
//!   start: the rows in order, each adding `amount · via · curve(source)` to its destination's sum
//!   (`via` is 1 when the row has none). A row whose source or destination is Off, or whose amount
//!   is 0, adds nothing and costs nothing. Fixed storage, no allocation.
//! - **Curves.** The clip fades' own shapes ([`crate::offline::FadeShape`]) on the source's
//!   magnitude, the sign kept: `curve(x) = shape(|x|)·sign(x)`, so a bipolar source bends the same
//!   way on both sides of 0 and 0 and ±1 are fixed points of every curve.
//! - **Destinations** and what 100% of a full-scale source does (each destination's sum is
//!   clamped where it is applied, see `macro_osc_voice`): Pitch ±24 semitones; Harmonics, Timbre,
//!   Morph and Decay the knob's whole travel (the knob plus the sum, clamped to 0..1, as the
//!   note-to-macro mapping of #196 did); Level the voice's gain ×(1 + sum), 0..2; Cutoff ±6
//!   octaves; Resonance the knob's whole travel; Filter env amount ±100%; LFO rates ×2^(±4)
//!   (free-rate LFOs; a tempo-synced one follows the tempo); LFO amounts ±100% of the LFO's
//!   output level (one block later: the LFOs' own outputs feed the matrix); Pan the whole width.
//! - **Migration.** The note-to-macro mapping of #196 (a velocity and a key row of depths with a
//!   curve each) is the matrix's first rows: Velocity or Key to Harmonics, Timbre, Morph or Decay,
//!   the depth as the amount, the curve as the curve. `transit_model::legacy_mapping` rewrites an
//!   old session's or preset's parameters that way, and the render is the same bits.

use crate::offline::FadeShape;

/// Rows in the matrix.
pub const ROWS: usize = 16;

/// The sources, in the order of the `mod<n>_source` and `mod<n>_via` parameters' choices.
pub const SOURCES: [&str; 21] = [
    "Off",
    "LFO 1",
    "LFO 2",
    "Amp envelope",
    "Filter envelope",
    "Velocity",
    "Key",
    "Release velocity",
    "Aftertouch",
    "Poly aftertouch",
    "Mod wheel",
    "Pitch bend",
    "Random",
    "Macro 1",
    "Macro 2",
    "Macro 3",
    "Macro 4",
    "Macro 5",
    "Macro 6",
    "Macro 7",
    "Macro 8",
];

/// The destinations, in the order of the `mod<n>_dest` parameters' choices.
pub const DESTINATIONS: [&str; 15] = [
    "Off",
    "Pitch",
    "Harmonics",
    "Timbre",
    "Morph",
    "Decay",
    "Level",
    "Cutoff",
    "Resonance",
    "Filter env amount",
    "LFO 1 rate",
    "LFO 2 rate",
    "LFO 1 amount",
    "LFO 2 amount",
    "Pan",
];

/// The curves, in the order of the `mod<n>_curve` parameters' choices. The first three are the
/// note-to-macro mapping's curves of #196, in its order, so a migrated curve keeps its index.
pub const CURVES: [&str; 5] = ["Linear", "Exponential", "S-curve", "Equal power", "Logarithmic"];

/// Source indices (into [`SOURCES`] and a voice's source values).
pub mod src {
    pub const OFF: usize = 0;
    pub const LFO1: usize = 1;
    pub const LFO2: usize = 2;
    pub const AMP_ENV: usize = 3;
    pub const FILTER_ENV: usize = 4;
    pub const VELOCITY: usize = 5;
    pub const KEY: usize = 6;
    pub const RELEASE_VELOCITY: usize = 7;
    pub const AFTERTOUCH: usize = 8;
    pub const POLY_AFTERTOUCH: usize = 9;
    pub const MOD_WHEEL: usize = 10;
    pub const PITCH_BEND: usize = 11;
    pub const RANDOM: usize = 12;
    /// The first of the eight rack macros.
    pub const MACRO1: usize = 13;
}

/// Destination indices (into [`DESTINATIONS`] and a voice's destination sums).
pub mod dst {
    pub const OFF: usize = 0;
    pub const PITCH: usize = 1;
    pub const HARMONICS: usize = 2;
    pub const TIMBRE: usize = 3;
    pub const MORPH: usize = 4;
    pub const DECAY: usize = 5;
    pub const LEVEL: usize = 6;
    pub const CUTOFF: usize = 7;
    pub const RESONANCE: usize = 8;
    pub const ENV_AMOUNT: usize = 9;
    pub const LFO1_RATE: usize = 10;
    pub const LFO2_RATE: usize = 11;
    pub const LFO1_AMOUNT: usize = 12;
    pub const LFO2_AMOUNT: usize = 13;
    pub const PAN: usize = 14;
}

/// Number of sources (Off included).
pub const NSRC: usize = SOURCES.len();
/// Number of destinations (Off included).
pub const NDST: usize = DESTINATIONS.len();

/// Semitones Pitch moves at 100% and full swing.
pub const PITCH_SEMITONES: f32 = 24.0;
/// Octaves an LFO rate destination moves the rate at 100% and full swing.
pub const RATE_OCTAVES: f32 = 4.0;

/// A source position through curve `curve` ([`CURVES`]), sign kept: the fade's gain of `|x|`
/// (clamped to 0..1) with the sign of `x`. 0 and ±1 are fixed points; NaN reads 0.
#[must_use]
pub fn curve(curve: usize, x: f32) -> f32 {
    let fade = match curve {
        1 => FadeShape::Exponential,
        2 => FadeShape::SCurve,
        3 => FadeShape::EqualPower,
        4 => FadeShape::Logarithmic,
        _ => FadeShape::Linear,
    };
    fade.gain(x.abs()).copysign(x)
}

/// One row as its parameters set it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Row {
    pub source: usize,
    /// [`src::OFF`]: no via, the amount as set.
    pub via: usize,
    pub dest: usize,
    pub curve: usize,
    /// -1..1.
    pub amount: f32,
}

impl Row {
    /// Whether the row can add anything: a source, a destination and a non-zero amount.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.source != src::OFF && self.source < NSRC && self.dest != dst::OFF && self.dest < NDST && self.amount != 0.0 && self.amount.is_finite()
    }
}

/// The matrix of one block: its active rows, in row order, in fixed storage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    rows: [Row; ROWS],
    len: usize,
    /// Bit `d`: an active row has destination `d`.
    dests: u32,
    /// Bit `s`: an active row reads source `s` (as its source or its via).
    sources: u32,
}

impl Default for Matrix {
    fn default() -> Self {
        Matrix { rows: [Row::default(); ROWS], len: 0, dests: 0, sources: 0 }
    }
}

impl Matrix {
    /// The matrix of `rows` (at most [`ROWS`] are read): the active ones, kept in order.
    #[must_use]
    pub fn new(rows: impl IntoIterator<Item = Row>) -> Matrix {
        let mut m = Matrix::default();
        for r in rows.into_iter().take(ROWS).filter(Row::is_active) {
            let via = if r.via < NSRC { r.via } else { src::OFF };
            if let Some(slot) = m.rows.get_mut(m.len) {
                *slot = Row { via, ..r };
                m.len += 1;
                m.dests |= 1 << r.dest;
                m.sources |= 1 << r.source | 1 << via;
            }
        }
        m
    }

    /// No active row.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether an active row has destination `d`.
    #[must_use]
    pub fn targets(&self, d: usize) -> bool {
        d < NDST && self.dests >> d & 1 == 1
    }

    /// Whether an active row reads source `s`, as its source or its via.
    #[must_use]
    pub fn reads(&self, s: usize) -> bool {
        s != src::OFF && s < NSRC && self.sources >> s & 1 == 1
    }

    /// The active rows, in order.
    pub fn rows(&self) -> &[Row] {
        self.rows.get(..self.len).unwrap_or_default()
    }

    /// Each destination's sum for the source values `values` (index [`src`]): the rows in order.
    /// A destination no row targets sums to exactly 0.
    #[must_use]
    pub fn evaluate(&self, values: &[f32; NSRC]) -> [f32; NDST] {
        let mut out = [0.0f32; NDST];
        for r in self.rows() {
            let x = curve(r.curve, values.get(r.source).copied().unwrap_or(0.0));
            let amount = if r.via == src::OFF { r.amount } else { r.amount * values.get(r.via).copied().unwrap_or(0.0) };
            if let Some(o) = out.get_mut(r.dest) {
                *o += amount * x;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(usize, f32)]) -> [f32; NSRC] {
        let mut v = [0.0; NSRC];
        for (i, x) in pairs {
            v[*i] = *x;
        }
        v
    }

    #[test]
    fn inactive_rows_are_dropped_and_the_rest_keep_their_order() {
        let rows = [
            Row { source: src::VELOCITY, dest: dst::TIMBRE, amount: 0.5, ..Row::default() },
            Row { source: src::OFF, dest: dst::TIMBRE, amount: 1.0, ..Row::default() },
            Row { source: src::KEY, dest: dst::OFF, amount: 1.0, ..Row::default() },
            Row { source: src::KEY, dest: dst::MORPH, amount: 0.0, ..Row::default() },
            Row { source: src::LFO1, dest: dst::PAN, amount: -0.25, via: src::MOD_WHEEL, curve: 2 },
        ];
        let m = Matrix::new(rows);
        assert_eq!(m.rows().len(), 2);
        assert_eq!(m.rows()[0].source, src::VELOCITY);
        assert_eq!(m.rows()[1].dest, dst::PAN);
        assert!(m.targets(dst::TIMBRE) && m.targets(dst::PAN) && !m.targets(dst::MORPH));
        assert!(m.reads(src::LFO1) && m.reads(src::MOD_WHEEL) && !m.reads(src::KEY));
        assert!(Matrix::new([Row::default(); ROWS]).is_empty());
    }

    /// Sums per destination in row order; the via scales the amount; a destination no row targets
    /// is exactly 0.
    #[test]
    fn rows_add_into_their_destinations_scaled_by_the_via() {
        let m = Matrix::new([
            Row { source: src::VELOCITY, dest: dst::HARMONICS, amount: 0.5, ..Row::default() },
            Row { source: src::KEY, dest: dst::HARMONICS, amount: -1.0, ..Row::default() },
            Row { source: src::LFO1, dest: dst::CUTOFF, amount: 1.0, via: src::MOD_WHEEL, ..Row::default() },
        ]);
        let v = values(&[(src::VELOCITY, 1.0), (src::KEY, 0.25), (src::LFO1, -0.5), (src::MOD_WHEEL, 0.5)]);
        let out = m.evaluate(&v);
        assert_eq!(out[dst::HARMONICS], 0.5 - 0.25);
        assert_eq!(out[dst::CUTOFF], -0.25);
        for d in [dst::PITCH, dst::TIMBRE, dst::PAN, dst::LEVEL] {
            assert_eq!(out[d].to_bits(), 0.0f32.to_bits(), "{}", DESTINATIONS[d]);
        }
    }

    /// Every curve keeps 0 and ±1 and the sign, is monotonic on 0..1, and clamps past ±1.
    #[test]
    fn curves_fix_the_ends_and_keep_the_sign() {
        for c in 0..CURVES.len() {
            assert_eq!(curve(c, 0.0), 0.0, "{}", CURVES[c]);
            assert!((curve(c, 1.0) - 1.0).abs() < 1e-6 && (curve(c, -1.0) + 1.0).abs() < 1e-6, "{}", CURVES[c]);
            assert_eq!(curve(c, 2.0), curve(c, 1.0));
            let mut last = 0.0;
            for i in 1..=100 {
                let x = i as f32 / 100.0;
                let y = curve(c, x);
                assert!(y >= last && curve(c, -x) == -y, "{} at {x}", CURVES[c]);
                last = y;
            }
        }
        assert_eq!(curve(0, f32::NAN), 0.0);
    }
}
