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

    /// The tooltip's first line: the control's name, with "(pending)" while the request waits.
    pub(crate) fn low_latency_title(&self) -> &'static str {
        if self.low_latency.pending { LABEL_PENDING } else { LABEL }
    }

    pub(crate) fn low_latency_button(&mut self, ui: &mut egui::Ui) {
        use crate::icons::{self, Icon};
        // An icon button the size of its neighbors, so it is the same width in every
        // state and never moves what sits to its right (#417).
        let (r, b) = ui.allocate_exact_size(crate::transport::BUTTON_SIZE, egui::Sense::click());
        let (fill, glyph) = look(self.ui.low_latency, self.low_latency.pending, b.hovered());
        ui.painter().rect_filled(r, crate::theme::RADIUS, fill);
        icons::paint_in(ui.painter(), r, Icon::Zap, glyph);
        self.arrange.layout.transport.insert("low_latency".into(), [r.min.x, r.min.y, r.max.x, r.max.y]);
        self.arrange.layout.transport_icons.insert("low_latency".into(), Icon::Zap.name().into());
        crate::help::hover(ui, &b, crate::help::Id::TransportLowLatency);
        let tip = format!(
            "{}
{}
Shared/Main path, including device output latency",
            self.low_latency_title(),
            self.engine.path_latency.text()
        );
        if b.on_hover_text(tip).clicked() {
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

/// The button's fill and glyph. Off: the standard transport button with a `text` glyph. On: the
/// metronome's lit look, `signal` glyph on a 35 percent `signal` fill. Pending: the lit look "half lit"
/// (`tokens::alpha::VOICE_RELEASING`, 45 percent, as a voice light whose release fades): in progress, not unavailable.
pub(crate) fn look(on: bool, pending: bool, hovered: bool) -> (egui::Color32, egui::Color32) {
    use crate::theme::TOKENS;
    use crate::tokens::alpha::VOICE_RELEASING as HALF_LIT;
    if pending {
        (TOKENS.signal.gamma_multiply(0.35 * HALF_LIT), TOKENS.signal.gamma_multiply(HALF_LIT))
    } else if on {
        (TOKENS.signal.gamma_multiply(0.35), TOKENS.signal)
    } else {
        (if hovered { TOKENS.button_hover } else { TOKENS.button }, TOKENS.text)
    }
}

fn notice(on: bool, count: usize, saved: f64) -> String {
    if on { format!("Low Latency on: {count} inserts bypassed, {saved:.1} ms saved") } else { "Low Latency off".into() }
}

#[cfg(test)]
mod tests;
