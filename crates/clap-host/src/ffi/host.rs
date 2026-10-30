//! The host callbacks the plug-in calls back into: the host state, the extension vtables and `get_extension`.

use super::*;

// ---- host side -----------------------------------------------------------------------------

/// Flags the plugin raises through the host callbacks (any thread). Who services each (audit F08):
/// `process` is read by the audio side (`ClapPlugin::process_status` keeps a sleeping plugin
/// processed until a block clears it); `restart`, `callback` and `params_rescan` are serviced on
/// the instance's main thread (`plugin::Shared::service`); `latency_changed` is only legal during
/// `activate`, after which the host reads the latency anyway.
pub(crate) struct HostState {
    pub restart: AtomicBool,
    pub process: AtomicBool,
    pub callback: AtomicBool,
    pub latency_changed: AtomicBool,
    pub params_rescan: AtomicBool,
    /// The `clap_param_rescan_flags` of every `params.rescan` since the last service, or-ed.
    pub params_rescan_flags: AtomicU32,
    /// `clap_host_state.mark_dirty`.
    pub dirty: AtomicBool,
    /// `clap_host_gui` requests, serviced by the editor's `idle` on the main thread.
    pub gui_closed: AtomicBool,
    pub gui_destroyed: AtomicBool,
    pub gui_show: AtomicBool,
    pub gui_hide: AtomicBool,
    /// A size an embedded editor asked for (`width << 32 | height`; 0 = none pending).
    pub gui_resize: AtomicU64,
    /// Parameter changes the plugin reported (output events: typically edits in its GUI),
    /// waiting for the UI. Bounded by `EDIT_CAP`; the audio thread only `try_lock`s it.
    pub edits: Mutex<Vec<(u32, f64)>>,
    /// Linux: this state itself, for the run-loop registrations, which hold it weakly.
    #[cfg(target_os = "linux")]
    pub me: std::sync::Weak<HostState>,
    /// Linux: the instance's editor link, once made (its timers and descriptors call the plug-in
    /// through it).
    #[cfg(target_os = "linux")]
    pub link: OnceLock<std::sync::Weak<GuiLink>>,
    /// Linux: the plug-in's timer and descriptor registrations (issue #147).
    #[cfg(target_os = "linux")]
    pub run_loop: Mutex<super::run_loop::Registrations>,
}

impl HostState {
    /// A shared state for a new instance (on Linux it knows itself, for the run loop).
    pub(crate) fn new_shared() -> Arc<HostState> {
        #[cfg(target_os = "linux")]
        return Arc::new_cyclic(|me| HostState { me: me.clone(), ..HostState::default() });
        #[cfg(not(target_os = "linux"))]
        Arc::new(HostState::default())
    }

    /// Records the instance's editor link (Linux: its timers and descriptors call through it).
    pub(crate) fn set_link(&self, _link: &Arc<GuiLink>) {
        #[cfg(target_os = "linux")]
        let _ = self.link.set(Arc::downgrade(_link));
    }
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            restart: AtomicBool::new(false),
            process: AtomicBool::new(false),
            callback: AtomicBool::new(false),
            latency_changed: AtomicBool::new(false),
            params_rescan: AtomicBool::new(false),
            params_rescan_flags: AtomicU32::new(0),
            dirty: AtomicBool::new(false),
            gui_closed: AtomicBool::new(false),
            gui_destroyed: AtomicBool::new(false),
            gui_show: AtomicBool::new(false),
            gui_hide: AtomicBool::new(false),
            gui_resize: AtomicU64::new(0),
            edits: Mutex::new(Vec::with_capacity(EDIT_CAP)),
            #[cfg(target_os = "linux")]
            me: std::sync::Weak::new(),
            #[cfg(target_os = "linux")]
            link: OnceLock::new(),
            #[cfg(target_os = "linux")]
            run_loop: Mutex::new(super::run_loop::Registrations::default()),
        }
    }
}

/// SAFETY contract: `host` is null or one of our `clap_host` structs, whose `host_data` points
/// to the `HostState` owned by the same `Instance`.
pub(crate) unsafe fn state<'a>(host: *const clap_host) -> Option<&'a HostState> {
    if host.is_null() {
        return None;
    }
    // SAFETY: non-null host created by `Bundle::instantiate` (contract above).
    let data = unsafe { (*host).host_data } as *const HostState;
    // SAFETY: `host_data` points to the boxed `HostState`, alive as long as the instance.
    unsafe { data.as_ref() }
}

