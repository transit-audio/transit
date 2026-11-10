//! Every libjack call (ADR 0021, decision 2), through transit's own function table (#430): the
//! library is loaded at run time with `dlopen` (nothing links against it) and every function is
//! resolved with `dlsym` on THAT library's handle ([`JackFns::load`]). Each `unsafe` block quotes
//! the jack.h (or types.h) rule it relies on.
//!
//! Which library (#230, #430): [`choose_library`] probes the candidates of `crate::library` in
//! order: each is loaded, its table resolved (a library lacking a required function is unusable),
//! and a probe client opened and closed through that table with `JackNoStartServer`. The first that
//! connects is bound for the life of the process ([`BOUND`], set once, never replaced), so two
//! libjacks are never both bound. Resolving through the handle, not by the soname `libjack.so.0`,
//! is what lets PipeWire's library serve the process when jackd2's is installed and already mapped
//! (by an earlier probe, or by anything else in the process such as ALSA's JACK plug-in): the
//! loader's soname lookup returns the first object of that name mapped, whichever it is.
//!
//! No library this module loads is ever unloaded, the bound one or a probed one: a library whose
//! code may still run (a callback, a thread a failed open left behind) must not be unmapped, and
//! jackd2's libjack stays mapped after `dlclose` anyway.

use crate::api::{ClientError, JackApi, JackClient, PhysicalPort};
use crate::callback::{self, CallbackState};
use crate::engine::{Engine, Shared};
use crate::library;
use crate::sys::{self, JackFns};
use std::ffi::{CStr, CString, c_char, c_int, c_ulong, c_void};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// `JACK_DEFAULT_AUDIO_TYPE` (types.h: `#define JACK_DEFAULT_AUDIO_TYPE "32 bit float mono audio"`).
const AUDIO_TYPE: &CStr = c"32 bit float mono audio";
/// `jack_connect`'s "connection is already made" (Linux `EEXIST`).
const EEXIST: i32 = 17;

/// The real libjack.
pub(crate) struct LibJack;

/// The functions of the library the process is bound to. Set once, by the first selection whose
/// probe connected; never replaced or cleared.
static BOUND: OnceLock<JackFns> = OnceLock::new();
/// The last selection made (for the Audio Devices line); the lock also serializes selections.
static LAST: Mutex<Option<library::Selection>> = Mutex::new(None);

/// A symbol's address as the pointer type `T`, `None` when the library does not export it.
///
/// # Safety
///
/// `T` must be the function pointer type of `name`'s jack.h prototype, and the pointer may be called
/// only while `library` stays loaded (in this module: forever, nothing is unloaded).
unsafe fn symbol<T: Copy>(library: &libloading::Library, name: &CStr) -> Option<T> {
    // SAFETY: the caller's contract above. libloading's `get` returns an error, not a dangling
    // symbol, when the name is missing; the pointer is copied out of the `Symbol` (a `Copy` function
    // pointer), valid while the library is mapped.
    unsafe { library.get::<T>(name.to_bytes_with_nul()) }.ok().map(|f| *f)
}

/// [`symbol`] for a function every libjack exports: its absence makes the library unusable.
///
/// # Safety
///
/// As for [`symbol`].
unsafe fn required<T: Copy>(library: &libloading::Library, name: &CStr) -> Result<T, String> {
    // SAFETY: the caller's contract, which is `symbol`'s.
    unsafe { symbol(library, name) }.ok_or_else(|| format!("lacks {}", name.to_string_lossy()))
}

