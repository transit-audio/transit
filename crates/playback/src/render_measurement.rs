//! The render graph tail's acceptance measurement (#174, #635; ADR 0029 section 11;
//! docs/contributing/knowledge/render-graph-measurement.md): a run's row from two edges, each a
//! render measurement mailbox response with the callback telemetry read beside it, and the
//! paired acceptance check of the serial tail against the tail graph.
//!
//! Pure: no clock, no device, no audio-thread access. The callback side only fills preallocated
//! lock-free counters; the mailbox response is copied by its one control-side consumer, and the
//! telemetry is a set of relaxed loads ([`crate::player::Player::callback_telemetry`]).

use crate::telemetry::CallbackTelemetry;
use transit_rt::worker_set::queue::{Measurement, QueueSnapshot};

/// One edge of a run: the mailbox response (`None`: none arrived) and the telemetry read when it
/// was copied.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RunEdge {
    /// The render measurement mailbox's response at this edge.
    pub mailbox: Option<Measurement>,
    /// The callback telemetry read beside it (render-worker figures included).
    pub telemetry: CallbackTelemetry,
}

/// Which tail a run measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TailPath {
    /// The serial tail on the audio thread.
    Serial,
    /// The tail graph on the render workers.
    Graph,
}

impl TailPath {
    /// `serial` or `graph`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Serial => "serial",
            Self::Graph => "graph",
        }
    }
}

/// One run's row: every figure the acceptance rule names, as differences between its two edges
/// (the longest callback is the end's, the peak having been reset at the start).
#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    /// The run's label, for example `64 idle`; a serial and a graph row with one label are a pair.
    pub label: String,
    /// The tail the run measured.
    pub path: TailPath,
    /// The device callback's frames at the end.
    pub frames: u32,
    /// Callbacks.
    pub callbacks: u64,
    /// Over-budget callbacks (`OB`).
    pub over_budget: u64,
    /// The longest callback of the run, nanoseconds.
    pub longest_ns: u64,
    /// Blocks with a strip overrun.
    pub overrun_blocks: u64,
    /// Strip overruns by cause.
    pub busy: u64,
    /// Late strip overruns.
    pub late: u64,
    /// Unstarted strip overruns.
    pub unstarted: u64,
    /// Aux task overruns, busy (a subset of `busy`).
    pub aux_busy: u64,
    /// Aux task overruns, late (a subset of `late`).
    pub aux_late: u64,
    /// Aux hole task-blocks (`H`): per host block, per aux task Late or Busy in any sub-block.
    pub holes: u64,
    /// Upstream-missed skips.
    pub upstream_missed: u64,
    /// Strip deadline overshoots.
    pub overshoots: u64,
    /// Driver overloads (`None`: the backend keeps no such count).
    pub overloads: Option<u64>,
    /// Stream errors (backend error callbacks).
    pub stream_errors: u64,
    /// Every mailbox counter, end less start.
    pub counters: QueueSnapshot,
    /// Why the row is invalid; empty for a valid row.
    pub problems: Vec<String>,
}

/// The table's header lines (Markdown, the columns of the measurement page).
pub const TABLE_HEADER: [&str; 2] = [
    "| run | tail | frames | callbacks | OB | longest callback | overrun blocks | busy | late | unstarted | aux busy | aux late | H | upstream missed | tail callback renders | deadline reclaims | overshoots | driver overloads | stream errors |",
    "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|",
];

fn sub(end: u64, start: u64) -> u64 {
    end.saturating_sub(start)
}

fn delta(end: QueueSnapshot, start: QueueSnapshot) -> QueueSnapshot {
    QueueSnapshot {
        published: sub(end.published, start.published),
        claimed: sub(end.claimed, start.claimed),
        inline_pre: sub(end.inline_pre, start.inline_pre),
        unclaimed_at_deadline: sub(end.unclaimed_at_deadline, start.unclaimed_at_deadline),
        deadline_reclaimed_tail: sub(end.deadline_reclaimed_tail, start.deadline_reclaimed_tail),
        classified: sub(end.classified, start.classified),
        queue_over: sub(end.queue_over, start.queue_over),
        late_w: sub(end.late_w, start.late_w),
        late_after_queue: sub(end.late_after_queue, start.late_after_queue),
        forgone: sub(end.forgone, start.forgone),
        tail_callback: sub(end.tail_callback, start.tail_callback),
    }
}

