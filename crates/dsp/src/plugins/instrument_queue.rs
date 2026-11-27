//! Shared plumbing for the instruments: the built-ins' event queues' capacity and release
//! reserve, the ADSR, and ADR 0059's ownership ledger ([`ledger`]) with the hosted adapters' event
//! queue built on it ([`delivery`]).

// The envelope moved to the shared instrument primitives (ADR 0041); its old path stays.
pub use transit_instrument_core::adsr::{Adsr, AdsrRates, Stage};

pub mod delivery;
pub mod ledger;

/// Capacity of a built-in instrument's event queue.
pub(crate) const MAX_EVENTS: usize = 1024;
/// Queue entries kept for releases: one per note and pedal entry the mix can hold open (224 +
/// 32), plus one reset (F10). Other events are refused once only these remain.
pub(crate) const RELEASE_RESERVE: usize = 257;

#[cfg(test)]
#[path = "instrument_queue/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "instrument_queue/separation_tests.rs"]
mod separation_tests;