impl JackFns {
    /// Resolves every function from `library`'s own handle (`dlsym` on it: its own definitions, not
    /// another object's of the same name). `Err` names the first required function it lacks.
    ///
    /// # Safety
    ///
    /// `library` must be a libjack (its exports have jack.h's prototypes) and must never be unloaded
    /// while the table or a copy of it is used.
    unsafe fn load(library: &libloading::Library) -> Result<JackFns, String> {
        // SAFETY: each name is paired with the field whose type sys.rs declares from that name's
        // jack.h prototype (jack_get_cycle_times and jack_get_version_string are
        // JACK_OPTIONAL_WEAK_EXPORT there, hence optional here); the caller keeps the library loaded.
        unsafe {
            Ok(JackFns {
                client_open: required(library, c"jack_client_open")?,
                client_close: required(library, c"jack_client_close")?,
                get_client_name: required(library, c"jack_get_client_name")?,
                get_sample_rate: required(library, c"jack_get_sample_rate")?,
                get_buffer_size: required(library, c"jack_get_buffer_size")?,
                get_ports: required(library, c"jack_get_ports")?,
                free: required(library, c"jack_free")?,
                port_register: required(library, c"jack_port_register")?,
                port_name: required(library, c"jack_port_name")?,
                set_process_callback: required(library, c"jack_set_process_callback")?,
                set_buffer_size_callback: required(library, c"jack_set_buffer_size_callback")?,
                set_sample_rate_callback: required(library, c"jack_set_sample_rate_callback")?,
                set_xrun_callback: required(library, c"jack_set_xrun_callback")?,
                on_info_shutdown: required(library, c"jack_on_info_shutdown")?,
                activate: required(library, c"jack_activate")?,
                deactivate: required(library, c"jack_deactivate")?,
                connect: required(library, c"jack_connect")?,
                set_buffer_size: required(library, c"jack_set_buffer_size")?,
                #[cfg(test)]
                port_by_name: required(library, c"jack_port_by_name")?,
                #[cfg(test)]
                port_get_all_connections: required(library, c"jack_port_get_all_connections")?,
                port_get_buffer: required(library, c"jack_port_get_buffer")?,
                get_time: required(library, c"jack_get_time")?,
                get_cycle_times: symbol(library, c"jack_get_cycle_times"),
                get_version_string: symbol(library, c"jack_get_version_string"),
            })
        }
    }
}

/// The bound library, choosing one first when none is bound. Once one is bound no library is
/// loaded or probed again. A failed selection binds nothing: the next open probes again (a server
/// may have started since).
fn choose_library() -> Result<&'static JackFns, ClientError> {
    if let Some(bound) = BOUND.get() {
        return Ok(bound);
    }
    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(bound) = BOUND.get() {
        return Ok(bound);
    }
    let named = std::env::var(library::LIBRARY_ENV).ok();
    let candidates = library::candidates(named.as_deref(), library::pipewire_socket(), |path| std::path::Path::new(path).exists());
    let mut loader = DlLoader::default();
    let selection = library::select(&candidates, &mut loader);
    let result = match loader.connected.take() {
        Some((path, fns)) => {
            log::info!("JACK: using {path}");
            Ok(BOUND.get_or_init(|| fns))
        }
        None => {
            log::info!("JACK: {}", selection.summary());
            Err(selection.error().unwrap_or(ClientError::ServerNotRunning))
        }
    };
    *last = Some(selection);
    result
}

/// [`library::Loader`] over `dlopen`: loads each library (and keeps it loaded), resolves its table
/// and opens and closes a probe client through it with `JackNoStartServer`. Keeps the table of the
/// library that connected.
#[derive(Default)]
struct DlLoader {
    connected: Option<(String, JackFns)>,
}

impl library::Loader for DlLoader {
    fn probe(&mut self, path: &str) -> library::Outcome {
        use library::Outcome;
        // SAFETY: libloading::Library::new runs the library's initializers. Every candidate is a
        // libjack (the library a user named, the loader's libjack.so.0, or PipeWire's libjack found
        // where `pw-jack` or the loader's directories put it), whose initializers only set up its
        // own globals, as for any program linked against it.
        let lib = match unsafe { libloading::Library::new(path) } {
            Ok(lib) => lib,
            Err(e) => return Outcome::LoadFailed(e.to_string()),
        };
        // SAFETY: `lib` is a libjack (above) and is never unloaded: it is forgotten just below,
        // whatever the probe finds, so its handle is never `dlclose`d and every copied pointer stays
        // valid for the life of the process.
        let fns = unsafe { JackFns::load(&lib) };
        std::mem::forget(lib);
        let fns = match fns {
            Ok(fns) => fns,
            Err(why) => return Outcome::LoadFailed(format!("{path} {why}")),
        };
        let outcome = probe_client(&fns);
        if outcome == Outcome::Connected {
            self.connected = Some((path.to_owned(), fns));
        }
        outcome
    }
}

