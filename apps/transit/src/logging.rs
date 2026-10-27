// Copied from apps/soundcraft/src/logging.rs at transit 2af868e (file last changed 74926df); only the log file
// name and the default filter differ.
//! The desktop app's logger: `log::` records go to standard error and to a log file.
//!
//! Without a logger every `log::warn!`/`log::error!` in the workspace (the engine's panic report,
//! audio devices that failed to open or broke, plugins that refused their stored state or failed
//! to activate, CLAP plugins' own log messages, decode errors on import) vanished. A launch from a
//! desktop menu has no terminal, so the file is what a bug report can attach:
//! `<settings dir>/logs/soundcraft.log`, beside `ui.json`, `Autosave/` and `Presets/`. Each start
//! moves the previous log to `soundcraft.1.log` (and that one to `.2`), so the log of a run that
//! crashed survives the next launch. Runs without preferences (`SOUNDCRAFT_NO_PREFS`, agents' test
//! runs) log to standard error only, so they don't rotate away the user's own logs.
//!
//! Levels: `info` for SoundCraft's own crates, `warn` for everything else (wgpu and naga are
//! chatty). `RUST_LOG` replaces that with env_logger-style directives: `debug`,
//! `warn,transit_mix=trace`, `wgpu_core=info`. A directive ending in `*` matches every target
//! that starts with it (`soundcraft*=debug`).
//!
//! Choosing the filter (issue #84), strongest first: `--log <directives>` (the same syntax), then
//! `--verbose` (the default filter with debug for every soundcraft and transit target), then
//! `RUST_LOG`, then the default ([`resolve`]). Help > Debug Logging raises our targets to debug for
//! the rest of the session without a restart and puts them back ([`AppLogger::set_debug`]); the
//! filter the logger reads is the startup one plus that one atomic switch.
//!
//! Standard error gets a copy of every line when it is a terminal or `--log-stderr` is passed
//! ([`stderr_mirror`]); the file always gets them.
//!
//! Records logged before the settings directory is known are kept (up to [`MAX_PENDING`]) and
//! written once the file is attached. Writing never panics: a file that can't be created or
//! written leaves standard error as the only sink.
//!
//! Realtime audio: a record takes a lock and writes to standard error and a file, which the audio
//! callback must never do. Code that runs on the audio thread logs anyway (the cpal error callback,
//! the synth's full event queue, a hosted plugin's failed `process`, a CLAP plugin calling the
//! host's `log` from its audio thread, the panic hook). So the cpal callbacks mark their thread
//! (`transit_playback::mark_audio_thread`; the mix engine carries the mark into the worker
//! threads that render strips for it), and a record from a marked thread is not written
//! there: the first one is kept in a fixed buffer (`try_lock`, no allocation, never waits) and the
//! rest are only counted (an atomic). Another thread writes them out: the UI thread every frame
//! ([`AppLogger::report_audio_thread`]) and any other thread before its own next record.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use log::LevelFilter;

/// The current log file's name inside the log directory.
pub const LOG_FILE: &str = "transit.log";
/// The log directory inside the settings directory.
pub const LOG_DIR: &str = "logs";
/// How many previous logs are kept (`soundcraft.1.log` … `soundcraft.<KEEP>.log`).
pub const KEEP: usize = 2;
/// The log file stops growing past this size (a runaway warning can't fill the disk).
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Records kept in memory until the log file is attached.
pub const MAX_PENDING: usize = 512;
/// The built-in filter when `RUST_LOG` is unset or empty.
pub const DEFAULT_FILTER: &str = "warn,soundcraft*=info,transit*=info";
/// The targets `--verbose` and Help > Debug Logging raise to debug: every crate of ours.
const OWN_TARGET_PREFIXES: [&str; 2] = ["soundcraft", "transit"];
/// The longest message kept from the audio thread (longer ones are cut, marked with `…`).
pub const REALTIME_MESSAGE_BYTES: usize = 480;
/// The longest target kept from the audio thread.
const REALTIME_TARGET_BYTES: usize = 96;
/// The thread name shown for records from the audio thread.
const AUDIO_THREAD: &str = "audio";

/// Per-target level filter parsed from env_logger-style directives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    default: LevelFilter,
    directives: Vec<(String, LevelFilter)>,
}

impl Filter {
    /// Parse `spec`; unknown levels and empty parts are skipped. Without a bare level, targets no
    /// directive names log errors only.
    pub fn parse(spec: &str) -> Filter {
        let mut f = Filter { default: LevelFilter::Error, directives: Vec::new() };
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some((name, level)) => {
                    let name = name.trim();
                    if let (false, Some(level)) = (name.is_empty(), parse_level(level)) {
                        f.directives.push((name.to_owned(), level));
                    }
                }
                None => match parse_level(part) {
                    Some(level) => f.default = level,
                    // A bare target name: everything from it (env_logger does the same).
                    None => f.directives.push((part.to_owned(), LevelFilter::Trace)),
                },
            }
        }
        f
    }

    /// The level that applies to `target` (the most specific matching directive wins).
    pub fn level_for(&self, target: &str) -> LevelFilter {
        self.directives.iter().filter(|(name, _)| matches(name, target)).max_by_key(|(name, _)| name.len()).map_or(self.default, |(_, level)| *level)
    }

    /// The most verbose level any target can reach (for `log::set_max_level`).
    pub fn max(&self) -> LevelFilter {
        self.directives.iter().map(|(_, level)| *level).fold(self.default, Ord::max)
    }
}

