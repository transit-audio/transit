//! transit's band-limited fractional reader (ADR 0056, decision 8,
//! `docs/adr/0056-audio-clip-editor.md`).
//!
//! One output sample at a fractional source position `x` and an effective rate `q` (source frames per
//! output sample; above 1 the output is pitched up or decimated, below 1 pitched down or interpolated):
//!
//! - **The kernel** ([`kernel`]): a Kaiser-windowed sinc of 16 zero crossings a side, β = 7.85726, so 32
//!   taps at `q ≤ 1` with the cutoff at half the source rate. For `q > 1` it stretches, `h(t/q)/q` over
//!   `|t| < 16q`, so the cutoff follows the rate to half the output rate and the tap count is about
//!   `2·16q` (up to 1,024 at `q = 32`).
//! - **The table** ([`SincTable`]): `h` at 512 points per zero crossing (8,193 entries) and its delta
//!   table, built once off the audio thread through the portable math seam (ADR 0055), so it is the same
//!   in every bit on every platform; 64 KiB shared read-only.
//! - **The read** ([`read_frame`]): table interpolation in `f32`, accumulation in `f32` in ascending
//!   tap order, exactly as decision 8.3 states, so two implementations agree bit for bit.
//! - **Boundaries** (decision 8.5): a tap outside the source reads 0 ([`Frames`]); a clip's region
//!   edges, loop seams and intros are the caller's mapping, never a boundary here.
//! - **Exactness**: at `q ≤ 1` and an integer position the output is the source frame exactly; at
//!   `q = 1` the caller takes the direct index ([`direct`]) and never calls the reader.
//!
//! Stateless: the output depends only on the source frames, the position and the rate, never on how a
//! caller cuts its work into blocks. Real time: [`read_frame`] neither allocates nor locks and does at
//! most `2·16·max(1, q)` taps; [`SincTable::new`] allocates and runs off the audio thread. Latency for a
//! streaming caller is [`reach`]`(q)` source frames of look-ahead (16 at `q ≤ 1`); [`halo`] is the
//! margin a buffered caller keeps on each side.
//!
//! ADR 0037 Tier B: the default `std` feature adds only the process-wide [`SincTable::shared`]; with
//! default features off the crate is `no_std` + `alloc` and the caller owns its table. CI checks that
//! configuration for a bare-metal ARM target.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod read;
mod table;

pub use read::{Frames, Interleaved, MAX_POSITION, MAX_RATE, Mono, Planar, accepts, direct, halo, reach, read, read_frame, taps};
pub use table::{BETA, I0_TERMS, PHASES, SincTable, TABLE_LEN, ZERO_CROSSINGS, bessel_i0, kernel};
