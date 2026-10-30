//! The run loop's one system call, `poll(2)`: the only `unsafe` of [`super`] (ADR 0013).

use std::io;

pub(super) use libc::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT, pollfd};

/// `poll(2)` over `fds` for at most `timeout_ms` milliseconds (0: do not wait). Returns how many
/// entries have a nonzero `revents`.
///
/// # Errors
/// The `errno` of a failed call (`EINTR` when a signal interrupted it).
pub(super) fn poll(fds: &mut [pollfd], timeout_ms: i32) -> io::Result<usize> {
    let n = libc::nfds_t::try_from(fds.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: `fds` is a valid, writable slice of `n` initialized `pollfd`s that lives for the
    // whole call; poll(2) reads `fd` and `events` of those `n` entries and writes only their
    // `revents`. Any `fd` value is accepted: a negative one is skipped and a closed one is reported
    // as POLLNVAL, never dereferenced. A zero `n` with a non-null pointer is valid too.
    let r = unsafe { libc::poll(fds.as_mut_ptr(), n, timeout_ms) };
    usize::try_from(r).map_err(|_| io::Error::last_os_error())
}