/// Where the filter came from, for the line the log starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    LogFlag,
    VerboseFlag,
    Env,
    Default,
}

impl Source {
    pub fn describe(self) -> &'static str {
        match self {
            Source::LogFlag => "--log",
            Source::VerboseFlag => "--verbose",
            Source::Env => "RUST_LOG",
            Source::Default => "the default",
        }
    }
}

/// The logging flags of the command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogArgs {
    /// `--verbose`: debug for every soundcraft and transit target.
    pub verbose: bool,
    /// `--log <directives>` (or `--log=<directives>`).
    pub log: Option<String>,
    /// `--log-stderr`: copy log lines to standard error even when it is not a terminal.
    pub stderr: bool,
}

impl LogArgs {
    /// Read the logging flags out of `args`; other arguments are left to the caller.
    pub fn parse(args: &[String]) -> Result<LogArgs, String> {
        let mut out = LogArgs::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--verbose" => out.verbose = true,
                "--log-stderr" => out.stderr = true,
                "--log" => match it.next() {
                    Some(value) if !value.starts_with("--") => out.log = Some(value.clone()),
                    _ => return Err("--log requires directives such as debug or warn,transit_mix=trace".to_owned()),
                },
                other => {
                    if let Some(value) = other.strip_prefix("--log=") {
                        out.log = Some(value.to_owned());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Whether `args[i]` is the value of a `--log` flag, not a file to open.
    pub fn is_value(args: &[String], i: usize) -> bool {
        i > 0 && args.get(i - 1).is_some_and(|p| p == "--log")
    }
}

/// The filter this run starts with: the directives, where they came from, and whether our own
/// targets start at debug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub spec: String,
    pub source: Source,
    pub debug: bool,
}

/// Flag over environment over default. `--log` names the directives; `--verbose` keeps the default
/// ones and starts with debug on (so Help > Debug Logging can turn it off again); `env` is the
/// value of `RUST_LOG`, blank counting as unset.
pub fn resolve(args: &LogArgs, env: Option<&str>) -> Resolved {
    let env = env.map(str::trim).filter(|s| !s.is_empty());
    let (spec, source) = match (&args.log, args.verbose, env) {
        (Some(log), _, _) => (log.clone(), Source::LogFlag),
        (None, true, _) => (DEFAULT_FILTER.to_owned(), Source::VerboseFlag),
        (None, false, Some(env)) => (env.to_owned(), Source::Env),
        (None, false, None) => (DEFAULT_FILTER.to_owned(), Source::Default),
    };
    Resolved { spec, source, debug: args.verbose }
}

/// Lines go to standard error when it is a terminal, or when `--log-stderr` asks.
pub fn stderr_mirror(flag: bool, is_terminal: bool) -> bool {
    flag || is_terminal
}

fn is_own_target(target: &str) -> bool {
    OWN_TARGET_PREFIXES.iter().any(|p| target.starts_with(p))
}

fn parse_level(s: &str) -> Option<LevelFilter> {
    s.trim().parse().ok()
}

/// `name` matches `target` itself and its submodules (`a` matches `a` and `a::b`, not `ab`);
/// `name*` matches every target starting with `name`.
fn matches(name: &str, target: &str) -> bool {
    match name.strip_suffix('*') {
        Some(prefix) => target.starts_with(prefix),
        None => target.strip_prefix(name).is_some_and(|rest| rest.is_empty() || rest.starts_with("::")),
    }
}

/// `2026-10-08T07:59:17.728Z` (UTC, milliseconds). A clock before 1970 reads as the epoch.
pub fn timestamp(t: SystemTime) -> String {
    let since = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, rem % 3600 / 60, rem % 60, since.subsec_millis())
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day) (Howard Hinnant's
/// `civil_from_days`, unsigned because `timestamp` clamps at the epoch; saturating so an absurd
/// clock can't overflow).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days.saturating_add(719_468);
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = era.saturating_mul(400).saturating_add(yoe).saturating_add(u64::from(m <= 2));
    (y, m, d)
}

fn numbered(dir: &Path, n: usize) -> PathBuf {
    if n == 0 { dir.join(LOG_FILE) } else { dir.join(format!("transit.{n}.log")) }
}

/// Shift the previous logs up one (`.log` → `.1.log` → … → `.<KEEP>.log`, the oldest dropped)
/// and create a fresh, empty log file in `dir` (created if missing).
pub fn rotate(dir: &Path) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    for n in (1..=KEEP).rev() {
        match std::fs::rename(numbered(dir, n - 1), numbered(dir, n)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let path = numbered(dir, 0);
    let file = File::create(&path)?;
    Ok((path, file))
}

/// One formatted log line: `<timestamp> <LEVEL> [<thread>] <target>: <message>\n`.
pub fn format_line(ts: &str, level: log::Level, thread: &str, target: &str, message: &str) -> String {
    let message = message.strip_suffix('\n').unwrap_or(message);
    format!("{ts} {level:<5} [{thread}] {target}: {message}\n")
}

/// Where formatted lines go besides standard error: kept in memory until a file is attached,
/// then appended to it until it reaches its size cap.
pub struct Sink {
    file: Option<File>,
    pending: Vec<String>,
    dropped: usize,
    written: u64,
    max_bytes: u64,
    capped: bool,
}

impl Sink {
    pub fn new(max_bytes: u64) -> Sink {
        Sink { file: None, pending: Vec::new(), dropped: 0, written: 0, max_bytes, capped: false }
    }

    /// Append `line` to the file, or keep it until [`Sink::attach`].
    pub fn write(&mut self, line: &str) {
        if self.file.is_none() {
            if self.capped {
                return;
            }
            if self.pending.len() < MAX_PENDING {
                self.pending.push(line.to_owned());
            } else {
                self.dropped = self.dropped.saturating_add(1);
            }
            return;
        }
        self.append(line);
    }

    /// Start writing to `file`, first the lines kept so far.
    pub fn attach(&mut self, file: File) {
        self.file = Some(file);
        for line in std::mem::take(&mut self.pending) {
            self.append(&line);
        }
        if self.dropped > 0 {
            let note = format!("{} earlier log lines were dropped before the log file was opened\n", self.dropped);
            self.dropped = 0;
            self.append(&note);
        }
    }

    /// There will be no log file: forget the lines kept for it and keep no more.
    pub fn no_file(&mut self) {
        self.file = None;
        self.pending = Vec::new();
        self.dropped = 0;
        self.capped = true;
    }

    fn append(&mut self, line: &str) {
        if self.capped {
            return;
        }
        let Some(file) = self.file.as_mut() else { return };
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        let text = if self.written.saturating_add(len) > self.max_bytes {
            self.capped = true;
            format!("log file reached {} bytes; later records go to standard error only\n", self.max_bytes)
        } else {
            self.written = self.written.saturating_add(len);
            line.to_owned()
        };
        // A full disk or a vanished file must not take the app down: drop the file sink (and
        // stop buffering, since no file will come).
        if file.write_all(text.as_bytes()).is_err() {
            self.file = None;
            self.capped = true;
        }
    }
}

/// Text formatted into a fixed buffer: no allocation, cut at a character boundary when full.
struct Bounded<const N: usize> {
    buf: [u8; N],
    len: usize,
    cut: bool,
}

impl<const N: usize> Bounded<N> {
    fn new() -> Self {
        Bounded { buf: [0; N], len: 0, cut: false }
    }

    fn as_str(&self) -> &str {
        // Only whole characters are ever copied in, so this never fails.
        self.buf.get(..self.len).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl<const N: usize> std::fmt::Write for Bounded<N> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let room = N.saturating_sub(self.len);
        let mut n = s.len().min(room);
        while !s.is_char_boundary(n) {
            n = n.saturating_sub(1);
        }
        if let (Some(dst), Some(src)) = (self.buf.get_mut(self.len..self.len.saturating_add(n)), s.as_bytes().get(..n)) {
            dst.copy_from_slice(src);
            self.len = self.len.saturating_add(n);
        }
        if n < s.len() {
            self.cut = true;
        }
        Ok(())
    }
}

/// The first record the audio thread logged since the last report.
struct Held {
    at: SystemTime,
    level: log::Level,
    target: Bounded<REALTIME_TARGET_BYTES>,
    message: Bounded<REALTIME_MESSAGE_BYTES>,
}

/// Records from the realtime audio thread, waiting for another thread to write them.
#[derive(Default)]
struct Realtime {
    /// Records logged on the audio thread since the last report (the held one included).
    records: AtomicUsize,
    /// The first of them; the audio thread only `try_lock`s it.
    held: Mutex<Option<Held>>,
}

impl Realtime {
    /// Audio thread: count `record` and keep it if it is the first. Never waits, never allocates.
    fn keep(&self, record: &log::Record<'_>) {
        self.records.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut held) = self.held.try_lock()
            && held.is_none()
        {
            let mut target = Bounded::new();
            let _ = target.write_str(record.target());
            let mut message = Bounded::new();
            let _ = write!(message, "{}", record.args());
            *held = Some(Held { at: SystemTime::now(), level: record.level(), target, message });
        }
    }

    /// Another thread: the lines to write for what the audio thread logged (empty when nothing).
    fn take_lines(&self) -> String {
        if self.records.load(Ordering::Relaxed) == 0 {
            return String::new();
        }
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner).take();
        let records = self.records.swap(0, Ordering::Relaxed);
        let mut lines = String::new();
        if let Some(h) = &held {
            let mut message = h.message.as_str().to_owned();
            if h.message.cut {
                message.push('…');
            }
            lines.push_str(&format_line(&timestamp(h.at), h.level, AUDIO_THREAD, h.target.as_str(), &message));
        }
        let more = records.saturating_sub(usize::from(held.is_some()));
        if more > 0 {
            let thread = std::thread::current();
            let note = format!("{more} more log records from the realtime audio thread were not written (only the first is kept)");
            lines.push_str(&format_line(&timestamp(SystemTime::now()), log::Level::Warn, thread.name().unwrap_or("?"), module_path!(), &note));
        }
        lines
    }
}

