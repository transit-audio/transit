//! The scan cache file: what each plugin file held when it was last loaded, keyed by the size and
//! modification time of its binary, so a scan loads only new and changed files (as REAPER does).
//! A file that failed to load is remembered too, with the app build that tried it ([`this_host`]),
//! and not retried until the file changes or another build scans (an update can fix the cause:
//! the macOS app once refused every plug-in, #659).
//!
//! Files can also be loaded in a child process (see [`load_in_child`]), so that a plugin's
//! start-up never runs in the app: on Windows a library initialising holds the loader lock, which
//! stalls every other thread of the process that loads a library or starts a thread (the app's
//! window stayed black at launch while a plugin took seconds to initialise), and a plugin that
//! crashes while it loads takes only the child down.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Bumped when the layout changes; a file of another version is ignored (everything is loaded).
/// 2: every class carries its probe outcome (ADR 0064, decision 2.4; [`crate::probe`]).
const VERSION: u32 = 2;

/// Largest cache file read (a plugin takes a few hundred bytes).
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Starts the line a child process answers with (plugins may print to standard output too).
pub(crate) const REPLY: &str = "SOUNDCRAFT-SCAN ";

/// Longest wait for a child process; a plugin that takes longer (or waits on a dialog) is killed.
/// Each probe child has the same budget (ADR 0064, decision 2.4).
pub const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

/// When a child process has run out of time: checked between polls while it runs. The scan uses
/// [`Timeout`]; a test can decide the moment itself instead of waiting on the clock.
pub trait Deadline: Send {
    /// Whether the child has run out of time.
    fn passed(&mut self) -> bool;
    /// The budget, for the reason a stopped child gives.
    fn limit(&self) -> Duration;
}

/// A wall-clock budget from the moment it is made.
pub struct Timeout {
    started: Instant,
    limit: Duration,
}

impl Timeout {
    /// A budget of `limit` from now.
    pub fn new(limit: Duration) -> Self {
        Timeout { started: Instant::now(), limit }
    }

    /// The budget of every scan and probe child, [`CHILD_TIMEOUT`].
    pub fn child() -> Self {
        Timeout::new(CHILD_TIMEOUT)
    }
}

impl Deadline for Timeout {
    fn passed(&mut self) -> bool {
        self.started.elapsed() >= self.limit
    }
    fn limit(&self) -> Duration {
        self.limit
    }
}

/// One plugin file: the plugins it holds, or why it did not load.
#[derive(Clone, Serialize, Deserialize)]
#[serde(bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
pub struct Entry<D> {
    pub path: PathBuf,
    size: u64,
    modified_ns: u64,
    pub plugins: Vec<D>,
    pub error: Option<String>,
    /// The app build that made this entry ([`this_host`]). A failure is reused only by the same
    /// build; entries written before builds were recorded have none, so their failures are retried.
    #[serde(default)]
    host: String,
}

/// What one scan found for a format: the plug-in files, how many loaded and how many did not, and
/// the first failure. Remembered failures count, so a scan that reuses them still reports them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Plug-in files (bundles) found.
    pub found: usize,
    /// Files that loaded (some may hold no plug-in this host lists).
    pub loaded: usize,
    /// Files that did not load.
    pub failed: usize,
    /// Why the first file that did not load failed.
    pub first_error: Option<String>,
}

impl ScanSummary {
    /// The counts of a scan's entries.
    pub fn of<D>(files: &[Entry<D>]) -> Self {
        let failed = files.iter().filter(|f| f.error.is_some()).count();
        ScanSummary { found: files.len(), loaded: files.len() - failed, failed, first_error: files.iter().find_map(|f| f.error.clone()) }
    }

    /// When plug-in files were found and none of them loaded: what to tell the user instead of
    /// "none found", with the first error (`format` is "VST3" or "CLAP"). `None` otherwise.
    pub fn none_loaded_message(&self, format: &str) -> Option<String> {
        if self.found == 0 || self.loaded > 0 {
            return None;
        }
        let error = self.first_error.as_deref().unwrap_or("no reason given");
        Some(match self.found {
            1 => format!("1 {format} plug-in file found, but it could not be loaded: {error}"),
            n => format!("{n} {format} plug-in files found, but none could be loaded. The first error: {error}"),
        })
    }
}

