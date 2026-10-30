//! The output-callback telemetry readout in the transport bar, and its details window.
//!
//! Fed by `Player::callback_telemetry()` (a handful of relaxed atomic loads), read at most
//! [`POLL_SECS`] apart, never per frame. The readout is absent without a player (`--no-audio`).
//! The snapshot is approximate: fields are loaded independently, so nothing here derives an exact
//! average or rate from it, and every cross-field subtraction saturates. The teardown-only
//! counters (`contention_silence_frames`, `unavailable_silence_frames`) are never shown.

use crate::StudioApp;

use crate::theme::{SPACE, TOKENS};
use transit_playback::telemetry::{CallbackTelemetry, DURATION_LIMITS_NS, StatsBackend, StreamStats, WorstCallback};

/// Minimum time between two reads of the counters (about four a second).
pub const POLL_SECS: f64 = 0.25;

/// How long the readout keeps its overload color after an over-budget callback.
pub const OVERLOAD_HOLD_SECS: f64 = 2.0;

/// What "nominal" means, and where the rest is explained: the catalog's wording (#178), so the
/// window, the Info panel and docs/troubleshooting.md say the same.
pub fn explain() -> String {
    let nominal = crate::help::entry(crate::help::Id::TelemetryNominal).map_or("", |e| e.text.as_str());
    format!("{nominal}\nHover a figure, or press ? (Info panel), for what it means and what a healthy value is.")
}

/// Space the details window keeps under its scrolling rows: title bar, button row, frame margins
/// and a gap above the bottom edge.
const SCROLL_RESERVE: f32 = 176.0;

/// The least height the rows get on a very short window.
const MIN_SCROLL_H: f32 = 80.0;

/// Where the readout and its window were last drawn, for `ui.inspect` and tests.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TelemetryLayout {
    /// The readout is drawn (there is a player, or a test snapshot).
    pub shown: bool,
    pub text: String,
    pub rect: crate::arrange::R4,
    /// The warning icon is drawn (backend or device-loss errors), with its count.
    pub warning: Option<u64>,
    /// How loaded the readout looks (its color): ok, warning or alert.
    pub level: crate::load_readout::Level,
    pub open: bool,
    pub window: crate::arrange::R4,
    /// The window's lines: histogram rows, then raw counts, as `label: value`.
    pub rows: Vec<String>,
    /// The details window's "Reset peak" button.
    pub reset_peak: crate::arrange::R4,
    /// The details window's "Copy diagnostics" button.
    pub copy_diagnostics: crate::arrange::R4,
}

#[derive(Default)]
pub struct TelemetryState {
    pub open: bool,
    pub snap: CallbackTelemetry,
    last_read: Option<f64>,
    /// How many times the counters were read (tests check it follows the poll interval).
    pub reads: u64,
    /// A snapshot standing in for a player, so tests can drive the readout without audio.
    pub injected: Option<CallbackTelemetry>,
    /// Host name, callback frames and rate behind the readout, from the latest read.
    pub host: String,
    pub frames: u32,
    pub rate: u32,
    /// Time (egui seconds) until which the readout keeps flashing its overload color.
    pub alert_until: f64,
    /// Injected host name for tests, next to `injected`.
    pub injected_host: Option<String>,
    /// Peak resets requested (the details window button).
    pub peak_resets: u64,
    /// Audio takes recorded without a validated device clock (aligned approximately) this run.
    pub approximate_takes: u64,
    pub layout: TelemetryLayout,
}

impl TelemetryState {
    /// Whether a read is due at `now` (seconds); records it when so.
    fn due(&mut self, now: f64) -> bool {
        if self.last_read.is_some_and(|t| now >= t && now - t < POLL_SECS) {
            return false;
        }
        self.last_read = Some(now);
        self.reads += 1;
        true
    }
}