/// The installed logger: a [`Filter`], standard error and a [`Sink`].
pub struct AppLogger {
    filter: Filter,
    stderr: bool,
    /// Our own targets log at debug at least (`--verbose`, Help > Debug Logging); read on every record.
    debug: AtomicBool,
    /// The log file of this run once attached.
    file: Mutex<Option<PathBuf>>,
    sink: Mutex<Sink>,
    realtime: Realtime,
}

impl AppLogger {
    pub fn new(filter: Filter, stderr: bool) -> AppLogger {
        AppLogger {
            filter,
            stderr,
            debug: AtomicBool::new(false),
            file: Mutex::new(None),
            sink: Mutex::new(Sink::new(MAX_FILE_BYTES)),
            realtime: Realtime::default(),
        }
    }

    /// Whether our own targets are raised to debug (the flag, or Help > Debug Logging).
    pub fn debug(&self) -> bool {
        self.debug.load(Ordering::Relaxed)
    }

    /// Raise our own targets to debug, or put the startup filter back; takes effect on the next record.
    pub fn set_debug(&self, on: bool) {
        self.debug.store(on, Ordering::Relaxed);
        log::set_max_level(self.max_level());
    }

    /// The most verbose level any target reaches now (for `log::set_max_level`).
    pub fn max_level(&self) -> LevelFilter {
        let max = self.filter.max();
        if self.debug() { max.max(LevelFilter::Debug) } else { max }
    }

