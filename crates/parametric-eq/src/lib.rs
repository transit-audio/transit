//! transit's Parametric EQ core (ADR 0054): the zero-latency DSP of the built-in Parametric EQ
//! without the desktop host around it.
//!
//! - **Band designs**: every band type is an analog prototype (decision 2.1) realized
//!   by magnitude-matched second-order sections at the native rate (decision 2.2): poles mapped
//!   exactly by `z = e^(sT)`, zeros fitted to the prototype's magnitude at DC, Nyquist and the band
//!   frequency in the σ domain. Bells, cuts, notch and band pass use that single fit; shelves and the
//!   tilt shelf use two factor sections with a continuous transition to an all-pole form near
//!   Nyquist; flat tilt is fifteen first-order pairs.
//! - **Realization**: every section is a 64-bit trapezoidal state-variable filter with mixed outputs
//!   (decision 2.3); the whole chain runs in f64 from input to output.
//! - **The processor** ([`ParametricEq`]): 24 band slots with smoothing in the log2 and dB domains,
//!   designs on a 16-sample control clock, 10 ms wet ramps for discrete changes, identity skipping,
//!   a left/right stage then a mid/side stage, input sanitizing, the Output gain and the applied gain
//!   match. Everything runs on the processor's own sample clock, so the output does not depend on the
//!   host block size.
//! - **Pure functions for the panel**: [`response_db`], [`band_response_db`] and [`gain_match_db`]
//!   compute the curve and the match from stored parameter values with the same code the engine runs.
//!
//! Real time: [`ParametricEq::prepare`] allocates; processing, setters and [`ParametricEq::reset`]
//! neither allocate, lock nor block. Transcendental functions go through [`math`] only (`exp2`,
//! `log2`, `sin`, `cos`, decision 11), never per sample.
//!
//! ADR 0037 Tier B: the default `std` feature keeps the desktop arithmetic (the inherent float
//! methods); with default features off the crate is `no_std` + `alloc` and its float functions come
//! from `libm`. CI checks that configuration for a bare-metal ARM target.

#![cfg_attr(all(not(feature = "std"), not(test)), no_std)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

extern crate alloc;

mod band;
mod butterworth;
mod chain;
mod design;
mod gain_match;
mod math;
mod prototype;
mod response;
mod section;
mod smooth;
mod weights;

#[cfg(test)]
mod tests;

pub use chain::ParametricEq;
pub use gain_match::gain_match_db;
pub use response::{band_response_db, response_db};

/// Band slots of the device (decision 5.1). A band's number on the panel is its index plus one.
pub const BANDS: usize = 24;

/// The lowest band frequency in hertz (decision 5.1).
pub const FREQ_MIN: f64 = 20.0;
/// The highest band frequency in hertz (decision 5.1); below 40.8 kHz the used frequency is further
/// clamped to 0.49 of the sample rate (decision 12).
pub const FREQ_MAX: f64 = 20_000.0;
/// The band gain range in dB, symmetric (decision 5.1).
pub const GAIN_MAX_DB: f64 = 30.0;
/// The lowest band Q (decision 5.1).
pub const Q_MIN: f64 = 0.1;
/// The highest band Q (decision 5.1).
pub const Q_MAX: f64 = 40.0;
/// The Output range in dB, symmetric (decision 5.1).
pub const OUTPUT_MAX_DB: f64 = 24.0;
/// The applied gain match is clamped to this many dB either way (decision 7).
pub const GAIN_MATCH_MAX_DB: f64 = 12.0;

/// The nine band types, in the panel's and the parameter table's order (decision 5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum BandType {
    #[default]
    Bell,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
    BandPass,
    TiltShelf,
    FlatTilt,
}

impl BandType {
    /// Every type in table order.
    pub const ALL: [BandType; 9] = [
        BandType::Bell,
        BandType::LowShelf,
        BandType::HighShelf,
        BandType::LowCut,
        BandType::HighCut,
        BandType::Notch,
        BandType::BandPass,
        BandType::TiltShelf,
        BandType::FlatTilt,
    ];

    /// The type at a choice index, clamped to the last.
    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    /// Whether the type uses Slope (the cuts only).
    pub fn uses_slope(self) -> bool {
        matches!(self, BandType::LowCut | BandType::HighCut)
    }

    /// Whether the type uses Gain (not the cuts, Notch and Band pass).
    pub fn uses_gain(self) -> bool {
        !matches!(self, BandType::LowCut | BandType::HighCut | BandType::Notch | BandType::BandPass)
    }

    /// Whether the type uses Q (not the cuts and Flat tilt).
    pub fn uses_q(self) -> bool {
        !matches!(self, BandType::LowCut | BandType::HighCut | BandType::FlatTilt)
    }

    /// Whether a band of this type is the identity at 0 dB (decision 2.5): the types with gain.
    pub fn identity_at_zero_gain(self) -> bool {
        self.uses_gain()
    }
}

