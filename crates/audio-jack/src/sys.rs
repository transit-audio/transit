//! The libjack declarations this crate uses (jack.h, types.h), and [`JackFns`], the function table
//! `crate::ffi` resolves from the one library it chose (#430). Declarations only: nothing here is
//! `unsafe` to write; calling a pointer of the table is, and happens in `crate::ffi` and
//! `crate::callback` alone.
//!
//! The table replaces `jack-sys`, whose static table resolves every function from
//! `dlopen("libjack.so.0")`: the dynamic loader returns the first object with that soname already
//! mapped in the process, which is jackd2's libjack whenever anything loaded it first, so
//! PipeWire's library could not serve the process. Each pointer here comes from `dlsym` on the
//! handle of the library whose probe client connected.
#![allow(non_camel_case_types, non_upper_case_globals)]

use std::ffi::{c_char, c_float, c_int, c_uint, c_ulong, c_void};

/// `typedef struct _jack_client jack_client_t;` (opaque).
#[repr(C)]
pub(crate) struct jack_client_t {
    _private: [u8; 0],
}

/// `typedef struct _jack_port jack_port_t;` (opaque).
#[repr(C)]
pub(crate) struct jack_port_t {
    _private: [u8; 0],
}

/// types.h: `typedef uint32_t jack_nframes_t;`
pub(crate) type jack_nframes_t = u32;
/// types.h: `typedef uint64_t jack_time_t;`
pub(crate) type jack_time_t = u64;
/// types.h: `typedef enum JackOptions jack_options_t;` (a C enum: `int` sized).
pub(crate) type jack_options_t = c_uint;
/// types.h: `typedef enum JackStatus jack_status_t;`
pub(crate) type jack_status_t = c_uint;

/// types.h, JackOptions: "Do not automatically start the JACK server when it is not already running."
pub(crate) const JackNoStartServer: jack_options_t = 0x01;
/// types.h, JackStatus: "Unable to connect to the JACK server."
pub(crate) const JackServerFailed: jack_status_t = 0x10;
/// types.h, JackPortFlags: "if JackPortIsInput is set, then the port can receive data."
pub(crate) const JackPortIsInput: c_ulong = 0x1;
/// types.h, JackPortFlags: "if JackPortIsOutput is set, then data can be read from the port."
pub(crate) const JackPortIsOutput: c_ulong = 0x2;
/// types.h, JackPortFlags: "if JackPortIsPhysical is set, then the port corresponds to some kind of
/// physical I/O connector."
pub(crate) const JackPortIsPhysical: c_ulong = 0x4;

/// types.h: `typedef int (*JackProcessCallback)(jack_nframes_t nframes, void *arg);`
pub(crate) type ProcessCallback = unsafe extern "C" fn(jack_nframes_t, *mut c_void) -> c_int;
/// types.h: `typedef int (*JackBufferSizeCallback)(jack_nframes_t nframes, void *arg);` (and
/// `JackSampleRateCallback`, the same shape).
pub(crate) type FramesCallback = unsafe extern "C" fn(jack_nframes_t, *mut c_void) -> c_int;
/// types.h: `typedef int (*JackXRunCallback)(void *arg);`
pub(crate) type XrunCallback = unsafe extern "C" fn(*mut c_void) -> c_int;
/// types.h: `typedef void (*JackInfoShutdownCallback)(jack_status_t code, const char* reason, void *arg);`
pub(crate) type ShutdownCallback = unsafe extern "C" fn(jack_status_t, *const c_char, *mut c_void);

/// `jack_client_open` without its trailing `...` (see the SAFETY note at its call in `crate::ffi`).
pub(crate) type ClientOpenFn = unsafe extern "C" fn(*const c_char, jack_options_t, *mut jack_status_t) -> *mut jack_client_t;

