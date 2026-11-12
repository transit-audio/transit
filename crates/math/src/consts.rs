//! The conversion literals the devices share (ADR 0055, 2.2), written as literals and never computed at
//! run time. Each is the correctly rounded value of its exact constant, checked by the crate's tests
//! against values computed with arbitrary precision.

/// 20·log10(2): decibels per doubling (the `6.020_6` of ADR 0053). `dB = LOG2_TO_DB · log2(gain)`.
pub const LOG2_TO_DB: f32 = 6.020_6;

/// log2(10)/20, the reciprocal of [`LOG2_TO_DB`]: `gain = exp2(dB · DB_TO_LOG2)`.
pub const DB_TO_LOG2: f32 = 0.166_096_4;

/// log2(e). Equal to `core::f32::consts::LOG2_E`, written out here as the record asks.
#[allow(clippy::approx_constant)] // The literal is the rule (ADR 0055 2.2); a test checks it against core's.
pub const LOG2_E: f32 = 1.442_695;

/// log2(10). Equal to `core::f32::consts::LOG2_10`.
#[allow(clippy::approx_constant)] // The literal is the rule (ADR 0055 2.2); a test checks it against core's.
pub const LOG2_10: f32 = 3.321_928;

/// 20·log10(2) as the correctly rounded `f64`: the gain module's `gain_to_db` factor.
pub const LOG2_TO_DB_F64: f64 = 6.020_599_913_279_624;

/// log2(10)/20 as the correctly rounded `f64` (relative error 5.0e-17): the gain module's
/// `db_to_gain` factor (ADR 0055, 2.9).
pub const DB_TO_LOG2_F64: f64 = 0.166_096_404_744_368_13;

/// log2(e) as the correctly rounded `f64`. Equal to `core::f64::consts::LOG2_E`.
#[allow(clippy::approx_constant)] // The literal is the rule (ADR 0055 2.2); a test checks it against core's.
pub const LOG2_E_F64: f64 = 1.442_695_040_888_963_4;

/// log2(10) as the correctly rounded `f64`. Equal to `core::f64::consts::LOG2_10`.
#[allow(clippy::approx_constant)] // The literal is the rule (ADR 0055 2.2); a test checks it against core's.
pub const LOG2_10_F64: f64 = 3.321_928_094_887_362;