/// Opens and closes a probe client through `fns`, never starting a server.
fn probe_client(fns: &JackFns) -> library::Outcome {
    let mut status: sys::jack_status_t = 0;
    // SAFETY: as in `LibJack::open_client`: jack.h, jack_client_open, "@param client_name of at most
    // jack_client_name_size() characters" (a short NUL-terminated constant); "@param status (if
    // non-NULL) an address for JACK to return information" (a live local); JackNoStartServer alone,
    // so no variadic argument follows (the x86_64/aarch64 argument passing note there applies
    // unchanged). `fns` was resolved from a library that stays loaded.
    let client = unsafe { (fns.client_open)(c"transit-probe".as_ptr(), sys::JackNoStartServer, &mut status) };
    if client.is_null() {
        // jack.h: "If this is NULL, the open operation failed"; types.h, JackServerFailed: "Unable
        // to connect to the JACK server."
        return library::outcome_of_status(status);
    }
    // SAFETY: jack.h, jack_client_close: "Disconnects an external client from a JACK server."
    // `client` is the probe client just opened through this table, closed once, from this control
    // thread (never a JACK callback); no callback was installed.
    unsafe { (fns.client_close)(client) };
    library::Outcome::Connected
}

impl JackApi for LibJack {
    fn open_client(&self, name: &str) -> Result<Box<dyn JackClient>, ClientError> {
        let fns = choose_library()?;
        let c_name = CString::new(name).map_err(|_| ClientError::Failed(format!("the client name {name:?} contains a NUL")))?;
        let mut status: sys::jack_status_t = 0;
        // SAFETY: jack.h, jack_client_open: "@param client_name of at most jack_client_name_size()
        // characters" (a short constant, NUL-terminated, alive across the call); "@param status (if
        // non-NULL) an address for JACK to return information from the open operation" (a live
        // local); "@param options formed by OR-ing together @ref JackOptions bits. Only the @ref
        // JackOpenOptions bits are allowed." We pass JackNoStartServer alone (types.h: "Do not
        // automatically start the JACK server when it is not already running"), so per "depending
        // on corresponding [@a options bits] additional parameters may follow @a status" no
        // variadic argument follows; the table declares the function without its `...`. On the
        // x86_64 and aarch64 Linux ABIs the named arguments are passed in the same registers either
        // way; the one difference on x86_64 is that `%al` (the vector-register count a variadic
        // caller sets) is left unset, which a callee's `va_start` prologue only tests for zero, so
        // the worst case is a redundant register save, and no variadic argument is read here. The
        // bound library is never unloaded.
        let client = unsafe { (fns.client_open)(c_name.as_ptr(), sys::JackNoStartServer, &mut status) };
        if client.is_null() {
            // jack.h: "If this is NULL, the open operation failed, @a *status includes @ref
            // JackFailure and the caller is not a JACK client." types.h, JackServerFailed:
            // "Unable to connect to the JACK server."
            return Err(if status & sys::JackServerFailed != 0 {
                ClientError::ServerNotRunning
            } else {
                ClientError::Failed(format!("jack_client_open failed (status 0x{status:x})"))
            });
        }
        Ok(Box::new(LibClient { fns, client, inputs: Vec::new(), outputs: Vec::new(), state: std::ptr::null_mut() }))
    }

