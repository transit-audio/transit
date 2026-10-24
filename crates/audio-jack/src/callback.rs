//! The `extern "C"` functions libjack calls (ADR 0021, decision 2), each under `catch_unwind` so
//! nothing unwinds into C. Each reads the [`CallbackState`] its `arg` points to and hands the work
//! to the safe handlers of `crate::engine`; none allocates, locks, logs or formats.
#![deny(clippy::disallowed_macros, clippy::disallowed_methods)]

use crate::engine::{self, OutputPlanes, Shared};
use jack_sys as j;
use std::ffi::{c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

/// What every callback of one client reads: the `arg` libjack passes back.
///
/// It holds raw port pointers, so it is not `Sync` by itself. It is shared across threads (the
/// process thread and the notification thread) strictly read-only: nothing mutates it while it is
/// shared, and its mutable parts are atomics, so there is no data race.
pub(crate) struct CallbackState {
    shared: Arc<Shared>,
    /// The client's output ports, `out_1` first, registered before activation.
    outputs: Box<[*mut j::jack_port_t]>,
}

impl CallbackState {
    pub(crate) fn new(shared: Arc<Shared>, outputs: Box<[*mut j::jack_port_t]>) -> Self {
        Self { shared, outputs }
    }
}

/// The state behind a callback's `arg`.
///
/// # Safety
///
/// `arg` must be the pointer `crate::ffi` passed with the callback: a `CallbackState` leaked by
/// `Box::into_raw` and freed only after `jack_client_close` returned, so it is alive for every call
/// libjack makes. Nothing mutates it while it is shared.
unsafe fn state<'a>(arg: *mut c_void) -> &'a CallbackState {
    // SAFETY: the caller's contract above; libjack passes `arg` back unchanged (jack.h,
    // jack_set_process_callback: "passing @a arg as the second argument"; the other setters "passing
    // @a arg as a parameter" or "@param arg The arguments for the jack_info_shutdown function").
    unsafe { &*arg.cast::<CallbackState>() }
}

/// The output port buffers of one process callback.
struct PortPlanes<'a> {
    ports: &'a [*mut j::jack_port_t],
    frames: j::jack_nframes_t,
}

impl OutputPlanes for PortPlanes<'_> {
    fn count(&self) -> usize {
        self.ports.len()
    }

    fn plane(&mut self, index: usize) -> Option<&mut [f32]> {
        let port = *self.ports.get(index)?;
        // SAFETY: jack.h, jack_port_get_buffer: "This returns a pointer to the memory area associated
        // with the specified port. For an output port, it will be a memory area that can be written
        // to" and "Port buffers have to be retrieved in each callback for proper functioning." `port`
        // is an output port this client registered (alive until jack_client_close), the call is
        // made inside the process callback that received `frames`, and nothing caches the address
        // past it.
        let buffer = unsafe { j::jack_port_get_buffer(port, self.frames) }.cast::<f32>();
        if buffer.is_null() {
            return None;
        }
        // SAFETY: an audio port holds `frames` samples of "typedef float jack_default_audio_sample_t;"
        // (types.h, the sample type of JACK_DEFAULT_AUDIO_TYPE) for this cycle (types.h,
        // JackProcessCallback: "@pre nframes == jack_get_buffer_size()"). The slice borrows `self`
        // mutably, so one plane is borrowed at a time, and `PortPlanes` lives only for this callback.
        Some(unsafe { std::slice::from_raw_parts_mut(buffer, self.frames as usize) })
    }
}

/// `JackProcessCallback`: counts the cycle and writes silence to the output ports.
///
/// # Safety
///
/// Called by libjack only, with the `arg` registered by `crate::ffi` (see [`state`]).
pub(crate) unsafe extern "C" fn process(frames: j::jack_nframes_t, arg: *mut c_void) -> c_int {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libjack passes the `arg` registered with jack_set_process_callback.
        let state = unsafe { state(arg) };
        let mut planes = PortPlanes { ports: &state.outputs, frames };
        engine::process(&state.shared, frames, &mut planes);
    }));
    // types.h: "@return zero on success, non-zero on error"; a non-zero return would make the server
    // drop the client, and the shell has nothing to fail.
    0
}

/// `JackBufferSizeCallback`: stores the new quantum.
///
/// # Safety
///
/// Called by libjack only, with the `arg` registered by `crate::ffi` (see [`state`]).
pub(crate) unsafe extern "C" fn buffer_size(frames: j::jack_nframes_t, arg: *mut c_void) -> c_int {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libjack passes the `arg` registered with jack_set_buffer_size_callback.
        unsafe { state(arg) }.shared.on_buffer_size(frames);
    }));
    0
}

/// `JackSampleRateCallback`: stores the new rate.
///
/// # Safety
///
/// Called by libjack only, with the `arg` registered by `crate::ffi` (see [`state`]).
pub(crate) unsafe extern "C" fn sample_rate(rate: j::jack_nframes_t, arg: *mut c_void) -> c_int {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libjack passes the `arg` registered with jack_set_sample_rate_callback.
        unsafe { state(arg) }.shared.on_sample_rate(rate);
    }));
    0
}

/// `JackXRunCallback`: counts the xrun.
///
/// # Safety
///
/// Called by libjack only, with the `arg` registered by `crate::ffi` (see [`state`]).
pub(crate) unsafe extern "C" fn xrun(arg: *mut c_void) -> c_int {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libjack passes the `arg` registered with jack_set_xrun_callback.
        unsafe { state(arg) }.shared.on_xrun();
    }));
    0
}

/// `JackInfoShutdownCallback`: one atomic store, as jack.h requires of it ("as if it were an
/// asynchonrous POSIX signal handler --- use only async-safe functions"). `reason` is not read.
///
/// # Safety
///
/// Called by libjack only, with the `arg` registered by `crate::ffi` (see [`state`]).
pub(crate) unsafe extern "C" fn shutdown(_code: j::jack_status_t, _reason: *const c_char, arg: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libjack passes the `arg` registered with jack_on_info_shutdown.
        unsafe { state(arg) }.shared.on_shutdown();
    }));
}
