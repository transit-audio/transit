use crate::*;
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct State {
    pub revision: u64,
    pub published: u64,
    pub pending: bool,
    notice: Option<bool>,
    pub bypass: BTreeSet<u64>,
}

pub(crate) fn deserialize_limit<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    use serde::Deserialize;
    let v = serde_json::Value::deserialize(d)?;
    Ok(v.as_f64().filter(|n| n.is_finite() && *n >= 0.0).unwrap_or(0.0))
}

impl StudioApp {
    pub(crate) fn set_low_latency(&mut self, p: &Value) -> Result<Value, String> {
        let invalid = |why: &str| format!("options.low_latency: {why}");
        let limit = match p.get("limit_ms") {
            None => self.ui.low_latency_limit_ms,
            Some(v) => v.as_f64().filter(|n| n.is_finite() && *n >= 0.0).ok_or_else(|| invalid("limit_ms must be finite and nonnegative"))?,
        };
        let value = match p.get("value") {
            Some(v) => v.as_bool().ok_or_else(|| invalid("value must be a boolean"))?,
            None if p.get("limit_ms").is_some() => self.ui.low_latency,
            None => !self.ui.low_latency,
        };
        if value != self.ui.low_latency || limit != self.ui.low_latency_limit_ms {
            if value != self.ui.low_latency {
                self.low_latency.notice = Some(value);
            }
            self.ui.low_latency = value;
            self.ui.low_latency_limit_ms = limit;
            self.low_latency.revision = self.low_latency.revision.wrapping_add(1);
            self.low_latency.pending = self.player.is_some();
            self.save_audio_preferences();
            self.sync_player_session();
            self.poll_low_latency();
        }
        Ok(json!({"value": self.ui.low_latency, "limit_ms": self.ui.low_latency_limit_ms}))
    }

    pub(crate) fn poll_low_latency(&mut self) {
        let Some(p) = &self.player else {
            self.engine.path_latency = Default::default();
            self.low_latency.bypass.clear();
            self.low_latency.pending = false;
            if let Some(on) = self.low_latency.notice.take() {
                self.status = notice(on, 0, 0.0);
            }
            return;
        };
        if self.engine.path_latency.device_generation != p.audio_device_generation() {
            self.engine.path_latency = Default::default();
            self.low_latency.bypass.clear();
        }
        if let Some(report) = p.path_latency_status() {
            let s = report.status;
            self.engine.path_latency = transit_engine::path_latency::PathLatency {
                device_generation: report.device_generation.0,
                plan_generation: report.generation.get(),
                sample_rate: report.sample_rate,
                low_latency: s.low_latency,
                aligned: s.aligned,
                engine: s.engine_latency,
                master: s.master_latency,
                device_seconds: p.output_latency.map(|d| d.as_secs_f64()),
            };
            self.low_latency.bypass = p.low_latency_bypasses();
            if self.low_latency.published == self.low_latency.revision
                && matches!(p.latest_plan_state(), Some(transit_playback::player::RequestState::Finished(transit_playback::player::SessionOutcome::Applied { generation })) if generation == report.generation)
                && s.low_latency == self.ui.low_latency
            {
                self.low_latency.pending = false;
                if let Some(on) = self.low_latency.notice.take() {
                    self.status = notice(on, self.low_latency.bypass.len(), f64::from(s.saved_latency) * 1000.0 / f64::from(report.sample_rate));
                }
            }
        }
    }

    pub(crate) fn mode_bypassed(&self, uid: u64) -> bool {
        self.low_latency.bypass.contains(&uid)
    }

    pub(crate) fn mode_bypass_tip(&self, uid: u64) -> Option<&'static str> {
        self.mode_bypassed(uid).then_some("bypassed by Low Latency")
    }

    pub(crate) fn low_latency_button(&mut self, ui: &mut egui::Ui) {
        let label = if self.low_latency.pending { LABEL_PENDING } else { LABEL };
        // The button is as wide as its longer label in the current font, so "(pending)" appearing
        // never moves what sits to its right.
        let width = low_latency_width(ui, LABEL_PENDING);
        let b = ui.add(egui::Button::new(label).selected(self.ui.low_latency).min_size(egui::vec2(width, 0.0)));
        self.arrange.layout.transport.insert("low_latency".into(), [b.rect.min.x, b.rect.min.y, b.rect.max.x, b.rect.max.y]);
        crate::help::hover(ui, &b, crate::help::Id::TransportLowLatency);
        if b.on_hover_text(format!("{}\nShared/Main path, including device output latency", self.engine.path_latency.text())).clicked() {
            let _ = self.run("options.low_latency", json!({}));
        }
    }

    pub(crate) fn low_latency_limit(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Low Latency limit");
            let mut limit = self.ui.low_latency_limit_ms;
            let b = crate::chrome::field(ui, |ui| ui.add(egui::DragValue::new(&mut limit).range(0.0..=f64::MAX).speed(0.1).suffix(" ms")));
            crate::help::hover(ui, &b, crate::help::Id::AudioDevicesLowLatencyLimit);
            if b.changed() {
                let _ = self.run("options.low_latency", json!({"limit_ms": limit}));
            }
        });
        let b = ui.label(self.engine.path_latency.text());
        crate::help::hover(ui, &b, crate::help::Id::DiagnosticsPathLatency);
    }
}

const LABEL: &str = "Low Latency";
const LABEL_PENDING: &str = "Low Latency (pending)";

/// A button's width for `label` in the current button font and padding.
pub(crate) fn low_latency_width(ui: &egui::Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.painter().layout_no_wrap(label.into(), font, egui::Color32::WHITE).size().x + 2.0 * ui.spacing().button_padding.x
}

fn notice(on: bool, count: usize, saved: f64) -> String {
    if on { format!("Low Latency on: {count} inserts bypassed, {saved:.1} ms saved") } else { "Low Latency off".into() }
}

#[cfg(test)]
mod tests;
