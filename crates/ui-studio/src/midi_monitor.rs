//! Window > MIDI Monitor (#91, slice 1): every message from every open input port, routed or not,
//! and every message transit sends to an output port, in one scrolling list with filters, plus
//! the header LED that blinks on input.
//!
//! Fed by `Player::take_midi_monitor` (two fixed-size rings behind the ingress worker and the
//! output thread; contract in `docs/contributing/knowledge/midi-monitor.md`). This module only
//! runs on the UI thread: it decodes ring elements to text, keeps the last [`MAX_ROWS`] rows,
//! filters, highlights and copies. System Real-Time Clock never reaches the list unless "Show
//! clock pulses" is on; it is a per-port tempo readout instead.

use crate::StudioApp;
use crate::theme::{RADIUS, SPACE, TOKENS};
use egui::{Align2, Color32, Sense, pos2, vec2};
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use transit_playback::player::monitor::{Activity, Dir, MonitorEvent, NO_CHANNEL, PortReport, PortState};

/// Rows kept; the oldest is evicted past it.
pub const MAX_ROWS: usize = 5000;
/// Events taken from each ring per frame.
const PER_FRAME: usize = 1000;
/// "Copy last N lines".
pub const COPY_LINES: usize = 100;
/// Lines in "Copy report".
pub const REPORT_LINES: usize = 200;
const PORT_WIDTH: usize = 18;
/// An open input that sent nothing for this long says so.
pub const SILENT_SECS: f64 = 10.0;
/// The header LED fades out over this long.
pub const LED_DECAY_SECS: f64 = 0.12;
const SPOTLIGHT_SECS: f64 = 1.5;

/// What kind of message a row is, for the type chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Note,
    Cc,
    /// Pitch bend and both aftertouches.
    Touch,
    Program,
    SysEx,
    /// Clock, Start, Continue, Stop, Song Position, Active Sensing, Reset, MTC and the rest of
    /// the system messages.
    RealTime,
}

/// Which directions are listed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DirFilter {
    #[default]
    Both,
    In,
    Out,
}

/// The filters, kept with the window (saved with the preferences).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Filters {
    pub dir: DirFilter,
    /// Ports switched off in the device list; a port not named here is shown, so a new device
    /// appears without a click.
    pub hidden_ports: Vec<String>,
    /// Channels shown, bit 0 for channel 1. Full means no filter; any other value also hides
    /// messages that have no channel.
    pub channels: u16,
    pub notes: bool,
    pub cc: bool,
    pub touch: bool,
    pub program: bool,
    pub sysex: bool,
    pub real_time: bool,
    /// Matches the decoded text and the hex bytes, case-insensitively ("CC 74", "C3", "Start").
    pub text: String,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            dir: DirFilter::Both,
            hidden_ports: Vec::new(),
            channels: u16::MAX,
            notes: true,
            cc: true,
            touch: true,
            program: true,
            sysex: true,
            real_time: true,
            text: String::new(),
        }
    }
}

/// The window's saved state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub open: bool,
    pub filters: Filters,
    /// List Clock pulses as rows (they are always counted into the tempo readout).
    pub show_clock: bool,
    /// Follow: keep the view on the newest row.
    pub autoscroll: bool,
    /// The window's last size in points.
    pub size: [f32; 2],
}

impl Default for Prefs {
    fn default() -> Self {
        Self { open: false, filters: Filters::default(), show_clock: false, autoscroll: true, size: [820.0, 480.0] }
    }
}

/// One line of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Counts up for every row ever added, so a paused view can stop at one.
    pub seq: u64,
    pub host_ns: u64,
    /// Time sampled on ingestion: transport position while playing, UTC wall clock otherwise.
    pub time: String,
    pub dir: Dir,
    pub port: String,
    /// 0..=15, or [`NO_CHANNEL`].
    pub channel: u8,
    pub class: Class,
    pub text: String,
    pub raw: [u8; 64],
    pub raw_len: u8,
    /// The note or controller number, for the hover highlight.
    pub key: Option<u8>,
    /// Lower-case `text` and `hex`, for the text filter.
    search: String,
}

impl Row {
    /// The bytes as hex, `90 3C 64`.
    pub fn hex(&self) -> String {
        self.raw.iter().take(usize::from(self.raw_len)).map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }
}

/// The learn spotlight: "press a button on your controller and see what it sends".
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Learn {
    #[default]
    Off,
    /// Waiting for the next input message.
    Armed,
    Caught(Box<Row>),
}

/// Messages per second over about a second.
#[derive(Clone, Copy, Debug, Default)]
pub struct Rate {
    start: f64,
    count: u32,
    pub per_sec: f32,
}

impl Rate {
    pub fn add(&mut self, n: u32) {
        self.count += n;
    }

