//! The typed transport lifecycle (ADR 0062). Slice S1: the transport phase is derived read-only
//! from today's data plus the request metadata the host keeps beside its tracked requests (the
//! Play start, the record start's ticket, start position and count-in, the record-off kind and
//! the reason for a pending Stop). Nothing reads the phase to decide anything yet: every status
//! text, flag and guard answers as before.
//!
//! Slice S2a: the pure reducer `next(phase, event, facts)` over the transition matrix of the
//! record's appendix (tables.md section A), written as a literal table. Unused by the app until
//! slice S2d.
//!
//! The module names no window type and reads no `StudioApp` field of a window type, and it reads
//! no clock, so it can move into its own crate unchanged (ADR 0062 section 2).

mod matrix;
mod observe;
mod phase;
mod reducer;
mod requests;

pub use matrix::{Col, Op, Refusal, Row, header, row};
pub use phase::{AfterStop, CancelReason, Observation, ParkEnd, PassObservation, RenderJob, RenderOutcome, StopReason, TransportPhase, state_text};
pub use reducer::{
    DEFERRED_LIMIT, Disposition, Effect, Event, Facts, Held, Message, OWED_STOP_DEADLINE, Outcome, PlayOf, RecordPrep, RecordStart, Speed,
    StartCommand, StartGroup, Step, Tracked, cell, next,
};
pub(crate) use requests::{Classified, PassMeta, RequestMeta};

#[cfg(test)]
mod closure_tests;
#[cfg(test)]
mod matrix_tests;
#[cfg(test)]
mod tests;
