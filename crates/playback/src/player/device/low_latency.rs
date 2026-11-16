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

    /// Options > Monitor Armed Tracks (a per-machine preference): the audio callback reads it from the
    /// next block, and the next prepared plan's Low Latency selection follows it (request the
    /// session again after a change).
    pub fn set_monitor_armed(&self, on: bool) -> Result<(), PlayerError> {
        self.shared.monitor_armed.store(on, std::sync::atomic::Ordering::Relaxed);
        self.owner()?.set_monitor_armed(on);
        Ok(())
    }

    pub fn requested_monitor_armed(&self) -> bool {
        self.owner().map_or(true, |o| o.monitor_armed())
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
