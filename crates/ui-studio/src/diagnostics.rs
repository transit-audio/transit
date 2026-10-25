//! The diagnostics report (#114): one plain-text block a tester attaches to an issue.
//!
//! The same text comes from the "Copy diagnostics" buttons (Audio Devices, Callback Telemetry) and
//! from `transit --report`. [`Report::render`] is a pure formatter over [`Report`]; [`Machine::probe`]
//! gathers the facts about this computer (OS, CPU, audio stack versions) and [`tail_log`] reads the
//! end of the log. Nothing here opens an audio stream: devices are only listed.
//!
//! Privacy: no session content is read. [`Redaction`] replaces the home folder with `~` and the
//! user name with `<user>` everywhere in the finished text, the log lines and the log folder path
//! included.

use soundcraft_playback::devices::{AudioConfig, AudioSystem};
use soundcraft_playback::telemetry::CallbackTelemetry;
use std::path::{Path, PathBuf};

/// How many log lines the report ends with.
pub const LOG_LINES: usize = 200;
/// The log file's name (the app's `logging::LOG_FILE`).
pub const LOG_FILE: &str = "transit.log";
/// The most of the log file's end that is read (a runaway log stays cheap).
const LOG_TAIL_BYTES: u64 = 512 * 1024;
/// A helper program (`pipewire --version`, ...) that has not answered by then is reported as such.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Facts about this computer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Machine {
    pub os: String,
    pub kernel: String,
    pub cpu: String,
    pub cores: usize,
    /// `name: value` lines about the audio stack of this OS.
    pub stack: Vec<(String, String)>,
}

impl Machine {
    /// Reads the OS, CPU and audio stack by asking the system (files and short-lived helper
    /// programs, each with a timeout). Never opens an audio device.
    pub fn probe() -> Machine {
        let cores = std::thread::available_parallelism().map_or(0, usize::from);
        #[allow(unused_mut)]
        let mut m = Machine { cores, ..Machine::default() };
        #[cfg(target_os = "linux")]
        {
            m.os = linux_os_name();
            m.kernel = read_trim("/proc/sys/kernel/osrelease");
            m.cpu = linux_cpu();
            m.stack = linux_stack();
        }
        #[cfg(target_os = "windows")]
        {
            m.os = run("cmd", &["/C", "ver"]).unwrap_or_else(|e| format!("Windows ({e})"));
            m.kernel = std::env::var("OS").unwrap_or_default();
            m.cpu = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default();
            m.stack = vec![
                ("WASAPI".into(), "Windows Audio Session API (always present)".into()),
                ("ASIO".into(), "installed drivers are listed under Audio devices".into()),
            ];
        }
        #[cfg(target_os = "macos")]
        {
            m.os = run("sw_vers", &["-productVersion"]).map_or_else(|e| format!("macOS ({e})"), |v| format!("macOS {v}"));
            m.kernel = run("uname", &["-r"]).unwrap_or_default();
            m.cpu = run("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_default();
            m.stack = vec![("CoreAudio".into(), "system audio (always present)".into())];
        }
        if m.os.is_empty() {
            m.os = std::env::consts::OS.into();
        }
        m
    }
}

#[cfg(target_os = "linux")]
fn read_trim(path: &str) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_owned()).unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn linux_os_name() -> String {
    let release = read_trim("/etc/os-release");
    release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map_or_else(|| "Linux".into(), |v| v.trim_matches('"').to_owned())
}

