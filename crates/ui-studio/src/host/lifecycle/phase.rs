//! The transport phase (ADR 0062 section 3) and its derivation from observed data (slice S1).

use transit_playback::CountIn;
use transit_playback::record::start::{Point, Ticket};
use transit_time::Samples;

/// What the transport is doing: one value for starts, stops, recording and the real-time render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPhase {
    Stopped,
    /// A Play-family start is tracked and not yet acknowledged.
    StartPending,
    Playing,
    /// The record start is submitted and not yet acknowledged.
    RecordPending {
        ticket: Ticket,
        from: Option<Samples>,
        count_in: Option<CountIn>,
    },
    /// The record start's count-in is clicking.
    CountIn {
        ticket: Ticket,
        from: Samples,
        plan: CountIn,
    },
    /// The record start is acknowledged and waits for the device clock before it latches.
    WaitingForClock {
        ticket: Ticket,
        from: Option<Samples>,
        count_in: Option<CountIn>,
    },
    /// The pass latched at `latch` and captures.
    Recording {
        latch: Point,
        count_in: Option<CountIn>,
    },
    /// A Stop is tracked; `then` is what follows its completion.
    StopPending {
        then: AfterStop,
    },
    /// A real-time render owns the transport.
    Rendering {
        job: RenderJob,
    },
}

/// What follows a pending Stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterStop {
    /// The transport parks.
    Park { end: ParkEnd },
    /// A locate during a pass: the take is finalized and the pass resumes at the target.
    Resume(Samples),
    /// The session was replaced.
    Reset,
}

/// Where a parked transport ends up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkEnd {
    AtCursor,
    Target(Samples),
    ReturnToZero,
}

/// The real-time render's job. The retired job with its collected result arrives with the render
/// as a phase (ADR 0062 slice S2e); until then a running render is always live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderJob {
    Live,
}

/// Why the host required the Stop it has pending, kept with the request so a pending Reset is
/// never read as Park.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StopReason {
    /// Stop, Pause, Toggle, Record during a pass, a lap limit, device trouble, a replace.
    #[default]
    User,
    /// A locate during a pass (the take-safe locate).
    Locate,
    /// The session was replaced.
    Reset,
}

/// The record start under way: what was submitted with it and what its ticket reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassObservation {
    pub ticket: Ticket,
    pub from: Option<Samples>,
    pub count_in: Option<CountIn>,
    /// The applied start (the ticket's `Started` stage), once the audio thread latched.
    pub latch: Option<Point>,
    /// The Record request is still tracked (no receipt yet).
    pub admission_pending: bool,
}

/// Today's data the phase is derived from, sampled together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Observation {
    /// A real-time render is running.
    pub render_active: bool,
    /// A Stop is tracked.
    pub stop_tracked: bool,
    /// The persisted reason for the last Stop required.
    pub stop_reason: StopReason,
    /// A take-safe locate waits to resume its pass here.
    pub resume_at: Option<Samples>,
    /// The prepared pass, when a take is planned and its record start was submitted. A tracked
    /// record-off alone is not a pass.
    pub pass: Option<PassObservation>,
    /// A record-off is tracked. Never a pass on its own: the derivation does not read it.
    pub record_off_tracked: bool,
    /// The Player reports a count-in under way.
    pub count_in_observed: bool,
    /// A Play-family start (not the Record request's own sequence) is tracked.
    pub play_start_tracked: bool,
    /// Playback is observed.
    pub playing: bool,
}

impl TransportPhase {
    /// The phase by ADR 0062's derivation precedence (slice S1): an active render; a pending Stop
    /// with its persisted reason; the current prepared pass (the Started latch, an observed
    /// count-in, admission pending, otherwise the clock wait); a pending Play group; observed
    /// playback; Stopped.
    pub fn derive(o: &Observation) -> Self {
        if o.render_active {
            return Self::Rendering { job: RenderJob::Live };
        }
        if o.stop_tracked || o.resume_at.is_some() {
            let then = match (o.stop_reason, o.resume_at) {
                (StopReason::Reset, _) => AfterStop::Reset,
                (_, Some(at)) => AfterStop::Resume(at),
                (_, None) => AfterStop::Park { end: ParkEnd::AtCursor },
            };
            return Self::StopPending { then };
        }
        if let Some(p) = o.pass {
            if let Some(latch) = p.latch {
                return Self::Recording { latch, count_in: p.count_in };
            }
            if o.count_in_observed
                && let (Some(from), Some(plan)) = (p.from, p.count_in)
            {
                return Self::CountIn { ticket: p.ticket, from, plan };
            }
            return if p.admission_pending {
                Self::RecordPending { ticket: p.ticket, from: p.from, count_in: p.count_in }
            } else {
                Self::WaitingForClock { ticket: p.ticket, from: p.from, count_in: p.count_in }
            };
        }
        if o.play_start_tracked {
            return Self::StartPending;
        }
        if o.playing { Self::Playing } else { Self::Stopped }
    }
}

/// The state text the phase names (ADR 0062 sections 5 and 12): the phase's own text over "All
/// MIDI notes off pending" while a release is tracked. Not shown yet: the display arbitration
/// that presents it lands in slice S2f.
pub fn state_text(phase: &TransportPhase, release_tracked: bool) -> Option<&'static str> {
    let own = match phase {
        TransportPhase::StartPending => Some("Play pending"),
        TransportPhase::RecordPending { count_in: Some(_), .. } | TransportPhase::CountIn { .. } => Some("Count-in"),
        TransportPhase::RecordPending { count_in: None, .. } | TransportPhase::WaitingForClock { .. } => Some("Record pending"),
        TransportPhase::StopPending { then: AfterStop::Park { .. } | AfterStop::Resume(_) } => Some("Stop pending"),
        TransportPhase::Stopped
        | TransportPhase::Playing
        | TransportPhase::Recording { .. }
        | TransportPhase::StopPending { then: AfterStop::Reset }
        | TransportPhase::Rendering { .. } => None,
    };
    own.or(release_tracked.then_some("All MIDI notes off pending"))
}
