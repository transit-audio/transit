//! Shared plumbing for the built-in instruments: their event queues' capacity and release
//! reserve, and the ADSR.

// The envelope moved to the shared instrument primitives (ADR 0041); its old path stays.
pub use transit_instrument_core::adsr::{Adsr, AdsrRates, Stage};

/// Capacity of a built-in instrument's event queue.
pub(crate) const MAX_EVENTS: usize = 1024;
/// Queue entries kept for releases: one per note and pedal entry the mix can hold open (224 +
/// 32), plus one reset (F10). Other events are refused once only these remain.
pub(crate) const RELEASE_RESERVE: usize = 257;
