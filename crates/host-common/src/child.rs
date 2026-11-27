//! Running one scan child (a listing or a probe) to its answer: contained before it loads
//! anything, its output read without blocking into bounded storage, its whole tree ended before it
//! is reaped, and nothing of it (process, pipe, buffer, thread) left once its run is over.

use crate::scan_cache::{Deadline, REPLY};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;
use transit_rt::child_tree::{Output, Read, Tree};

/// Longest line of a child's output kept (a listing's answer of a large bundle is one line of a
/// few hundred kilobytes); a longer line is an error, not more memory.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// Most output read from one child; more is an error.
pub const MAX_OUTPUT: usize = 64 * 1024 * 1024;

/// Names the parent process of a scan child (its id), set by [`run`] when it starts one.
const PARENT_ENV: &str = "TRANSIT_SCAN_PARENT";

/// A child's output as it arrives: the first complete answer line, and the line being read. Other
/// lines are dropped. A line longer than [`MAX_LINE`] or output past [`MAX_OUTPUT`] in all is an
/// error, so storage never grows past those bounds.
pub(crate) struct Collector {
    line: Vec<u8>,
    reply: Option<String>,
    total: usize,
    max_line: usize,
    max_total: usize,
}

impl Collector {
    pub(crate) fn new(max_line: usize, max_total: usize) -> Self {
        Collector { line: Vec::new(), reply: None, total: 0, max_line, max_total }
    }

    /// Takes the next bytes of output.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.total = self.total.saturating_add(bytes.len());
        if self.total > self.max_total {
            return Err(format!("its output passed {} bytes", self.max_total));
        }
        let mut rest = bytes;
        while let Some(at) = rest.iter().position(|&b| b == b'\n') {
            self.extend(&rest[..at])?;
            self.end_line();
            rest = &rest[at + 1..];
        }
        self.extend(rest)
    }

    /// The output ended: an unterminated last line counts as a line.
    pub(crate) fn finish(&mut self) {
        if !self.line.is_empty() {
            self.end_line();
        }
    }

    pub(crate) fn reply(&self) -> Option<&str> {
        self.reply.as_deref()
    }

    fn extend(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.line.len() + bytes.len() > self.max_line {
            return Err(format!("a line of its output is longer than {} bytes", self.max_line));
        }
        self.line.extend_from_slice(bytes);
        Ok(())
    }

    fn end_line(&mut self) {
        // Bytes, converted lossily: a plug-in printing non-UTF-8 cannot hide the answer.
        let s = String::from_utf8_lossy(&self.line);
        if self.reply.is_none()
            && let Some(json) = s.trim_end_matches('\r').strip_prefix(REPLY)
        {
            self.reply = Some(json.to_string());
        }
        self.line.clear();
    }
}

