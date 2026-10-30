//! From the text a window draws to the help id of the figure in it: the Callback Telemetry rows
//! and the pieces of the Audio Devices status lines. Pure functions over strings, so a test can
//! walk every row the windows produce and check that each has an entry.

use super::Id;

/// The id of one Callback Telemetry row (`label: value`, as `telemetry::detail_rows_named` makes
/// them). `None` for a row with no figure of its own (the histogram total).
pub fn telemetry_row(row: &str) -> Option<Id> {
    const BY_PREFIX: &[(&str, Id)] = &[
        ("Load now (", Id::TelemetryLoadNow),
        ("Over-budget callbacks in that window:", Id::TelemetryRecentOverBudget),
        ("Peak callback load", Id::TelemetryPeakLoad),
        ("Over-budget callbacks (nominal):", Id::TelemetryOverBudget),
        ("Over-budget callbacks while", Id::TelemetryOverBudgetLoading),
        ("Render workers:", Id::TelemetryRenderWorkers),
        ("Strip deadline:", Id::TelemetryStripDeadline),
        ("Strip overruns, busy:", Id::TelemetryOverrunsBusy),
        ("Strip overruns, late:", Id::TelemetryOverrunsLate),
        ("Strip overruns, unstarted:", Id::TelemetryOverrunsUnstarted),
        ("Strip overruns, top:", Id::TelemetryStripTop),
        ("Strip skips, upstream missed:", Id::TelemetryUpstreamMissed),
        ("Blocks with a strip overrun:", Id::TelemetryOverrunBlocks),
        ("Strip deadline overshoots:", Id::TelemetryDeadlineOvershoots),
        ("Late strip results:", Id::TelemetryLateResults),
        ("Strips rendered inline:", Id::TelemetryInlineRenders),
        ("Inline renders refused, no history:", Id::TelemetryInlineRefusedNoHistory),
        ("Inline renders refused, worst case:", Id::TelemetryInlineRefusedWorstCase),
        ("Inline renders refused, recent overrun:", Id::TelemetryInlineRefusedRecentOverrun),
        ("Strips rendered on the callback:", Id::TelemetryCallbackRenders),
        ("Faulted strip renders:", Id::TelemetryFaulted),
        ("Suspect strip blocks:", Id::TelemetrySuspect),
        ("Deferred MIDI events:", Id::TelemetryMidiDeferred),
        ("Deferred MIDI overflows:", Id::TelemetryMidiDropped),
        ("Deferred MIDI silences:", Id::TelemetryMidiSilences),
        ("Render worker protocol faults:", Id::TelemetryProtocolFaults),
        ("Strip lateness, ", Id::TelemetryStripLateness),
        ("Worker wake, ", Id::TelemetryWorkerWake),
        ("Callbacks without a timing:", Id::TelemetryUntimedCallbacks),
        ("Timed callbacks:", Id::TelemetryTimedCallbacks),
        ("Callbacks:", Id::TelemetryCallbacks),
        ("Frames silenced by a render fault:", Id::TelemetryFaultSilence),
        ("Disk underruns:", Id::TelemetryDiskUnderruns),
        ("Frames:", Id::TelemetryFrames),
        ("Total work time", Id::TelemetryTotalWork),
        ("Longest callback", Id::TelemetryLongestCallback),
        ("Backend errors:", Id::TelemetryBackendErrors),
        ("Device-lost errors:", Id::TelemetryDeviceLost),
    ];
    if let Some((_, id)) = BY_PREFIX.iter().find(|(prefix, _)| row.starts_with(prefix)) {
        return Some(*id);
    }
    // A per-strip row: `Strip <track>: <plug-in>: overruns busy 1, late 0, ...`.
    (row.starts_with("Strip ") && row.contains("overruns busy")).then_some(Id::TelemetryStripRow)
}

/// A piece of one Audio Devices status line: its text (the pieces of a line join back to the whole
/// line) and the entry that explains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub id: Option<Id>,
}

fn segment(text: &str, id: Option<Id>) -> Segment {
    Segment { text: text.to_owned(), id }
}

