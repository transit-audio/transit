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
//!   [`Tree::kill`] (and dropping the [`Tree`]) sends `SIGKILL` to the group. In the child,
//!   [`exit_with_parent`] starts a thread that checks every 200 ms whether its parent is still the
//!   process that started it; once the parent is gone (a crash or an ordinary exit) it sends
//!   `SIGKILL` to its own group, itself included.
//!
//! What it does not cover: on Linux and macOS, a descendant that leaves the group (`setsid`,
//! `setpgid`) and, between the parent's death and the next check, up to 200 ms of the child
//! running on; on Windows, a child that was not yet assigned when it started processes (callers
//! hold the child back until [`Tree::contain`] returned, see `transit_host_common::scan_cache`).
//! Anything else: nothing is contained (the functions do nothing).

use std::process::{Child, Command};

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

/// A contained child's process tree. Dropping it kills what is left of the tree.
pub struct Tree {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    group: i32,
    /// The group was signalled already: a later call must not signal an id the system may have
    /// given to another group meanwhile.
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
    /// to it (the caller decides what to do with an uncontained child). Linux and macOS: a child
    /// id out of range for a process group.
    pub fn contain(child: &Child) -> std::io::Result<Tree> {
        imp::contain(child)
    }

    /// Kills every process of the tree that is still running. Never blocks. Linux and macOS: only
    /// the first call signals the group (call it before the child is waited for, so that its id
    /// still names this group).
    pub fn kill(&self) {
        imp::kill(self);
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        imp::close(self);
    }
}

/// In a contained child: ends this process and its process group once the process `parent` is
/// no longer its parent (it ended, a crash included). Linux and macOS only; on Windows the job of
/// [`Tree::contain`] does this, and the call does nothing.
pub fn exit_with_parent(parent: u32) {
    imp::exit_with_parent(parent);
}

#[cfg(windows)]
mod imp {
    use super::Tree;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

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
        // `Drop`); terminating a job whose processes have ended already is harmless.
        unsafe { TerminateJobObject(tree.job, 1) };
    }

    pub fn close(tree: &mut Tree) {
        // SAFETY: the handle is owned by this `Tree` and closed exactly once, here, from `Drop`;
        // the job's kill-on-close limit then ends whatever is left in it.
        unsafe { CloseHandle(tree.job) };
    }

    pub fn exit_with_parent(_parent: u32) {}
}

#[cfg(unix)]
mod imp {
    use super::Tree;

    pub fn contain(child: &std::process::Child) -> std::io::Result<Tree> {
        let group = i32::try_from(child.id()).map_err(|_| std::io::Error::other("process id out of range"))?;
        Ok(Tree { group, killed: std::sync::atomic::AtomicBool::new(false) })
    }

    pub fn kill(tree: &Tree) {
        if tree.group <= 1 || tree.killed.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        // SAFETY: `kill` takes any pid and signal number; a negative pid addresses the process
        // group `-pid`, here the group the child was started in (`prepare`), whose id is the
        // child's own id and greater than 1 (checked), so it never addresses every process.
        unsafe { libc::kill(-tree.group, libc::SIGKILL) };
    }

    pub fn close(tree: &mut Tree) {
        kill(tree);
    }

    pub fn exit_with_parent(parent: u32) {
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
        if let Err(e) = std::thread::Builder::new().name("parent-watch".into()).spawn(watch) {
            log::warn!("no parent watch thread: {e}");
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod imp {
    use super::Tree;

    pub fn contain(_child: &std::process::Child) -> std::io::Result<Tree> {
        Ok(Tree {})
    }
    pub fn kill(_tree: &Tree) {}
    pub fn close(_tree: &mut Tree) {}
    pub fn exit_with_parent(_parent: u32) {}
}

#[cfg(test)]
#[path = "child_tree_tests.rs"]
mod tests;