/// The libjack functions of one library, resolved once from its handle (`crate::ffi`). Each field's
/// type is the jack.h prototype of the function it names. `Copy` pointers into a library that is
/// never unloaded, so the table is `'static` once bound; the process callback calls three of them
/// (`port_get_buffer`, `get_cycle_times`, `get_time`) as plain indirect calls.
#[derive(Clone, Copy)]
pub(crate) struct JackFns {
    /// `jack_client_t *jack_client_open (const char *client_name, jack_options_t options, jack_status_t *status, ...)`
    pub(crate) client_open: ClientOpenFn,
    /// `int jack_client_close (jack_client_t *client)`
    pub(crate) client_close: unsafe extern "C" fn(*mut jack_client_t) -> c_int,
    /// `char *jack_get_client_name (jack_client_t *client)`
    pub(crate) get_client_name: unsafe extern "C" fn(*mut jack_client_t) -> *mut c_char,
    /// `jack_nframes_t jack_get_sample_rate (jack_client_t *)`
    pub(crate) get_sample_rate: unsafe extern "C" fn(*mut jack_client_t) -> jack_nframes_t,
    /// `jack_nframes_t jack_get_buffer_size (jack_client_t *)`
    pub(crate) get_buffer_size: unsafe extern "C" fn(*mut jack_client_t) -> jack_nframes_t,
    /// `const char ** jack_get_ports (jack_client_t *, const char *port_name_pattern, const char *type_name_pattern, unsigned long flags)`
    pub(crate) get_ports: unsafe extern "C" fn(*mut jack_client_t, *const c_char, *const c_char, c_ulong) -> *mut *const c_char,
    /// `void jack_free(void* ptr)`
    pub(crate) free: unsafe extern "C" fn(*mut c_void),
    /// `jack_port_t * jack_port_register (jack_client_t *client, const char *port_name, const char *port_type, unsigned long flags, unsigned long buffer_size)`
    pub(crate) port_register: unsafe extern "C" fn(*mut jack_client_t, *const c_char, *const c_char, c_ulong, c_ulong) -> *mut jack_port_t,
    /// `const char * jack_port_name (const jack_port_t *port)`
    pub(crate) port_name: unsafe extern "C" fn(*const jack_port_t) -> *const c_char,
    /// `int jack_set_process_callback (jack_client_t *client, JackProcessCallback process_callback, void *arg)`
    pub(crate) set_process_callback: unsafe extern "C" fn(*mut jack_client_t, Option<ProcessCallback>, *mut c_void) -> c_int,
    /// `int jack_set_buffer_size_callback (jack_client_t *client, JackBufferSizeCallback bufsize_callback, void *arg)`
    pub(crate) set_buffer_size_callback: unsafe extern "C" fn(*mut jack_client_t, Option<FramesCallback>, *mut c_void) -> c_int,
    /// `int jack_set_sample_rate_callback (jack_client_t *client, JackSampleRateCallback srate_callback, void *arg)`
    pub(crate) set_sample_rate_callback: unsafe extern "C" fn(*mut jack_client_t, Option<FramesCallback>, *mut c_void) -> c_int,
    /// `int jack_set_xrun_callback (jack_client_t *client, JackXRunCallback xrun_callback, void *arg)`
    pub(crate) set_xrun_callback: unsafe extern "C" fn(*mut jack_client_t, Option<XrunCallback>, *mut c_void) -> c_int,
    /// `void jack_on_info_shutdown (jack_client_t *client, JackInfoShutdownCallback shutdown_callback, void *arg)`
    pub(crate) on_info_shutdown: unsafe extern "C" fn(*mut jack_client_t, Option<ShutdownCallback>, *mut c_void),
    /// `int jack_activate (jack_client_t *client)`
    pub(crate) activate: unsafe extern "C" fn(*mut jack_client_t) -> c_int,
    /// `int jack_deactivate (jack_client_t *client)`
    pub(crate) deactivate: unsafe extern "C" fn(*mut jack_client_t) -> c_int,
    /// `int jack_connect (jack_client_t *client, const char *source_port, const char *destination_port)`
    pub(crate) connect: unsafe extern "C" fn(*mut jack_client_t, *const c_char, *const c_char) -> c_int,
    /// `int jack_set_buffer_size (jack_client_t *client, jack_nframes_t nframes)`
    pub(crate) set_buffer_size: unsafe extern "C" fn(*mut jack_client_t, jack_nframes_t) -> c_int,
    /// `jack_port_t * jack_port_by_name (jack_client_t *client, const char *port_name)` (tests only)
    #[cfg(test)]
    pub(crate) port_by_name: unsafe extern "C" fn(*mut jack_client_t, *const c_char) -> *mut jack_port_t,
    /// `const char ** jack_port_get_all_connections (const jack_client_t *client, const jack_port_t *port)` (tests only)
    #[cfg(test)]
    pub(crate) port_get_all_connections: unsafe extern "C" fn(*const jack_client_t, *const jack_port_t) -> *mut *const c_char,
    /// `void * jack_port_get_buffer (jack_port_t *port, jack_nframes_t)` (process thread)
    pub(crate) port_get_buffer: unsafe extern "C" fn(*mut jack_port_t, jack_nframes_t) -> *mut c_void,
    /// `jack_time_t jack_get_time(void)` (process thread)
    pub(crate) get_time: unsafe extern "C" fn() -> jack_time_t,
    /// `int jack_get_cycle_times(const jack_client_t *client, jack_nframes_t *current_frames, jack_time_t *current_usecs, jack_time_t *next_usecs, float *period_usecs) JACK_OPTIONAL_WEAK_EXPORT;`
    /// (process thread; `None` where the library lacks it)
    pub(crate) get_cycle_times:
        Option<unsafe extern "C" fn(*const jack_client_t, *mut jack_nframes_t, *mut jack_time_t, *mut jack_time_t, *mut c_float) -> c_int>,
    /// `const char * jack_get_version_string(void) JACK_OPTIONAL_WEAK_EXPORT;` (`None` where missing)
    pub(crate) get_version_string: Option<unsafe extern "C" fn() -> *const c_char>,
}