/// Runs `program args` and returns the JSON of its answer line.
///
/// The child is started held back ([`enter_child`]) and contained with everything it will start
/// (`transit_rt::child_tree`); only then is it told "go". When it cannot be contained it is killed
/// before it loads anything and the run is an error. Its output is read without blocking, into
/// bounded storage ([`Collector`]). The run ends at the first answer line (a plug-in that answers
/// and then hangs in its unload must not cost the whole timeout), when the child ends, when
/// `deadline` passes or when `stop` turns true; in every case the child's tree is killed while
/// the child is still unreaped (so its group id is still its own), what is already in the pipe is
/// read once more (a complete answer there still counts), and then the child is reaped and the
/// pipe closed.
pub(crate) fn run(
    program: &Path,
    args: &[&OsStr],
    file: &Path,
    deadline: &mut dyn Deadline,
    mut stop: impl FnMut() -> bool,
) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let fail = |m: String| format!("{}: {m}", file.display());
    let mut command = Command::new(program);
    command.args(args).env(PARENT_ENV, std::process::id().to_string()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(feature = "test-hooks")]
    if hooks::watch_fails(args) {
        command.env(hooks::WATCH_FAILS_ENV, "1");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW`: no console window flashes up for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    transit_rt::child_tree::prepare(&mut command);
    let mut child = command.spawn().map_err(|e| fail(format!("the scan process did not start: {e}")))?;
    let contained = Tree::contain(&child);
    #[cfg(feature = "test-hooks")]
    let contained = match hooks::contain_fails(args) {
        true => contained.and(Err(std::io::Error::other("containment failure injected by a test"))),
        false => contained,
    };
    let tree = match contained {
        Ok(tree) => tree,
        Err(e) => {
            // Not told "go": it has loaded nothing. A tree that was made is dropped (closed).
            let _ = child.kill();
            let _ = child.wait();
            return Err(fail(format!("the scan process could not be contained ({e}); it was stopped before loading anything")));
        }
    };
    let end = |child: &mut std::process::Child| {
        // The tree first, while the child is unreaped and its id still names its group.
        tree.kill();
        let _ = child.kill();
        let _ = child.wait();
    };
    if let Some(mut input) = child.stdin.take() {
        use std::io::Write as _;
        let _ = input.write_all(b"go\n");
    }
    let mut output = match child.stdout.take().map(Output::new) {
        Some(Ok(output)) => output,
        Some(Err(e)) => {
            end(&mut child);
            return Err(fail(format!("the scan process's output cannot be read: {e}")));
        }
        None => {
            end(&mut child);
            return Err(fail("no output from the scan process".into()));
        }
    };
    let mut collected = Collector::new(MAX_LINE, MAX_OUTPUT);
    let mut buf = vec![0u8; 64 * 1024];
    let mut eof = false;
    // Reads what is in the pipe now; `eof` turns true at the end of the output.
    let mut pump = |output: &mut Output, collected: &mut Collector, eof: &mut bool| -> Result<(), String> {
        while !*eof {
            match output.read_available(&mut buf).map_err(|e| format!("its output cannot be read: {e}"))? {
                Read::Bytes(n) => collected.push(&buf[..n])?,
                Read::Nothing => return Ok(()),
                Read::End => {
                    collected.finish();
                    *eof = true;
                }
            }
        }
        Ok(())
    };
    let status = loop {
        if let Err(e) = pump(&mut output, &mut collected, &mut eof) {
            end(&mut child);
            return Err(fail(e));
        }
        if collected.reply().is_some() {
            end(&mut child);
            break None;
        }
        match transit_rt::child_tree::exited(&mut child) {
            Ok(true) => {
                // Whatever the child started ends with it, before the child is reaped.
                tree.kill();
                // What it wrote before it ended is read to the end of the output, which a process
                // outside the tree (one that left it on purpose) can hold off: collection also
                // ends at the answer, the deadline or a stop.
                while !eof && collected.reply().is_none() && !deadline.passed() && !stop() {
                    if let Err(e) = pump(&mut output, &mut collected, &mut eof) {
                        let _ = child.wait();
                        return Err(fail(e));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                break child.wait().ok();
            }
            Ok(false) => {}
            Err(e) => {
                end(&mut child);
                return Err(fail(format!("the scan process's state cannot be read: {e}")));
            }
        }
        if deadline.passed() || stop() {
            end(&mut child);
            // A complete answer already in the pipe still counts.
            let _ = pump(&mut output, &mut collected, &mut eof);
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(output);
    drop(tree);
    match (collected.reply(), status) {
        (Some(json), _) => Ok(json.to_string()),
        (None, None) => Err(fail(format!("still loading after {} s; stopped", deadline.limit().as_secs()))),
        (None, Some(status)) => Err(fail(format!("the plugin ended the scan process ({status})"))),
    }
}

/// First thing in a scan child's work (the listing and probe replies call it): when this process
/// was started by [`run`], ties it to its parent (`transit_rt::child_tree::exit_with_parent`) and
/// waits until the parent says "go" on standard input, which it does once the child is contained.
/// When the parent watch cannot start, or the parent is gone before "go" (end of file, or anything
/// else), this process ends before it loads anything; for the former it first answers the error.
/// In any other process it does nothing.
pub fn enter_child() {
    let Some(parent) = std::env::var(PARENT_ENV).ok().and_then(|v| v.parse::<u32>().ok()) else { return };
    let watch = transit_rt::child_tree::exit_with_parent(parent);
    #[cfg(feature = "test-hooks")]
    let watch = match std::env::var_os(hooks::WATCH_FAILS_ENV) {
        Some(_) => watch.and(Err(std::io::Error::other("parent watch failure injected by a test"))),
        None => watch,
    };
    if let Err(e) = watch {
        use std::io::Write as _;
        let answer = serde_json::json!({ "error": format!("the scan process cannot watch its parent ({e}); it loaded nothing") });
        let _ = writeln!(std::io::stdout(), "{REPLY}{answer}");
        let _ = std::io::stdout().flush();
        std::process::exit(0);
    }
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(_) if line.trim() == "go" => {}
        _ => std::process::exit(0),
    }
}

/// Failure seams for the scan tests (feature `test-hooks`, off in every normal build).
#[cfg(feature = "test-hooks")]
pub mod hooks {
    use std::sync::Mutex;

    /// Set in a child's environment to make its parent watch fail ([`set_watch_fails`]).
    pub(crate) const WATCH_FAILS_ENV: &str = "TRANSIT_SCAN_TEST_WATCH_FAILS";

    static CONTAIN_FAILS: Mutex<Option<String>> = Mutex::new(None);
    static WATCH_FAILS: Mutex<Option<String>> = Mutex::new(None);

    /// Makes containment fail for every child whose first argument is `flag` (a listing or a probe
    /// flag); `None` stops it.
    pub fn set_contain_fails(flag: Option<&str>) {
        *CONTAIN_FAILS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = flag.map(str::to_string);
    }

    /// Makes every child whose first argument is `flag`, started from now on, find that its
    /// parent watch cannot start; `None` stops it.
    pub fn set_watch_fails(flag: Option<&str>) {
        *WATCH_FAILS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = flag.map(str::to_string);
    }

    fn matches(seam: &Mutex<Option<String>>, args: &[&std::ffi::OsStr]) -> bool {
        let wanted = seam.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        wanted.is_some_and(|f| args.first().is_some_and(|a| *a == f.as_str()))
    }

    pub(crate) fn contain_fails(args: &[&std::ffi::OsStr]) -> bool {
        matches(&CONTAIN_FAILS, args)
    }

    pub(crate) fn watch_fails(args: &[&std::ffi::OsStr]) -> bool {
        matches(&WATCH_FAILS, args)
    }
}

#[cfg(test)]
#[path = "child_tests.rs"]
mod tests;
