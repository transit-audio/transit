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

    /// Takes the next bytes of output. Once an answer is held the rest is not looked at (later
    /// output can neither replace the answer nor turn the run into an error).
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.reply.is_some() {
            return Ok(());
        }
        self.total = self.total.saturating_add(bytes.len());
        if self.total > self.max_total {
            return Err(format!("its output passed {} bytes", self.max_total));
        }
        let mut rest = bytes;
        while let Some(at) = rest.iter().position(|&b| b == b'\n') {
            self.extend(&rest[..at])?;
            self.end_line();
            if self.reply.is_some() {
                return Ok(());
            }
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
    #[allow(unused_mut)]
    let mut go: &[u8] = GO;
    #[cfg(feature = "test-hooks")]
    if hooks::watch_fails(args) {
        command.env(WATCH_FAILS_ENV, "1");
        go = ARMED_GO;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        /// `CREATE_NO_WINDOW`: no console window flashes up for the child.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    transit_rt::child_tree::prepare(&mut command);
    let child = command.spawn().map_err(|e| fail(format!("the scan process did not start: {e}")))?;
    let contained = Tree::contain(&child);
    #[cfg(feature = "test-hooks")]
    let contained = match hooks::contain_fails(args) {
        true => contained.and(Err(std::io::Error::other("containment failure injected by a test"))),
        false => contained,
    };
    // From here on the guard ends the child, however this function is left.
    let mut running = Running { child, tree: None, reaped: false };
    match contained {
        Ok(tree) => running.tree = Some(tree),
        Err(e) => {
            // Not told "go": it has loaded nothing. A tree that was made is dropped (closed).
            running.end();
            return Err(fail(format!("the scan process could not be contained ({e}); it was stopped before loading anything")));
        }
    }
    if let Some(mut input) = running.child.stdin.take() {
        use std::io::Write as _;
        let _ = input.write_all(go);
    }
    let mut output = match running.child.stdout.take().map(Output::new) {
        Some(Ok(output)) => output,
        Some(Err(e)) => return Err(fail(format!("the scan process's output cannot be read: {e}"))),
        None => return Err(fail("no output from the scan process".into())),
    };
    let mut collected = Collector::new(MAX_LINE, MAX_OUTPUT);
    let mut buf = vec![0u8; 64 * 1024];
    let mut eof = false;
    // Reads what is in the pipe now, until it is empty, the output ends (`eof`) or an answer is
    // held (the rest is not read).
    let mut pump = |output: &mut Output, collected: &mut Collector, eof: &mut bool| -> Result<(), String> {
        while !*eof && collected.reply().is_none() {
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
        pump(&mut output, &mut collected, &mut eof).map_err(fail)?;
        if collected.reply().is_some() {
            running.end();
            break None;
        }
        match transit_rt::child_tree::exited(&mut running.child) {
            Ok(true) => {
                // Whatever the child started ends with it, before the child is reaped.
                running.kill_tree();
                // What it wrote before it ended is read (at least once, whatever the deadline or
                // a stop say: a complete answer there counts), and on to the end of the output,
                // which a process outside the tree (one that left it on purpose) can hold off:
                // collection also ends at the answer, the deadline or a stop.
                loop {
                    pump(&mut output, &mut collected, &mut eof).map_err(fail)?;
                    if eof || collected.reply().is_some() || deadline.passed() || stop() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                break running.reap();
            }
            Ok(false) => {}
            Err(e) => return Err(fail(format!("the scan process's state cannot be read: {e}"))),
        }
        if deadline.passed() || stop() {
            running.end();
            // A complete answer already in the pipe still counts.
            let _ = pump(&mut output, &mut collected, &mut eof);
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(output);
    drop(running);
    match (collected.reply(), status) {
        (Some(json), _) => Ok(json.to_string()),
        (None, None) => Err(fail(format!("still loading after {} s; stopped", deadline.limit().as_secs()))),
        (None, Some(status)) => Err(fail(format!("the plugin ended the scan process ({status})"))),
    }
}

/// A started scan child and its tree. However a run is left (a return, an error, or a panic in a
/// deadline or stop callback), dropping it ends the child as [`Running::end`] does: no process
/// and no zombie is left.
struct Running {
    child: std::process::Child,
    tree: Option<Tree>,
    reaped: bool,
}

impl Running {
    /// Kills the tree while the child is unreaped (its id still names its group).
    fn kill_tree(&self) {
        if let Some(tree) = &self.tree {
            tree.kill();
        }
    }

    /// Kills the tree (first), then the child, and reaps it. Once.
    fn end(&mut self) {
        if self.reaped {
            return;
        }
        self.kill_tree();
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
    }

    /// Reaps a child that ended by itself (its tree killed first).
    fn reap(&mut self) -> Option<std::process::ExitStatus> {
        self.kill_tree();
        self.reaped = true;
        self.child.wait().ok()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.end();
    }
}

/// The handshake that lets a contained child go on.
const GO: &[u8] = b"go\n";

/// The handshake that also arms the parent watch failure seam of a test-hooks build: the seam acts
/// only when the child's environment names it and this handshake arrives, so the environment alone
/// (inherited by a test-hooks build of the app) cannot trigger it.
const ARMED_GO: &[u8] = b"go watch-fails\n";

/// Names the watch failure seam in a child's environment (test-hooks builds read it).
#[cfg(feature = "test-hooks")]
const WATCH_FAILS_ENV: &str = "TRANSIT_SCAN_TEST_WATCH_FAILS";

/// Whether the watch failure seam acts: only with both its environment variable and the armed
/// handshake.
#[cfg(any(test, feature = "test-hooks"))]
fn watch_armed(env: Option<&OsStr>, handshake: &str) -> bool {
    env.is_some() && handshake.as_bytes() == ARMED_GO
}

/// Whether a handshake line lets the child go on.
fn accepted(handshake: &str) -> bool {
    handshake.as_bytes() == GO || (cfg!(feature = "test-hooks") && handshake.as_bytes() == ARMED_GO)
}

fn read_handshake() -> String {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line
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
    // A test-hooks build reads the handshake first when the seam's variable is set: the seam acts
    // only on the armed handshake. Either way nothing is loaded before the checks below.
    #[cfg(feature = "test-hooks")]
    let (watch, handshake) = match std::env::var_os(WATCH_FAILS_ENV) {
        Some(env) => {
            let line = read_handshake();
            let armed = watch_armed(Some(&env), &line);
            (if armed { watch.and(Err(std::io::Error::other("parent watch failure injected by a test"))) } else { watch }, Some(line))
        }
        None => (watch, None),
    };
    if let Err(e) = watch {
        use std::io::Write as _;
        let answer = serde_json::json!({ "error": format!("the scan process cannot watch its parent ({e}); it loaded nothing") });
        let _ = writeln!(std::io::stdout(), "{REPLY}{answer}");
        let _ = std::io::stdout().flush();
        std::process::exit(0);
    }
    #[cfg(feature = "test-hooks")]
    let line = handshake.unwrap_or_else(read_handshake);
    #[cfg(not(feature = "test-hooks"))]
    let line = read_handshake();
    if !accepted(&line) {
        std::process::exit(0);
    }
}

/// Failure seams for the scan tests (feature `test-hooks`, off in every normal build).
#[cfg(feature = "test-hooks")]
pub mod hooks {
    use std::sync::Mutex;

    static CONTAIN_FAILS: Mutex<Option<String>> = Mutex::new(None);
    static WATCH_FAILS: Mutex<Option<String>> = Mutex::new(None);

    /// Makes containment fail for every child whose first argument is `flag` (a listing or a probe
    /// flag); `None` stops it.
    pub fn set_contain_fails(flag: Option<&str>) {
        *CONTAIN_FAILS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = flag.map(str::to_string);
    }

    /// Makes every child whose first argument is `flag`, started from now on, find that its
    /// parent watch cannot start (through its environment and the armed handshake); `None` stops
    /// it.
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