    /// Closes the window once a second has passed.
    pub fn tick(&mut self, now: f64) {
        let span = now - self.start;
        if span >= 1.0 {
            self.per_sec = self.count as f32 / span as f32;
            self.count = 0;
            self.start = now;
        }
    }
}

/// Runtime state (not saved).
#[derive(Default)]
pub struct State {
    pub rows: VecDeque<Row>,
    next_seq: u64,
    /// While paused, rows added after this sequence number are kept but not shown.
    pub paused_at: Option<u64>,
    names: HashMap<(bool, u16), String>,
    /// Port names in first-seen order: a port's dot color follows its place here.
    order: Vec<String>,
    /// The latest port list, with a header line each.
    pub ports: Vec<PortReport>,
    pub port_lines: Vec<String>,
    rates: HashMap<String, Rate>,
    first_seen: HashMap<String, f64>,
    last_seen: HashMap<String, f64>,
    pub dropped: (u64, u64),
    pub sysex_truncated: u64,
    pub learn: Learn,
    /// UI-only wall-clock snapshot in milliseconds since the Unix epoch.
    wall_clock_ms: Option<u64>,
    spotlight_at: Option<(u64, f64)>,
    spotlight_scroll: Option<u64>,
    drawn_spotlight: Option<u64>,
    /// The note or controller number under the pointer last frame.
    hover_key: Option<u8>,
    /// The text of the last copy (tests read it; the clipboard gets it too).
    pub last_copied: String,
    scratch: Vec<MonitorEvent>,
    /// Where the window was last drawn, for `ui.inspect`.
    pub window: crate::arrange::R4,
    pub led: Led,
    /// Where the header LED was last drawn.
    pub led_rect: crate::arrange::R4,
}

const CC_NAMES: [(u8, &str); 15] = [
    (0, "Bank Select"),
    (1, "Mod Wheel"),
    (7, "Volume"),
    (10, "Pan"),
    (11, "Expression"),
    (64, "Sustain"),
    (74, "Brightness"),
    (120, "All Sound Off"),
    (121, "Reset All Controllers"),
    (122, "Local Control"),
    (123, "All Notes Off"),
    (124, "Omni Off"),
    (125, "Omni On"),
    (126, "Mono On"),
    (127, "Poly On"),
];

/// The common name of a controller number, if it has one in the table.
pub fn cc_name(cc: u8) -> Option<&'static str> {
    CC_NAMES.iter().find(|(n, _)| *n == cc).map(|(_, name)| *name)
}

fn note(n: u8) -> String {
    format!("{} ({n})", transit_midi::note_name(n))
}

/// The class and the decoded text of one ring element. A note-on with velocity 0 is a note-off.
pub fn describe(e: &MonitorEvent) -> (Class, String) {
    let (d1, d2) = (e.data1, e.data2);
    match e.status {
        0x80..=0x8F => (Class::Note, format!("Note Off {} vel {d2}", note(d1))),
        0x90..=0x9F if d2 == 0 => (Class::Note, format!("Note Off {} vel 0", note(d1))),
        0x90..=0x9F => (Class::Note, format!("Note On {} vel {d2}", note(d1))),
        0xA0..=0xAF => (Class::Touch, format!("Poly Aftertouch {} {d2}", note(d1))),
        0xB0..=0xBF => (
            Class::Cc,
            match cc_name(d1) {
                Some(name) => format!("CC {d1} {name} {d2}"),
                None => format!("CC {d1} {d2}"),
            },
        ),
        0xC0..=0xCF => (Class::Program, format!("Program {d1}")),
        0xD0..=0xDF => (Class::Touch, format!("Channel Aftertouch {d1}")),
        0xE0..=0xEF => (Class::Touch, format!("Pitch Bend {:+}", (i32::from(d1) | (i32::from(d2) << 7)) - 8192)),
        0xF0 => (Class::SysEx, format!("SysEx {} bytes", e.sysex_len)),
        0xF1 => (Class::RealTime, "MTC Quarter Frame".into()),
        0xF2 => (Class::RealTime, format!("Song Position {}", i32::from(d1) | (i32::from(d2) << 7))),
        0xF3 => (Class::RealTime, format!("Song Select {d1}")),
        0xF6 => (Class::RealTime, "Tune Request".into()),
        0xF8 => (Class::RealTime, "Clock".into()),
        0xFA => (Class::RealTime, "Start".into()),
        0xFB => (Class::RealTime, "Continue".into()),
        0xFC => (Class::RealTime, "Stop".into()),
        0xFE => (Class::RealTime, "Active Sensing".into()),
        0xFF => (Class::RealTime, "System Reset".into()),
        s => (Class::RealTime, format!("Status {s:#04X} {d1} {d2}")),
    }
}

