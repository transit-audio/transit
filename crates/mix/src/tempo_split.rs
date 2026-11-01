//! The host boundary around complete keyed or plain render blocks.
use super::*;

#[cfg(test)]
mod tests;

impl MixEngine {
    /// Audio thread: renders a host block, splitting at tempo and meter changes on the frame
    /// grid (see [`transit_time::TempoMap::render_span`]). Every sub-block runs the complete
    /// render graph with its own transport and event-frame origin. The host's deadline is
    /// resolved once and shared; sub-blocks never buy another period of worker time.
    /// Input stays in host coordinates; output and bus snapshots are assembled there too.
    pub fn render_active(&mut self, pos: Samples, frames: usize, out: &mut [Vec<f32>]) {
        let frames = frames.min(self.max_block);
        let callback_deadline = self.block_deadline.take();
        let rate = self.active().target.sample_rate;
        if let Some(pool) = self.pool.as_mut() {
            pool.host_entry(frames, rate);
        }
        if self.pool.as_ref().is_some_and(|p| p.measurement_requested()) {
            let settled = self.render_workers_settled();
            if let Some(pool) = self.pool.as_mut() {
                pool.capture_measurement(settled);
            }
        }
        let deadline = self.pool.as_mut().map(|pool| {
            pool.begin_host_block();
            let start = std::time::Instant::now();
            match (self.strip_deadline, callback_deadline) {
                (None, Some(d)) => d,
                (budget, _) => start + budget.unwrap_or_else(|| pool.strip_budget(frames, rate)).min(std::time::Duration::from_secs(3600)),
            }
        });
        let mut offset = 0;
        loop {
            let at = pos.saturating_add(offset as i64);
            let remaining = frames - offset;
            let n = self
                .plan
                .as_deref()
                .filter(|p| !p.faulted && !self.monitor_only)
                .map_or(remaining, |p| p.session.tempo.render_span(at, remaining, p.session.sample_rate));
            self.block_deadline = deadline;
            self.render_sub_block(at, n, out, offset);
            offset += n;
            if offset >= frames {
                break;
            }
        }
        #[cfg(feature = "testing")]
        self.mutes.clear();
        if let (Some(pool), Some(plan)) = (&mut self.pool, &self.plan) {
            pool.end_host_block(&plan.session);
        }
    }
}
