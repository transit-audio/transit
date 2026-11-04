//! Host-supplied, adopted audio status. Never serialized into a session.
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, Default)]
pub struct PathLatency {
    pub device_generation: u64,
    pub plan_generation: u64,
    pub sample_rate: u32,
    pub low_latency: bool,
    pub aligned: bool,
    pub engine: u32,
    pub master: u32,
    pub device_seconds: Option<f64>,
}

impl PathLatency {
    pub fn query(self) -> Value {
        let running = self.plan_generation != 0 && self.sample_rate != 0;
        let aligned = running && self.aligned;
        let component = |frames: f64| json!({"frames": frames, "ms": frames * 1000.0 / f64::from(self.sample_rate)});
        let engine = aligned.then(|| component(f64::from(self.engine)));
        let master = aligned.then(|| component(f64::from(self.master)));
        let padding = aligned.then(|| component(f64::from(self.engine.saturating_sub(self.master))));
        let device_frames = self.device_seconds.filter(|n| n.is_finite() && *n >= 0.0).filter(|_| running).map(|n| n * f64::from(self.sample_rate));
        let device = device_frames.map(component);
        let total = device_frames.filter(|_| aligned).map(|n| component(n + f64::from(self.engine)));
        json!({"available": total.is_some(), "device_generation": running.then_some(self.device_generation),
            "plan_generation": running.then_some(self.plan_generation), "sample_rate": running.then_some(self.sample_rate),
            "low_latency": running.then_some(self.low_latency), "padding": padding, "master": master,
            "engine": engine, "device": device, "total": total})
    }

    pub fn text(self) -> String {
        if self.plan_generation == 0 || self.sample_rate == 0 {
            return "Path latency: unavailable (no running plan)".into();
        }
        if !self.aligned {
            return "Path latency: unavailable (Delay Compensation off; paths not aligned)".into();
        }
        match self.device_seconds.filter(|n| n.is_finite() && *n >= 0.0) {
            Some(seconds) => format!("Path latency: {:.1} ms", f64::from(self.engine) * 1000.0 / f64::from(self.sample_rate) + seconds * 1000.0),
            None => "Path latency: unknown (device output latency unavailable)".into(),
        }
    }
}
