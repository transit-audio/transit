//! The `extern "C"` functions libjack calls (ADR 0021, decision 2), each under `catch_unwind` so
//! nothing unwinds into C. Each reads the [`CallbackState`] its `arg` points to and hands the work
//! to the safe engine and handlers of `crate::engine`; none allocates, locks, logs or formats.
#![deny(clippy::disallowed_macros, clippy::disallowed_methods)]

use crate::engine::{BusyFlag, Clock, Cycle, Engine, InputPlanes, OutputPlanes, Shared};
use jack_sys as j;
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

/// What every callback of one client reads: the `arg` libjack passes back.
///
/// It holds raw client and port pointers and the engine in an `UnsafeCell`, so it is not `Sync` by
/// itself. It is shared across threads (the process thread and the notification thread): the
/// notification callbacks touch only `shared` (atomics); the engine is touched only by the process
/// callback, under the `busy` flag.
pub(crate) struct CallbackState {
    shared: Arc<Shared>,
    client: *mut j::jack_client_t,
    /// The client's output ports, `out_1` first, registered before activation.
    outputs: Box<[*mut j::jack_port_t]>,
    /// The client's input ports, `in_1` first, registered before activation.
    inputs: Box<[*mut j::jack_port_t]>,
    /// Taken (acquire compare-exchange) by the process callback for the engine; released after.
    busy: BusyFlag,
    engine: UnsafeCell<Engine>,
    /// Tests: where `Drop` says the state was freed.
    #[cfg(test)]
    pub(crate) drop_log: Option<crate::api::CloseLog>,
}

#[cfg(test)]
impl Drop for CallbackState {
    fn drop(&mut self) {
        if let Some(log) = &self.drop_log {
            log.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push("state freed");
        }
    }
}

// SAFETY: the raw pointers are libjack handles of one open client: jack.h ties none of the calls
// made through them (jack_port_get_buffer, jack_get_cycle_times) to a thread, and they stay valid
// until jack_client_close, after which this state is freed. The `UnsafeCell` is reached only by
// the process callback under `busy` (acquire to take, release to give back), so no two references
// to the engine exist at once; the engine's user callbacks are `Send`.
unsafe impl Sync for CallbackState {}
// SAFETY: as above; the state is created on a control thread and freed there after the close.
unsafe impl Send for CallbackState {}

impl CallbackState {
    pub(crate) fn new(
        shared: Arc<Shared>,
        client: *mut j::jack_client_t,
        outputs: Box<[*mut j::jack_port_t]>,
        inputs: Box<[*mut j::jack_port_t]>,
        engine: Engine,
    ) -> Self {
        Self {
            shared,
            client,
            outputs,
            inputs,
            busy: BusyFlag::default(),
            engine: UnsafeCell::new(engine),
            #[cfg(test)]
            drop_log: None,
        }
    }
}

/// The state behind a callback's `arg`.
///
/// # Safety
///
/// `arg` must be the pointer `crate::ffi` passed with the callback: a `CallbackState` leaked by
/// `Box::into_raw` and freed only after `jack_client_close` returned, so it is alive for every call
/// libjack makes. Only the process callback reaches its engine, under `busy`.
unsafe fn state<'a>(arg: *mut c_void) -> &'a CallbackState {
    // SAFETY: the caller's contract above; libjack passes `arg` back unchanged (jack.h,
    // jack_set_process_callback: "passing @a arg as the second argument"; the other setters "passing
    // @a arg as a parameter" or "@param arg The arguments for the jack_info_shutdown function").
    unsafe { &*arg.cast::<CallbackState>() }
}

/// The port buffers of one process callback.
struct PortPlanes<'a> {
    ports: &'a [*mut j::jack_port_t],
    frames: j::jack_nframes_t,
}

impl PortPlanes<'_> {
    /// The buffer of port `index` for this cycle, as a pointer.
    fn buffer(&self, index: usize) -> Option<*mut f32> {
        let port = *self.ports.get(index)?;
        // SAFETY: jack.h, jack_port_get_buffer: "This returns a pointer to the memory area associated
        // with the specified port. For an output port, it will be a memory area that can be written
        // to; for an input port, it will be an area containing the data from the port's
        // connection(s), or zero-filled. if there are multiple inbound connections, the data will be
        // mixed appropriately." and "Port buffers have to be retrieved in each callback for proper
        // functioning." `port` is a port this client registered (alive until jack_client_close), the
        // call is made inside the process callback that received `frames`, and nothing caches the
        // address past it.
        let buffer = unsafe { j::jack_port_get_buffer(port, self.frames) }.cast::<f32>();
        (!buffer.is_null()).then_some(buffer)
    }
}