/// `12.3%` from millionths (1_000_000 = 100%), in integers: no rate is derived from other fields.
pub fn load_text(ppm: u64) -> String {
    let tenths = ppm / 1000;
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// The error count behind the warning icon: CPAL error callbacks (device loss is a subset of them,
/// so the larger of the two counters is the count), if any.
pub fn error_count(t: &CallbackTelemetry) -> Option<u64> {
    let n = t.backend_errors.max(t.device_lost_errors);
    (n > 0).then_some(n)
}

/// Histogram bin labels, from [`DURATION_LIMITS_NS`] and the open last bin.
pub fn bin_label(i: usize) -> String {
    limit_label(&DURATION_LIMITS_NS, i)
}

/// A histogram bin's label (`up to 50 µs`, `over 2 ms`) from its upper limits, nanoseconds, and
/// the open last bin.
fn limit_label(limits: &[u64], i: usize) -> String {
    let unit = |ns: u64| {
        if ns >= 1_000_000 { format!("{} ms", ns as f64 / 1_000_000.0) } else { format!("{} \u{b5}s", ns as f64 / 1_000.0) }
    };
    match (limits.get(i), i.checked_sub(1).and_then(|p| limits.get(p))) {
        (Some(hi), _) => format!("up to {}", unit(*hi)),
        (None, Some(lo)) => format!("over {}", unit(*lo)),
        (None, None) => String::new(),
    }
}

/// The details window's lines (`label: value`): the histogram, then the raw counts. Nothing is
/// derived but a saturating "without a timing" and a saturating histogram total. Render worker
/// strips are named by index; the report names them ([`detail_rows_named`]).
pub fn detail_rows(t: &CallbackTelemetry) -> Vec<String> {
    detail_rows_named(t, &[])
}

/// [`detail_rows`] with the session's strips by track index, for the render worker figures that
/// name a strip (#169).
pub fn detail_rows_named(t: &CallbackTelemetry, strips: &[StripName]) -> Vec<String> {
    let mut rows: Vec<String> = t.duration_histogram.iter().enumerate().map(|(i, n)| format!("{}: {n}", bin_label(i))).collect();
    rows.push(format!("Histogram total: {}", t.duration_histogram.iter().fold(0u64, |a, n| a.saturating_add(*n))));
    rows.extend([
        format!("Load now (about the last second): {}", load_text(t.current_load_ppm)),
        format!("Over-budget callbacks in that window: {}", t.recent_over_budget),
        format!("Peak callback load (since open): {}", load_text(t.max_load_ppm)),
        format!("Over-budget callbacks (nominal): {}", t.over_budget_callbacks),
        format!("Over-budget callbacks while a plug-in was loading: {}", t.over_budget_while_creating),
    ]);
    // Beside the late-block figures: the render workers' stream faults (ADR 0029), when the
    // engine has workers (the default; `TRANSIT_RENDER_POOL=0` renders serially). The
    // upstream-missed total row is there whenever the workers are; a keyed plan renders through
    // the task graph by default (#174), so it counts without a switch, and a strip's row gains its
    // `upstream missed` suffix only for a nonzero count (`TRANSIT_RENDER_GRAPH=0` keeps keyed
    // plans serial and the total at 0). Each name is defined once in docs/help/telemetry.toml.
    if let Some(f) = &t.strip_faults {
        rows.extend(strip_fault_rows(f));
        rows.extend(strip_figure_rows(f, strips));
    }
    rows.extend([
        format!("Callbacks: {}", t.callbacks),
        format!("Timed callbacks: {}", t.timed_callbacks),
        format!("Callbacks without a timing: {}", t.callbacks.saturating_sub(t.timed_callbacks)),
        format!("Frames: {}", t.frames),
        format!("Total work time (ns): {}", t.total_duration_ns),
        format!("Longest callback (ns): {}", t.max_duration_ns),
        format!("Backend errors: {}", t.backend_errors),
        format!("Device-lost errors: {}", t.device_lost_errors),
        format!("Frames silenced by a render fault: {}", t.fault_silence_frames),
    ]);
    rows
}

/// What a strip index of the callback's phase marker names: a track, and the plug-in (or the
/// chain of them) it runs. Resolved from the session by the reader (#162); the callback only
/// stores the index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StripName {
    pub track: String,
    /// The instrument's display name, else the inserts', comma separated; `None` for a track with no plug-in.
    pub plugin: Option<String>,
}

fn ms_text(ns: u64) -> String {
    format!("{:.1} ms", ns as f64 / 1e6)
}

/// A strip as the phase marker names it: `<track>: <plug-in>`, the track alone without a
/// plug-in, `strip N` for an index the session no longer has.
fn strip_text(i: u32, strips: &[StripName]) -> String {
    match strips.get(i as usize) {
        Some(s) => match &s.plugin {
            Some(p) => format!("{}: {p}", s.track),
            None => s.track.clone(),
        },
        None => format!("strip {i}"),
    }
}