    fn library(&self) -> Option<library::Selection> {
        LAST.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn version(&self) -> Option<String> {
        // Only once a library is bound: nothing is loaded for this question alone.
        let get = BOUND.get()?.get_version_string?;
        // SAFETY: jack.h, jack_get_version_string: "@return Human readable string describing JACK
        // version being used." A static C string of the bound library, which is never unloaded (or
        // NULL, which `owned` maps to empty), copied at once.
        let text = unsafe { owned(get()) };
        (!text.is_empty()).then_some(text)
    }
}

/// One open client of the real server.
struct LibClient {
    /// The bound library's functions (never unloaded).
    fns: &'static JackFns,
    /// Non-null until [`LibClient::close_now`].
    client: *mut sys::jack_client_t,
    inputs: Vec<*mut sys::jack_port_t>,
    outputs: Vec<*mut sys::jack_port_t>,
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
///
/// # Safety
///
/// A non-null `text` must point to a NUL-terminated C string that stays valid for the whole call,
/// such as a name or port-array entry libjack returned for a live client, not yet released.
unsafe fn owned(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the caller's contract above; every call site passes a string libjack returned for a live client or port: jack.h
    // documents each as a NUL-terminated C string ("@return pointer to actual client name", "@return
    // the full name of the jack_port_t", the entries of jack_get_ports' "NULL-terminated array of
    // ports"), valid until the client, port or array is released, which happens after this copy.
    unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
}

impl LibClient {
    /// `jack_get_ports` for physical audio ports with `flags`, copied, the array freed.
    fn ports(&self, flags: c_ulong, capture: bool) -> Vec<PhysicalPort> {
        let flags = flags | sys::JackPortIsPhysical;
        // SAFETY: jack.h, jack_get_ports: "@param port_name_pattern A regular expression used to
        // select ports by name. If NULL or of zero length, no selection based on name will be
        // carried out." (NULL); "@param type_name_pattern" (our NUL-terminated audio type);
        // "@return a NULL-terminated array of ports that match the specified arguments. The caller
        // is responsible for calling jack_free() any non-NULL returned value." `self.client` is open.
        let array = unsafe { (self.fns.get_ports)(self.client, std::ptr::null(), AUDIO_TYPE.as_ptr(), flags) };
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
            // SAFETY: `entry` is a non-NULL entry of the array above, a NUL-terminated port name
            // that stays valid until `jack_free` below, after this copy.
            let name = unsafe { owned(entry) };
            found.push(PhysicalPort { name, capture });
        }
        // SAFETY: jack.h, jack_free: "The free function to be used on memory returned by
        // jack_port_get_connections, jack_port_get_all_connections, jack_get_ports and
        // jack_get_internal_client_name functions." `array` came from jack_get_ports, is non-null,
        // is freed once, and no entry is used afterwards (each was copied above).
        unsafe { (self.fns.free)(array.cast::<c_void>()) };
        found
    }

    /// `jack_client_close` and then the callback state; idempotent.
    fn close_now(&mut self) -> Result<(), String> {
        let close = self.fns.client_close;
        self.close_with(|client| {
            // SAFETY: jack.h, jack_client_close: "Disconnects an external client from a JACK server."
            // `client` is the open client of `close_with`, which calls this once and nulls the field
            // after. It is never called from a JACK callback (types.h, JackInfoShutdownCallback:
            // "jack_client_close() cannot be safely used inside the shutdown callback and has to be
            // called outside of the callback context"): the backend closes on a control thread or on
            // its watcher thread.
            unsafe { close(client) }
        })
    }

    /// [`LibClient::close_now`] with the close call injected, so a test can observe the order: the
    /// callback state must be alive while `close` runs and is freed only after it returned.
    fn close_with(&mut self, close: impl FnOnce(*mut sys::jack_client_t) -> c_int) -> Result<(), String> {
        if self.client.is_null() {
            return Ok(());
        }
        let code = close(self.client);
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
        unsafe { owned((self.fns.get_client_name)(self.client)) }
    }

    fn sample_rate(&self) -> u32 {
        // SAFETY: jack.h, jack_get_sample_rate: "@return the sample rate of the jack system". The
        // client is open.
        unsafe { (self.fns.get_sample_rate)(self.client) }
    }

    fn buffer_size(&self) -> u32 {
        // SAFETY: jack.h, jack_get_buffer_size: "@return the current maximum size that will ever be
        // passed to the @a process_callback." The client is open.
        unsafe { (self.fns.get_buffer_size)(self.client) }
    }

    /// Capture ports are JACK outputs (audio leaves them), playback ports JACK inputs.
    fn physical_ports(&self) -> Vec<PhysicalPort> {
        let mut ports = self.ports(sys::JackPortIsOutput, true);
        ports.extend(self.ports(sys::JackPortIsInput, false));
        ports
    }