pub(super) unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.restart.store(true, Ordering::Relaxed);
    }
}

pub(super) unsafe extern "C" fn host_request_process(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.process.store(true, Ordering::Relaxed);
    }
}

pub(super) unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.callback.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_log(_host: *const clap_host, severity: clap_log_severity, msg: *const c_char) {
    // SAFETY: CLAP passes a NUL-terminated message (or null, handled).
    let m = unsafe { read_cstr(msg) }.unwrap_or_default();
    match severity {
        CLAP_LOG_ERROR | CLAP_LOG_FATAL => log::error!("clap plugin: {m}"),
        CLAP_LOG_WARNING => log::warn!("clap plugin: {m}"),
        _ => log::debug!("clap plugin: {m}"),
    }
}

unsafe extern "C" fn host_params_rescan(host: *const clap_host, flags: clap_param_rescan_flags) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.params_rescan_flags.fetch_or(flags, Ordering::Relaxed);
        s.params_rescan.store(true, Ordering::Release);
    }
}

unsafe extern "C" fn host_params_clear(_host: *const clap_host, _id: clap_id, _flags: clap_param_clear_flags) {}

unsafe extern "C" fn host_params_request_flush(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.process.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_latency_changed(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.latency_changed.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_state_mark_dirty(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.dirty.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_gui_resize_hints_changed(_host: *const clap_host) {}

unsafe extern "C" fn host_gui_request_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    // Floating windows belong to the plugin, which resizes them itself; an embedded editor's host
    // window is resized by the editor's `idle` on the main thread.
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) }
        && width > 0
        && height > 0
    {
        s.gui_resize.store((u64::from(width) << 32) | u64::from(height), Ordering::Relaxed);
    }
    true
}

unsafe extern "C" fn host_gui_request_show(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        Some(s) => {
            s.gui_show.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

unsafe extern "C" fn host_gui_request_hide(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        Some(s) => {
            s.gui_hide.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

unsafe extern "C" fn host_gui_closed(host: *const clap_host, was_destroyed: bool) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        if was_destroyed {
            s.gui_destroyed.store(true, Ordering::Relaxed);
        }
        s.gui_closed.store(true, Ordering::Relaxed);
    }
}

static HOST_STATE: clap_host_state = clap_host_state { mark_dirty: Some(host_state_mark_dirty) };
static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(host_gui_resize_hints_changed),
    request_resize: Some(host_gui_request_resize),
    request_show: Some(host_gui_request_show),
    request_hide: Some(host_gui_request_hide),
    closed: Some(host_gui_closed),
};
static HOST_LOG: clap_host_log = clap_host_log { log: Some(host_log) };
static HOST_PARAMS: clap_host_params =
    clap_host_params { rescan: Some(host_params_rescan), clear: Some(host_params_clear), request_flush: Some(host_params_request_flush) };
static HOST_LATENCY: clap_host_latency = clap_host_latency { changed: Some(host_latency_changed) };
pub(super) static HOST_TAIL: clap_host_tail = clap_host_tail { changed: Some(host_tail_changed) };

/// `clap_host_tail.changed` (audio thread): nothing to do, the tail is re-read after every
/// `process` call.
unsafe extern "C" fn host_tail_changed(_host: *const clap_host) {}

pub(super) unsafe extern "C" fn host_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    // SAFETY: extension ids are NUL-terminated strings (CLAP contract); non-null checked above.
    let id = unsafe { CStr::from_ptr(id) };
    // Linux: timers and descriptors on the plug-in host thread's run loop (issue #147).
    #[cfg(target_os = "linux")]
    if let Some(ext) = super::run_loop::extension(id) {
        return ext;
    }
    if id == CLAP_EXT_LOG {
        (&HOST_LOG as *const clap_host_log).cast()
    } else if id == CLAP_EXT_PARAMS {
        (&HOST_PARAMS as *const clap_host_params).cast()
    } else if id == CLAP_EXT_LATENCY {
        (&HOST_LATENCY as *const clap_host_latency).cast()
    } else if id == CLAP_EXT_TAIL {
        (&HOST_TAIL as *const clap_host_tail).cast()
    } else if id == CLAP_EXT_STATE {
        (&HOST_STATE as *const clap_host_state).cast()
    } else if id == CLAP_EXT_GUI {
        (&HOST_GUI as *const clap_host_gui).cast()
    } else {
        // Everything else (thread-check, and timers and fd support off Linux, …) is unsupported:
        // CLAP allows null.
        std::ptr::null()
    }
}
