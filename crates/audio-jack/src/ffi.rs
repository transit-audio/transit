//! Every libjack call (ADR 0021, decision 2), through `jack-sys` with run-time loading: the first
//! call dlopens `libjack.so.0` (JACK 2's, or PipeWire's `pipewire-jack`), nothing links against it.
//! Each `unsafe` block quotes the jack.h (or types.h) rule it relies on.
//!
//! `jack-sys` resolves every function it binds when the library is first used and panics if one is
//! missing; [`LibJack::open_client`] makes that first use under `catch_unwind` and reports it as a
//! missing library, so no later call can panic for that reason.

use crate::api::{ClientError, JackApi, JackClient, PhysicalPort};
use crate::callback::{self, CallbackState};
use crate::engine::Shared;
use jack_sys as j;
use std::ffi::{CStr, CString, c_char, c_ulong, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

/// `JACK_DEFAULT_AUDIO_TYPE` (types.h: `#define JACK_DEFAULT_AUDIO_TYPE "32 bit float mono audio"`).
const AUDIO_TYPE: &CStr = c"32 bit float mono audio";
/// `jack_connect`'s "connection is already made" (Linux `EEXIST`).
const EEXIST: i32 = 17;

/// The real libjack.
pub(crate) struct LibJack;

impl JackApi for LibJack {
    fn open_client(&self, name: &str) -> Result<Box<dyn JackClient>, ClientError> {
        if let Err(e) = j::library() {
            return Err(ClientError::LibraryMissing(e.to_string()));
        }
        let c_name = CString::new(name).map_err(|_| ClientError::Failed(format!("the client name {name:?} contains a NUL")))?;
        let mut status: j::jack_status_t = 0;
        let opened = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: jack.h, jack_client_open: "@param client_name of at most
            // jack_client_name_size() characters" (a short constant, NUL-terminated, alive across
            // the call); "@param status (if non-NULL) an address for JACK to return information
            // from the open operation" (a live local); "@param options formed by OR-ing together
            // @ref JackOptions bits. Only the @ref JackOpenOptions bits are allowed." We pass
            // JackNoStartServer alone (types.h: "Do not automatically start the JACK server when it
            // is not already running"), so per "depending on corresponding [@a options bits]
            // additional parameters may follow @a status" no variadic argument follows; jack-sys
            // declares the function without its `...`, which on the x86_64 and aarch64 Linux ABIs
            // passes the named arguments exactly as a variadic call with none would.
            unsafe { j::jack_client_open(c_name.as_ptr(), j::JackNoStartServer, &mut status) }
        }));
        let Ok(client) = opened else {
            return Err(ClientError::LibraryMissing("libjack.so.0 lacks a function jack-sys binds".into()));
        };
        if client.is_null() {
            // jack.h: "If this is NULL, the open operation failed, @a *status includes @ref
            // JackFailure and the caller is not a JACK client." types.h, JackServerFailed:
            // "Unable to connect to the JACK server."
            return Err(if status & j::JackServerFailed != 0 {
                ClientError::ServerNotRunning
            } else {
                ClientError::Failed(format!("jack_client_open failed (status 0x{status:x})"))
            });
        }
        Ok(Box::new(LibClient { client, inputs: Vec::new(), outputs: Vec::new(), state: std::ptr::null_mut() }))
    }
}

/// One open client of the real server.
struct LibClient {
    /// Non-null until [`LibClient::close_now`].
    client: *mut j::jack_client_t,
    inputs: Vec<*mut j::jack_port_t>,
    outputs: Vec<*mut j::jack_port_t>,
    /// The `arg` of every callback: a `Box` leaked at `activate` (null before) and freed only after
    /// `jack_client_close` returned. A raw pointer, not a `Box`, because libjack's threads read it
    /// while this value moves.
    state: *mut CallbackState,
}

// SAFETY: jack.h ties no call on a client to the thread that opened it; its one thread rule for the
// calls this type makes is in jack_on_info_shutdown: "application should typically signal another
// thread to correctly finish cleanup, that is by calling "jack_client_close" (since
// "jack_client_close" cannot be called directly in the context of the thread that calls the shutdown
// callback)". This type is never touched from a JACK callback (`crate::callback` reads only the
// `CallbackState`, through its own pointer), and the backend keeps a `LibClient` behind its session
// `Mutex`, so one thread at a time uses the client and port pointers.
unsafe impl Send for LibClient {}