/// `m:ss.mmm` from a host time in nanoseconds.
pub fn time_text(host_ns: u64) -> String {
    let ms = host_ns / 1_000_000;
    format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

fn wall_time(ms: u64) -> String {
    format!("{:02}:{:02}:{:02}.{:03}Z", ms / 3_600_000 % 24, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn dir_text(dir: Dir) -> &'static str {
    match dir {
        Dir::In => "In",
        Dir::Out => "Out",
    }
}

fn channel_text(channel: u8) -> String {
    if channel == NO_CHANNEL { "--".into() } else { format!("{}", u32::from(channel) + 1) }
}

fn clip(port: &str) -> String {
    let n = port.chars().count();
    if n <= PORT_WIDTH { format!("{port:<PORT_WIDTH$}") } else { format!("{}…", port.chars().take(PORT_WIDTH - 1).collect::<String>()) }
}

/// A row as plain text (the copy buttons use it): time, direction, port, channel and message in
/// aligned columns.
pub fn line(r: &Row) -> String {
    format!("{:>10}  {:<3}  {}  {:>2}  {}", r.time, dir_text(r.dir), clip(&r.port), channel_text(r.channel), r.text)
}

/// Whether `r` passes `f`.
pub fn passes(f: &Filters, r: &Row) -> bool {
    let class = match r.class {
        Class::Note => f.notes,
        Class::Cc => f.cc,
        Class::Touch => f.touch,
        Class::Program => f.program,
        Class::SysEx => f.sysex,
        Class::RealTime => f.real_time,
    };
    let dir = match f.dir {
        DirFilter::Both => true,
        DirFilter::In => r.dir == Dir::In,
        DirFilter::Out => r.dir == Dir::Out,
    };
    let channel = f.channels == u16::MAX || (r.channel < 16 && f.channels & (1 << r.channel) != 0);
    let text = f.text.trim();
    class && dir && channel && !f.hidden_ports.contains(&r.port) && (text.is_empty() || r.search.contains(&text.to_lowercase()))
}

/// The plain-text copy of the last `n` rows that pass `f` (and that a pause does not hide),
/// oldest first.
pub fn copy_text(rows: &VecDeque<Row>, f: &Filters, upto: Option<u64>, n: usize) -> String {
    let mut shown: Vec<&Row> = rows.iter().rev().filter(|r| upto.is_none_or(|u| r.seq <= u) && passes(f, r)).take(n).collect();
    shown.reverse();
    shown.iter().map(|r| line(r)).collect::<Vec<_>>().join("\n")
}

/// One port's header line: state, tracks, message rate, Clock tempo, Active Sensing, silence.
/// `silent_secs` is how long an open input has sent nothing.
pub fn port_line(p: &PortReport, tracks: &[String], per_sec: f32, silent_secs: f64) -> String {
    let mut parts = vec![format!("{} ({})", p.name, dir_text(p.dir))];
    parts.push(
        match p.state {
            PortState::Open => "open",
            PortState::Failed => "closed: could not be opened (another program may hold it)",
            PortState::Missing => "closed: missing (unplugged?)",
        }
        .into(),
    );
    if p.dir == Dir::In && p.state == PortState::Open {
        parts.push(if tracks.is_empty() { "no track listens".into() } else { format!("tracks: {}", tracks.join(", ")) });
        parts.push(format!("{per_sec:.0} msg/s"));
        if let Some(bpm) = p.clock_bpm {
            parts.push(format!("Clock {bpm:.1} BPM"));
        }
        if p.sensing {
            parts.push("Active Sensing present".into());
        }
        if silent_secs >= SILENT_SECS {
            parts.push(format!("no data in {} s", silent_secs as u64));
        }
    }
    parts.join(" · ")
}

/// The text of "Copy report": ports, drop counts and the last [`REPORT_LINES`] lines.
pub fn report_text(port_lines: &[String], dropped: (u64, u64), rows: &VecDeque<Row>) -> String {
    let mut out = String::from("transit MIDI Monitor report\n\nPorts:\n");
    if port_lines.is_empty() {
        out.push_str("  (none open)\n");
    }
    for l in port_lines {
        out.push_str(&format!("  {l}\n"));
    }
    out.push_str(&format!("\nDropped events: {} in, {} out\n\nLast {REPORT_LINES} lines:\n", dropped.0, dropped.1));
    let skip = rows.len().saturating_sub(REPORT_LINES);
    for r in rows.iter().skip(skip) {
        out.push_str(&format!("{}  [{}]  host_ns={}\n", line(r), r.hex(), r.host_ns));
    }
    out
}

impl State {
    fn port_name(&self, dir: Dir, id: u16) -> String {
        self.names.get(&(dir == Dir::Out, id)).cloned().unwrap_or_else(|| format!("port {id}"))
    }

    fn learn_ports(&mut self, ports: &[PortReport]) {
        for p in ports {
            self.names.insert((p.dir == Dir::Out, p.id), p.name.clone());
            if !self.order.contains(&p.name) {
                self.order.push(p.name.clone());
            }
        }
        let missing: Vec<_> = self
            .ports
            .iter()
            .filter(|old| !ports.iter().any(|p| p.dir == old.dir && p.name == old.name))
            .cloned()
            .map(|mut p| {
                p.state = PortState::Missing;
                p.clock_bpm = None;
                p.sensing = false;
                p
            })
            .collect();
        self.ports = ports.to_vec();
        self.ports.extend(missing);
    }

    /// Adds one decoded ring element.
    pub fn push(&mut self, e: &MonitorEvent) {
        self.push_at(e, None);
    }

    /// `transport` is the frame snapshot in seconds, or `None` while stopped.
    fn push_at(&mut self, e: &MonitorEvent, transport: Option<f64>) {
        self.push_captured(e, transport, None);
    }

    fn push_captured(&mut self, e: &MonitorEvent, transport: Option<f64>, payload: Option<&transit_playback::player::monitor::SysEx>) {
        let (class, mut text) = describe(e);
        let raw_len = match e.status {
            0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 3,
            0xC0..=0xDF | 0xF1 | 0xF3 => 2,
            _ => 1,
        };
        let mut raw = [0; 64];
        raw[..3].copy_from_slice(&[e.status, e.data1, e.data2]);
        let mut raw_len = raw_len;
        if let Some(payload) = payload.filter(|p| e.status == 0xF0 && e.sysex_id != 0 && p.id == e.sysex_id && p.dir == e.dir) {
            raw = payload.bytes;
            raw_len = payload.len.min(64);
            if u32::from(raw_len) < e.sysex_len {
                text.push_str(&format!(" (first {raw_len} shown)"));
            }
        }
        let mut row = Row {
            seq: self.next_seq,
            host_ns: e.host_ns,
            time: transport.filter(|t| t.is_finite()).map_or_else(
                || self.wall_clock_ms.map_or_else(|| time_text(e.host_ns), wall_time),
                |t| format!("{}{}", if t < 0.0 { "-" } else { "" }, time_text((t.abs() * 1e9) as u64)),
            ),
            dir: e.dir,
            port: self.port_name(e.dir, e.port),
            channel: e.channel,
            class,
            text,
            raw,
            raw_len,
            key: matches!(class, Class::Note | Class::Cc).then_some(e.data1),
            search: String::new(),
        };
        row.search = format!("{} {}", row.text.to_lowercase(), row.hex().to_lowercase());
        self.next_seq += 1;
        if self.learn == Learn::Armed && e.dir == Dir::In {
            self.learn = Learn::Caught(Box::new(row.clone()));
        }
        self.rates.entry(row.port.clone()).or_default().add(1);
        if self.rows.len() == MAX_ROWS {
            self.rows.pop_front();
        }
        self.rows.push_back(row);
    }

    /// Starts the spotlight once, using the injected UI frame clock.
    fn update_spotlight(&mut self, now: f64) {
        if let Learn::Caught(row) = &self.learn {
            if self.spotlight_at.is_none_or(|(seq, _)| seq != row.seq) {
                self.spotlight_at = Some((row.seq, now));
                self.spotlight_scroll = Some(row.seq);
            }
        } else {
            self.spotlight_at = None;
            self.spotlight_scroll = None;
        }
    }

    fn spotlight_level(&self, seq: u64, now: f64) -> f32 {
        self.spotlight_at.filter(|(id, _)| *id == seq).map_or(0.0, |(_, start)| (1.0 - (now - start) / SPOTLIGHT_SECS).clamp(0.0, 1.0) as f32)
    }

    /// The port names seen so far (both directions), sorted, for the device list.
    pub fn port_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.names.values().cloned().collect();
        names.sort();
        names.dedup();
        names
    }

    /// The dot color of a port, stable for the session.
    pub fn port_color(&self, name: &str) -> Color32 {
        let i = self.order.iter().position(|n| n == name).unwrap_or(0);
        let hue = (i as f32 * 0.618_034).fract();
        egui::ecolor::Hsva::new(hue, 0.55, 0.9, 1.0).into()
    }

    /// Rows added after the pause point (events behind the view).
    pub fn behind(&self) -> u64 {
        self.paused_at.map_or(0, |p| self.next_seq.saturating_sub(p + 1))
    }

    /// Updates header lines and silence tracking once a frame. `track_name` resolves a track id.
    fn refresh_ports(&mut self, now: f64, track_name: &dyn Fn(u64) -> String) {
        let ports = self.ports.clone();
        let mut lines = Vec::new();
        for p in &ports {
            let rate = self.rates.entry(p.name.clone()).or_default();
            rate.tick(now);
            let per_sec = rate.per_sec;
            let busy = per_sec > 0.0 || p.clock_bpm.is_some() || p.sensing || self.rates.get(&p.name).is_some_and(|r| r.count > 0);
            self.first_seen.entry(p.name.clone()).or_insert(now);
            if busy {
                self.last_seen.insert(p.name.clone(), now);
            }
            let since = self.last_seen.get(&p.name).or_else(|| self.first_seen.get(&p.name)).copied().unwrap_or(now);
            let tracks: Vec<String> = p.tracks.iter().map(|t| track_name(*t)).collect();
            lines.push(port_line(p, &tracks, per_sec, now - since));
        }
        self.port_lines = lines;
    }
}

/// The header LED's decay: lit when the input activity counter moves, then fading.
#[derive(Clone, Debug, Default)]
pub struct Led {
    last: Option<u64>,
    lit_at: f64,
    /// The last message, for the tooltip.
    pub tip: (Activity, String),
}

impl Led {
    /// Feeds the counter at `now` (seconds) and returns the brightness, 0 to 1. The first
    /// reading only sets the baseline, so messages from before the window opened do not light it.
    pub fn level(&mut self, count: u64, now: f64) -> f32 {
        match self.last {
            Some(last) if last != count => self.lit_at = now,
            None => self.lit_at = f64::NEG_INFINITY,
            _ => {}
        }
        self.last = Some(count);
        (1.0 - (now - self.lit_at) / LED_DECAY_SECS).clamp(0.0, 1.0) as f32
    }
}

/// The tooltip line for the last message: `Keys: Note On C3 (60) vel 100`.
pub fn led_tip(a: &Activity, port: &str) -> String {
    if a.count == 0 {
        return "No MIDI input yet. Click to open the MIDI Monitor.".into();
    }
    let e = MonitorEvent::message(Dir::In, a.port, 0, &a.bytes);
    let (_, text) = describe(&e);
    format!("{}: {text}. Click to open the MIDI Monitor.", if port.is_empty() { "input" } else { port })
}

/// The round "MIDI" indicator at the right of the transport bar.
pub fn led(app: &mut StudioApp, ui: &mut egui::Ui) {
    let Some(player) = &app.player else { return };
    let (activity, name) = player.midi_activity();
    let now = ui.input(|i| i.time);
    let level = app.midi_monitor.led.level(activity.count, now);
    app.midi_monitor.led.tip = (activity, name);
    if level > 0.0 {
        ui.ctx().request_repaint();
    } else {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
    }
    let font = egui::FontId::proportional(11.0);
    let label = ui.painter().layout_no_wrap("MIDI".into(), font.clone(), TOKENS.text_dim);
    let (r, resp) = ui.allocate_exact_size(vec2(label.size().x + 28.0, 22.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, RADIUS, TOKENS.button_hover);
    }
    let c = pos2(r.min.x + 12.0, r.center().y);
    let lit = TOKENS.monitor;
    let fill = Color32::from_rgba_unmultiplied(lit.r(), lit.g(), lit.b(), (level * 255.0) as u8);
    p.circle_filled(c, 5.0, fill);
    p.circle_stroke(c, 5.0, egui::Stroke::new(1.0, if level > 0.0 { lit } else { TOKENS.line }));
    p.text(pos2(c.x + 11.0, r.center().y), Align2::LEFT_CENTER, "MIDI", font, TOKENS.text_dim);
    app.midi_monitor.led_rect = crate::arrange::r4(r);
    let tip = led_tip(&app.midi_monitor.led.tip.0, &app.midi_monitor.led.tip.1);
    if resp.on_hover_text(tip).clicked() {
        let _ = app.run("window.midi_monitor", json!({"value": true}));
    }
}

/// The inspect output.
pub fn inspect(app: &StudioApp) -> Value {
    let s = &app.midi_monitor;
    json!({
        "open": app.ui.midi_monitor.open,
        "rows": s.rows.len(),
        "paused": s.paused_at.is_some(),
        "ports": s.port_lines,
        "dropped": [s.dropped.0, s.dropped.1],
        "window": s.window,
        "led": s.led_rect,
        "learn": format!("{:?}", s.learn),
        "last_rows": s.rows.iter().rev().take(5).rev().map(line).collect::<Vec<_>>(),
    })
}

impl StudioApp {
    /// `window.midi_monitor {value?}`: open or close the window (without `value` it flips).
    pub(crate) fn toggle_midi_monitor(&mut self, value: Option<bool>) -> Value {
        let open = value.unwrap_or(!self.ui.midi_monitor.open);
        self.ui.midi_monitor.open = open;
        json!({"open": open})
    }

    /// Once a frame: switch the taps with the window and take what they hold.
    fn midi_monitor_feed(&mut self, now: f64) {
        let open = self.ui.midi_monitor.open;
        let clock = self.ui.midi_monitor.show_clock;
        let Some(player) = &self.player else {
            self.midi_monitor.learn_ports(&[]);
            self.midi_monitor.refresh_ports(now, &|id| format!("track {id}"));
            return;
        };
        let transport = player.is_playing().then(|| player.position() as f64 / f64::from(self.engine.session().sample_rate.hz()));
        let s = &mut self.midi_monitor;
        s.wall_clock_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().and_then(|d| u64::try_from(d.as_millis()).ok());
        // Every frame (two relaxed stores): an audio reconnect swaps in a Player whose monitor is off.
        player.set_midi_monitor(open, clock);
        if !open {
            return;
        }
        s.scratch.clear();
        let report = player.take_midi_monitor(&mut s.scratch, PER_FRAME);
        s.learn_ports(&report.ports);
        s.dropped = (report.dropped_in, report.dropped_out);
        s.sysex_truncated = report.sysex_truncated;
        let events = std::mem::take(&mut s.scratch);
        for e in &events {
            let payload = report.sysex.iter().find(|p| p.id == e.sysex_id && p.dir == e.dir);
            s.push_captured(e, transport, payload);
        }
        s.scratch = events;
        let session = self.engine.session();
        let name = |id: u64| session.tracks.iter().find(|t| t.id.0 == id).map_or_else(|| format!("track {id}"), |t| t.name.clone());
        self.midi_monitor.refresh_ports(now, &name);
        self.midi_monitor.port_lines.push(format!("SysEx truncated to 64 bytes: {}", report.sysex_truncated));
    }
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(12.0, 14.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
}

fn filter_bar(ui: &mut egui::Ui, prefs: &mut Prefs, s: &State) {
    ui.horizontal_wrapped(|ui| {
        let f = &mut prefs.filters;
        for (d, label) in [(DirFilter::Both, "In + Out"), (DirFilter::In, "In"), (DirFilter::Out, "Out")] {
            ui.selectable_value(&mut f.dir, d, label);
        }
        ui.separator();
        let names = s.port_names();
        let shown = names.iter().filter(|n| !f.hidden_ports.contains(n)).count();
        ui.menu_button(format!("Devices ({shown}/{})", names.len()), |ui| {
            if names.is_empty() {
                ui.label(egui::RichText::new("No ports yet").color(TOKENS.text_dim));
            }
            for n in &names {
                ui.horizontal(|ui| {
                    dot(ui, s.port_color(n));
                    let mut on = !f.hidden_ports.contains(n);
                    if ui.checkbox(&mut on, n).changed() {
                        f.hidden_ports.retain(|h| h != n);
                        if !on {
                            f.hidden_ports.push(n.clone());
                        }
                    }
                });
            }
        });
        let chans = if f.channels == u16::MAX { "all".to_string() } else { format!("{}/16", f.channels.count_ones()) };
        ui.menu_button(format!("Channels ({chans})"), |ui| {
            ui.horizontal(|ui| {
                if ui.button("All").clicked() {
                    f.channels = u16::MAX;
                }
                if ui.button("None").clicked() {
                    f.channels = 0;
                }
            });
            egui::Grid::new("midi_monitor_channels").show(ui, |ui| {
                for c in 0..16u16 {
                    let mut on = f.channels & (1 << c) != 0;
                    if ui.checkbox(&mut on, format!("{}", c + 1)).changed() {
                        f.channels = if on { f.channels | (1 << c) } else { f.channels & !(1 << c) };
                    }
                    if c % 4 == 3 {
                        ui.end_row();
                    }
                }
            });
        });
        ui.separator();
        for (flag, label) in [
            (&mut f.notes, "Notes"),
            (&mut f.cc, "CC"),
            (&mut f.touch, "Pitch/Touch"),
            (&mut f.program, "Program"),
            (&mut f.sysex, "SysEx"),
            (&mut f.real_time, "Real-time"),
        ] {
            ui.toggle_value(flag, label);
        }
        ui.add(egui::TextEdit::singleline(&mut f.text).hint_text("Filter text: CC 74, C3, Start").desired_width(170.0));
    });
}

fn row_ui(ui: &mut egui::Ui, r: &Row, s: &State, mono: &egui::FontId, cw: f32, hover: &mut Option<u8>, height: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    if resp.hovered() {
        *hover = r.key;
    }
    let p = ui.painter();
    let flash = s.spotlight_level(r.seq, ui.input(|i| i.time));
    if flash > 0.0 {
        p.rect_filled(rect, 0.0, TOKENS.accent.gamma_multiply(0.5 * flash));
        ui.ctx().request_repaint();
    } else if r.key.is_some() && r.key == s.hover_key {
        p.rect_filled(rect, 0.0, TOKENS.accent.gamma_multiply(0.18));
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, TOKENS.button_hover);
    }
    let y = rect.center().y;
    let ink = if r.class == Class::RealTime { TOKENS.accent } else { TOKENS.text };
    let mut x = rect.min.x + 4.0;
    p.text(pos2(x, y), Align2::LEFT_CENTER, format!("{:>13}", r.time), mono.clone(), TOKENS.text_dim);
    x += cw * 15.0;
    let dir_ink = if r.dir == Dir::Out { TOKENS.record } else { TOKENS.monitor };
    let direction_icon = if r.dir == Dir::In { crate::icons::Icon::ChevronDown } else { crate::icons::Icon::OpenEditor };
    crate::icons::paint(p, pos2(x + 6.0, y), direction_icon, 12.0, dir_ink);
    p.text(pos2(x + 15.0, y), Align2::LEFT_CENTER, dir_text(r.dir), mono.clone(), dir_ink);
    x += cw * 6.0;
    p.circle_filled(pos2(x + 4.0, y), 4.0, s.port_color(&r.port));
    p.text(pos2(x + 14.0, y), Align2::LEFT_CENTER, clip(&r.port), mono.clone(), TOKENS.text);
    x += 14.0 + cw * (PORT_WIDTH as f32 + 1.0);
    p.text(pos2(x, y), Align2::LEFT_CENTER, format!("{:>2}", channel_text(r.channel)), mono.clone(), TOKENS.text_dim);
    x += cw * 4.0;
    p.text(pos2(x, y), Align2::LEFT_CENTER, &r.text, mono.clone(), ink);
    p.text(pos2(rect.max.x - 6.0, y), Align2::RIGHT_CENTER, r.hex(), mono.clone(), TOKENS.text_dim);
}

fn spotlight(ui: &mut egui::Ui, row: &Row, dismiss: &mut bool) {
    egui::Frame::NONE.fill(TOKENS.panel_raised).corner_radius(egui::CornerRadius::same(RADIUS as u8)).inner_margin(SPACE).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let ch = if row.channel == NO_CHANNEL { String::new() } else { format!("  ·  Channel {}", channel_text(row.channel)) };
                ui.label(egui::RichText::new(format!("{}{ch}", row.port)).size(22.0).strong().color(TOKENS.accent));
                ui.label(egui::RichText::new(&row.text).size(18.0).color(TOKENS.text));
                ui.label(egui::RichText::new(row.hex()).size(12.0).color(TOKENS.text_dim));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| *dismiss = ui.button("Dismiss").clicked());
        });
    });
}