/// The app build a scan runs in: this crate's version and the size and modification time of the
/// running program, so an updated, rebuilt or re-signed app is another build. Only the version
/// when the program cannot be read.
pub fn this_host() -> String {
    let version = env!("CARGO_PKG_VERSION");
    match std::env::current_exe().ok().and_then(|exe| stamp(&exe)) {
        Some((size, modified_ns)) => format!("{version} {size} {modified_ns}"),
        None => version.to_string(),
    }
}

#[derive(Deserialize)]
#[serde(bound = "D: DeserializeOwned")]
struct FileIn<D> {
    version: u32,
    files: Vec<Entry<D>>,
}

#[derive(Serialize)]
#[serde(bound = "D: Serialize")]
struct FileOut<'a, D> {
    version: u32,
    files: &'a [Entry<D>],
}

/// Size and modification time (ns since 1970) of a file; `None` when unreadable.
fn stamp(file: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(file).ok()?;
    let ns = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((m.len(), u64::try_from(ns).ok()?))
}

/// The entries of a cache file; none when it is missing, unreadable or of another version.
pub fn read<D: DeserializeOwned>(file: &Path) -> Vec<Entry<D>> {
    use std::io::Read as _;
    let Ok(f) = std::fs::File::open(file) else { return Vec::new() };
    let mut bytes = Vec::new();
    if f.take(MAX_BYTES).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    match serde_json::from_slice::<FileIn<D>>(&bytes) {
        Ok(c) if c.version == VERSION => c.files,
        Ok(_) => Vec::new(),
        Err(e) => {
            log::warn!("{}: {e}", file.display());
            Vec::new()
        }
    }
}

/// Writes a cache file (through a temporary file, so a crash never leaves half of one).
pub fn write<D: Serialize>(file: &Path, files: &[Entry<D>]) {
    let tmp = file.with_extension("json.tmp");
    let written = serde_json::to_vec_pretty(&FileOut { version: VERSION, files })
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&tmp, bytes).map_err(|e| e.to_string())
        })
        .and_then(|()| std::fs::rename(&tmp, file).map_err(|e| e.to_string()));
    if let Err(e) = written {
        log::warn!("{}: {e}", file.display());
    }
}

/// One entry per plugin file in `files`: the `known` entry when the file's `binary` (else the
/// file itself) has the same size and modification time (and, for a failure, it was made by the
/// same `host` build, see [`this_host`]), else what `load` reads now. Before each
/// load, `ahead(index, count, file)` is told what comes; when it returns `false` the file is left
/// out (it is neither listed nor cached, so the next scan loads it), as is a file whose `load`
/// gives `None` (interrupted).
pub fn scan<D: Clone>(
    files: Vec<PathBuf>,
    known: &[Entry<D>],
    binary: impl Fn(&Path) -> Option<PathBuf>,
    load: impl Fn(&Path) -> Option<Result<Vec<D>, String>>,
    ahead: impl Fn(usize, usize, &Path) -> bool,
    host: &str,
) -> Vec<Entry<D>> {
    let count = files.len();
    files
        .into_iter()
        .enumerate()
        .filter_map(|(i, path)| {
            let stamp = stamp(&binary(&path).unwrap_or_else(|| path.clone()));
            if let Some((size, modified_ns)) = stamp
                && let Some(e) =
                    known.iter().find(|e| e.path == path && e.size == size && e.modified_ns == modified_ns && (e.error.is_none() || e.host == host))
            {
                return Some(e.clone());
            }
            if !ahead(i, count, &path) {
                return None;
            }
            let (plugins, error) = match load(&path)? {
                Ok(p) => (p, None),
                Err(e) => {
                    log::warn!("{e}");
                    (Vec::new(), Some(e))
                }
            };
            let (size, modified_ns) = stamp.unwrap_or_default();
            Some(Entry { path, size, modified_ns, plugins, error, host: host.to_string() })
        })
        .collect()
}

/// What a child process started with [`load_in_child`] prints: one line with the plugins that
/// `loaded` found, or why it failed.
pub fn reply<D: Serialize, E: std::fmt::Display>(loaded: Result<Vec<D>, E>) -> String {
    let body = match loaded {
        Ok(plugins) => serde_json::json!({ "plugins": plugins }),
        Err(e) => serde_json::json!({ "error": e.to_string() }),
    };
    format!("{REPLY}{body}")
}

