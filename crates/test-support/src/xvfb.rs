//! A private X server for tests that must own their display (#697).
//!
//! A test on a shared display meets the display's other clients: a window manager configures the
//! test's top-levels on its own schedule, and one still handling a closed client's window acts on a
//! test window that was given the same id (E74). [`PrivateXvfb`] starts an `Xvfb` that only the
//! test talks to: no window manager and no other client, so every configure is the test's own.
//!
//! Shared by transit-rt's `editor_x11` runner and transit-clap-host's XEmbed end-to-end test.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, ExitStatus, Stdio};

/// A running private `Xvfb`, killed and reaped on drop.
#[derive(Debug)]
pub struct PrivateXvfb {
    child: Child,
    display: String,
}

impl PrivateXvfb {
    /// Starts a private `Xvfb`; `None` when `Xvfb` is not installed. It listens on the abstract
    /// socket only (`-nolisten unix`: `WSLg` mounts `/tmp/.X11-unix` read-only) and runs with
    /// `-noreset` (E68: by default it resets when its last client leaves, dropping the connections
    /// that arrive meanwhile). Panics when it starts but reports no display.
    pub fn start() -> Option<PrivateXvfb> {
        let mut child = Command::new("Xvfb")
            .args(["-displayfd", "1", "-nolisten", "tcp", "-nolisten", "unix", "-noreset", "-screen", "0", "1280x1024x24"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut line = String::new();
        let read = child.stdout.take().map(|out| BufReader::new(out).read_line(&mut line));
        let number = line.trim().to_owned();
        if !matches!(read, Some(Ok(_))) || number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Xvfb did not report its display");
        }
        Some(PrivateXvfb { child, display: format!(":{number}") })
    }

    /// The display name for `DISPLAY`, for example `:99`.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// Runs the current executable again with `args` on this display, with `marker` set to `1` so
    /// the run knows it is the one on the private server, and without `WAYLAND_DISPLAY`. Its output
    /// goes straight to this process's stdout and stderr.
    pub fn rerun_current_exe(&self, args: &[String], marker: &str) -> ExitStatus {
        let exe = std::env::current_exe().unwrap_or_else(|e| panic!("the current test executable: {e}"));
        Command::new(exe)
            .args(args)
            .env("DISPLAY", &self.display)
            .env(marker, "1")
            .env_remove("WAYLAND_DISPLAY")
            .status()
            .unwrap_or_else(|e| panic!("the test executable did not start again: {e}"))
    }
}

impl Drop for PrivateXvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
