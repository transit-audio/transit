use super::*;

impl Player {
    pub fn audio_device_generation(&self) -> u64 {
        self.epoch.0
    }
    /// Set the preference for the next ordinary session publication, without touching capture.
    pub fn set_low_latency(&self, limit_ms: Option<f64>) -> Result<(), PlayerError> {
        self.owner()?.set_low_latency(limit_ms);
        Ok(())
    }

    pub fn requested_low_latency(&self) -> Option<f64> {
        self.owner().ok().and_then(|o| o.low_latency_limit())
    }

    pub fn path_latency_status(&self) -> Option<plan_bridge::StatusReport> {
        self.owner().ok()?.path_latency_status()
    }

    pub fn low_latency_bypasses(&self) -> std::collections::BTreeSet<u64> {
        self.owner().map(|o| o.low_latency_bypasses()).unwrap_or_default()
    }

    pub fn latest_plan_state(&self) -> Option<super::super::RequestState> {
        self.owner().ok().map(|o| o.requested_state().1)
    }
}