fn check(problems: &mut Vec<String>, which: &str, m: Option<&Measurement>) {
    match m {
        None => problems.push(format!("no {which} snapshot")),
        Some(m) if !m.complete => problems.push(format!("the {which} snapshot is incomplete")),
        Some(m) if !m.counters.conserves() => problems.push(format!("the {which} snapshot does not conserve")),
        Some(_) => {}
    }
}

impl RunRow {
    /// The row between `start` and `end`. Invalid (with its reasons) when a snapshot is missing,
    /// incomplete or not conserving, when there are no render workers, or when the counters went
    /// backwards (the device reopened during the run).
    #[must_use]
    pub fn between(label: &str, path: TailPath, start: &RunEdge, end: &RunEdge) -> RunRow {
        let mut problems = Vec::new();
        check(&mut problems, "start", start.mailbox.as_ref());
        check(&mut problems, "end", end.mailbox.as_ref());
        let (s, e) = (&start.telemetry, &end.telemetry);
        if e.callbacks < s.callbacks {
            problems.push("the callback count went backwards (the device reopened during the run)".into());
        }
        let (Some(sf), Some(ef)) = (s.strip_faults, e.strip_faults) else {
            problems.push("no render workers".into());
            return RunRow::empty(label, path, e, problems);
        };
        let counters = match (start.mailbox, end.mailbox) {
            (Some(a), Some(b)) => delta(b.counters, a.counters),
            _ => QueueSnapshot::default(),
        };
        let overloads = match (s.backend, e.backend) {
            (Some(a), Some(b)) => Some(sub(b.overloads, a.overloads)),
            _ => None,
        };
        RunRow {
            label: label.to_owned(),
            path,
            frames: e.callback_frames,
            callbacks: sub(e.callbacks, s.callbacks),
            over_budget: sub(e.over_budget_callbacks, s.over_budget_callbacks),
            longest_ns: e.max_duration_ns,
            overrun_blocks: sub(ef.overrun_blocks, sf.overrun_blocks),
            busy: sub(ef.overruns_busy, sf.overruns_busy),
            late: sub(ef.overruns_late, sf.overruns_late),
            unstarted: sub(ef.overruns_unstarted, sf.overruns_unstarted),
            aux_busy: sub(ef.aux_overruns_busy, sf.aux_overruns_busy),
            aux_late: sub(ef.aux_overruns_late, sf.aux_overruns_late),
            holes: sub(ef.aux_holes, sf.aux_holes),
            upstream_missed: sub(ef.upstream_missed, sf.upstream_missed),
            overshoots: sub(ef.deadline_overshoots, sf.deadline_overshoots),
            overloads,
            stream_errors: sub(e.backend_errors, s.backend_errors),
            counters,
            problems,
        }
    }

    fn empty(label: &str, path: TailPath, e: &CallbackTelemetry, problems: Vec<String>) -> RunRow {
        RunRow {
            label: label.to_owned(),
            path,
            frames: e.callback_frames,
            callbacks: 0,
            over_budget: 0,
            longest_ns: 0,
            overrun_blocks: 0,
            busy: 0,
            late: 0,
            unstarted: 0,
            aux_busy: 0,
            aux_late: 0,
            holes: 0,
            upstream_missed: 0,
            overshoots: 0,
            overloads: None,
            stream_errors: 0,
            counters: QueueSnapshot::default(),
            problems,
        }
    }