    fn register_port(&mut self, short: &str, input: bool) -> Result<String, String> {
        let c_short = CString::new(short).map_err(|_| format!("the port name {short:?} contains a NUL"))?;
        let flags = if input { sys::JackPortIsInput } else { sys::JackPortIsOutput };
        // SAFETY: jack.h, jack_port_register: "@param port_name non-empty short name for the new port
        // (not including the leading @a "client_name:"). Must be unique." (`in_n`/`out_n`, once
        // each); "@param port_type port type name" (JACK_DEFAULT_AUDIO_TYPE, a built-in type);
        // "@param buffer_size must be non-zero if this is not a built-in @a port_type. Otherwise, it
        // is ignored." (0). "@return jack_port_t pointer on success, otherwise NULL." The client is
        // open; both strings are NUL-terminated and outlive the call.
        let port = unsafe { (self.fns.port_register)(self.client, c_short.as_ptr(), AUDIO_TYPE.as_ptr(), flags, 0) };
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
        Ok(unsafe { owned((self.fns.port_name)(port)) })
    }

    fn activate(&mut self, shared: Arc<Shared>, engine: Engine) -> Result<(), String> {
        if !self.state.is_null() {
            return Err("the callbacks are installed once per client".into());
        }
        let ports = (self.outputs.clone().into_boxed_slice(), self.inputs.clone().into_boxed_slice());
        self.state = Box::into_raw(Box::new(CallbackState::new(shared, self.fns, self.client, ports.0, ports.1, engine)));
        // Every callback gets `arg`, the leaked state, which stays allocated until `close_now` has
        // returned from jack_client_close.
        let arg = self.state.cast::<c_void>();
        // SAFETY: jack.h, jack_set_process_callback: "Tell the Jack server to call @a
        // process_callback whenever there is work be done, passing @a arg as the second argument.
        // The code in the supplied function must be suitable for real-time execution." (see
        // `callback::process`); "NOTE: this function cannot be called while the client is activated
        // (after jack_activate has been called.)" (it is not activated yet: `state` was null).
        let code = unsafe { (self.fns.set_process_callback)(self.client, Some(callback::process), arg) };
        check(code, "jack_set_process_callback")?;
        // SAFETY: jack.h, jack_set_buffer_size_callback: "Tell JACK to call @a bufsize_callback
        // whenever the size of the the buffer that will be passed to the @a process_callback is
        // about to change." and "NOTE: this function cannot be called while the client is
        // activated" (not yet). `arg` as above.
        let code = unsafe { (self.fns.set_buffer_size_callback)(self.client, Some(callback::buffer_size), arg) };
        check(code, "jack_set_buffer_size_callback")?;
        // SAFETY: jack.h, jack_set_sample_rate_callback: "Tell the Jack server to call @a
        // srate_callback whenever the system sample rate changes." and "NOTE: this function cannot
        // be called while the client is activated" (not yet). `arg` as above.
        let code = unsafe { (self.fns.set_sample_rate_callback)(self.client, Some(callback::sample_rate), arg) };
        check(code, "jack_set_sample_rate_callback")?;
        // SAFETY: jack.h, jack_set_xrun_callback: "Tell the JACK server to call @a xrun_callback
        // whenever there is a xrun, passing @a arg as a parameter." and "NOTE: this function cannot
        // be called while the client is activated" (not yet).
        let code = unsafe { (self.fns.set_xrun_callback)(self.client, Some(callback::xrun), arg) };
        check(code, "jack_set_xrun_callback")?;
        // SAFETY: jack.h, jack_on_info_shutdown: "Register a function (and argument) to be called if
        // and when the JACK server shuts down the client thread. The function must be written as if
        // it were an asynchonrous POSIX signal handler --- use only async-safe functions" (see
        // `callback::shutdown`: one atomic store) and "It should be called before
        // jack_client_activate()" (it is).
        unsafe { (self.fns.on_info_shutdown)(self.client, Some(callback::shutdown), arg) };
        // SAFETY: jack.h, jack_activate: "Tell the Jack server that the program is ready to start
        // processing audio. @return 0 on success, otherwise a non-zero error code". Every callback
        // and its `arg` are installed above; the client is open.
        let code = unsafe { (self.fns.activate)(self.client) };
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
        let code = unsafe { (self.fns.connect)(self.client, c_source.as_ptr(), c_destination.as_ptr()) };
        if code == 0 || code == EEXIST { Ok(()) } else { Err(format!("jack_connect returned {code}")) }
    }

