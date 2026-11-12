//! The acceptance row's arithmetic and formatting, with injected figures only (no device).
#![allow(clippy::unwrap_used)]

use super::*;
use transit_mix::StripFaults;

fn counters(published: u64, claimed: u64, inline_pre: u64, unclaimed: u64) -> QueueSnapshot {
    QueueSnapshot {
        published,
        claimed,
        inline_pre,
        unclaimed_at_deadline: unclaimed,
        deadline_reclaimed_tail: unclaimed / 2,
        classified: claimed,
        tail_callback: published / 10,
        ..QueueSnapshot::default()
    }
}

fn edge(callbacks: u64, over_budget: u64, holes: u64, q: QueueSnapshot) -> RunEdge {
    let faults = StripFaults {
        workers: 8,
        overrun_blocks: holes * 2,
        overruns_busy: holes,
        overruns_late: holes + 1,
        aux_overruns_busy: holes,
        aux_holes: holes,
        ..StripFaults::default()
    };
    RunEdge {
        mailbox: Some(Measurement { request: callbacks, complete: true, counters: q }),
        telemetry: CallbackTelemetry {
            callbacks,
            over_budget_callbacks: over_budget,
            max_duration_ns: 2_100_000,
            callback_frames: 64,
            strip_faults: Some(faults),
            ..CallbackTelemetry::default()
        },
    }
}

/// A valid row is the end less the start for every figure; the longest callback is the end's.
#[test]
fn a_row_is_the_difference_of_its_two_edges() {
    let start = edge(1_000, 10, 2, counters(100, 60, 30, 10));
    let end = edge(481_000, 374, 6, counters(10_100, 6_060, 3_030, 1_010));
    let row = RunRow::between("64 idle", TailPath::Graph, &start, &end);
    assert!(row.valid(), "{:?}", row.problems);
    assert_eq!((row.callbacks, row.over_budget, row.holes, row.aux_busy, row.busy, row.late), (480_000, 364, 4, 4, 4, 4));
    assert_eq!(
        (row.counters.published, row.counters.claimed, row.counters.inline_pre, row.counters.unclaimed_at_deadline),
        (10_000, 6_000, 3_000, 1_000)
    );
    assert_eq!(row.counters.deadline_reclaimed_tail, 500);
    assert_eq!(row.overloads, None, "no backend stats: n/a");
    assert_eq!(row.table_row(), "| 64 idle | graph | 64 | 480000 | 364 | 2.1 ms | 8 | 4 | 4 | 0 | 4 | 0 | 4 | 0 | 1000 | 500 | 0 | n/a | 0 |");
    assert_eq!(TABLE_HEADER[0].matches('|').count(), row.table_row().matches('|').count(), "one cell per column");
    // a: 1000 * 10000 > 10000; b: no late completions; c: 0 * 100 > 10000 is false.
    assert_eq!(row.slice4(), (true, false, false));
    assert!(row.detail().starts_with("64 idle graph: published 10000, claimed 6000, classified 6000, pre-deadline reclaims 3000"));
}

/// A missing, incomplete or non-conserving snapshot makes the row invalid, and it says why.
#[test]
fn a_non_conserving_or_incomplete_snapshot_is_an_invalid_row() {
    let good = edge(1_000, 10, 0, counters(100, 60, 30, 10));
    let mut leaky = edge(2_000, 12, 0, counters(200, 60, 30, 10));
    let row = RunRow::between("64 idle", TailPath::Serial, &good, &leaky);
    assert_eq!(row.problems, ["the end snapshot does not conserve"]);
    assert!(row.detail().contains("invalid row (the end snapshot does not conserve); make no decision from it"));
    leaky.mailbox = Some(Measurement { complete: false, ..good.mailbox.unwrap() });
    assert_eq!(RunRow::between("x", TailPath::Serial, &good, &leaky).problems, ["the end snapshot is incomplete"]);
    leaky.mailbox = None;
    assert_eq!(RunRow::between("x", TailPath::Serial, &good, &leaky).problems, ["no end snapshot"]);
    let reopened = edge(10, 0, 0, counters(100, 60, 30, 10));
    assert!(RunRow::between("x", TailPath::Serial, &good, &reopened).problems[0].contains("went backwards"));
    let serial_only = RunEdge { telemetry: CallbackTelemetry { strip_faults: None, ..good.telemetry }, ..good };
    assert!(RunRow::between("x", TailPath::Serial, &good, &serial_only).problems.contains(&"no render workers".to_owned()));
}