    /// No reason to refuse the row.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.problems.is_empty()
    }

    /// The slice 4 terms of the measurement page, in integers: (a) `unclaimed_at_deadline * 10000 >
    /// published`, (b) `late_w > 0 && 2 * late_after_queue >= late_w`, (c) `queue_over * 100 >
    /// published`.
    #[must_use]
    pub fn slice4(&self) -> (bool, bool, bool) {
        let c = &self.counters;
        let w = |v: u64| u128::from(v);
        (
            w(c.unclaimed_at_deadline) * 10_000 > w(c.published),
            c.late_w > 0 && 2 * w(c.late_after_queue) >= w(c.late_w),
            w(c.queue_over) * 100 > w(c.published),
        )
    }

    /// The row in the table's columns ([`TABLE_HEADER`]).
    #[must_use]
    pub fn table_row(&self) -> String {
        let overloads = self.overloads.map_or_else(|| "n/a".to_owned(), |n| n.to_string());
        format!(
            "| {} | {} | {} | {} | {} | {:.1} ms | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            self.label,
            self.path.name(),
            self.frames,
            self.callbacks,
            self.over_budget,
            self.longest_ns as f64 / 1e6,
            self.overrun_blocks,
            self.busy,
            self.late,
            self.unstarted,
            self.aux_busy,
            self.aux_late,
            self.holes,
            self.upstream_missed,
            self.counters.tail_callback,
            self.counters.deadline_reclaimed_tail,
            self.overshoots,
            overloads,
            self.stream_errors,
        )
    }

    /// The row's counters, conservation and slice 4 terms on one line, or why it is invalid.
    #[must_use]
    pub fn detail(&self) -> String {
        let name = format!("{} {}", self.label, self.path.name());
        if !self.valid() {
            return format!("{name}: invalid row ({}); make no decision from it", self.problems.join("; "));
        }
        let c = &self.counters;
        let (a, b, cc) = self.slice4();
        format!(
            "{name}: published {}, claimed {}, classified {}, pre-deadline reclaims {}, unclaimed at deadline {} (tail rendered {}), forgone {}, tail callback renders {}, queued over a quarter period {}, late completions {} (after a long queue {}); conserving at both edges; slice 4 terms a {a}, b {b}, c {cc}",
            c.published,
            c.claimed,
            c.classified,
            c.inline_pre,
            c.unclaimed_at_deadline,
            c.deadline_reclaimed_tail,
            c.forgone,
            c.tail_callback,
            c.queue_over,
            c.late_w,
            c.late_after_queue,
        )
    }
}

/// The acceptance check over the rows: for each label with a serial and a graph row (the latest
/// of each), `OB_graph + H_graph <= OB_serial`, the graph row with 0 driver overloads and 0
/// stream errors, both rows valid. Returns one line per pair and a verdict line. The maintainer's
/// listening pass at 64 frames is not a figure and stays his.
#[must_use]
pub fn acceptance(rows: &[RunRow]) -> Vec<String> {
    let mut labels: Vec<&str> = Vec::new();
    for r in rows {
        if !labels.contains(&r.label.as_str()) {
            labels.push(&r.label);
        }
    }
    let latest = |label: &str, path| rows.iter().rev().find(|r| r.label == label && r.path == path);
    let mut lines = Vec::new();
    let (mut pairs, mut failed) = (0, 0);
    for label in labels {
        let (Some(s), Some(g)) = (latest(label, TailPath::Serial), latest(label, TailPath::Graph)) else {
            lines.push(format!("{label}: no pair yet (a serial and a graph row are needed)"));
            continue;
        };
        pairs += 1;
        let mut why = Vec::new();
        if !s.valid() || !g.valid() {
            why.push("an invalid row".to_owned());
        }
        let graph_side = u128::from(g.over_budget) + u128::from(g.holes);
        if graph_side > u128::from(s.over_budget) {
            why.push(format!("OB graph {} + H {} = {graph_side} > OB serial {}", g.over_budget, g.holes, s.over_budget));
        }
        if g.overloads.unwrap_or(0) > 0 {
            why.push(format!("{} driver overloads on the graph row", g.overloads.unwrap_or(0)));
        }
        if g.stream_errors > 0 {
            why.push(format!("{} stream errors on the graph row", g.stream_errors));
        }
        if why.is_empty() {
            lines.push(format!("{label}: pass (OB graph {} + H {} = {graph_side} <= OB serial {})", g.over_budget, g.holes, s.over_budget));
        } else {
            failed += 1;
            lines.push(format!("{label}: fail ({})", why.join("; ")));
        }
    }
    lines.push(match (pairs, failed) {
        (0, _) => "Verdict: no pair measured yet".to_owned(),
        (n, 0) => format!("Verdict: {n} of {n} pairs pass the figures; the listening pass at 64 frames decides the rest"),
        (n, f) => format!("Verdict: not accepted ({f} of {n} pairs fail)"),
    });
    lines
}

/// The report section: the table, each row's detail, then the acceptance lines. Empty without
/// rows.
#[must_use]
pub fn report_lines(rows: &[RunRow]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<String> = TABLE_HEADER.iter().map(|&h| h.to_owned()).collect();
    lines.extend(rows.iter().map(RunRow::table_row));
    lines.extend(rows.iter().map(RunRow::detail));
    lines.extend(acceptance(rows));
    lines
}

#[cfg(test)]
mod tests;