    #[cfg(test)]
    fn connections(&self, port: &str) -> Vec<String> {
        let Ok(c_port) = CString::new(port) else { return Vec::new() };
        // SAFETY: jack.h, jack_port_by_name: "@return address of the jack_port_t named @a
        // port_name." (NULL when there is none). The client is open; the name is NUL-terminated.
        let handle = unsafe { (self.fns.port_by_name)(self.client, c_port.as_ptr()) };
        if handle.is_null() {
            return Vec::new();
        }
        // SAFETY: jack.h, jack_port_get_all_connections: "@return a null-terminated array of full
        // port names to which the @a port is connected. If none, returns NULL." and "You may not
        // call this function from code that is executed in response to a JACK event" (a control
        // thread calls it); "You need not be the owner of the port". `handle` came from
        // jack_port_by_name on this open client.
        let array = unsafe { (self.fns.port_get_all_connections)(self.client, handle) };
        let mut found = Vec::new();
        if array.is_null() {
            return found;
        }
        for i in 0.. {
            // SAFETY: the array is NULL-terminated (above) and `i` stops at that entry.
            let entry = unsafe { *array.add(i) };
            if entry.is_null() {
                break;
            }
            // SAFETY: a non-NULL entry of the array, a NUL-terminated port name valid until the
            // jack_free below, after this copy.
            found.push(unsafe { owned(entry) });
        }
        // SAFETY: jack.h: "The caller is responsible for calling jack_free() on any non-NULL
        // returned value." Freed once; no entry is used afterwards.
        unsafe { (self.fns.free)(array.cast::<c_void>()) };
        found
    }

    fn set_buffer_size(&mut self, frames: u32) -> Result<(), String> {
        // SAFETY: jack.h, jack_set_buffer_size: "Change the buffer size passed to the @a
        // process_callback." "@param nframes new buffer size. Must be a power of two." (any size the
        // server accepts is fine: a non-power-of-two is passed through, and the server's answer or
        // refusal shows in the achieved quantum) "@return 0 on success, otherwise a non-zero error code". The client
        // is open; the call is made on the thread that opens the client (the Player's open path), never
        // from a JACK callback.
        let code = unsafe { (self.fns.set_buffer_size)(self.client, frames) };
        check(code, "jack_set_buffer_size")
    }

