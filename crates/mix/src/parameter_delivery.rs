//! Document parameter acknowledgment, read only by the thread owning the render engine.

use crate::MixEngine;

impl MixEngine {
    /// Whether every live insert and instrument has visited this plan's parameter pass and
    /// delivered its values. Format hosts keep `params_pending` true across skipped process
    /// calls and clear it when a processed block consumes their queued values.
    /// A bypassed insert with a changed value waits for reactivation. Inactive tracks/inserts
    /// do not acknowledge a new document. No waits, allocations, or access to a worker's cell.
    #[must_use]
    pub fn parameters_processed(&self) -> bool {
        if self.pool.as_ref().is_some_and(|p| p.any_away()) {
            return false;
        }
        self.plan.as_ref().is_some_and(|p| {
            !p.faulted
                && p.tables.strips.iter().all(|st| {
                    st.plugins
                        .iter()
                        .flatten()
                        .chain(st.instrument.iter())
                        .all(|slot| slot.parameter_pass && !slot.failed && !slot.woken && !slot.plugin.params_pending())
                })
        })
    }
}