/// A C string from libjack as an owned `String` (empty for null).
fn owned(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: every caller passes a string libjack returned for a live client or port: jack.h
    // documents each as a NUL-terminated C string ("@return pointer to actual client name", "@return
    // the full name of the jack_port_t", the entries of jack_get_ports' "NULL-terminated array of
    // ports"), valid until the client, port or array is released, which happens after this copy.
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

impl LibClient {
    /// `jack_get_ports` for physical audio ports with `flags`, copied, the array freed.
    fn ports(&self, flags: c_ulong, capture: bool) -> Vec<PhysicalPort> {
        let flags = flags | c_ulong::from(j::JackPortIsPhysical);
        // SAFETY: jack.h, jack_get_ports: "@param port_name_pattern A regular expression used to
        // select ports by name. If NULL or of zero length, no selection based on name will be
        // carried out." (NULL); "@param type_name_pattern" (our NUL-terminated audio type);
        // "@return a NULL-terminated array of ports that match the specified arguments. The caller
        // is responsible for calling jack_free() any non-NULL returned value." `self.client` is open.
        let array = unsafe { j::jack_get_ports(self.client, std::ptr::null(), AUDIO_TYPE.as_ptr(), flags) };
        let mut found = Vec::new();
        if array.is_null() {
            return found;
        }
        for i in 0.. {
            // SAFETY: the array is NULL-terminated (jack.h, above) and `i` stops at that entry, so
            // every read is inside it.
            let entry = unsafe { *array.add(i) };
            if entry.is_null() {
                break;
            }
            found.push(PhysicalPort { name: owned(entry), capture });
        }
        // SAFETY: jack.h, jack_free: "The free function to be used on memory returned by
        // jack_port_get_connections, jack_port_get_all_connections, jack_get_ports and
        // jack_get_internal_client_name functions." `array` came from jack_get_ports, is non-null,
        // is freed once, and no entry is used afterwards (each was copied above).
        unsafe { j::jack_free(array.cast::<c_void>()) };
        found
    }

    /// `jack_client_close` and then the callback state; idempotent.
    fn close_now(&mut self) -> Result<(), String> {
        if self.client.is_null() {
            return Ok(());
        }
        // SAFETY: jack.h, jack_client_close: "Disconnects an external client from a JACK server."
        // `self.client` is open and closed once (nulled below). It is never called from a JACK
        // callback (types.h, JackInfoShutdownCallback: "jack_client_close() cannot be safely used
        // inside the shutdown callback and has to be called outside of the callback context"): the
        // backend closes on a control thread or on its watcher thread.
        let code = unsafe { j::jack_client_close(self.client) };
        self.client = std::ptr::null_mut();
        self.inputs.clear();
        self.outputs.clear();
        // The callbacks' `arg` is freed only now that the server has let go of the client.
        let state = std::mem::replace(&mut self.state, std::ptr::null_mut());
        if !state.is_null() {
            // SAFETY: `state` came from `Box::into_raw` in `activate` and is freed once (the field is
            // nulled above). jack_client_close has returned, so libjack calls none of this client's
            // callbacks any more and no other pointer to it is used.
            drop(unsafe { Box::from_raw(state) });
        }
        if code == 0 { Ok(()) } else { Err(format!("jack_client_close returned {code}")) }
    }
}

impl JackClient for LibClient {
    fn name(&self) -> String {
        // SAFETY: jack.h, jack_get_client_name: "@return pointer to actual client name." The
        // client is open; the string is copied before any other call.
        owned(unsafe { j::jack_get_client_name(self.client) })
    }

    fn sample_rate(&self) -> u32 {
        // SAFETY: jack.h, jack_get_sample_rate: "@return the sample rate of the jack system". The
        // client is open.
        let rate = unsafe { j::jack_get_sample_rate(self.client) };
        // jack.h returns `jack_nframes_t` (uint32_t); jack-sys declares `c_int`, the same 32 bits.
        rate.cast_unsigned()
    }

    fn buffer_size(&self) -> u32 {
        // SAFETY: jack.h, jack_get_buffer_size: "@return the current maximum size that will ever be
        // passed to the @a process_callback." The client is open.
        unsafe { j::jack_get_buffer_size(self.client) }
    }

    /// Capture ports are JACK outputs (audio leaves them), playback ports JACK inputs.
    fn physical_ports(&self) -> Vec<PhysicalPort> {
        let mut ports = self.ports(c_ulong::from(j::JackPortIsOutput), true);
        ports.extend(self.ports(c_ulong::from(j::JackPortIsInput), false));
        ports
    }

    fn register_port(&mut self, short: &str, input: bool) -> Result<String, String> {
        let c_short = CString::new(short).map_err(|_| format!("the port name {short:?} contains a NUL"))?;
        let flags = c_ulong::from(if input { j::JackPortIsInput } else { j::JackPortIsOutput });
        // SAFETY: jack.h, jack_port_register: "@param port_name non-empty short name for the new port
        // (not including the leading @a "client_name:"). Must be unique." (`in_n`/`out_n`, once
        // each); "@param port_type port type name" (JACK_DEFAULT_AUDIO_TYPE, a built-in type);
        // "@param buffer_size must be non-zero if this is not a built-in @a port_type. Otherwise, it
        // is ignored." (0). "@return jack_port_t pointer on success, otherwise NULL." The client is
        // open; both strings are NUL-terminated and outlive the call.
        let port = unsafe { j::jack_port_register(self.client, c_short.as_ptr(), AUDIO_TYPE.as_ptr(), flags, 0) };
        if port.is_null() {
            return Err("jack_port_register failed".into());
        }
        if input {
            self.inputs.push(port)
        } else {
            self.outputs.push(port)
        }
        // SAFETY: jack.h, jack_port_name: "@return the full name of the jack_port_t (including the
        // @a "client_name:" prefix)." `port` was just registered on this open client.
        Ok(owned(unsafe { j::jack_port_name(port) }))
    }

    fn activate(&mut self, shared: Arc<Shared>) -> Result<(), String> {
        if !self.state.is_null() {
            return Err("the callbacks are installed once per client".into());
        }
        self.state = Box::into_raw(Box::new(CallbackState::new(shared, self.outputs.clone().into_boxed_slice())));
        // Every callback gets `arg`, the leaked state, which stays allocated until `close_now` has
        // returned from jack_client_close.
        let arg = self.state.cast::<c_void>();
        // SAFETY: jack.h, jack_set_process_callback: "Tell the Jack server to call @a
        // process_callback whenever there is work be done, passing @a arg as the second argument.
        // The code in the supplied function must be suitable for real-time execution." (see
        // `callback::process`); "NOTE: this function cannot be called while the client is activated
        // (after jack_activate has been called.)" (it is not activated yet: `state` was null).
        let code = unsafe { j::jack_set_process_callback(self.client, Some(callback::process), arg) };
        check(code, "jack_set_process_callback")?;
        // SAFETY: jack.h, jack_set_buffer_size_callback: "Tell JACK to call @a bufsize_callback
        // whenever the size of the the buffer that will be passed to the @a process_callback is
        // about to change." and "NOTE: this function cannot be called while the client is
        // activated" (not yet). `arg` as above.
        let code = unsafe { j::jack_set_buffer_size_callback(self.client, Some(callback::buffer_size), arg) };
        check(code, "jack_set_buffer_size_callback")?;
        // SAFETY: jack.h, jack_set_sample_rate_callback: "Tell the Jack server to call @a
        // srate_callback whenever the system sample rate changes." and "NOTE: this function cannot
        // be called while the client is activated" (not yet). `arg` as above.
        let code = unsafe { j::jack_set_sample_rate_callback(self.client, Some(callback::sample_rate), arg) };
        check(code, "jack_set_sample_rate_callback")?;
        // SAFETY: jack.h, jack_set_xrun_callback: "Tell the JACK server to call @a xrun_callback
        // whenever there is a xrun, passing @a arg as a parameter." and "NOTE: this function cannot
        // be called while the client is activated" (not yet).
        let code = unsafe { j::jack_set_xrun_callback(self.client, Some(callback::xrun), arg) };
        check(code, "jack_set_xrun_callback")?;
        // SAFETY: jack.h, jack_on_info_shutdown: "Register a function (and argument) to be called if
        // and when the JACK server shuts down the client thread. The function must be written as if
        // it were an asynchonrous POSIX signal handler --- use only async-safe functions" (see
        // `callback::shutdown`: one atomic store) and "It should be called before
        // jack_client_activate()" (it is).
        unsafe { j::jack_on_info_shutdown(self.client, Some(callback::shutdown), arg) };
        // SAFETY: jack.h, jack_activate: "Tell the Jack server that the program is ready to start
        // processing audio. @return 0 on success, otherwise a non-zero error code". Every callback
        // and its `arg` are installed above; the client is open.
        let code = unsafe { j::jack_activate(self.client) };
        check(code, "jack_activate")
    }

    fn connect(&mut self, source: &str, destination: &str) -> Result<(), String> {
        let c_source = CString::new(source).map_err(|_| format!("{source:?} contains a NUL"))?;
        let c_destination = CString::new(destination).map_err(|_| format!("{destination:?} contains a NUL"))?;
        // SAFETY: jack.h, jack_connect: "@pre The port types must be identical." (both audio);
        // "@pre The @ref JackPortFlags of the @a source_port must include @ref JackPortIsOutput."
        // and of "the @a destination_port must include @ref JackPortIsInput." (a physical capture
        // port or our `out_n` as the source, our `in_n` or a physical playback port as the
        // destination); "@return 0 on success, EEXIST if the connection is already made, otherwise
        // a non-zero error code". The client is open and active; both names are NUL-terminated.
        let code = unsafe { j::jack_connect(self.client, c_source.as_ptr(), c_destination.as_ptr()) };
        if code == 0 || code == EEXIST { Ok(()) } else { Err(format!("jack_connect returned {code}")) }
    }

    fn deactivate(&mut self) -> Result<(), String> {
        // SAFETY: jack.h, jack_deactivate: "Tell the Jack server to remove this @a client from the
        // process graph. Also, disconnect all ports belonging to it, since inactive clients have no
        // port connections. @return 0 on success, otherwise a non-zero error code". The client is
        // open; the call is made on a control thread, never from a JACK callback.
        let code = unsafe { j::jack_deactivate(self.client) };
        check(code, "jack_deactivate")
    }

    fn close(mut self: Box<Self>) -> Result<(), String> {
        self.close_now()
    }
}

impl Drop for LibClient {
    fn drop(&mut self) {
        if let Err(e) = self.close_now() {
            log::warn!("JACK: closing a dropped client: {e}");
        }
    }
}

fn check(code: i32, what: &str) -> Result<(), String> {
    if code == 0 { Ok(()) } else { Err(format!("{what} returned {code}")) }
}
