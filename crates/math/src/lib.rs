//! transit's portable math seam (ADR 0055, `docs/adr/0055-portable-math.md`).
//!
//! The only place a rendered crate gets a transcendental function. Rust's own `sin`, `exp2`, `powf`,
//! `powi` and the rest have unspecified precision: their last bit differs between MSVC's runtime,
//! glibc and Apple's libm, so a bounce made on one system was not the file made on another. Every
//! function here is plain Rust over the `libm` crate, pinned exactly in the workspace, so its output is
//! the same in every bit on every platform and CPU family Rust's IEEE arithmetic runs on.
//!
//! - [`f32`] and [`f64`]: free functions with the platform names. The transcendental ones call `libm`;
//!   the exact ones (`sqrt`, `floor`, `ceil`, `round`, `trunc`, `mul_add`) are re-exported for
//!   `no_std` callers; `powi` is transit's own exact binary exponentiation; `sin_cos` is two calls.
//! - [`consts`]: the conversion literals the devices share.
//! - [`gain`]: decibel and pan-law conversions computed in `f64` and rounded once to `f32`, so they
//!   return the correctly rounded value at every 0.01 dB step (0 dB is exactly 1, +20 dB exactly 10).
//!
//! Each function's outputs over a fixed argument set are pinned by a table test, so a change of the
//! library that moves one bit fails here first. `no_std`, no allocation, no `unsafe`.

#![no_std]
#![forbid(unsafe_code)]

pub mod f32;
pub mod f64;