/// Integer arithmetic of the slice 4 terms at their boundaries.
#[test]
fn the_slice_4_terms_at_their_boundaries() {
    let row = |q: QueueSnapshot| RunRow::between("r", TailPath::Graph, &edge(0, 0, 0, QueueSnapshot::default()), &edge(1, 0, 0, q));
    let base = QueueSnapshot { published: 10_000, claimed: 10_000, classified: 10_000, ..QueueSnapshot::default() };
    assert_eq!(row(base).slice4(), (false, false, false));
    assert!(!row(QueueSnapshot { unclaimed_at_deadline: 1, ..base }).slice4().0, "10000 > 10000 is false");
    assert!(row(QueueSnapshot { published: 9_999, unclaimed_at_deadline: 1, ..base }).slice4().0, "10000 > 9999");
    assert!(!row(QueueSnapshot { late_w: 3, late_after_queue: 1, ..base }).slice4().1, "2 >= 3 is false");
    assert!(row(QueueSnapshot { late_w: 2, late_after_queue: 1, ..base }).slice4().1, "2 >= 2");
    assert!(!row(QueueSnapshot { queue_over: 100, ..base }).slice4().2, "10000 > 10000 is false");
    assert!(row(QueueSnapshot { queue_over: 101, ..base }).slice4().2, "10100 > 10000");
}

/// The pair rule: `OB_graph + H_graph <= OB_serial`, with 0 overloads and stream errors on the
/// graph row, both rows valid; a tie passes.
#[test]
fn the_acceptance_rule_pairs_serial_and_graph_rows_by_label() {
    let mk = |label: &str, path, ob, h, errors| RunRow {
        over_budget: ob,
        holes: h,
        stream_errors: errors,
        ..RunRow::between(label, path, &edge(0, 0, 0, QueueSnapshot::default()), &edge(1, 0, 0, QueueSnapshot::default()))
    };
    let rows = [
        mk("64 idle", TailPath::Serial, 400, 0, 0),
        mk("64 idle", TailPath::Graph, 300, 100, 0),
        mk("128 idle", TailPath::Serial, 10, 0, 0),
        mk("128 idle", TailPath::Graph, 9, 2, 0),
        mk("64 compile", TailPath::Graph, 0, 0, 3),
        mk("64 compile", TailPath::Serial, 50, 0, 0),
    ];
    let lines = acceptance(&rows);
    assert_eq!(lines[0], "64 idle: pass (OB graph 300 + H 100 = 400 <= OB serial 400)");
    assert_eq!(lines[1], "128 idle: fail (OB graph 9 + H 2 = 11 > OB serial 10)");
    assert_eq!(lines[2], "64 compile: fail (3 stream errors on the graph row)");
    assert_eq!(lines[3], "Verdict: not accepted (2 of 3 pairs fail)");
    let pass = acceptance(&rows[..2]);
    assert_eq!(pass.last().unwrap(), "Verdict: 1 of 1 pairs pass the figures; the listening pass at 64 frames decides the rest");
    assert_eq!(acceptance(&rows[..1]), ["64 idle: no pair yet (a serial and a graph row are needed)", "Verdict: no pair measured yet"]);
    let mut invalid = rows[1].clone();
    invalid.problems.push("no start snapshot".into());
    assert_eq!(acceptance(&[rows[0].clone(), invalid])[0], "64 idle: fail (an invalid row)");
    let report = report_lines(&rows[..2]);
    assert_eq!(report.len(), 2 + 2 + 2 + 2, "header, rows, details, the pair and the verdict");
    assert!(report_lines(&[]).is_empty());
}