/// Where a longest callback was: `<track>: <plug-in> (strip render)` for a strip phase, the
/// phase's name otherwise. An index the session no longer has reads `strip N`.
fn where_text(w: &WorstCallback, strips: &[StripName], with_verdict: bool) -> String {
    let phase = w.phase.label();
    let verdict = w.verdict().filter(|_| with_verdict).map_or(String::new(), |v| format!(", {}", v.label()));
    match w.strip_index().filter(|_| w.phase.runs_strip()) {
        Some(i) => format!("{} ({phase}{verdict})", strip_text(i, strips)),
        None if verdict.is_empty() => phase.to_owned(),
        None => format!("{phase} ({})", verdict.trim_start_matches(", ")),
    }
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// The report's attribution lines for the longest callbacks (#162): the longest one with the
/// track, plug-in and phase it was in, the loading and editor flags and, for one over its
/// budget, `(preempted)`, `(stalled in one phase)` or `(slow)`; then the longest three. Empty before a timed callback.
pub fn longest_callback_lines(t: &CallbackTelemetry, strips: &[StripName]) -> Vec<String> {
    let Some(w) = t.worst_callback_snapshot() else { return Vec::new() };
    let mut line = format!(
        "Longest callback {} during {}; plug-in loading: {}; editor open: {}",
        ms_text(w.duration_ns),
        where_text(&w, strips, false),
        yes_no(w.plugin_loading()),
        yes_no(w.editor_open()),
    );
    if let Some(v) = w.verdict() {
        line.push_str(&format!(" ({})", v.label()));
    }
    let top: Vec<String> =
        t.worst.iter().filter(|w| w.duration_ns > 0).map(|w| format!("{} {}", ms_text(w.duration_ns), where_text(w, strips, true))).collect();
    vec![line, format!("Longest callbacks: {}", top.join("; "))]
}

/// The render workers' figures (ADR 0029), one name per figure, as `label: value`.
pub fn strip_fault_rows(f: &transit_mix::StripFaults) -> Vec<String> {
    vec![
        format!("Render workers: {}", f.workers),
        format!(
            "Strip deadline: {}",
            transit_mix::strip_deadline_text(f.deadline_percent, f.deadline_frames, f.deadline_rate, f.deadline_overridden)
        ),
        format!("Strip overruns, busy: {}", f.overruns_busy),
        format!("Strip overruns, late: {}", f.overruns_late),
        format!("Strip overruns, unstarted: {}", f.overruns_unstarted),
        format!("Strip skips, upstream missed: {}", f.upstream_missed),
        format!("Blocks with a strip overrun: {}", f.overrun_blocks),
        format!("Strip deadline overshoots: {}", f.deadline_overshoots),
        format!("Late strip results: {}", f.late_results),
        format!("Strips rendered inline: {}", f.inline_renders),
        format!("Inline renders refused, no history: {}", f.inline_refused_no_history),
        format!("Inline renders refused, worst case: {}", f.inline_refused_worst_case),
        format!("Inline renders refused, recent overrun: {}", f.inline_refused_recent_overrun),
        format!("Strips rendered on the callback: {}", f.callback_renders),
        format!("Faulted strip renders: {}", f.faulted),
        format!("Suspect strip blocks: {}", f.suspect),
        format!("Deferred MIDI events: {}", f.midi_deferred),
        format!("Deferred MIDI overflows: {}", f.midi_dropped),
        format!("Deferred MIDI silences: {}", f.midi_silences),
        format!("Render worker protocol faults: {}", f.conflicts),
    ]
}

/// The render workers' per-strip figures and histograms (#169), after [`strip_fault_rows`]: the
/// top three strips by overruns on one line, a row per strip with an overrun (the most first, at
/// most [`transit_mix::TOP_STRIPS`]), then the lateness of late results and the workers' wake
/// latency, one row per bucket. Strips are named as the phase marker names them.
pub fn strip_figure_rows(f: &transit_mix::StripFaults, strips: &[StripName]) -> Vec<String> {
    let listed: Vec<&transit_mix::StripFigure> = f.top_strips.iter().flatten().collect();
    // A strip listed only for its upstream misses (#174, the task graph: the default for keyed
    // plans) says so instead of a count of 0.
    let top: Vec<String> = listed
        .iter()
        .take(3)
        .map(|s| match (s.overruns(), s.upstream_missed) {
            (0, n) if n > 0 => format!("{} upstream missed {n}", strip_text(s.track, strips)),
            (own, _) => format!("{} {own}", strip_text(s.track, strips)),
        })
        .collect();
    let mut rows = vec![format!("Strip overruns, top: {}", if top.is_empty() { "none".to_owned() } else { top.join(", ") })];
    rows.extend(listed.iter().map(|s| {
        let mut row = format!(
            "Strip {}: overruns busy {}, late {}, unstarted {}; inline {}; cost {} \u{b5}s, worst {} \u{b5}s",
            strip_text(s.track, strips),
            s.overruns_busy,
            s.overruns_late,
            s.overruns_unstarted,
            s.inline_renders,
            s.cost_ns / 1_000,
            s.worst_ns / 1_000,
        );
        // #174: skipped because a key source missed; that overrun is counted at the source.
        if s.upstream_missed > 0 {
            row.push_str(&format!("; upstream missed {} (from {})", s.upstream_missed, strip_text(s.upstream_origin, strips)));
        }
        row
    }));
    let limits = &transit_mix::POOL_HISTOGRAM_LIMITS_NS;
    rows.extend(f.lateness.iter().enumerate().map(|(i, n)| format!("Strip lateness, {}: {n}", limit_label(limits, i))));
    rows.extend(f.wake.iter().enumerate().map(|(i, n)| format!("Worker wake, {}: {n}", limit_label(limits, i))));
    rows
}

/// The Audio Devices line for the device loop of a native stream, with the fields `backend` fills
/// (see [`StreamStats::line_for`]). `wakes_per_second` is the rate over the last panel samples;
/// `None` until there are two, and the `wakes` field is then left out (no dash).
pub fn backend_line(s: &StreamStats, capture: bool, wakes_per_second: Option<u64>, backend: StatsBackend) -> String {
    let wakes = wakes_per_second.map(|rate| format!("{rate}/s"));
    format!("Backend: {}", s.line_for(backend, capture, wakes.as_deref()))
}

/// The host shown in the load readout: the audio system, starred when a native backend serves it
/// (see `docs/contributing/knowledge/clocks.md`): native WASAPI with the mode it opened in
/// (`WASAPI* exclusive`, `WASAPI* shared`), native ASIO as `ASIO*` (it has one mode); cpal's
/// systems unstarred (`ASIO`, `WASAPI`).
pub(crate) fn host_name(player: &transit_playback::Player) -> String {
    native_host_name(&player.device_system, player.native_backend, player.device_exclusive)
}

/// [`host_name`] from its parts.
pub(crate) fn native_host_name(system: &str, native: bool, exclusive: bool) -> String {
    match (native, transit_playback::devices::is_asio_system(Some(system))) {
        (false, _) => system.to_owned(),
        (true, true) => format!("{system}*"),
        (true, false) => format!("{system}* {}", if exclusive { "exclusive" } else { "shared" }),
    }
}

/// Read the counters when due (about four times a second) and keep the overload hold running.
/// Returns false, with the readout and window closed, when there is no player and no test snapshot.
pub fn poll(app: &mut StudioApp, ctx: &egui::Context) -> bool {
    let now = ctx.input(|i| i.time);
    if app.player.is_none() && app.telemetry.injected.is_none() {
        app.telemetry.layout.shown = false;
        app.telemetry.layout.warning = None;
        app.telemetry.open = false;
        return false;
    }
    if app.telemetry.due(now) {
        let (snap, host, rate, buffer) = match (&app.player, app.telemetry.injected) {
            (Some(p), _) => (p.callback_telemetry(), host_name(p), p.device_rate, p.device_buffer.unwrap_or(0)),
            (None, Some(s)) => (s, app.telemetry.injected_host.clone().unwrap_or_default(), s.sample_rate, s.callback_frames),
            (None, None) => Default::default(),
        };
        let t = &mut app.telemetry;
        // A new over-budget callback, whether or not it is still inside the recent window.
        if snap.recent_over_budget > 0 || snap.over_budget_callbacks > t.snap.over_budget_callbacks {
            t.alert_until = now + OVERLOAD_HOLD_SECS;
        }
        t.snap = snap;
        t.host = host;
        // Before the first callback the device's own figures stand in.
        t.rate = if snap.sample_rate > 0 { snap.sample_rate } else { rate };
        t.frames = if snap.callback_frames > 0 { snap.callback_frames } else { buffer };
    }
    // Keep the poll alive on an idle UI.
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(POLL_SECS));
    true
}