#[derive(Deserialize)]
#[serde(bound = "D: DeserializeOwned")]
struct Reply<D> {
    plugins: Option<Vec<D>>,
    error: Option<String>,
}

/// Loads `file` in a child process: `program flag file`, which answers with [`reply`]. `None`
/// when `stop` turned true meanwhile: the child was killed.
pub fn load_in_child<D: DeserializeOwned>(program: &Path, flag: &str, file: &Path, stop: impl Fn() -> bool) -> Option<Result<Vec<D>, String>> {
    let fail = |m: String| format!("{}: {m}", file.display());
    in_child(program, &[flag.as_ref(), file.as_os_str()], file, stop, &mut Timeout::child()).map(|answer| {
        let json = answer?;
        let r: Reply<D> = serde_json::from_str(&json).map_err(|e| fail(format!("unreadable scan reply: {e}")))?;
        match (r.plugins, r.error) {
            (Some(plugins), _) => Ok(plugins),
            (None, e) => Err(e.unwrap_or_else(|| fail("the scan process found nothing".into()))),
        }
    })
}

/// Probes one class of `file` in a child process of its own: `program flag file id`, which answers
/// with [`crate::probe::reply`] (ADR 0064, decision 2.4). The child is stopped when `deadline`
/// passes or `stop` turns true; `None` for the latter (the scan was told to stop: the class stays
/// unprobed). An error the child answers, a child that ends without an answer and one stopped by
/// its deadline are each `Err` with the reason.
pub fn probe_in_child(
    program: &Path,
    flag: &str,
    file: &Path,
    id: &str,
    stop: impl Fn() -> bool,
    deadline: &mut dyn Deadline,
) -> Option<Result<crate::probe::Capability, String>> {
    let fail = |m: String| format!("{} ({id}): {m}", file.display());
    in_child(program, &[flag.as_ref(), file.as_os_str(), id.as_ref()], file, stop, deadline)
        .map(|answer| answer.map_err(|e| format!("{e} ({id})")).and_then(|json| crate::probe::parse_reply(&json, fail)))
}

/// Runs `program args` and returns the JSON of its answer line; `None` when `stop` turned true
/// meanwhile (the child was killed).
fn in_child(program: &Path, args: &[&OsStr], file: &Path, stop: impl Fn() -> bool, deadline: &mut dyn Deadline) -> Option<Result<String, String>> {
    let mut stopped = false;
    let r = run_child(program, args, file, deadline, || {
        stopped = stop();
        stopped
    });
    (!stopped).then_some(r)
}