#[cfg(target_os = "linux")]
fn linux_cpu() -> String {
    let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    info.lines()
        .find(|l| l.starts_with("model name") || l.starts_with("Model") || l.starts_with("Hardware"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_owned())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn linux_stack() -> Vec<(String, String)> {
    let first = |text: String| text.lines().next().unwrap_or_default().to_owned();
    let show = |r: Result<String, String>| r.map_or_else(|e| format!("not available ({e})"), first);
    let mut stack = vec![("pipewire".into(), show(run("pipewire", &["--version"])))];
    // The server's quantum and rate: the lines of `pw-cli info 0` that decide the buffer.
    let pw_info = match run("pw-cli", &["info", "0"]) {
        Ok(text) => {
            let keep: Vec<&str> = text.lines().map(str::trim).filter(|l| l.contains("clock.") || l.contains("quantum")).collect();
            if keep.is_empty() { "no quantum or rate lines".into() } else { keep.join("; ") }
        }
        Err(e) => format!("not available ({e})"),
    };
    stack.push(("pw-cli info 0".into(), pw_info));
    stack.push(("jackd".into(), show(run("jackd", &["--version"]))));
    let alsa = read_trim("/proc/asound/version");
    stack.push(("ALSA".into(), if alsa.is_empty() { "no /proc/asound/version".into() } else { first(alsa) }));
    let cards: Vec<String> = read_trim("/proc/asound/cards").lines().filter(|l| l.contains(']')).map(|l| l.trim().to_owned()).collect();
    stack.push(("ALSA cards".into(), if cards.is_empty() { "none".into() } else { cards.join("; ") }));
    stack
}

/// Runs a helper program and returns its trimmed standard output, or why it gave none. A program
/// that has not finished within [`PROBE_TIMEOUT`] is abandoned (its thread ends with it).
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (program, args): (String, Vec<String>) = (program.into(), args.iter().map(|a| (*a).into()).collect());
    std::thread::Builder::new()
        .name("transit-diagnostics-probe".into())
        .spawn(move || {
            let out = std::process::Command::new(&program)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "not installed".to_owned() } else { e.to_string() });
            let _ = tx.send(out);
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(PROBE_TIMEOUT) {
        Ok(Ok(out)) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned()),
        Ok(Ok(out)) => Err(format!("exit {}", out.status.code().map_or("signal".into(), |c| c.to_string()))),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("no answer in time".into()),
    }
}

/// The audio systems this build can list without opening a stream: cpal's, native JACK, and the
/// native ASIO driver names (none probed).
pub fn machine_systems() -> Vec<AudioSystem> {
    use soundcraft_playback::devices;
    let mut systems = devices::cpal_audio_systems();
    systems.extend(devices::native_jack_system());
    if devices::NATIVE_ASIO_COMPILED {
        systems.retain(|s| s.name != devices::ASIO_SYSTEM);
        systems.extend(devices::native_asio_system(None));
    }
    systems
}

/// The end of the log: at most `lines` lines of the file, oldest first. A missing file is an empty
/// list.
pub fn tail_log(path: &Path, lines: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return Vec::new() };
    let len = file.metadata().map_or(0, |m| m.len());
    let start = len.saturating_sub(LOG_TAIL_BYTES);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut all: Vec<&str> = text.lines().collect();
    if start > 0 && !all.is_empty() {
        all.remove(0); // a cut-off first line
    }
    all[all.len().saturating_sub(lines)..].iter().map(|l| (*l).to_owned()).collect()
}

/// What the finished text hides: the home folder and the user name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redaction {
    pub home: Option<String>,
    pub user: Option<String>,
}

impl Redaction {
    /// From the environment of this process.
    pub fn from_env() -> Redaction {
        let var = |names: &[&str]| names.iter().find_map(|n| std::env::var(n).ok()).filter(|v| !v.trim().is_empty());
        Redaction { home: var(&["USERPROFILE", "HOME"]), user: var(&["USERNAME", "USER", "LOGNAME"]) }
    }

    /// `text` with the home folder (either slash direction) replaced by `~` and the user name by
    /// `<user>`, case-insensitively. A user name shorter than three characters is left alone: it
    /// would damage ordinary words.
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_owned();
        if let Some(home) = self.home.as_deref().map(|h| h.trim_end_matches(['/', '\\'])).filter(|h| h.len() >= 2) {
            out = replace_ci(&out, home, "~");
            out = replace_ci(&out, &home.replace('\\', "/"), "~");
            out = replace_ci(&out, &home.replace('/', "\\"), "~");
        }
        if let Some(user) = self.user.as_deref().map(str::trim).filter(|u| u.len() >= 3) {
            out = replace_ci(&out, user, "<user>");
        }
        out
    }
}

