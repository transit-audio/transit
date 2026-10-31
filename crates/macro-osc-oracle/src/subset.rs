//! Full or strided runs of the long oracle tests (the CI-time rule, #241).
//!
//! The full self floors and the full null sweeps cost minutes each; running every one of them on
//! every push would double the CI time. The rule: the environment variable `TRANSIT_FULL_ORACLE=1`
//! runs the complete sweep (the release chain sets it); without it a test runs a strided subset,
//! every [`STRIDE`]th point of the sweep in its fixed order (indices 0, 10, 20, ...), so the subset
//! is a fixed, documented set and a failure in it reproduces anywhere. A test that uses [`strided`]
//! prints one line saying which of the two ran.

/// The environment variable that selects the full sweep.
pub const FULL_ENV: &str = "TRANSIT_FULL_ORACLE";

/// The subset keeps every `STRIDE`th point.
pub const STRIDE: usize = 10;

/// True when `TRANSIT_FULL_ORACLE=1` is set.
#[must_use]
pub fn full() -> bool {
    std::env::var(FULL_ENV).is_ok_and(|v| v == "1")
}

/// The points to run: all of them with `TRANSIT_FULL_ORACLE=1`, otherwise indices 0, [`STRIDE`],
/// 2 x [`STRIDE`], ... in the given order. `what` names the sweep in the one-line notice.
#[must_use]
pub fn strided<T>(what: &str, points: Vec<T>) -> Vec<T> {
    let total = points.len();
    let kept: Vec<T> = if full() { points } else { points.into_iter().step_by(STRIDE).collect() };
    println!(
        "{what}: {} of {total} sweep points ({})",
        kept.len(),
        if full() { format!("full, {FULL_ENV}=1") } else { format!("strided subset, every {STRIDE}th; {FULL_ENV}=1 runs all") }
    );
    kept
}

/// Same as [`strided`] without the notice, for the second call of a test that already printed it.
#[must_use]
pub fn strided_quiet<T>(points: Vec<T>) -> Vec<T> {
    if full() { points } else { points.into_iter().step_by(STRIDE).collect() }
}