/// The details window: the duration histogram as a text table and the raw counts.
pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    app.poll_diagnostics(ctx);
    app.telemetry.layout.window = [0.0; 4];
    app.telemetry.layout.rows.clear();
    if !app.telemetry.open {
        app.telemetry.layout.open = false;
        return;
    }
    let snap = app.telemetry.snap;
    let rows = detail_rows(&snap);
    let mut reset_clicked = false;
    let mut copy_clicked = false;
    let mut copy_rect = egui::Rect::NOTHING;
    let mut reset_rect = [0.0; 4];
    let mut open = true;
    let content = ctx.content_rect();
    let top = crate::theme::TRANSPORT_H + 40.0;
    // The rows scroll inside what is left under the title bar, so the buttons below stay on the
    // screen however many rows the render pool adds (#171).
    let scroll_h = (content.bottom() - top - SCROLL_RESERVE).max(MIN_SCROLL_H);
    let r = crate::theme::window(ctx, "Callback Telemetry")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .constrain(true)
        .default_pos(egui::pos2((content.right() - 560.0).max(content.left()), top))
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Hover a figure for what it means.").color(TOKENS.text_dim).size(11.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    crate::help::header_button(app, ui, crate::help::Id::TelemetryWindow)
                });
            });
            // Side by side, so the window stays short enough to sit under the transport bar.
            egui::ScrollArea::vertical().id_salt("telemetry_rows").max_height(scroll_h).auto_shrink([false, true]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        let heading = ui.label(egui::RichText::new("Callback work time (callbacks per bin)").strong().color(TOKENS.text));
                        crate::help::hover(ui, &heading, crate::help::Id::TelemetryHistogram);
                        egui::Grid::new("telemetry_hist").num_columns(2).spacing([2.0 * SPACE, SPACE / 2.0]).show(ui, |ui| {
                            for (i, n) in snap.duration_histogram.iter().enumerate() {
                                let a = ui.label(egui::RichText::new(bin_label(i)).color(TOKENS.text_dim));
                                let b = ui.label(egui::RichText::new(n.to_string()).font(crate::theme::mono(ui.ctx(), 12.0)).color(TOKENS.text));
                                crate::help::hover(ui, &a, crate::help::Id::TelemetryHistogram);
                                crate::help::hover(ui, &b, crate::help::Id::TelemetryHistogram);
                                ui.end_row();
                            }
                        });
                    });
                    ui.add_space(2.0 * SPACE);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Counts").strong().color(TOKENS.text));
                        egui::Grid::new("telemetry_counts").num_columns(2).spacing([2.0 * SPACE, SPACE / 2.0]).show(ui, |ui| {
                            for row in rows.iter().skip(snap.duration_histogram.len() + 1) {
                                let (k, v) = row.rsplit_once(": ").unwrap_or((row, ""));
                                let a = ui.label(egui::RichText::new(k).color(TOKENS.text_dim));
                                let b = ui.label(egui::RichText::new(v).font(crate::theme::mono(ui.ctx(), 12.0)).color(TOKENS.text));
                                if let Some(id) = crate::help::map::telemetry_row(row) {
                                    crate::help::hover(ui, &a, id);
                                    crate::help::hover(ui, &b, id);
                                }
                                ui.end_row();
                            }
                        });
                    });
                });
                ui.add_space(SPACE);
                ui.label(egui::RichText::new(explain()).color(TOKENS.text_dim).size(11.0));
            });
            ui.add_space(SPACE);
            ui.horizontal(|ui| {
                let reset = ui.button("Reset peak").on_hover_text("Start the since-open peak load and longest callback over");
                crate::help::hover(ui, &reset, crate::help::Id::TelemetryResetPeak);
                reset_rect = crate::arrange::r4(reset.rect);
                if reset.clicked() {
                    reset_clicked = true;
                }
                ui.label(egui::RichText::new(format!("Now {} (last second)", load_text(snap.current_load_ppm))).color(TOKENS.text_dim));
                (copy_rect, copy_clicked) = app.diagnostics_button(ui, crate::help::Id::TelemetryCopyDiagnostics);
            });
        });
    if let Some(r) = r {
        app.telemetry.layout.window = crate::arrange::r4(r.response.rect);
    }
    app.telemetry.layout.rows = rows;
    app.telemetry.layout.reset_peak = reset_rect;
    app.telemetry.layout.copy_diagnostics = crate::arrange::r4(copy_rect);
    if copy_clicked {
        app.request_diagnostics(ctx.input(|i| i.time));
    }
    if reset_clicked {
        app.reset_peak_load();
    }
    if !open {
        app.telemetry.open = false;
    }
    app.telemetry.layout.open = app.telemetry.open;
}

impl StudioApp {
    /// Start the since-open peak over: the audio side zeroes its maxima at its next block, and the
    /// figures shown drop at once.
    pub fn reset_peak_load(&mut self) {
        self.telemetry.peak_resets += 1;
        if let Some(p) = &self.player {
            p.reset_peak_load();
        }
        let clear = |t: &mut CallbackTelemetry| {
            t.max_load_ppm = 0;
            t.max_duration_ns = 0;
        };
        clear(&mut self.telemetry.snap);
        if let Some(s) = &mut self.telemetry.injected {
            clear(s);
        }
    }
}