    fn deactivate(&mut self) -> Result<(), String> {
        // SAFETY: jack.h, jack_deactivate: "Tell the Jack server to remove this @a client from the
        // process graph. Also, disconnect all ports belonging to it, since inactive clients have no
        // port connections. @return 0 on success, otherwise a non-zero error code". The client is
        // open; the call is made on a control thread, never from a JACK callback.
        let code = unsafe { (self.fns.deactivate)(self.client) };
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::CloseLog;
    use crate::library::{Loader, Outcome, select};
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A fake libjack, compiled by the test from this source: every function the table resolves,
    /// a `jack_client_open` that connects (returns a client) or answers as JACK 2 does with no
    /// server (NULL, status 0x11), and `jack_get_version_string` / `jack_get_client_name` returning
    /// `FAKE_ID`, so a call through a table says which library served it. No server is involved.
    const FAKE_SOURCE: &str = r#"
#include <stddef.h>
#include <stdint.h>
static int fake_client;
static int fake_port;
void *jack_client_open(const char *name, int options, int *status, ...) {
#if FAKE_CONNECTS
    if (status) *status = 0;
    return &fake_client;
#else
    if (status) *status = 0x11;
    return NULL;
#endif
}
int jack_client_close(void *client) { return 0; }
const char *jack_get_version_string(void) { return FAKE_ID; }
char *jack_get_client_name(void *client) { return (char *)FAKE_ID; }
uint32_t jack_get_sample_rate(void *client) { return 48000; }
uint32_t jack_get_buffer_size(void *client) { return 128; }
const char **jack_get_ports(void *client, const char *name, const char *type, unsigned long flags) { return NULL; }
void jack_free(void *ptr) {}
void *jack_port_register(void *client, const char *name, const char *type, unsigned long flags, unsigned long size) { return &fake_port; }
const char *jack_port_name(const void *port) { return FAKE_ID ":port"; }
int jack_set_process_callback(void *client, void *callback, void *arg) { return 0; }
int jack_set_buffer_size_callback(void *client, void *callback, void *arg) { return 0; }
int jack_set_sample_rate_callback(void *client, void *callback, void *arg) { return 0; }
int jack_set_xrun_callback(void *client, void *callback, void *arg) { return 0; }
void jack_on_info_shutdown(void *client, void *callback, void *arg) {}
int jack_activate(void *client) { return 0; }
int jack_deactivate(void *client) { return 0; }
int jack_connect(void *client, const char *source, const char *destination) { return 0; }
int jack_set_buffer_size(void *client, uint32_t frames) { return 0; }
void *jack_port_by_name(void *client, const char *name) { return NULL; }
const char **jack_port_get_all_connections(const void *client, const void *port) { return NULL; }
#ifndef FAKE_LACKS_PORT_BUFFER
void *jack_port_get_buffer(void *port, uint32_t frames) { return NULL; }
#endif
uint64_t jack_get_time(void) { return 0; }
"#;

    /// What one fake library is.
    struct Fake {
        /// The directory under the test's own folder, and the file name.
        file: &'static str,
        id: &'static str,
        connects: bool,
        lacks_port_buffer: bool,
    }

    /// Builds `fakes` under a folder of the test's own (`tag`), every one with the soname `soname`
    /// (as jackd2's and PipeWire's libjack share `libjack.so.0`), and returns their paths in order.
    /// Each test uses its own soname, so no other test's libraries are mapped under it.
    fn build(tag: &str, soname: &str, fakes: &[Fake]) -> Vec<String> {
        let root = std::env::temp_dir().join(format!("transit-jack-fakes-{}", std::process::id())).join(tag);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("fake.c");
        std::fs::write(&source, FAKE_SOURCE).unwrap();
        let compiler = std::env::var("CC").unwrap_or_else(|_| "cc".into());
        fakes
            .iter()
            .map(|fake| {
                let out: PathBuf = root.join(fake.file);
                std::fs::create_dir_all(out.parent().unwrap()).unwrap();
                let mut command = std::process::Command::new(&compiler);
                command.args(["-shared", "-fPIC", "-o"]).arg(&out).arg(format!("-Wl,-soname,{soname}"));
                command.arg(format!("-DFAKE_ID=\"{}\"", fake.id)).arg(format!("-DFAKE_CONNECTS={}", u8::from(fake.connects)));
                if fake.lacks_port_buffer {
                    command.arg("-DFAKE_LACKS_PORT_BUFFER");
                }
                let status = command.arg(&source).status().expect("a C compiler (cc) builds the fake libraries");
                assert!(status.success(), "compiling {}", fake.file);
                out.to_string_lossy().into_owned()
            })
            .collect()
    }

    /// Which library a table calls into: its `jack_get_version_string`.
    fn served_by(fns: &JackFns) -> String {
        let get = fns.get_version_string.expect("the fakes export jack_get_version_string");
        // SAFETY: a fake library's static string; the library is never unloaded (`DlLoader`).
        unsafe { owned(get()) }
    }

    /// #430, a fake pair in the probe order: jackd2's libjack (no jackd: status 0x11) first, then
    /// PipeWire's (connects), both with the same soname. The table is bound from the library that
    /// connected, while the loader's soname lookup (what `jack-sys` did) returns the first mapped.
    #[test]
    fn the_table_is_bound_from_the_library_that_connected_not_the_first_mapped_soname() {
        let soname = "libjackfake-pair.so.0";
        let paths = build(
            "pair",
            soname,
            &[
                Fake { file: "jackd2/libjack.so.0", id: "jackd2", connects: false, lacks_port_buffer: false },
                Fake { file: "pw/pipewire-0.3/jack/libjack.so.0", id: "pipewire", connects: true, lacks_port_buffer: false },
            ],
        );
        let mut loader = DlLoader::default();
        let selection = select(&paths, &mut loader);
        let outcomes: Vec<&Outcome> = selection.attempts.iter().map(|a| &a.outcome).collect();
        assert_eq!(outcomes, [&Outcome::NotRunning, &Outcome::Connected]);
        let (path, fns) = loader.connected.expect("PipeWire's fake connected");
        assert_eq!(path, paths[1]);
        assert_eq!(served_by(&fns), "pipewire", "every call goes to the library that connected");
        let mut status = 0;
        // SAFETY: the fake's jack_client_open (a constant name, a live status), no variadic argument.
        let client = unsafe { (fns.client_open)(c"transit".as_ptr(), sys::JackNoStartServer, &mut status) };
        assert!(!client.is_null());
        // SAFETY: the fake returns its static id for any client.
        assert_eq!(unsafe { owned((fns.get_client_name)(client)) }, "pipewire");

        // The cause (#430): a soname open returns the first object of that name mapped, jackd2's.
        // SAFETY: a fake library (initializers: none), never unloaded (forgotten).
        let by_soname = unsafe { libloading::Library::new(soname) }.expect("the soname matches a mapped fake");
        // SAFETY: a fake library, kept loaded (forgotten below); the fakes export all of the table.
        let theirs = unsafe { JackFns::load(&by_soname) }.unwrap();
        std::mem::forget(by_soname);
        assert_eq!(served_by(&theirs), "jackd2", "a soname binding is served by jackd2's library");
    }

    /// A library lacking a required function is not usable: the probe goes on to the next one, and
    /// nothing calls into the incomplete one.
    #[test]
    fn a_library_lacking_a_function_is_passed_over_for_the_next() {
        let paths = build(
            "partial",
            "libjackfake-partial.so.0",
            &[
                Fake { file: "partial/libjack.so.0", id: "partial", connects: true, lacks_port_buffer: true },
                Fake { file: "whole/libjack.so.0", id: "whole", connects: true, lacks_port_buffer: false },
            ],
        );
        let mut loader = DlLoader::default();
        let selection = select(&paths, &mut loader);
        assert_eq!(selection.attempts[0].outcome, Outcome::LoadFailed(format!("{} lacks jack_port_get_buffer", paths[0])));
        assert_eq!(selection.connected(), Some(paths[1].as_str()));
        assert_eq!(served_by(&loader.connected.unwrap().1), "whole");
    }

    /// No library connects: nothing is kept to bind, and the error is "no server".
    #[test]
    fn with_no_library_connecting_nothing_is_bound() {
        let paths =
            build("none", "libjackfake-none.so.0", &[Fake { file: "jackd2/libjack.so.0", id: "jackd2", connects: false, lacks_port_buffer: false }]);
        let mut loader = DlLoader::default();
        let selection = select(&paths, &mut loader);
        assert!(loader.connected.is_none());
        assert_eq!(selection.error(), Some(ClientError::ServerNotRunning));
        let missing = loader.probe("/nonexistent/libjack.so.0");
        assert!(matches!(missing, Outcome::LoadFailed(_)), "{missing:?}");
    }

    /// The callback state outlives `jack_client_close` and is freed only after it returned (a
    /// reordered `close_now` logs "state freed" first). No server is needed: the close call is
    /// injected and the client pointer is never dereferenced.
    #[test]
    fn the_callback_state_is_alive_during_the_close_and_freed_after_it() {
        let paths = build("close", "libjackfake-close.so.0", &[Fake { file: "libjack.so.0", id: "close", connects: true, lacks_port_buffer: false }]);
        let mut loader = DlLoader::default();
        assert_eq!(loader.probe(&paths[0]), Outcome::Connected);
        let fns: &'static JackFns = Box::leak(Box::new(loader.connected.unwrap().1));
        let log: CloseLog = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::new(Shared::new(48_000, 128));
        let client = std::ptr::NonNull::<sys::jack_client_t>::dangling().as_ptr();
        let mut state = CallbackState::new(Arc::clone(&shared), fns, client, Box::new([]), Box::new([]), Engine::new(shared));
        state.drop_log = Some(Arc::clone(&log));
        let mut lib = LibClient { fns, client, inputs: Vec::new(), outputs: Vec::new(), state: Box::into_raw(Box::new(state)) };
        let during = Arc::clone(&log);
        lib.close_with(move |_| {
            during.lock().unwrap().push("client closed");
            0
        })
        .unwrap();
        assert_eq!(*log.lock().unwrap(), ["client closed", "state freed"]);
        assert!(lib.client.is_null() && lib.state.is_null());
        assert!(lib.close_now().is_ok(), "idempotent: a second close calls nothing");
    }
}