    /// The level that applies to `target` now.
    pub fn level_for(&self, target: &str) -> LevelFilter {
        let level = self.filter.level_for(target);
        if self.debug() && is_own_target(target) { level.max(LevelFilter::Debug) } else { level }
    }

    /// The log file of this run, once attached.
    pub fn file(&self) -> Option<PathBuf> {
        self.file.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Whether lines are copied to standard error.
    pub fn mirrors_stderr(&self) -> bool {
        self.stderr
    }

    /// Rotate the logs in `dir` and send records to the fresh file; returns its path.
    /// An unusable directory leaves standard error as the only sink ([`AppLogger::no_file`]).
    pub fn attach_dir(&self, dir: &Path) -> Result<PathBuf, String> {
        match rotate(dir) {
            Ok((path, file)) => {
                self.sink.lock().unwrap_or_else(PoisonError::into_inner).attach(file);
                *self.file.lock().unwrap_or_else(PoisonError::into_inner) = Some(path.clone());
                Ok(path)
            }
            Err(e) => {
                self.no_file();
                Err(format!("{}: {e}", dir.display()))
            }
        }
    }

    /// No log file this run (no settings directory, `SOUNDCRAFT_NO_PREFS`, or the directory can't
    /// be used): stop keeping records for one.
    pub fn no_file(&self) {
        self.sink.lock().unwrap_or_else(PoisonError::into_inner).no_file();
    }

    /// Write what the audio thread logged since the last report (never call it on the audio thread).
    pub fn report_audio_thread(&self) {
        let lines = self.realtime.take_lines();
        if !lines.is_empty() {
            self.emit(&lines);
        }
    }

    /// Standard error, then the sink; the lock covers one write of already formatted text.
    fn emit(&self, text: &str) {
        if self.stderr {
            // No terminal (a Windows GUI build, a closed pipe) is not an error worth reporting.
            let _ = std::io::stderr().write_all(text.as_bytes());
        }
        self.sink.lock().unwrap_or_else(PoisonError::into_inner).write(text);
    }
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.level_for(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if transit_playback::on_audio_thread() {
            self.realtime.keep(record);
            return;
        }
        self.report_audio_thread();
        // Formatted before the sink is locked, so a message that logs while it is formatted can't deadlock.
        let thread = std::thread::current();
        let line =
            format_line(&timestamp(SystemTime::now()), record.level(), thread.name().unwrap_or("?"), record.target(), &record.args().to_string());
        self.emit(&line);
    }

    fn flush(&self) {}
}

/// Install the logger (filter by [`resolve`]: `--log`, `--verbose`, `RUST_LOG`, else
/// [`DEFAULT_FILTER`]); `None` when another logger was installed first. Call
/// [`AppLogger::attach_dir`] or [`AppLogger::no_file`] once the settings directory is known.
pub fn install(args: &LogArgs) -> Option<(&'static AppLogger, Resolved)> {
    use std::io::IsTerminal as _;
    static LOGGER: OnceLock<AppLogger> = OnceLock::new();
    let resolved = resolve(args, std::env::var("RUST_LOG").ok().as_deref());
    let mirror = stderr_mirror(args.stderr, std::io::stderr().is_terminal());
    let logger = LOGGER.get_or_init(|| {
        let logger = AppLogger::new(Filter::parse(&resolved.spec), mirror);
        logger.debug.store(resolved.debug, Ordering::Relaxed);
        logger
    });
    log::set_logger(logger).ok()?;
    log::set_max_level(logger.max_level());
    Some((logger, resolved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("transit-logging-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read log")
    }

    fn record(logger: &AppLogger, level: log::Level, target: &'static str, msg: &str) {
        logger.log(&log::Record::builder().level(level).target(target).args(format_args!("{msg}")).build());
    }

    /// Run `f` on a thread the playback crate marks as the realtime audio thread.
    fn on_audio_thread<T: Send>(f: impl FnOnce() -> T + Send) -> T {
        std::thread::scope(|s| {
            s.spawn(|| {
                transit_playback::mark_audio_thread();
                f()
            })
            .join()
            .expect("audio thread")
        })
    }

    #[test]
    fn the_default_filter_shows_soundcraft_info_and_other_crates_warnings() {
        let f = Filter::parse(DEFAULT_FILTER);
        assert_eq!(f.level_for("soundcraft"), LevelFilter::Info);
        assert_eq!(f.level_for("soundcraft::control_server"), LevelFilter::Info);
        assert_eq!(f.level_for("transit_playback"), LevelFilter::Info);
        assert_eq!(f.level_for("transit_clap_host::ffi"), LevelFilter::Info);
        assert_eq!(f.level_for("wgpu_core::device"), LevelFilter::Warn);
        assert_eq!(f.level_for("cpal"), LevelFilter::Warn);
        assert_eq!(f.max(), LevelFilter::Info);
    }

    #[test]
    fn directives_follow_env_logger_and_the_most_specific_one_wins() {
        let f = Filter::parse("info,wgpu_core=error,transit_mix=trace,transit_mix::pdc=off");
        assert_eq!(f.level_for("eframe"), LevelFilter::Info);
        assert_eq!(f.level_for("wgpu_core::instance"), LevelFilter::Error);
        assert_eq!(f.level_for("transit_mix::render"), LevelFilter::Trace);
        assert_eq!(f.level_for("transit_mix::pdc"), LevelFilter::Off);
        assert_eq!(f.max(), LevelFilter::Trace);
        // A module name is matched at `::` boundaries, not as a bare prefix.
        assert_eq!(f.level_for("wgpu_core_extra"), LevelFilter::Info);
        // A trailing `*` is a prefix.
        assert_eq!(Filter::parse("warn,transit_ui*=debug").level_for("transit_ui_egui::mix_window"), LevelFilter::Debug);
        assert_eq!(Filter::parse("warn,transit_ui*=debug").level_for("transit_engine"), LevelFilter::Warn);
        // A bare target name sets that target to the most verbose level, as env_logger does.
        assert_eq!(Filter::parse("naga").level_for("naga::front"), LevelFilter::Trace);
        assert_eq!(Filter::parse("naga").level_for("eframe"), LevelFilter::Error);
        // Levels are case-insensitive; the last bare level wins.
        assert_eq!(Filter::parse("debug,WARN").level_for("x"), LevelFilter::Warn);
    }

    #[test]
    fn hostile_specs_never_panic_and_fall_back_sensibly() {
        let long = "a".repeat(100_000);
        let specs = [
            "",
            ",,,",
            "=",
            "==",
            "=debug",
            "soundcraft=",
            "nonsense=loud",
            "🦀=info",
            "*",
            "*=debug",
            "a=b=c",
            "warn,,soundcraft=DEBUG",
            " \t ",
            "\u{0}",
            "é*=trace",
            &long,
        ];
        for spec in specs {
            let f = Filter::parse(spec);
            for target in ["soundcraft", "", "🦀", "é", "a::b"] {
                let _ = f.level_for(target);
            }
            let _ = f.max();
        }
        assert_eq!(Filter::parse("warn,,soundcraft=DEBUG").level_for("soundcraft"), LevelFilter::Debug);
        assert_eq!(Filter::parse("nonsense=loud").level_for("nonsense"), LevelFilter::Error);
        assert_eq!(Filter::parse("=debug").max(), LevelFilter::Error, "a directive without a name is skipped");
        assert_eq!(Filter::parse("*=debug").level_for("anything"), LevelFilter::Debug, "a lone `*` matches every target");
        assert_eq!(Filter::parse("é*=trace").level_for("éa"), LevelFilter::Trace);
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_flag_beats_the_environment_which_beats_the_default() {
        let none = LogArgs::default();
        let log = LogArgs::parse(&args(&["--log", "warn,transit_mix=trace"])).expect("--log");
        let verbose = LogArgs::parse(&args(&["--verbose"])).expect("--verbose");
        assert_eq!(resolve(&none, None), Resolved { spec: DEFAULT_FILTER.into(), source: Source::Default, debug: false });
        assert_eq!(resolve(&none, Some("  ")).source, Source::Default, "a blank RUST_LOG is unset");
        assert_eq!(resolve(&none, Some("trace")), Resolved { spec: "trace".into(), source: Source::Env, debug: false });
        assert_eq!(resolve(&log, Some("trace")), Resolved { spec: "warn,transit_mix=trace".into(), source: Source::LogFlag, debug: false });
        assert_eq!(resolve(&verbose, Some("error")), Resolved { spec: DEFAULT_FILTER.into(), source: Source::VerboseFlag, debug: true });
        // Both flags: the directives of --log, and our targets at debug at least.
        let both = LogArgs::parse(&args(&["--verbose", "--log=warn"])).expect("both");
        assert_eq!(resolve(&both, Some("trace")), Resolved { spec: "warn".into(), source: Source::LogFlag, debug: true });
    }

    #[test]
    fn the_command_line_flags_parse_and_a_log_flag_needs_directives() {
        assert_eq!(LogArgs::parse(&args(&["--demo", "song.transit"])), Ok(LogArgs::default()));
        let a = LogArgs::parse(&args(&["--log-stderr", "--log", "debug", "song.transit"])).expect("flags");
        assert_eq!(a, LogArgs { verbose: false, log: Some("debug".into()), stderr: true });
        assert!(LogArgs::parse(&args(&["--log"])).is_err());
        assert!(LogArgs::parse(&args(&["--log", "--verbose"])).is_err(), "a flag is not directives");
        let list = args(&["--log", "debug", "a.transit"]);
        assert!(LogArgs::is_value(&list, 1) && !LogArgs::is_value(&list, 0) && !LogArgs::is_value(&list, 2));
    }

    #[test]
    fn standard_error_is_mirrored_for_a_terminal_or_the_flag() {
        assert!(stderr_mirror(true, false));
        assert!(stderr_mirror(false, true));
        assert!(!stderr_mirror(false, false), "a launch from a menu has no terminal to write to");
        let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), stderr_mirror(true, false));
        assert!(logger.mirrors_stderr());
        assert!(!AppLogger::new(Filter::parse(DEFAULT_FILTER), stderr_mirror(false, false)).mirrors_stderr());
    }

    #[test]
    fn the_help_toggle_raises_our_targets_to_debug_and_puts_them_back() {
        let dir = temp_dir("toggle");
        let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
        let path = logger.attach_dir(&dir).expect("attach");
        assert_eq!(logger.file(), Some(path.clone()));
        let debug_meta = |target| log::Metadata::builder().level(log::Level::Debug).target(target).build();
        assert!(!logger.debug());
        assert!(!logger.enabled(&debug_meta("transit_playback")));
        assert_eq!(logger.max_level(), LevelFilter::Info);
        logger.set_debug(true);
        assert!(logger.debug() && logger.enabled(&debug_meta("transit_playback")) && logger.enabled(&debug_meta("transit")));
        assert!(!logger.enabled(&debug_meta("wgpu_core::device")), "other crates stay as they were");
        assert_eq!(logger.level_for("transit_mix"), LevelFilter::Debug);
        assert_eq!(logger.max_level(), LevelFilter::Debug);
        record(&logger, log::Level::Debug, "transit_playback", "a debug line");
        logger.set_debug(false);
        assert!(!logger.enabled(&debug_meta("transit_playback")));
        assert_eq!(logger.max_level(), LevelFilter::Info);
        record(&logger, log::Level::Debug, "transit_playback", "dropped again");
        let text = read(&path);
        assert!(text.contains("a debug line") && !text.contains("dropped again"), "{text}");
        // A filter that already asks for more is not lowered by the toggle.
        let louder = AppLogger::new(Filter::parse("transit_mix=trace"), false);
        louder.set_debug(true);
        assert_eq!(louder.level_for("transit_mix"), LevelFilter::Trace);
        louder.set_debug(false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timestamps_are_utc_with_milliseconds() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_millis(1_791_446_357_728)), "2026-10-08T07:59:17.728Z");
        // Leap day, and the day after it.
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29T00:00:00.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_251_199)), "2024-02-29T23:59:59.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_251_200)), "2024-03-01T00:00:00.000Z");
        // 2000 is a leap year (divisible by 400), 2100 is not.
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(4_107_542_400)), "2100-03-01T00:00:00.000Z");
        // Before the epoch (a clock set wrong) clamps instead of panicking.
        assert_eq!(timestamp(UNIX_EPOCH - Duration::from_secs(5)), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn rotation_keeps_the_previous_logs_and_starts_an_empty_file() {
        let dir = temp_dir("rotate");
        for run in 1..=4 {
            let (path, _file) = rotate(&dir).expect("rotate");
            assert_eq!(path, dir.join(LOG_FILE));
            assert_eq!(read(&path), "");
            std::fs::write(&path, format!("run {run}")).expect("write");
        }
        assert_eq!(read(&dir.join(LOG_FILE)), "run 4");
        assert_eq!(read(&dir.join("transit.1.log")), "run 3");
        assert_eq!(read(&dir.join("transit.2.log")), "run 2");
        assert!(!dir.join("transit.3.log").exists(), "only {KEEP} old logs are kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_into_an_unusable_directory_is_an_error_not_a_panic() {
        let dir = temp_dir("blocked");
        std::fs::create_dir_all(dir.parent().expect("parent")).expect("tmp");
        std::fs::write(&dir, "a file where the directory should be").expect("block");
        assert!(rotate(&dir).is_err());
        assert!(rotate(&dir.join("below")).is_err());
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn a_line_carries_time_level_thread_target_and_message() {
        assert_eq!(
            format_line("2026-10-08T07:59:17.728Z", log::Level::Warn, "main", "transit_mix", "eq_7band: the plugin did not accept its stored state"),
            "2026-10-08T07:59:17.728Z WARN  [main] transit_mix: eq_7band: the plugin did not accept its stored state\n"
        );
        // A multi-line message keeps its lines; the record still ends in exactly one newline.
        assert!(format_line("t", log::Level::Error, "w", "x", "a\nb\n").ends_with("x: a\nb\n"));
    }

    #[test]
    fn lines_logged_before_the_file_exists_are_written_when_it_is_attached() {
        let dir = temp_dir("pending");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(MAX_FILE_BYTES);
        sink.write("early 1\n");
        sink.write("early 2\n");
        sink.attach(file);
        sink.write("late\n");
        assert_eq!(read(&path), "early 1\nearly 2\nlate\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pending_buffer_is_bounded_and_says_how_much_it_dropped() {
        let dir = temp_dir("bounded");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(MAX_FILE_BYTES);
        for i in 0..MAX_PENDING + 5 {
            sink.write(&format!("line {i}\n"));
        }
        sink.attach(file);
        let text = read(&path);
        assert_eq!(text.lines().filter(|l| l.starts_with("line ")).count(), MAX_PENDING);
        assert_eq!(text.matches("5 earlier log lines were dropped").count(), 1, "{text}");
        assert!(text.contains("line 0\n") && !text.contains(&format!("line {MAX_PENDING}\n")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exactly_max_pending_lines_drop_nothing() {
        let dir = temp_dir("exact");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(MAX_FILE_BYTES);
        for i in 0..MAX_PENDING {
            sink.write(&format!("line {i}\n"));
        }
        sink.attach(file);
        let text = read(&path);
        assert_eq!(text.lines().count(), MAX_PENDING);
        assert!(!text.contains("dropped"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_stops_at_its_size_cap_with_one_note() {
        let dir = temp_dir("cap");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(100);
        sink.attach(file);
        for i in 0..50 {
            sink.write(&format!("line {i:02} with some padding\n"));
        }
        let text = read(&path);
        assert_eq!(text.lines().filter(|l| l.starts_with("line ")).count(), 3, "{text}");
        assert_eq!(text.matches("log file reached 100 bytes").count(), 1, "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_logger_filters_records_and_writes_them_to_the_attached_file() {
        let dir = temp_dir("logger");
        let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
        record(&logger, log::Level::Info, "soundcraft", "SoundCraft starting");
        record(&logger, log::Level::Info, "wgpu_core::device", "too chatty");
        let path = logger.attach_dir(&dir).expect("attach");
        assert_eq!(path, dir.join(LOG_FILE));
        record(&logger, log::Level::Warn, "wgpu_hal::vulkan", "a real warning");
        record(&logger, log::Level::Debug, "soundcraft", "below info");
        let text = read(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines.first().is_some_and(|l| l.contains(" INFO  [") && l.ends_with("soundcraft: SoundCraft starting")), "{text}");
        assert!(lines.get(1).is_some_and(|l| l.ends_with("wgpu_hal::vulkan: a real warning")), "{text}");
        assert!(logger.enabled(&log::Metadata::builder().level(log::Level::Info).target("transit_engine").build()));
        assert!(!logger.enabled(&log::Metadata::builder().level(log::Level::Info).target("naga").build()));
        // A second launch rotates: the first log becomes `.1`.
        let second = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
        assert_eq!(second.attach_dir(&dir).expect("attach again"), dir.join(LOG_FILE));
        assert_eq!(read(&dir.join("transit.1.log")), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_from_many_threads_are_whole_lines() {
        let dir = temp_dir("threads");
        let logger = AppLogger::new(Filter::parse("info"), false);
        let path = logger.attach_dir(&dir).expect("attach");
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..100 {
                        record(&logger, log::Level::Info, "soundcraft", "from a worker");
                    }
                });
            }
        });
        let text = read(&path);
        assert_eq!(text.lines().count(), 400);
        assert!(text.lines().all(|l| l.ends_with("soundcraft: from a worker")), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attaching_to_an_unusable_directory_reports_an_error_and_keeps_logging() {
        let dir = temp_dir("unusable");
        std::fs::write(&dir, "not a directory").expect("block");
        let logger = AppLogger::new(Filter::parse("info"), false);
        record(&logger, log::Level::Info, "soundcraft", "kept for a file");
        let e = logger.attach_dir(&dir).expect_err("a file is not a log directory");
        assert!(e.contains(&dir.display().to_string()), "{e}");
        record(&logger, log::Level::Error, "x", "still fine");
        let sink = logger.sink.lock().expect("sink");
        assert!(sink.file.is_none() && sink.pending.is_empty(), "nothing is kept for a file that will not come");
        drop(sink);
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn a_run_without_a_log_file_keeps_nothing_for_one() {
        let logger = AppLogger::new(Filter::parse("info"), false);
        record(&logger, log::Level::Info, "soundcraft", "early");
        logger.no_file();
        record(&logger, log::Level::Info, "soundcraft", "late");
        let sink = logger.sink.lock().expect("sink");
        assert!(sink.file.is_none() && sink.pending.is_empty() && sink.dropped == 0);
    }

    #[test]
    fn a_poisoned_lock_does_not_stop_logging() {
        let dir = temp_dir("poison");
        let logger = AppLogger::new(Filter::parse("info"), false);
        let path = logger.attach_dir(&dir).expect("attach");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = logger.sink.lock();
            panic!("poison the sink");
        }));
        assert!(logger.sink.is_poisoned());
        record(&logger, log::Level::Info, "soundcraft", "after the panic");
        assert!(read(&path).contains("after the panic"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Realtime audio: the audio thread never takes the sink lock or touches the file.

    #[test]
    fn an_audio_thread_record_never_waits_for_the_sink() {
        let dir = temp_dir("rt-nowait");
        let logger = AppLogger::new(Filter::parse("info"), false);
        let path = logger.attach_dir(&dir).expect("attach");
        // Another thread holds the sink (a slow disk, say) while the audio thread logs.
        let held = logger.sink.lock().expect("sink");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|s| {
            s.spawn(|| {
                transit_playback::mark_audio_thread();
                record(&logger, log::Level::Warn, "transit_playback", "audio stream error: device unplugged");
                let _ = tx.send(());
            });
            let done = rx.recv_timeout(Duration::from_secs(10)).is_ok();
            drop(held);
            assert!(done, "the audio thread blocked on the sink lock");
        });
        assert_eq!(read(&path), "", "nothing is written from the audio thread itself");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_audio_thread_record_is_written_later_with_a_count_of_the_rest() {
        let dir = temp_dir("rt-report");
        let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
        let path = logger.attach_dir(&dir).expect("attach");
        on_audio_thread(|| {
            record(&logger, log::Level::Warn, "transit_playback", "audio stream error: device unplugged");
            for _ in 0..41 {
                record(&logger, log::Level::Warn, "transit_playback", "audio stream error: still unplugged");
            }
            // Filtered out: neither kept nor counted.
            record(&logger, log::Level::Debug, "transit_clap_host::plugin", "process failed");
        });
        assert_eq!(read(&path), "");
        logger.report_audio_thread();
        let text = read(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines.first().is_some_and(|l| l.contains(" WARN  [audio] transit_playback: audio stream error: device unplugged")), "{text}");
        assert!(lines.get(1).is_some_and(|l| l.contains("41 more log records from the realtime audio thread were not written")), "{text}");
        // Reported once.
        logger.report_audio_thread();
        assert_eq!(read(&path), text);
        // The next record from any other thread reports what the audio thread kept since, first.
        on_audio_thread(|| record(&logger, log::Level::Error, "transit_clap_host::ffi", "clap plugin: out of voices"));
        record(&logger, log::Level::Info, "soundcraft", "UI thread");
        let text = read(&path);
        let tail: Vec<&str> = text.lines().skip(2).collect();
        assert_eq!(tail.len(), 2, "{text}");
        assert!(tail.first().is_some_and(|l| l.contains(" ERROR [audio] transit_clap_host::ffi: clap plugin: out of voices")), "{text}");
        assert!(tail.get(1).is_some_and(|l| l.ends_with("soundcraft: UI thread")), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_long_audio_thread_record_is_cut_at_a_character_boundary() {
        let dir = temp_dir("rt-long");
        let logger = AppLogger::new(Filter::parse("info"), false);
        let path = logger.attach_dir(&dir).expect("attach");
        let long = "é".repeat(REALTIME_MESSAGE_BYTES);
        on_audio_thread(|| record(&logger, log::Level::Warn, "soundcraft", &long));
        logger.report_audio_thread();
        let text = read(&path);
        let message = text.split("soundcraft: ").nth(1).expect("message");
        assert!(message.ends_with("…\n"), "{text}");
        assert!(message.len() <= REALTIME_MESSAGE_BYTES + "…\n".len(), "{}", message.len());
        assert!(message.trim_end_matches(['…', '\n']).chars().all(|c| c == 'é'));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
