//! Document parameter acknowledgment, read only by the thread owning the render engine.
use crate::{INSTRUMENT_SLOT, MixEngine, TrackId};

/// Delivery of one instance's values in the current plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterDelivery {
    Pending,
    Ready,
    Failed,
}

impl MixEngine {
    /// Confirm only this slot. Inactive slots keep their stored values; failed slots refuse.
    /// A worker-owned strip is pending without touching its cell. Other strips never gate it.
    #[must_use]
    pub fn parameter_delivery(&self, track: TrackId, index: usize) -> ParameterDelivery {
        let Some(p) = self.plan.as_ref().filter(|p| !p.faulted) else { return ParameterDelivery::Pending };
        let Some(i) = p.tables.strip_of(track) else { return ParameterDelivery::Pending };
        if self.pool.as_ref().is_some_and(|pool| pool.is_away(i)) {
            return ParameterDelivery::Pending;
        }
        let Some(st) = p.tables.strips.get(i) else { return ParameterDelivery::Pending };
        let Some(t) = p.session.tracks.get(usize::from(st.track)) else { return ParameterDelivery::Pending };
        let (slot, insert) = if index == INSTRUMENT_SLOT {
            (st.instrument.as_ref(), t.instrument.as_ref())
        } else {
            (st.plugins.get(index).and_then(Option::as_ref), t.mixer.inserts.get(index).and_then(Option::as_ref))
        };
        let (Some(slot), Some(insert)) = (slot, insert) else { return ParameterDelivery::Pending };
        if slot.failed {
            ParameterDelivery::Failed
        } else if t.inactive || !insert.active || (slot.parameter_pass && !slot.woken && !slot.plugin.params_pending()) {
            ParameterDelivery::Ready
        } else {
            ParameterDelivery::Pending
        }
    }

    /// Aggregate diagnostic; header jobs use the per-instance acknowledgment instead.
    #[must_use]
    pub fn parameters_processed(&self) -> bool {
        self.plan.as_ref().is_some_and(|p| {
            !p.faulted
                && p.session.tracks.iter().all(|t| {
                    t.mixer
                        .inserts
                        .iter()
                        .enumerate()
                        .filter(|(_, ins)| ins.is_some())
                        .all(|(i, _)| self.parameter_delivery(t.id, i) == ParameterDelivery::Ready)
                        && (t.instrument.is_none() || self.parameter_delivery(t.id, INSTRUMENT_SLOT) == ParameterDelivery::Ready)
                })
        })
    }
}