/// The cut slopes, 6 to 96 dB per octave (decision 2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Slope {
    Db6,
    Db12,
    Db18,
    #[default]
    Db24,
    Db36,
    Db48,
    Db72,
    Db96,
}

impl Slope {
    /// Every slope in table order.
    pub const ALL: [Slope; 8] = [Slope::Db6, Slope::Db12, Slope::Db18, Slope::Db24, Slope::Db36, Slope::Db48, Slope::Db72, Slope::Db96];

    /// The slope at a choice index, clamped to the last.
    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    /// The table index.
    pub fn index(self) -> usize {
        self as usize
    }

    /// The Butterworth order `N = slope / 6`.
    pub fn order(self) -> usize {
        [1, 2, 3, 4, 6, 8, 12, 16][self.index()]
    }

    /// Decibels per octave.
    pub fn db_per_octave(self) -> u32 {
        6 * self.order() as u32
    }
}

/// Per-band stereo placement (decision 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Placement {
    #[default]
    Stereo,
    Left,
    Right,
    Mid,
    Side,
}

impl Placement {
    /// Every placement in table order.
    pub const ALL: [Placement; 5] = [Placement::Stereo, Placement::Left, Placement::Right, Placement::Mid, Placement::Side];

    /// The placement at a choice index, clamped to the last.
    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }

    /// Whether the band runs in the mid/side stage (stage 2).
    pub fn is_mid_side(self) -> bool {
        matches!(self, Placement::Mid | Placement::Side)
    }
}

/// The phase modes the core can compute. Zero latency only in this slice; Natural and Linear join
/// with their own slices (ADR 0054 Slices 5 and 6), so the enum is non-exhaustive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum PhaseMode {
    #[default]
    ZeroLatency,
}

/// Which channel curve a composite response describes (decision 8). Each curve is the path of one
/// channel through its stage: Left = Stereo and Left bands, Right = Stereo and Right bands, Mid =
/// Stereo and Mid bands, Side = Stereo and Side bands. With no placed band all four are the single
/// composite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum CurveChannel {
    #[default]
    Left,
    Right,
    Mid,
    Side,
}

impl CurveChannel {
    /// Whether a band at `place` is on this channel's path.
    pub fn includes(self, place: Placement) -> bool {
        matches!(
            (self, place),
            (_, Placement::Stereo)
                | (CurveChannel::Left, Placement::Left)
                | (CurveChannel::Right, Placement::Right)
                | (CurveChannel::Mid, Placement::Mid)
                | (CurveChannel::Side, Placement::Side)
        )
    }
}

/// One band slot's stored values (decision 5.1), as the parameter table holds them. Values outside
/// their ranges are clamped where used; a non-finite value reads as the default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandParams {
    pub used: bool,
    pub on: bool,
    pub kind: BandType,
    pub freq_hz: f64,
    pub gain_db: f64,
    pub q: f64,
    pub slope: Slope,
    pub place: Placement,
}

impl Default for BandParams {
    fn default() -> Self {
        Self { used: false, on: true, kind: BandType::Bell, freq_hz: 1_000.0, gain_db: 0.0, q: 1.0, slope: Slope::Db24, place: Placement::Stereo }
    }
}

impl BandParams {
    /// Whether the band is heard (used and on).
    pub fn audible(&self) -> bool {
        self.used && self.on
    }
}

/// A band frequency clamped to the parameter range; non-finite reads as the default 1 kHz.
pub(crate) fn clamp_freq(hz: f64) -> f64 {
    if hz.is_finite() { hz.clamp(FREQ_MIN, FREQ_MAX) } else { 1_000.0 }
}

/// A band gain clamped to the parameter range; non-finite reads as the default 0 dB.
pub(crate) fn clamp_gain(db: f64) -> f64 {
    if db.is_finite() { db.clamp(-GAIN_MAX_DB, GAIN_MAX_DB) } else { 0.0 }
}

/// A band Q clamped to the parameter range; non-finite reads as the default 1.0.
pub(crate) fn clamp_q(q: f64) -> f64 {
    if q.is_finite() { q.clamp(Q_MIN, Q_MAX) } else { 1.0 }
}

/// A sample rate as the core uses it: `sane_sr`'s range, 1 kHz to 1.536 MHz; non-finite reads 48 kHz.
pub(crate) fn sane_rate(fs: f64) -> f64 {
    if fs.is_finite() && fs > 0.0 { fs.clamp(1_000.0, 1_536_000.0) } else { 48_000.0 }
}

/// The rate class `m = clamp(2^round(log2(fs / 48 kHz)), 1, 4)` (decision 12), without a
/// transcendental: `round(log2 x) = k` exactly when `x` lies in `[2^(k - 1/2), 2^(k + 1/2))`.
pub fn rate_class(sample_rate: f64) -> usize {
    let x = sane_rate(sample_rate) / 48_000.0;
    let x2 = x * x;
    if x2 < 2.0 {
        1
    } else if x2 < 8.0 {
        2
    } else {
        4
    }
}
