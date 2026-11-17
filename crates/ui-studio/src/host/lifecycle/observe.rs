//! The read-only adapter: samples today's host data into an [`Observation`] (ADR 0062 slice S1).

use super::phase::{Observation, PassObservation, TransportPhase};
use crate::StudioApp;
use transit_playback::record::start::Status;

impl StudioApp {
    /// What the transport is doing, derived from today's data and the request metadata. Read
    /// only: nothing decides by it yet.
    pub fn transport_phase(&self) -> TransportPhase {
        TransportPhase::derive(&self.transport_observation())
    }

    /// Today's data for [`StudioApp::transport_phase`], sampled together.
    pub(crate) fn transport_observation(&self) -> Observation {
        let r = &self.player_requests;
        // A pass is the planned take whose record start was submitted; a tracked record-off alone
        // is none.
        let pass = self.take_plan.as_ref().and(r.meta.pass.as_ref()).map(|m| PassObservation {
            ticket: m.reader.ticket(),
            from: m.from,
            count_in: m.count_in,
            latch: match m.reader.status() {
                Status::Started(point) => Some(point),
                _ => None,
            },
            admission_pending: r.recording == Some(m.request),
        });
        Observation {
            render_active: self.finish.realtime.running.is_some(),
            stop_tracked: r.stop.is_some(),
            stop_reason: r.meta.stop_reason,
            resume_at: self.midi_transport.take_locate,
            pass,
            record_off_tracked: r.recording.is_some() && r.recording == r.meta.record_off,
            count_in_observed: self.count_in_state().is_some(),
            play_start_tracked: r.play.is_some() && r.play == r.meta.play_start.map(|(sequence, _)| sequence),
            playing: self.is_playing(),
        }
    }
}
