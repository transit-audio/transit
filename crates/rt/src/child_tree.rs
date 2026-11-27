//! Containing a helper child process and everything it starts (ADR 0064, decision 2.4: the
//! plug-in scan's listing and probe children). A plug-in loaded in such a child can hang, start
//! helper processes of its own, or outlive the app; the parent's own timeout protects nothing once
//! the parent is gone. This module ties the child's whole process tree to the parent:
//!
//! - **Windows:** each child is assigned to a job object of its own with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Processes the child starts afterwards are in the job too
//!   (a process cannot leave a job that does not allow breakaway, and this one does not).
//!   [`Tree::kill`] terminates the job; dropping the [`Tree`] closes the job handle, which kills
//!   whatever is left. When the parent ends in any way, a crash included, the system closes its
//!   handles and so kills the tree.
//! - **Linux and macOS:** the child runs in a process group of its own ([`prepare`]), so its
//!   descendants share it unless they move to another group or session on purpose.
//!   [`Tree::kill`] sends `SIGKILL` to the group, and only while the child that leads it is still
//!   this process's unreaped child (so the group id cannot have been given to another group):
//!   observe the child's end with [`exited`], which does not reap it, kill the tree, then wait for
//!   the child. In the child, [`exit_with_parent`] starts a thread that checks about every 200 ms
//!   whether its parent is still the process that started it; once the parent is gone (a crash or
//!   an ordinary exit) it sends `SIGKILL` to its own group, itself included.
//!
//! What it does not cover: on Linux and macOS, a descendant that leaves the group (`setsid`,
//! `setpgid`), and the child running on for a short while after its parent's death (the check
//! interval, as the system schedules it); on Windows, a child that was not yet assigned when it
//! started processes (callers hold the child back until [`Tree::contain`] returned, see
//! `transit_host_common`). Anything else: nothing is contained (the functions do nothing).
//!
//! [`Output`] reads a child's standard output without blocking, so that reading ends whenever its
//! caller stops calling, whoever else still holds the pipe.

use std::process::{Child, ChildStdout, Command};

/// Sets up `command` so that the child it starts can be contained: on Linux and macOS a process
/// group of its own (the child's id is the group's id); nothing elsewhere.
pub fn prepare(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// A contained child's process tree. Dropping it kills what is left of the tree (on Linux and
/// macOS only while its child is unreaped, as [`Tree::kill`]).
///
/// On Linux and macOS the guarantee that a kill reaches this tree and nothing else holds only
/// while the caller has not reaped the child that leads it and nothing else in the process waits
/// for children (no other `wait`, no reaping `SIGCHLD` disposition): the group id is the child's,
/// and only an unreaped child keeps it reserved. [`Tree::kill`] checks that the child is still an
/// unreaped child of this process; it cannot tell the original child from a later child that was
/// given the same id after a reap by someone else.
pub struct Tree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    group: i32,
    /// The group was signalled already.
    #[cfg(unix)]
    killed: std::sync::atomic::AtomicBool,
}

// SAFETY: the job handle is an owned kernel handle, usable from any thread; it is closed once, in
// `Drop`.
#[cfg(windows)]
unsafe impl Send for Tree {}

impl Tree {
    /// Contains `child`, started from a command passed to [`prepare`]. Windows: a new job object
    /// that kills its processes when its last handle closes, with the child assigned to it.
    /// Linux and macOS: the child's process group.
    ///
    /// # Errors
    /// Windows: the system's error when the job could not be made, limited or the child assigned
    /// to it; the child is then not contained and must not be let run (callers kill it). Linux and
    /// macOS: a child id out of range for a process group.
    pub fn contain(child: &Child) -> std::io::Result<Tree> {
        imp::contain(child)
    }

    /// Kills every process of the tree that is still running. Never blocks. Linux and macOS: the
    /// group is signalled once, and only while the child that leads it has not been reaped (call
    /// it before waiting for the child: after [`exited`] said it ended, before `Child::wait`); a
    /// call after the child was reaped signals nothing and is counted ([`refused_kills`]).
    pub fn kill(&self) {
        imp::kill(self);
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        imp::close(self);
    }
}

/// Whether `child` has ended, without reaping it on Linux and macOS (its id, and its group's, stay
/// this process's until it is waited for, so [`Tree::kill`] still names its group).
///
/// # Errors
/// The system's error when the child's state cannot be read.
pub fn exited(child: &mut Child) -> std::io::Result<bool> {
    imp::exited(child)
}

/// How many [`Tree::kill`] calls signalled nothing because the tree's child had been reaped
/// already (a caller that waited before killing). Linux and macOS; 0 elsewhere.
#[must_use]
pub fn refused_kills() -> usize {
    REFUSED.load(std::sync::atomic::Ordering::SeqCst)
}

static REFUSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// In a contained child: ends this process and its process group once the process `parent` is
/// no longer its parent (it ended, a crash included). Linux and macOS only; on Windows the job of
/// [`Tree::contain`] does this, and the call does nothing.
///
/// # Errors
/// Linux and macOS: the watch thread could not be started. The caller must then end before it does
/// anything its parent's death should end.
pub fn exit_with_parent(parent: u32) -> std::io::Result<()> {
    imp::exit_with_parent(parent)
}

/// What one [`Output::read_available`] call found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    /// This many bytes were read into the buffer.
    Bytes(usize),
    /// Nothing to read right now; the pipe is still open.
    Nothing,
    /// Every writer has closed the pipe.
    End,
}

/// A child's standard output, read without ever blocking. Dropping it closes this process's end
/// of the pipe.
pub struct Output {
    out: ChildStdout,
}

impl Output {
    /// Takes the child's standard output. Linux and macOS: the pipe is switched to non-blocking.
    ///
    /// # Errors
    /// Linux and macOS: the system's error when the pipe's mode cannot be changed.
    pub fn new(out: ChildStdout) -> std::io::Result<Output> {
        imp::nonblocking(&out)?;
        Ok(Output { out })
    }

    /// Reads what can be read now into `buf`, without waiting.
    ///
    /// # Errors
    /// A read error other than "nothing yet" or "interrupted".
    pub fn read_available(&mut self, buf: &mut [u8]) -> std::io::Result<Read> {
        imp::read_available(&mut self.out, buf)
    }
}

