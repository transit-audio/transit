//! The typed transport lifecycle (ADR 0062). Slice S1: the transport phase is derived read-only
//! from today's data plus the request metadata the host keeps beside its tracked requests (the
//! Play start, the record start's ticket, start position and count-in, the record-off kind and
//! the reason for a pending Stop). Nothing reads the phase to decide anything yet: every status
//! text, flag and guard answers as before.
//!
//! The module names no window type and reads no `StudioApp` field of a window type, and it reads
//! no clock, so it can move into its own crate unchanged (ADR 0062 section 2).

mod observe;
mod phase;
mod requests;

pub use phase::{AfterStop, Observation, ParkEnd, PassObservation, RenderJob, StopReason, TransportPhase, state_text};
pub(crate) use requests::{Classified, PassMeta, RequestMeta};

#[cfg(test)]
mod tests;