impl OutputPlanes for PortPlanes<'_> {
    fn count(&self) -> usize {
        self.ports.len()
    }

    fn plane(&mut self, index: usize) -> Option<&mut [f32]> {
        let buffer = self.buffer(index)?;
        // SAFETY: an audio port holds `frames` samples of "typedef float jack_default_audio_sample_t;"
        // (types.h, the sample type of JACK_DEFAULT_AUDIO_TYPE) for this cycle (types.h,
        // JackProcessCallback: "@pre nframes == jack_get_buffer_size()"). Each output port has its
        // own buffer, writable ("a memory area that can be written to", above). The slice borrows
        // `self` mutably, so one plane is borrowed at a time, and `PortPlanes` lives only for this
        // callback.
        Some(unsafe { std::slice::from_raw_parts_mut(buffer, self.frames as usize) })
    }
}

/// The input port buffers of one process callback (read only).
struct InPlanes<'a>(PortPlanes<'a>);

impl InputPlanes for InPlanes<'_> {
    fn count(&self) -> usize {
        self.0.ports.len()
    }

    fn plane(&self, index: usize) -> Option<&[f32]> {
        let buffer = self.0.buffer(index)?;
        // SAFETY: as for the output planes: `frames` `f32` samples for this cycle, "an area
        // containing the data from the port's connection(s), or zero-filled" (jack.h,
        // jack_port_get_buffer), only read, and borrowed for this callback only. The engine reads
        // every input (the capture callback runs first) before it writes an output.
        Some(unsafe { std::slice::from_raw_parts(buffer.cast_const(), self.0.frames as usize) })
    }
}

/// The cycle's times: `jack_get_cycle_times` and `jack_get_time`.
fn clock(client: *mut j::jack_client_t) -> Option<Clock> {
    let (mut current_frames, mut current_usecs, mut next_usecs, mut period_usecs) = (0, 0, 0, 0.0f32);
    // SAFETY: jack.h, jack_get_cycle_times: "This function may only be used from the process
    // callback." (`process` alone calls this); "If the return value is zero, the following
    // information is provided in the variables pointed to by the arguments" (four live locals of
    // the declared types; read only when it returned zero). jack-sys binds it weak (`None` when the
    // library lacks it). `client` is open while its callbacks run.
    let code = unsafe { j::jack_get_cycle_times(client, &mut current_frames, &mut current_usecs, &mut next_usecs, &mut period_usecs) };
    if code != Some(0) {
        return None;
    }
    // SAFETY: jack.h, jack_get_time: "@return return JACK's current system time in microseconds,
    // using the JACK clock source." No arguments and no client state.
    let now_usecs = unsafe { j::jack_get_time() };
    Some(Clock { current_frames, current_usecs, now_usecs })
}

/// `JackProcessCallback`: one cycle of the duplex engine (capture, then render).
///
/// # Safety
///
/// Called by libjack only, with the `arg` registered by `crate::ffi` (see [`state`]).
pub(crate) unsafe extern "C" fn process(frames: j::jack_nframes_t, arg: *mut c_void) -> c_int {
    let arrival = Instant::now();
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: libjack passes the `arg` registered with jack_set_process_callback.
        let state = unsafe { state(arg) };
        let mut outputs = PortPlanes { ports: &state.outputs, frames };
        // Released when `_busy` drops, also if the engine panics (it unwinds to the catch below).
        let Some(_busy) = state.busy.try_take() else {
            state.shared.busy_skip();
            crate::engine::silence(&mut outputs, frames as usize);
            return;
        };
        let inputs = InPlanes(PortPlanes { ports: &state.inputs, frames });
        let cycle = Cycle { frames, arrival, clock: clock(state.client) };
        // SAFETY: `busy` was taken above and `_busy` holds it to the end of this closure, so this is
        // the only reference to the engine until then (see `CallbackState`).
        let engine = unsafe { &mut *state.engine.get() };
        engine.process(cycle, &mut outputs, &inputs);
    }));
    // types.h: "@return zero on success, non-zero on error"; a non-zero return would make the server
    // drop the client. A user callback's panic is caught inside the engine and reported by the
    // watcher instead.
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