/// The backend counters the Backend line names, by the word each starts with.
const BACKEND_FIGURES: &[(&str, Id)] = &[
    ("wakes", Id::AudioDevicesWakes),
    ("gap over period", Id::AudioDevicesGapOverPeriod),
    ("late", Id::AudioDevicesLate),
    ("service", Id::AudioDevicesService),
    ("skipped", Id::AudioDevicesSkipped),
    ("timeouts", Id::AudioDevicesTimeouts),
    ("min padding", Id::AudioDevicesMinPadding),
    ("max packets", Id::AudioDevicesMaxPackets),
    ("faults", Id::AudioDevicesFaults),
    ("overloads", Id::AudioDevicesOverloads),
    ("busy skips", Id::AudioDevicesBusySkips),
    ("position jumps", Id::AudioDevicesPositionJumps),
    ("resets", Id::AudioDevicesResets),
];

/// What a ` · `-separated piece of an `Output open` or `Input open` line is.
fn open_piece(piece: &str, first: bool) -> Option<Id> {
    if first {
        return Some(Id::AudioDevicesDevice);
    }
    let sample_format = || {
        let mut chars = piece.chars();
        matches!(chars.next(), Some('i' | 'u' | 'f')) && chars.as_str().chars().next().is_some_and(|c| c.is_ascii_digit())
    };
    Some(if piece.ends_with(" Hz") {
        Id::AudioDevicesRate
    } else if piece.ends_with(" ch") {
        Id::AudioDevicesChannels
    } else if piece.ends_with("-sample buffer") || piece == "default buffer" {
        Id::AudioDevicesBuffer
    } else if piece.ends_with(" ms out") || piece.ends_with(" ms in") {
        Id::AudioDevicesLatency
    } else if piece.starts_with("MMCSS") || piece == "raised priority" {
        Id::AudioDevicesPriority
    } else if sample_format() {
        Id::AudioDevicesSampleFormat
    } else {
        Id::AudioDevicesHost
    })
}

/// Split one status line of the Audio Devices card (`window::opened_lines`) into pieces that each
/// get a hover: the open lines by ` · `, the Backend lines by figure.
pub fn status_segments(line: &str) -> Vec<Segment> {
    for (prefix, id) in [("Output open: ", Id::AudioDevicesOutputOpen), ("Input open: ", Id::AudioDevicesInputOpen)] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let mut out = vec![segment(prefix, Some(id))];
            // `none` and `none yet (...)` are the label's own state.
            if rest.starts_with("none") {
                out.push(segment(rest, Some(id)));
                return out;
            }
            for (i, piece) in rest.split(" · ").enumerate() {
                let text = if i == 0 { piece.to_owned() } else { format!(" \u{b7} {piece}") };
                out.push(Segment { text, id: open_piece(piece, i == 0) });
            }
            return out;
        }
    }
    if line.starts_with("Backend") {
        return backend_segments(line);
    }
    let id = if line.starts_with("Requested") {
        Some(Id::AudioDevicesRequested)
    } else if line.starts_with("Recovery:") {
        Some(Id::AudioDevicesRecovery)
    } else {
        None
    };
    vec![segment(line, id)]
}

fn backend_segments(line: &str) -> Vec<Segment> {
    let Some(colon) = line.find(": ") else { return vec![segment(line, Some(Id::AudioDevicesBackend))] };
    let (prefix, body) = line.split_at(colon + 2);
    let mut out = vec![segment(prefix, Some(Id::AudioDevicesBackend))];
    // A chunk starts where a separator is followed by the name of a figure; the words between
    // (`p95 12 µs, max 30 µs`) stay with the figure before.
    let mut starts = vec![0usize];
    for (at, sep) in body.match_indices("; ").chain(body.match_indices(", ")) {
        let rest = &body[at + sep.len()..];
        if BACKEND_FIGURES.iter().any(|(word, _)| rest.starts_with(word)) {
            starts.push(at);
        }
    }
    starts.sort_unstable();
    starts.dedup();
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(body.len());
        let chunk = &body[*start..end];
        let word = chunk.trim_start_matches([';', ',', ' ']);
        let id = BACKEND_FIGURES.iter().find(|(w, _)| word.starts_with(w)).map(|(_, id)| *id);
        out.push(segment(chunk, id));
    }
    out
}
