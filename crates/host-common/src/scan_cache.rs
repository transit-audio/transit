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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Bumped when the layout changes; a file of another version is ignored (everything is loaded).
const VERSION: u32 = 1;

/// Largest cache file read (a plugin takes a few hundred bytes).
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Starts the line a child process answers with (plugins may print to standard output too).
const REPLY: &str = "SOUNDCRAFT-SCAN ";

/// Longest wait for a child process; a plugin that takes longer (or waits on a dialog) is killed.
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

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
    let mut stopped = false;
    let r = run_child(program, flag, file, || {
        stopped = stop();
        stopped
    });
    (!stopped).then_some(r)
}

fn run_child<D: DeserializeOwned>(program: &Path, flag: &str, file: &Path, mut stop: impl FnMut() -> bool) -> Result<Vec<D>, String> {
    use std::process::{Command, Stdio};
    let fail = |m: String| format!("{}: {m}", file.display());
    let mut command = Command::new(program);
    command.arg(flag).arg(file).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW`: no console window flashes up for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|e| fail(format!("the scan process did not start: {e}")))?;
    // Read while it runs, so that a chatty plugin cannot fill the pipe and stall it. The reader
    // also flags the answer line the moment it arrives: a plugin that answers and then hangs the
    // process on exit (Native Instruments' plug-ins do, in their library unload) must not cost
    // the whole timeout, so the parent stops waiting on the answer, not on the exit.
    let out = child.stdout.take().ok_or_else(|| fail("no output from the scan process".into()))?;
    let answered = Arc::new(AtomicBool::new(false));
    let reader = {
        let answered = Arc::clone(&answered);
        std::thread::spawn(move || {
            use std::io::BufRead as _;
            let mut reader = std::io::BufReader::new(out);
            let mut text = String::new();
            let mut line = Vec::new();
            // Bytes, converted lossily: a plugin printing non-UTF-8 cannot hide the answer.
            while let Ok(n) = reader.read_until(b'\n', &mut line) {
                if n == 0 {
                    break;
                }
                let s = String::from_utf8_lossy(&line);
                if s.starts_with(REPLY) {
                    answered.store(true, Ordering::Release);
                }
                text.push_str(&s);
                line.clear();
            }
            text
        })
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            // Answered: the plugin's start-up is done; its unload is not waited for.
            Ok(None) if answered.load(Ordering::Acquire) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) if started.elapsed() < CHILD_TIMEOUT && !stop() => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = reader.join().unwrap_or_default();
    let answer = text.lines().rev().find_map(|l| l.strip_prefix(REPLY));
    match (answer, status) {
        (Some(json), _) => {
            let r: Reply<D> = serde_json::from_str(json).map_err(|e| fail(format!("unreadable scan reply: {e}")))?;
            match (r.plugins, r.error) {
                (Some(plugins), _) => Ok(plugins),
                (None, e) => Err(e.unwrap_or_else(|| fail("the scan process found nothing".into()))),
            }
        }
        (None, None) => Err(fail(format!("still loading after {} s; stopped", CHILD_TIMEOUT.as_secs()))),
        (None, Some(status)) => Err(fail(format!("the plugin ended the scan process ({status})"))),
    }
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