/// Draws the window (and feeds it) once a frame.
pub fn window(app: &mut StudioApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    app.midi_monitor_feed(now);
    app.midi_monitor.window = [0.0; 4];
    app.midi_monitor.update_spotlight(now);
    if !app.ui.midi_monitor.open {
        return;
    }
    let mut open = true;
    let size = app.ui.midi_monitor.size;
    let mut prefs = app.ui.midi_monitor.clone();
    let (mut clear, mut copy, mut copy_report, mut dismiss) = (false, false, false, false);
    let mut pause = app.midi_monitor.paused_at.is_some();
    let mut learn = app.midi_monitor.learn != Learn::Off;
    let has_player = app.player.is_some();
    let mut hover = None;
    let mut drawn_spotlight = None;
    let r = egui::Window::new("MIDI Monitor")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(size)
        .default_pos(egui::pos2(ctx.content_rect().center().x - size[0] / 2.0, crate::theme::TRANSPORT_H + 60.0))
        .show(ctx, |ui| {
            let s = &app.midi_monitor;
            if !has_player {
                ui.label(egui::RichText::new("No audio engine is running (started with --no-audio), so no MIDI ports are open.").color(TOKENS.text_dim));
            } else if s.ports.iter().all(|p| p.dir == Dir::Out) {
                ui.label(
                    egui::RichText::new(
                        "No MIDI input ports found. Connect a controller: while this window is open transit opens every input port it sees and lists it here.",
                    )
                    .color(TOKENS.text_dim),
                );
            }
            for (p, text) in s.ports.iter().zip(&s.port_lines) {
                ui.horizontal(|ui| {
                    dot(ui, s.port_color(&p.name));
                    let bad = p.state != PortState::Open || text.contains("no data in");
                    ui.label(egui::RichText::new(text).size(11.5).color(if bad { TOKENS.mute } else { TOKENS.text_dim }));
                });
            }
            ui.separator();
            filter_bar(ui, &mut prefs, s);
            ui.horizontal_wrapped(|ui| {
                ui.toggle_value(&mut pause, "Pause");
                clear = ui.button("Clear").clicked();
                ui.toggle_value(&mut prefs.autoscroll, "Follow");
                ui.toggle_value(&mut learn, "Learn").on_hover_text("Press a button on your controller: the next message is shown here in large type.");
                copy = ui.button("Copy last 100 lines").clicked();
                copy_report = ui.button("Copy report").clicked();
                ui.checkbox(&mut prefs.show_clock, "Show clock pulses");
                let (din, dout) = s.dropped;
                if din + dout > 0 {
                    ui.label(egui::RichText::new(format!("Dropped events: {din} in, {dout} out")).color(TOKENS.record))
                        .on_hover_text("The monitor's ring was full: events were skipped, not delayed.");
                } else {
                    ui.label(egui::RichText::new("No dropped events").color(TOKENS.text_dim));
                }
                if s.sysex_truncated > 0 {
                    ui.label(format!("SysEx truncated: {}", s.sysex_truncated))
                        .on_hover_text("Messages longer than 64 bytes retain their total length and first 64 bytes.");
                }
                if s.paused_at.is_some() {
                    ui.label(egui::RichText::new(format!("Paused: {} new events behind the view", s.behind())).color(TOKENS.mute));
                }
            });
            match &s.learn {
                Learn::Armed => {
                    ui.label(egui::RichText::new("Waiting for a MIDI message… press a button or move a control.").size(16.0).color(TOKENS.accent));
                }
                Learn::Caught(row) => spotlight(ui, row, &mut dismiss),
                Learn::Off => {}
            }
            ui.separator();
            let mono = crate::theme::mono(ui.ctx(), 12.0);
            let cw = ui.painter().layout_no_wrap("0".into(), mono.clone(), TOKENS.text).size().x;
            let head = format!("{:>13}  {:<5} {:<w$}  {:>2}  Message", "Time", "Dir", "  Port", "Ch", w = PORT_WIDTH + 2);
            ui.label(egui::RichText::new(head).font(mono.clone()).strong().color(TOKENS.text_dim))
                .on_hover_text("Transport position while playing; UTC wall clock (Z) while stopped. Times are sampled as rows arrive.");
            let upto = s.paused_at;
            let caught = match &s.learn { Learn::Caught(row) => Some(row.seq), _ => None };
            let shown: Vec<&Row> = s.rows.iter().filter(|r| caught == Some(r.seq) || (upto.is_none_or(|u| r.seq <= u) && passes(&prefs.filters, r))).collect();
            let height = ui.text_style_height(&egui::TextStyle::Monospace).max(14.0) + 3.0;
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut scroll = egui::ScrollArea::both().auto_shrink([false, false]).stick_to_bottom(prefs.autoscroll && caught.is_none());
            if let Some(index) = s.spotlight_scroll.and_then(|seq| shown.iter().position(|r| r.seq == seq)) {
                scroll = scroll.vertical_scroll_offset(index as f32 * height);
            }
            let content_width = shown.iter().map(|r| (usize::from(r.raw_len) * 3 + r.text.chars().count() + 53) as f32 * cw).fold(0.0, f32::max);
            scroll.show_rows(ui, height, shown.len(), |ui, range| {
                ui.set_min_width(content_width);
                ui.spacing_mut().item_spacing.y = 0.0;
                for r in shown.get(range).into_iter().flatten() {
                    row_ui(ui, r, s, &mono, cw, &mut hover, height);
                    if caught == Some(r.seq) { drawn_spotlight = Some(r.seq); }
                }
            });
        });
    if let Some(r) = r {
        app.midi_monitor.window = crate::arrange::r4(r.response.rect);
        prefs.size = [r.response.rect.width(), r.response.rect.height()];
    }
    let s = &mut app.midi_monitor;
    s.hover_key = hover;
    s.drawn_spotlight = drawn_spotlight;
    s.spotlight_scroll = None;
    if clear {
        s.rows.clear();
        s.paused_at = s.paused_at.map(|_| s.next_seq.saturating_sub(1));
    }
    if copy {
        s.last_copied = copy_text(&s.rows, &prefs.filters, s.paused_at, COPY_LINES);
        ctx.copy_text(s.last_copied.clone());
    }
    if copy_report {
        s.last_copied = report_text(&s.port_lines, s.dropped, &s.rows);
        ctx.copy_text(s.last_copied.clone());
    }
    if pause != s.paused_at.is_some() {
        s.paused_at = pause.then(|| s.next_seq.saturating_sub(1));
    }
    if dismiss {
        s.learn = Learn::Off;
    } else if learn != (s.learn != Learn::Off) {
        s.learn = if learn { Learn::Armed } else { Learn::Off };
    }
    prefs.open = open;
    app.ui.midi_monitor = prefs;
}

#[cfg(test)]
mod tests;