fn run_child(program: &Path, args: &[&OsStr], file: &Path, deadline: &mut dyn Deadline, mut stop: impl FnMut() -> bool) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let fail = |m: String| format!("{}: {m}", file.display());
    let mut command = Command::new(program);
    // The child waits for "go" on its standard input before it loads anything ([`enter_child`]),
    // so that it and everything it starts are contained first.
    command.args(args).env(PARENT_ENV, std::process::id().to_string()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW`: no console window flashes up for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    transit_rt::child_tree::prepare(&mut command);
    let mut child = command.spawn().map_err(|e| fail(format!("the scan process did not start: {e}")))?;
    // The child's process tree ends with this call (and with this process, a crash included):
    // see `transit_rt::child_tree` for what each system guarantees.
    let tree = match transit_rt::child_tree::Tree::contain(&child) {
        Ok(tree) => Some(tree),
        Err(e) => {
            log::warn!("{}: the scan process could not be contained ({e}); it runs uncontained", file.display());
            None
        }
    };
    if let Some(mut input) = child.stdin.take() {
        use std::io::Write as _;
        let _ = input.write_all(b"go\n");
    }
    // Read while it runs, so that a chatty plugin cannot fill the pipe and stall it. The reader
    // also flags the answer line the moment it arrives: a plugin that answers and then hangs the
    // process on exit (Native Instruments' plug-ins do, in their library unload) must not cost
    // the whole timeout, so the parent stops waiting on the answer, not on the exit.
    let out = child.stdout.take().ok_or_else(|| fail("no output from the scan process".into()))?;
    let answered = Arc::new(AtomicBool::new(false));
    let text = Arc::new(std::sync::Mutex::new(String::new()));
    let reader = {
        let (answered, text) = (Arc::clone(&answered), Arc::clone(&text));
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            let mut reader = std::io::BufReader::new(out);
            let mut line = Vec::new();
            // Bytes, converted lossily: a plugin printing non-UTF-8 cannot hide the answer.
            while let Ok(n) = reader.read_until(b'\n', &mut line) {
                if n == 0 {
                    break;
                }
                let s = String::from_utf8_lossy(&line);
                text.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push_str(&s);
                // Flagged once the line is in the text, so whoever sees the flag finds the line.
                if s.starts_with(REPLY) {
                    answered.store(true, Ordering::Release);
                }
                line.clear();
            }
        })
    };
    let end = |child: &mut std::process::Child| {
        // The tree first, while the child's id still names it; then the child itself.
        if let Some(tree) = &tree {
            tree.kill();
        }
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            // Answered: the plugin's start-up is done; its unload is not waited for.
            Ok(None) if answered.load(Ordering::Acquire) => {
                end(&mut child);
                break None;
            }
            Ok(None) if !deadline.passed() && !stop() => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                end(&mut child);
                break None;
            }
        }
    };
    // Whatever the child started ends with it (a helper process may hold the output pipe open).
    if let Some(tree) = &tree {
        tree.kill();
    }
    // The output ends at end of file, which a process outside the tree (one that left it on
    // purpose) can hold off for ever: collection also ends at the answer, the deadline or a stop.
    while !reader.is_finished() && !answered.load(Ordering::Acquire) && !deadline.passed() && !stop() {
        std::thread::sleep(Duration::from_millis(10));
    }
    if reader.is_finished() {
        let _ = reader.join();
    }
    drop(tree);
    let text = text.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let answer = text.lines().rev().find_map(|l| l.strip_prefix(REPLY));
    match (answer, status) {
        (Some(json), _) => Ok(json.to_string()),
        (None, None) => Err(fail(format!("still loading after {} s; stopped", deadline.limit().as_secs()))),
        (None, Some(status)) => Err(fail(format!("the plugin ended the scan process ({status})"))),
    }
}

/// Names the parent process of a scan child (its id), set by this module when it starts one.
const PARENT_ENV: &str = "TRANSIT_SCAN_PARENT";

/// First thing in a scan child's work (the listing and probe replies call it): when this process
/// was started by [`load_in_child`] or [`probe_in_child`], ties it to its parent
/// (`transit_rt::child_tree::exit_with_parent`) and waits until the parent says "go" on standard
/// input, which it does once the child is contained. A parent that is gone before that (end of
/// file, or anything else) ends this process before it loads anything. In any other process it
/// does nothing.
pub fn enter_child() {
    let Some(parent) = std::env::var(PARENT_ENV).ok().and_then(|v| v.parse::<u32>().ok()) else { return };
    transit_rt::child_tree::exit_with_parent(parent);
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) if line.trim() == "go" => {}
        _ => std::process::exit(0),
    }
}

/// An entry for `path` holding `plugins` (tests of the probe phase).
#[cfg(test)]
pub(crate) fn entry_for_test<D>(path: PathBuf, plugins: Vec<D>, error: Option<String>) -> Entry<D> {
    Entry { path, size: 0, modified_ns: 0, plugins, error, host: String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn dir(name: &str) -> transit_test_support::TestDir {
        let d = transit_test_support::TestDir::new(&format!("scan-cache-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn unchanged_files_are_not_loaded_again() {
        let d = dir("unchanged");
        let (a, b) = (d.join("a.plug"), d.join("b.plug"));
        std::fs::write(&a, "a").ok();
        std::fs::write(&b, "bb").ok();
        let loads = Cell::new(0);
        let load = |p: &Path| {
            loads.set(loads.get() + 1);
            Some(if p == b { Err("b is broken".to_string()) } else { Ok(vec![p.display().to_string()]) })
        };
        let first = scan(vec![a.clone(), b.clone()], &[], |_| None, load, |_, _, _| true, "build 1");
        assert_eq!(loads.get(), 2);
        let file = d.join("cache.json");
        write(&file, &first);
        let known: Vec<Entry<String>> = read(&file);
        assert_eq!(known.len(), 2);

        // Both unchanged: nothing is loaded, the failure is remembered.
        let again = scan(vec![a.clone(), b.clone()], &known, |_| None, load, |_, _, _| true, "build 1");
        assert_eq!(loads.get(), 2);
        assert_eq!(again[0].plugins, vec![a.display().to_string()]);
        assert_eq!(again[1].error.as_deref(), Some("b is broken"));

        // A changed file and a new one are loaded; a removed one is dropped.
        std::fs::write(&b, "fixed, longer").ok();
        let c = d.join("c.plug");
        std::fs::write(&c, "c").ok();
        let next = scan(vec![b.clone(), c.clone()], &known, |_| None, load, |_, _, _| true, "build 1");
        assert_eq!(loads.get(), 4);
        assert_eq!(next.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![b.clone(), c.clone()]);

        // Skipped while loading: the known file is still listed, the skipped ones are left out.
        let told = std::cell::RefCell::new(Vec::new());
        let skipped = scan(
            vec![a.clone(), b.clone(), c],
            &known,
            |_| None,
            load,
            |i, n, _| {
                told.borrow_mut().push((i, n));
                false
            },
            "build 1",
        );
        assert_eq!(loads.get(), 4);
        assert_eq!(told.into_inner(), vec![(1, 3), (2, 3)]);
        assert_eq!(skipped.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![a]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_remembered_failure_is_retried_by_another_build_and_reported_until_then() {
        let d = dir("host");
        let (good, bad) = (d.join("good.plug"), d.join("bad.plug"));
        std::fs::write(&good, "good").ok();
        std::fs::write(&bad, "bad").ok();
        let loads = std::cell::RefCell::new(Vec::new());
        let load = |p: &Path| {
            loads.borrow_mut().push(p.to_path_buf());
            Some(if p == bad { Err("refused by the loader".to_string()) } else { Ok(vec!["Good".to_string()]) })
        };
        let file = d.join("cache.json");
        write(&file, &scan(vec![good.clone(), bad.clone()], &[], |_| None, load, |_, _, _| true, "build 1"));
        let known: Vec<Entry<String>> = read(&file);
        loads.borrow_mut().clear();

        // The same build reuses both entries, and the summary still reports the failure.
        let same = scan(vec![good.clone(), bad.clone()], &known, |_| None, load, |_, _, _| true, "build 1");
        assert!(loads.borrow().is_empty(), "{:?}", loads.borrow());
        assert_eq!(ScanSummary::of(&same), ScanSummary { found: 2, loaded: 1, failed: 1, first_error: Some("refused by the loader".into()) });

        // Another build loads the failed file again, never the one that loaded.
        let other = scan(vec![good.clone(), bad.clone()], &known, |_| None, load, |_, _, _| true, "build 2");
        assert_eq!(*loads.borrow(), vec![bad.clone()]);
        assert_eq!(other.len(), 2);
        loads.borrow_mut().clear();

        // A cache file written before builds were recorded: its failures are retried by any build.
        let old = std::fs::read_to_string(&file).unwrap_or_default().replace("\"host\": \"build 1\"", "\"_was\": 0");
        assert!(!old.contains("\"host\""), "{old}");
        std::fs::write(&file, old).ok();
        let older: Vec<Entry<String>> = read(&file);
        assert_eq!(older.len(), 2);
        scan(vec![good, bad.clone()], &older, |_| None, load, |_, _, _| true, "build 1");
        assert_eq!(*loads.borrow(), vec![bad]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn none_loaded_is_said_only_when_files_were_found_and_every_one_failed() {
        let entry = |error: Option<&str>| Entry::<String> {
            path: PathBuf::from("x.plug"),
            size: 0,
            modified_ns: 0,
            plugins: Vec::new(),
            error: error.map(Into::into),
            host: String::new(),
        };
        assert_eq!(ScanSummary::of::<String>(&[]).none_loaded_message("VST3"), None, "nothing found is \"none found\"");
        assert_eq!(ScanSummary::of(&[entry(None), entry(Some("a"))]).none_loaded_message("VST3"), None, "one loaded");
        assert_eq!(
            ScanSummary::of(&[entry(Some("first")), entry(Some("second"))]).none_loaded_message("CLAP").as_deref(),
            Some("2 CLAP plug-in files found, but none could be loaded. The first error: first")
        );
        assert_eq!(
            ScanSummary::of(&[entry(Some("refused"))]).none_loaded_message("VST3").as_deref(),
            Some("1 VST3 plug-in file found, but it could not be loaded: refused")
        );
    }

    #[test]
    fn an_unreadable_or_foreign_cache_file_is_ignored() {
        let d = dir("foreign");
        let file = d.join("cache.json");
        assert!(read::<String>(&file).is_empty());
        std::fs::write(&file, "not json").ok();
        assert!(read::<String>(&file).is_empty());
        std::fs::write(&file, r#"{"version":999,"files":[]}"#).ok();
        assert!(read::<String>(&file).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// ADR 0064, decision 2.4: the layout with probe outcomes is version 2, so a version 1 file
    /// (no probe fields) is ignored: every file is listed again, and the file is rewritten as 2.
    #[test]
    fn a_version_1_cache_file_is_ignored_and_rewritten_as_version_2() {
        let d = dir("version-1");
        let a = d.join("a.plug");
        std::fs::write(&a, "a").ok();
        let file = d.join("cache.json");
        let (size, modified_ns) = stamp(&a).unwrap_or_default();
        let old = serde_json::json!({ "version": 1, "files": [{
            "path": a, "size": size, "modified_ns": modified_ns, "plugins": ["Stale"], "error": null, "host": "build 1"
        }] });
        std::fs::write(&file, old.to_string()).ok();
        let known: Vec<Entry<String>> = read(&file);
        assert!(known.is_empty(), "a version 1 file is not read");
        let loads = Cell::new(0);
        let listed = scan(
            vec![a.clone()],
            &known,
            |_| None,
            |_| {
                loads.set(loads.get() + 1);
                Some(Ok(vec!["Fresh".to_string()]))
            },
            |_, _, _| true,
            "build 1",
        );
        assert_eq!((loads.get(), listed[0].plugins.clone()), (1, vec!["Fresh".to_string()]), "the file is listed again");
        write(&file, &listed);
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        assert_eq!(v["version"], 2, "{text}");
        assert_eq!(read::<String>(&file).len(), 1, "the rewritten file is read back");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_reply_reads_back_past_other_output() {
        let ok = reply::<String, String>(Ok(vec!["Reverb".into()]));
        let text = format!(
            "plugin says hello
{ok}
"
        );
        let json = text.lines().rev().find_map(|l| l.strip_prefix(REPLY)).unwrap_or_default();
        let r: Reply<String> = serde_json::from_str(json).unwrap_or(Reply { plugins: None, error: None });
        assert_eq!(r.plugins, Some(vec!["Reverb".to_string()]));
        let err = reply::<String, String>(Err("broken".into()));
        let r: Reply<String> = serde_json::from_str(err.trim_start_matches(REPLY)).unwrap_or(Reply { plugins: None, error: None });
        assert_eq!(r.error.as_deref(), Some("broken"));
    }

    #[test]
    fn a_missing_scan_program_is_a_load_error() {
        let r = load_in_child::<String>(Path::new("no-such-transit-scanner.exe"), "--scan", Path::new("x.plug"), || false);
        assert!(r.is_some_and(|r| r.is_err_and(|e| e.contains("did not start"))));
    }

    #[test]
    fn a_child_that_answers_and_then_hangs_is_not_waited_for() {
        // Like a plugin hanging in its library unload: the answer, then no exit.
        let answer = reply::<String, String>(Ok(vec!["Bite".into()]));
        #[cfg(windows)]
        let (program, flag, script) = ("powershell", "-Command", format!("Write-Output '{answer}'; Start-Sleep 60"));
        #[cfg(not(windows))]
        let (program, flag, script) = ("sh", "-c", format!("echo '{answer}'; exec sleep 60"));
        let started = Instant::now();
        let r = load_in_child::<String>(Path::new(program), flag, Path::new(&script), || false);
        assert_eq!(r, Some(Ok(vec!["Bite".to_string()])));
        assert!(started.elapsed() < Duration::from_secs(30), "waited {:?}", started.elapsed());
    }
}