/// ASCII case-insensitive replace of every `needle`.
fn replace_ci(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    if !needle.is_ascii() {
        return text.replace(needle, with);
    }
    let (lower, needle) = (text.to_ascii_lowercase(), needle.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(i) = lower[at..].find(&needle) {
        out.push_str(&text[at..at + i]);
        out.push_str(with);
        at += i + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// Everything the report prints.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub version: String,
    /// `<short sha> <date>`, or empty when the build has none.
    pub build: String,
    pub machine: Machine,
    pub systems: Vec<AudioSystem>,
    pub settings: AudioConfig,
    /// The lines the Audio Devices window shows for what is open (output, input, backend, reopen,
    /// and the requested-versus-running note).
    pub opened: Vec<String>,
    /// Counters of the running output, or `None` when no stream is open.
    pub telemetry: Option<CallbackTelemetry>,
    pub log_dir: Option<PathBuf>,
    pub log_lines: Vec<String>,
}

fn or_unknown(s: &str) -> &str {
    if s.is_empty() { "unknown" } else { s }
}

impl Report {
    /// The plain text, redacted with `redaction`.
    pub fn render(&self, redaction: &Redaction) -> String {
        let mut o = String::new();
        let section = |o: &mut String, name: &str| {
            o.push_str("\n== ");
            o.push_str(name);
            o.push_str(" ==\n");
        };
        o.push_str("transit diagnostics report\n");
        section(&mut o, "transit");
        o.push_str(&format!("Version: {}\n", self.version));
        o.push_str(&format!("Build: {}\n", or_unknown(&self.build)));
        section(&mut o, "System");
        o.push_str(&format!("OS: {}\n", self.machine.os));
        o.push_str(&format!("Kernel: {}\n", or_unknown(&self.machine.kernel)));
        o.push_str(&format!("Architecture: {}\n", std::env::consts::ARCH));
        o.push_str(&format!("CPU: {}\n", or_unknown(&self.machine.cpu)));
        o.push_str(&format!("Cores: {}\n", self.machine.cores));
        section(&mut o, "Audio stack");
        for (name, value) in &self.machine.stack {
            o.push_str(&format!("{name}: {value}\n"));
        }
        section(&mut o, "Audio devices");
        if self.systems.is_empty() {
            o.push_str("No audio systems listed.\n");
        }
        for s in &self.systems {
            o.push_str(&format!("{}{}\n", s.name, if s.is_default { " (default system)" } else { "" }));
            for (label, devices) in [("Output", &s.outputs), ("Input", &s.inputs)] {
                if devices.is_empty() {
                    o.push_str(&format!("  {label}: none\n"));
                }
                for d in devices {
                    let rates: Vec<String> = d.sample_rates.iter().map(u32::to_string).collect();
                    let buffer = match (d.buffer_min, d.buffer_max) {
                        (Some(a), Some(b)) if b != u32::MAX => format!("{a}-{b}"),
                        _ => "not reported".into(),
                    };
                    o.push_str(&format!(
                        "  {label}: {}{} \u{b7} {} ch \u{b7} rates {} \u{b7} buffer {buffer}\n",
                        d.name,
                        if d.is_default { " (default)" } else { "" },
                        d.max_channels,
                        if rates.is_empty() { "none".into() } else { rates.join(" ") },
                    ));
                }
            }
            for d in &s.diagnostics {
                o.push_str(&format!("  Note: {d}\n"));
            }
        }
        section(&mut o, "Settings");
        let c = &self.settings;
        let some = |v: &Option<String>| v.clone().unwrap_or_else(|| "default".into());
        o.push_str(&format!("Audio system: {}\n", some(&c.system)));
        o.push_str(&format!("Backend: {:?}\n", c.backend));
        o.push_str(&format!("Output device: {}\n", some(&c.output)));
        o.push_str(&format!("Input device: {}\n", some(&c.input)));
        o.push_str(&format!("Sample rate: {}\n", c.sample_rate.map_or("default".into(), |r| format!("{r} Hz"))));
        o.push_str(&format!("Buffer size: {}\n", c.buffer_size.map_or("default".into(), |b| format!("{b} samples"))));
        o.push_str(&format!("Shared device (WASAPI): {}\n", c.share_device));
        section(&mut o, "Open devices");
        if self.opened.is_empty() {
            o.push_str("No stream is open (report mode opens none).\n");
        }
        for line in &self.opened {
            o.push_str(line);
            o.push('\n');
        }
        section(&mut o, "Callback telemetry");
        match &self.telemetry {
            Some(t) => {
                for row in crate::telemetry::detail_rows(t) {
                    o.push_str(&row);
                    o.push('\n');
                }
            }
            None => o.push_str("No output stream, so no counters.\n"),
        }
        section(&mut o, "Log");
        match &self.log_dir {
            Some(dir) => o.push_str(&format!("Log folder: {}\n", dir.display())),
            None => o.push_str("Log folder: none (preferences are off in this run)\n"),
        }
        o.push_str(&format!("Last {LOG_LINES} lines of {LOG_FILE}:\n"));
        if self.log_lines.is_empty() {
            o.push_str("(no log lines)\n");
        }
        for line in &self.log_lines {
            o.push_str(line);
            o.push('\n');
        }
        redaction.apply(&o)
    }
}

/// `transit-report-<YYYYMMDD-HHMM>.txt` for the given seconds since the Unix epoch (UTC).
pub fn report_file_name(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let rem = epoch_secs % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("transit-report-{year:04}{month:02}{day:02}-{:02}{:02}.txt", rem / 3600, rem % 3600 / 60)
}

/// The Copy diagnostics button's state: the collection in flight, and when "Copied" stops showing.
#[derive(Default)]
pub struct DiagState {
    pending: Option<std::sync::mpsc::Receiver<String>>,
    /// egui time until which the buttons say "Copied".
    copied_until: f64,
    /// The text last put on the clipboard (tests read it).
    pub copied: String,
    /// Stands in for [`Machine::probe`] so a test never runs helper programs.
    #[cfg(test)]
    pub machine: Option<Machine>,
}

/// How long "Copied" shows.
pub const COPIED_SECS: f64 = 2.0;

impl crate::StudioApp {
    /// Starts collecting the report for the clipboard. The helper programs run on a worker thread;
    /// [`Self::poll_diagnostics`] copies the text when it is ready. One collection at a time.
    pub(crate) fn request_diagnostics(&mut self, now: f64) {
        if self.diag.pending.is_some() {
            return;
        }
        let mut report = self.diagnostics_report(now);
        let redaction = Redaction::from_env();
        let log_file = report.log_dir.as_ref().map(|d| d.join(LOG_FILE));
        #[cfg(test)]
        if let Some(machine) = self.diag.machine.clone() {
            report.machine = machine;
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(report.render(&redaction));
            self.diag.pending = Some(rx);
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("transit-diagnostics".into()).spawn(move || {
            report.machine = Machine::probe();
            report.log_lines = log_file.map(|f| tail_log(&f, LOG_LINES)).unwrap_or_default();
            let _ = tx.send(report.render(&redaction));
        });
        match spawned {
            Ok(_) => self.diag.pending = Some(rx),
            Err(e) => self.status = format!("Diagnostics could not be collected: {e}"),
        }
    }

    /// Puts a finished report on the clipboard. Called every frame by both windows.
    pub(crate) fn poll_diagnostics(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.diag.pending else { return };
        match rx.try_recv() {
            Ok(text) => {
                ctx.copy_text(text.clone());
                self.diag.copied = text;
                self.diag.copied_until = ctx.input(|i| i.time) + COPIED_SECS;
                self.diag.pending = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(50)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.diag.pending = None;
                self.status = "Diagnostics could not be collected.".into();
            }
        }
    }

    /// Draws the "Copy diagnostics" button and, for two seconds after a copy, "Copied". Returns the
    /// button's rect (for the layout) and whether it was clicked.
    pub(crate) fn diagnostics_button(&self, ui: &mut egui::Ui) -> (egui::Rect, bool) {
        let now = ui.input(|i| i.time);
        ui.horizontal(|ui| {
            let b = ui
                .add_enabled(self.diag.pending.is_none(), egui::Button::new("Copy diagnostics"))
                .on_hover_text("Copy a report of this computer's audio setup, the device list and the log's last lines.");
            if now < self.diag.copied_until {
                ui.label(egui::RichText::new("Copied").color(crate::theme::TOKENS.text_dim).size(11.5));
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(self.diag.copied_until - now));
            }
            (b.rect, b.clicked())
        })
        .inner
    }

    /// Whether the buttons are saying "Copied" at `now`.
    pub fn diagnostics_copied(&self, now: f64) -> bool {
        now < self.diag.copied_until
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_playback::devices::DeviceInfo;

    fn fake() -> Report {
        let dev = |name: &str| DeviceInfo {
            name: name.into(),
            is_default: true,
            max_channels: 2,
            sample_rates: vec![44100, 48000],
            buffer_min: Some(32),
            buffer_max: Some(4096),
        };
        Report {
            version: "9.9.9".into(),
            build: "abc1234 2026-10-25".into(),
            machine: Machine {
                os: "Test OS 1".into(),
                kernel: "6.1.0".into(),
                cpu: "Fake CPU".into(),
                cores: 8,
                stack: vec![("pipewire".into(), "pipewire 1.0.0".into()), ("ALSA".into(), "Kernel 6.1".into())],
            },
            systems: vec![AudioSystem {
                name: "ALSA".into(),
                is_default: true,
                outputs: vec![dev("Speakers")],
                inputs: Vec::new(),
                diagnostics: vec!["driver X: busy".into()],
            }],
            settings: AudioConfig {
                system: Some("ALSA".into()),
                output: Some("Speakers".into()),
                sample_rate: Some(48000),
                buffer_size: Some(128),
                ..Default::default()
            },
            opened: vec![
                "Output open: Speakers \u{b7} 48000 Hz \u{b7} 2 ch \u{b7} 256-sample buffer".into(),
                "Requested 128 \u{b7} running at 256".into(),
            ],
            telemetry: Some(CallbackTelemetry { callbacks: 42, ..Default::default() }),
            log_dir: Some(PathBuf::from("/home/alice/.config/transit/logs")),
            log_lines: vec!["INFO opened /home/alice/Music/song.transit".into(), "WARN something".into()],
        }
    }

    #[test]
    fn report_has_every_section_and_the_window_lines() {
        let text = fake().render(&Redaction::default());
        for header in [
            "== transit ==",
            "== System ==",
            "== Audio stack ==",
            "== Audio devices ==",
            "== Settings ==",
            "== Open devices ==",
            "== Callback telemetry ==",
            "== Log ==",
        ] {
            assert!(text.contains(header), "missing {header}\n{text}");
        }
        for want in [
            "Version: 9.9.9",
            "Build: abc1234 2026-10-25",
            "Kernel: 6.1.0",
            "CPU: Fake CPU",
            "Cores: 8",
            "pipewire: pipewire 1.0.0",
            "Output: Speakers (default) \u{b7} 2 ch \u{b7} rates 44100 48000 \u{b7} buffer 32-4096",
            "Note: driver X: busy",
            "Audio system: ALSA",
            "Buffer size: 128 samples",
            "Output open: Speakers \u{b7} 48000 Hz \u{b7} 2 ch \u{b7} 256-sample buffer",
            "Requested 128 \u{b7} running at 256",
            "Callbacks: 42",
            "Log folder: /home/alice/.config/transit/logs",
            "WARN something",
        ] {
            assert!(text.contains(want), "missing {want:?}\n{text}");
        }
    }

    #[test]
    fn report_without_devices_or_stream_still_has_every_section() {
        let text = Report::default().render(&Redaction::default());
        assert!(text.contains("No audio systems listed."));
        assert!(text.contains("No output stream, so no counters."));
        assert!(text.contains("== Log =="));
    }

    #[test]
    fn redaction_hides_the_home_folder_and_user_name_everywhere() {
        let r = Redaction { home: Some("/home/alice".into()), user: Some("alice".into()) };
        let text = fake().render(&r);
        assert!(!text.to_lowercase().contains("alice"), "{text}");
        assert!(text.contains("Log folder: ~/.config/transit/logs"));
        assert!(text.contains("INFO opened ~/Music/song.transit"));
        let win = Redaction { home: Some(r"C:\Users\Alice".into()), user: Some("Alice".into()) };
        assert_eq!(win.apply(r"C:\Users\Alice\AppData and C:/users/alice/x by ALICE"), r"~\AppData and ~/x by <user>");
        let short = Redaction { home: None, user: Some("al".into()) };
        assert_eq!(short.apply("alone"), "alone", "a short name is not replaced");
    }

    #[test]
    fn log_tail_keeps_the_last_lines_in_order() {
        let dir = std::env::temp_dir().join(format!("transit-diag-{}-tail", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("transit.log");
        std::fs::write(&path, (0..300).map(|i| format!("line {i}\n")).collect::<String>()).expect("write");
        let tail = tail_log(&path, LOG_LINES);
        assert_eq!((tail.len(), tail[0].as_str(), tail[199].as_str()), (200, "line 100", "line 299"));
        assert!(tail_log(&dir.join("missing.log"), 5).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_file_name_is_the_utc_date_and_minute() {
        assert_eq!(report_file_name(0), "transit-report-19700101-0000.txt");
        assert_eq!(report_file_name(1_700_000_000), "transit-report-20231114-2213.txt");
    }
}
