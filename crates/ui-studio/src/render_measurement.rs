//! The render graph tail's paired measurement through the control channel (G1 of the tail audit;
//! docs/contributing/knowledge/render-graph-measurement.md, "Running the paired measurement").
//!
//! `measure.start {label, tail}` sets the tail switch for the run (`serial` or `graph`, the
//! process-wide measurement switch), starts the since-open peak over and posts the start request
//! to the engine's measurement mailbox; `measure.stop` posts the end request; each frame the UI
//! copies an arrived response together with the callback telemetry ([`StudioApp::poll_render_measurement`]);
//! `measure.rows` returns the table, each row's counters and the acceptance lines, which Copy
//! diagnostics also prints under "Render measurement". Off the audio thread; the callback only
//! fills preallocated counters.

use crate::StudioApp;
use serde_json::{Value, json};
use transit_playback::render_measurement::{RunEdge, RunRow, TailPath, report_lines};

/// Where a measurement stands.
#[derive(Debug, Default, Clone, PartialEq)]
enum Phase {
    /// No run under way.
    #[default]
    Idle,
    /// The start request is posted; its response has not arrived.
    Starting { label: String, path: TailPath },
    /// The run is under way from `start`.
    Running { label: String, path: TailPath, start: RunEdge },
    /// The end request is posted.
    Ending { label: String, path: TailPath, start: RunEdge },
}

/// The control channel's measurement state: the finished rows and the run under way.
#[derive(Debug, Default)]
pub struct MeasureState {
    /// Finished rows, oldest first.
    pub rows: Vec<RunRow>,
    phase: Phase,
    /// The last request identity posted.
    next: u64,
}

impl MeasureState {
    fn state(&self) -> &'static str {
        match self.phase {
            Phase::Idle => "idle",
            Phase::Starting { .. } => "starting",
            Phase::Running { .. } => "running",
            Phase::Ending { .. } => "ending",
        }
    }

    /// The report section's lines: the state of a run under way, then the rows and the verdict.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = match &self.phase {
            Phase::Idle => Vec::new(),
            Phase::Starting { label, path } | Phase::Running { label, path, .. } | Phase::Ending { label, path, .. } => {
                vec![format!("Run under way: {label} ({} tail, {})", path.name(), self.state())]
            }
        };
        lines.extend(report_lines(&self.rows));
        lines
    }
}

impl StudioApp {
    /// `measure.start`, `measure.stop` and `measure.rows` (see the module docs).
    pub(crate) fn measure_command(&mut self, method: &str, p: &Value) -> Result<Value, String> {
        match method {
            "measure.rows" => Ok(json!({"state": self.measure.state(), "lines": self.measure.lines()})),
            "measure.start" => {
                if self.measure.phase != Phase::Idle {
                    return Err(format!("a measurement is {} (measure.stop ends it)", self.measure.state()));
                }
                let label = p
                    .get("label")
                    .and_then(Value::as_str)
                    .filter(|l| !l.trim().is_empty())
                    .ok_or("measure.start needs a label, for example \"64 idle\"")?;
                let path = match p.get("tail").and_then(Value::as_str) {
                    Some("serial") => TailPath::Serial,
                    Some("graph") => TailPath::Graph,
                    _ => return Err("measure.start needs tail: \"serial\" or \"graph\"".into()),
                };
                if path == TailPath::Graph && !transit_mix::MixEngine::graph_enabled() {
                    return Err("the tail graph needs the task graph (TRANSIT_RENDER_GRAPH=0 turns both off)".into());
                }
                let Some(player) = &self.player else { return Err("no audio device is open".into()) };
                transit_mix::MixEngine::force_tail(path == TailPath::Graph);
                player.reset_peak_load();
                let id = self.measure.next + 1;
                if !player.request_render_measurement(id) {
                    return Err("the engine has no render workers, or a measurement response is still unread".into());
                }
                self.measure.next = id;
                self.measure.phase = Phase::Starting { label: label.to_owned(), path };
                Ok(json!({"requested": id, "label": label, "tail": path.name()}))
            }
            "measure.stop" => {
                let Phase::Running { label, path, start } = self.measure.phase.clone() else {
                    return Err(format!("no run to stop (the measurement is {})", self.measure.state()));
                };
                let Some(player) = &self.player else { return Err("no audio device is open".into()) };
                let id = self.measure.next + 1;
                if !player.request_render_measurement(id) {
                    return Err("the measurement mailbox refused the end request".into());
                }
                self.measure.next = id;
                self.measure.phase = Phase::Ending { label, path, start };
                Ok(json!({"requested": id}))
            }
            other => Err(format!("unknown method `{other}`")),
        }
    }

    /// Copies an arrived mailbox response with the callback telemetry beside it: the start edge
    /// of a run, or its end and so a finished row. Called every frame; a few relaxed loads.
    pub(crate) fn poll_render_measurement(&mut self) {
        if matches!(self.measure.phase, Phase::Idle | Phase::Running { .. }) {
            return;
        }
        let Some(player) = &self.player else { return };
        let Some(m) = player.read_render_measurement() else { return };
        let edge = RunEdge { mailbox: Some(m), telemetry: player.callback_telemetry() };
        self.measure.phase = match std::mem::take(&mut self.measure.phase) {
            Phase::Starting { label, path } => Phase::Running { label, path, start: edge },
            Phase::Ending { label, path, start } => {
                self.measure.rows.push(RunRow::between(&label, path, &start, &edge));
                Phase::Idle
            }
            other => other,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without a device the commands refuse and say why; the rows command answers with the state.
    #[test]
    fn the_commands_refuse_without_a_device_and_check_their_parameters() {
        let mut app = StudioApp::new(transit_engine::Engine::default(), None, crate::Services::default());
        let start = |app: &mut StudioApp, p: Value| app.measure_command("measure.start", &p);
        assert!(start(&mut app, json!({"tail": "graph"})).unwrap_err().contains("needs a label"));
        assert!(start(&mut app, json!({"label": "64 idle", "tail": "both"})).unwrap_err().contains("\"serial\" or \"graph\""));
        assert_eq!(start(&mut app, json!({"label": "64 idle", "tail": "serial"})).unwrap_err(), "no audio device is open");
        assert_eq!(app.measure_command("measure.stop", &json!({})).unwrap_err(), "no run to stop (the measurement is idle)");
        assert_eq!(app.measure_command("measure.rows", &json!({})).unwrap(), json!({"state": "idle", "lines": []}));
        let mut m = MeasureState::default();
        assert_eq!(m.state(), "idle");
        assert!(m.lines().is_empty());
        m.phase = Phase::Starting { label: "64 idle".into(), path: TailPath::Graph };
        assert_eq!(m.lines(), ["Run under way: 64 idle (graph tail, starting)"]);
    }
}