#[cfg(windows)]
mod imp {
    use super::{Read, Tree};
    use std::io::Read as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_BROKEN_PIPE, GetLastError, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    pub fn contain(child: &std::process::Child) -> std::io::Result<Tree> {
        // SAFETY: both arguments may be null (default security, no name); the call has no other
        // preconditions and returns an owned handle or null.
        let job: HANDLE = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // From here on `tree` owns the handle and closes it on every return path.
        let tree = Tree { job };
        // SAFETY: plain data (integers and nested structs of integers); all-zero is valid and
        // means "no limits".
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(u32::MAX);
        // SAFETY: `job` is the live job handle just created; the pointer is to a local
        // `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` of exactly `size` bytes, which is the type the
        // `JobObjectExtendedLimitInformation` class takes, and it lives across the call.
        if unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&raw const limits).cast(), size) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `job` is live; `child.as_raw_handle()` is the process handle `Child` owns and
        // keeps open while `child` is borrowed, so it is valid for the call.
        if unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(tree)
    }

    pub fn kill(tree: &Tree) {
        // SAFETY: `tree.job` is the live job handle this `Tree` owns (closed only in `close`, by
        // `Drop`); terminating a job whose processes have ended already is harmless. A job handle
        // names this job alone for as long as it is open, so no other process can be reached.
        unsafe { TerminateJobObject(tree.job, 1) };
    }

    pub fn close(tree: &mut Tree) {
        // SAFETY: the handle is owned by this `Tree` and closed exactly once, here, from `Drop`.
        // When the kill-on-close limit was set, closing it ends whatever is left in the job; on
        // `contain`'s error paths (the limit or the assignment failed) no child was assigned to
        // the job yet, so closing it leaves nothing behind either.
        unsafe { CloseHandle(tree.job) };
    }

    pub fn exited(child: &mut std::process::Child) -> std::io::Result<bool> {
        // Windows keeps the process's id while `Child` holds its handle: checking does no harm.
        Ok(child.try_wait()?.is_some())
    }

    // The same signature on every system (the Unix ones can fail).
    #[allow(clippy::unnecessary_wraps)]
    pub fn exit_with_parent(_parent: u32) -> std::io::Result<()> {
        Ok(())
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn nonblocking(_out: &std::process::ChildStdout) -> std::io::Result<()> {
        Ok(())
    }

    pub fn read_available(out: &mut std::process::ChildStdout, buf: &mut [u8]) -> std::io::Result<Read> {
        let mut available: u32 = 0;
        // SAFETY: `out.as_raw_handle()` is the pipe's read handle, owned by `out` and open for the
        // call; no buffer is passed (null, size 0) and the other null pointers are optional
        // outputs; `available` is a writable `u32`.
        let peeked =
            unsafe { PeekNamedPipe(out.as_raw_handle(), std::ptr::null_mut(), 0, std::ptr::null_mut(), &raw mut available, std::ptr::null_mut()) };
        if peeked == 0 {
            // SAFETY: reads the calling thread's last error, set by the failed call just above.
            return if unsafe { GetLastError() } == ERROR_BROKEN_PIPE { Ok(Read::End) } else { Err(std::io::Error::last_os_error()) };
        }
        if available == 0 || buf.is_empty() {
            return Ok(Read::Nothing);
        }
        // `available` bytes are in the pipe, so this read returns at once.
        let want = buf.len().min(usize::try_from(available).unwrap_or(usize::MAX));
        match out.read(&mut buf[..want]) {
            Ok(0) => Ok(Read::End),
            Ok(n) => Ok(Read::Bytes(n)),
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(Read::End),
            Err(e) => Err(e),
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{REFUSED, Read, Tree};
    use std::io::Read as _;
    use std::os::fd::AsRawFd as _;

    pub fn contain(child: &std::process::Child) -> std::io::Result<Tree> {
        let group = i32::try_from(child.id()).map_err(|_| std::io::Error::other("process id out of range"))?;
        Ok(Tree { group, killed: std::sync::atomic::AtomicBool::new(false) })
    }

    /// `waitid` on `pid` without reaping and without waiting: `Some(true)` when it is this
    /// process's child and has ended (a zombie), `Some(false)` when it is this process's child and
    /// runs, `None` when it is not (or no longer) this process's child: it was reaped.
    fn unreaped(pid: i32) -> Option<bool> {
        let id = libc::id_t::try_from(pid).ok()?;
        // SAFETY: plain data; all-zero is valid (and means "no child has changed state").
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: `waitid` with `P_PID` looks only at `id`; `WNOWAIT` leaves the child waitable
        // (nothing is reaped), `WNOHANG` returns at once; `info` is a writable `siginfo_t`.
        let r = unsafe { libc::waitid(libc::P_PID, id, &raw mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
        if r != 0 {
            return None;
        }
        // SAFETY: the pid field is initialized: by `waitid` when it reports an ended child, and
        // otherwise by the zeroing above (on a "nothing has ended" return some systems leave
        // `info` untouched).
        Some(unsafe { info.si_pid() } != 0)
    }

    pub fn kill(tree: &Tree) {
        if tree.group <= 1 || tree.killed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        // The group's id is the child's: it names this tree's group only while the child is this
        // process's unreaped child (a zombie keeps its id and its group's reserved).
        if unreaped(tree.group).is_none() {
            REFUSED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            return;
        }
        // SAFETY: a negative pid addresses the process group `-pid`. Just above, `waitid` showed
        // that the group's leader (id `tree.group`, greater than 1, so never "every process") is
        // still this process's unreaped child. Nothing reaps it but its own `Child` (this process
        // installs no `SIGCHLD` disposition that reaps and waits for no other children), and the
        // caller waits for it only after this call, so the id still names the group that
        // `prepare` made for this child and no other.
        unsafe { libc::kill(-tree.group, libc::SIGKILL) };
    }

    pub fn close(tree: &mut Tree) {
        kill(tree);
    }

    pub fn exited(child: &mut std::process::Child) -> std::io::Result<bool> {
        let pid = i32::try_from(child.id()).map_err(|_| std::io::Error::other("process id out of range"))?;
        unreaped(pid).ok_or_else(|| std::io::Error::other("not an unreaped child of this process"))
    }

    pub fn exit_with_parent(parent: u32) -> std::io::Result<()> {
        let watch = move || {
            loop {
                if std::os::unix::process::parent_id() != parent {
                    // SAFETY: pid 0 addresses the caller's own process group, which `prepare`
                    // made this child's own; `SIGKILL` ends it and every process still in it.
                    unsafe { libc::kill(0, libc::SIGKILL) };
                    std::process::abort();
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        };
        std::thread::Builder::new().name("parent-watch".into()).spawn(watch).map(drop)
    }

    pub fn nonblocking(out: &std::process::ChildStdout) -> std::io::Result<()> {
        let fd = out.as_raw_fd();
        // SAFETY: `fd` is the pipe's read end, owned by `out` and open for the call; `F_GETFL`
        // takes no argument.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: as above; `F_SETFL` takes the new status flags as an int.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn read_available(out: &mut std::process::ChildStdout, buf: &mut [u8]) -> std::io::Result<Read> {
        if buf.is_empty() {
            return Ok(Read::Nothing);
        }
        match out.read(buf) {
            Ok(0) => Ok(Read::End),
            Ok(n) => Ok(Read::Bytes(n)),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => Ok(Read::Nothing),
            Err(e) => Err(e),
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use super::{Read, Tree};

    pub fn contain(_child: &std::process::Child) -> std::io::Result<Tree> {
        Ok(Tree {})
    }
    pub fn kill(_tree: &Tree) {}
    pub fn close(_tree: &mut Tree) {}
    pub fn exited(child: &mut std::process::Child) -> std::io::Result<bool> {
        Ok(child.try_wait()?.is_some())
    }
    pub fn exit_with_parent(_parent: u32) -> std::io::Result<()> {
        Ok(())
    }
    pub fn nonblocking(_out: &std::process::ChildStdout) -> std::io::Result<()> {
        Ok(())
    }
    pub fn read_available(out: &mut std::process::ChildStdout, buf: &mut [u8]) -> std::io::Result<Read> {
        use std::io::Read as _;
        match out.read(buf)? {
            0 => Ok(Read::End),
            n => Ok(Read::Bytes(n)),
        }
    }
}

#[cfg(test)]
#[path = "child_tree_tests.rs"]
mod tests;
