//! The editor (`clap.gui`): the main-thread check, the GUI link and its window handling.

use super::*;

// ---- editor (clap.gui) ---------------------------------------------------------------------

/// True on the process's main thread (AppKit requires GUI calls there).
#[cfg(target_os = "macos")]
pub(crate) fn on_main_thread() -> bool {
    unsafe extern "C" {
        fn pthread_main_np() -> std::ffi::c_int;
    }
    // SAFETY: a libSystem function without arguments or preconditions.
    unsafe { pthread_main_np() == 1 }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn on_main_thread() -> bool {
    true
}

/// The windowing API of this platform.
fn window_api() -> Option<&'static CStr> {
    if cfg!(target_os = "macos") {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_COCOA)
    } else if cfg!(windows) {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_WIN32)
    } else if cfg!(all(unix, not(target_os = "android"))) {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_X11)
    } else {
        None
    }
}

pub(super) struct GuiInner {
    pub(super) plugin: *const clap_plugin,
    pub(super) gui: *const clap_plugin_gui,
    pub(super) state: *const clap_plugin_state,
    /// The plugin instance exists (cleared by `Instance::drop` before it destroys the plugin).
    pub(super) alive: bool,
    pub(super) created: bool,
    pub(super) visible: bool,
    /// The host window of an embedded editor (`None` for floating editors).
    pub(super) window: Option<hostwin::HostWindow>,
}

/// The link between an instance and its editor and state handles. The instance may process on
/// the audio thread while the editor is driven and the state saved from the main thread: CLAP
/// allows `clap.gui` and `clap.state` (main-thread) calls concurrently with audio-thread calls.
/// Every such call happens under this lock, and the instance clears `alive` (destroying any GUI)
/// under the same lock before it destroys the plugin, so a handle can never call into a destroyed
/// plugin.
pub(crate) struct GuiLink {
    /// GUI/controller calls belong to the instance's creating thread, regardless of its name.
    pub(super) main: std::thread::ThreadId,
    pub(super) inner: Mutex<GuiInner>,
    pub(super) host: Arc<HostState>,
    /// The embedded editor window's header strip (issue #68): kept across opens.
    pub(super) header: Arc<transit_rt::editor_window::HeaderCell>,
}

impl Default for GuiLink {
    fn default() -> Self {
        GuiLink {
            main: std::thread::current().id(),
            inner: Mutex::new(GuiInner {
                plugin: std::ptr::null(),
                gui: std::ptr::null(),
                state: std::ptr::null(),
                alive: false,
                created: false,
                visible: false,
                window: None,
            }),
            host: Arc::new(HostState::default()),
            header: transit_rt::editor_window::HeaderCell::new(),
        }
    }
}

// SAFETY: the raw pointers are only dereferenced under the mutex, while `alive` guarantees the
// plugin exists; CLAP's threading rules (main-thread GUI calls) are checked at each call.
unsafe impl Send for GuiInner {}

impl GuiLink {
    pub(super) fn on_main_thread(&self) -> bool {
        self.main == std::thread::current().id() && on_main_thread()
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, GuiInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn available(&self) -> bool {
        let g = self.lock();
        g.alive && !g.gui.is_null()
    }

    pub fn host(&self) -> &HostState {
        &self.host
    }

    /// The header strip's model (any thread). Only an embedded editor's window shows it.
    pub fn header(&self) -> &transit_rt::editor_window::HeaderCell {
        &self.header
    }

    fn vt(g: &GuiInner) -> Option<clap_plugin_gui> {
        // SAFETY: `gui` is null or the plugin's static extension vtable, valid while `alive`.
        (g.alive).then(|| unsafe { g.gui.as_ref() }.copied()).flatten()
    }

    /// Opens (creates and shows) the plugin's editor: embedded in a host window of ours where
    /// there is one, floating otherwise.
    pub fn open(&self, title: &str) -> Result<(), String> {
        self.open_with(title, |_| {})?;
        if let Some(w) = &self.lock().window {
            w.raise();
        }
        Ok(())
    }

    /// [`GuiLink::open`] without raising a new host window; `prepare` sees an embedded editor's
    /// window before the plug-in is scaled and parented (tests change its DPI there).
    pub(super) fn open_with(&self, title: &str, prepare: impl FnOnce(&hostwin::HostWindow)) -> Result<(), String> {
        if !self.on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let mut g = self.lock();
        let vt = GuiLink::vt(&g).ok_or_else(|| "this plugin has no editor".to_string())?;
        if g.created {
            if let Some(show) = vt.show {
                // SAFETY: live plugin with a created GUI; main thread (checked).
                unsafe { show(g.plugin) };
            }
            if let Some(w) = &g.window {
                w.raise();
            }
            g.visible = true;
            return Ok(());
        }
        let api = window_api().ok_or_else(|| "plugin editors are not supported on this platform".to_string())?;
        let (Some(supported), Some(create)) = (vt.is_api_supported, vt.create) else { return Err("incomplete clap.gui extension".into()) };
        // Embedded where this host has its own window (Windows): the window carries the header
        // strip (issue #68). Floating otherwise, when the plug-in offers nothing else, or when
        // embedding fails and the plug-in also offers a floating editor (as before #68).
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread.
        let floating = unsafe { supported(g.plugin, api.as_ptr(), true) };
        // SAFETY: as above.
        if hostwin::SUPPORTED && unsafe { supported(g.plugin, api.as_ptr(), false) } {
            match self.open_embedded(&mut g, vt, api, title, prepare) {
                Ok(()) => return Ok(()),
                Err(e) if floating => log::warn!("clap: the embedded editor failed ({e}); opening the floating one"),
                Err(e) => return Err(e),
            }
        }
        if !floating {
            return Err("this plugin offers only an embedded editor, which SoundCraft hosts on Windows only".into());
        }
        self.host.gui_closed.store(false, Ordering::Relaxed);
        self.host.gui_destroyed.store(false, Ordering::Relaxed);
        // SAFETY: as above; no GUI exists yet.
        if !unsafe { create(g.plugin, api.as_ptr(), true) } {
            return Err("the plugin could not create its editor".into());
        }
        g.created = true;
        // No `set_scale` for a floating window (issue #101): the plug-in owns it and reads its
        // monitor's scale; the call "overrides any OS info" (clap gui.h), and 1.0 drew it at 100%.
        if let (Some(f), Ok(t)) = (vt.suggest_title, CString::new(title)) {
            // SAFETY: live plugin with a created floating GUI; `t` outlives the call.
            unsafe { f(g.plugin, t.as_ptr()) };
        }
        let shown = match vt.show {
            // SAFETY: as above.
            Some(f) => unsafe { f(g.plugin) },
            None => false,
        };
        if !shown {
            GuiLink::destroy(&mut g);
            return Err("the plugin could not show its editor".into());
        }
        g.visible = true;
        Ok(())
    }

    /// Embeds the editor in a host window of ours (shown by the caller).
    ///
    /// The order is clap gui.h's for an embedded window: `create`, `set_scale`, `get_size`,
    /// `set_parent`, `show`. The host window is made (hidden) right after `create` so its DPI is
    /// known for `set_scale` (issue #101). gui.h on `set_scale`: "Set the absolute GUI scaling
    /// factor, and override any OS info. [...] If the plugin prefers to work out the scaling
    /// factor itself by querying the OS directly, then ignore the call. [...] Returns false if the
    /// call was ignored, or the scaling could not be applied." Either way the window takes the
    /// size `get_size` reports (physical pixels on Win32) and adds no transform of its own, so a
    /// plug-in that scales itself is not scaled twice.
    fn open_embedded(
        &self,
        g: &mut GuiInner,
        vt: clap_plugin_gui,
        api: &CStr,
        title: &str,
        prepare: impl FnOnce(&hostwin::HostWindow),
    ) -> Result<(), String> {
        let (Some(create), Some(set_parent)) = (vt.create, vt.set_parent) else { return Err("incomplete clap.gui extension".into()) };
        self.host.gui_closed.store(false, Ordering::Relaxed);
        self.host.gui_destroyed.store(false, Ordering::Relaxed);
        self.host.gui_resize.store(0, Ordering::Relaxed);
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread; no GUI yet.
        if !unsafe { create(g.plugin, api.as_ptr(), false) } {
            return Err("the plugin could not create its editor".into());
        }
        g.created = true;
        let shown = self.header.view().title;
        let title = if shown.is_empty() { title } else { shown.as_str() };
        let win = match hostwin::HostWindow::new(title, 640.0, 480.0, Some(Arc::clone(&self.header))) {
            Ok(win) => win,
            Err(e) => {
                GuiLink::destroy(g);
                return Err(e);
            }
        };
        prepare(&win);
        if let Some(scale) = hostwin::content_scale(&win) {
            GuiLink::set_scale(g, vt, scale);
        }
        let (w, h) = GuiLink::size(g, vt).unwrap_or((640, 480));
        hostwin::resize(win.raw(), f64::from(w), f64::from(h));
        let parent = clap_window { api: api.as_ptr(), specific: clap_window_handle { win32: win.parent() } };
        // SAFETY: live plugin with a created embedded GUI; `parent` names our live window and
        // outlives the call.
        if !unsafe { set_parent(g.plugin, &parent) } {
            win.close();
            GuiLink::destroy(g);
            return Err("the editor could not attach to its window".into());
        }
        g.window = Some(win);
        let shown = match vt.show {
            // SAFETY: live plugin with a created, parented GUI.
            Some(f) => unsafe { f(g.plugin) },
            None => true,
        };
        if !shown {
            GuiLink::destroy(g);
            return Err("the plugin could not show its editor".into());
        }
        g.visible = true;
        Ok(())
    }

    /// `clap_plugin_gui.set_scale` (an embedded GUI on Win32): whether the plug-in applied it.
    fn set_scale(g: &GuiInner, vt: clap_plugin_gui, scale: f64) -> bool {
        let Some(f) = vt.set_scale.filter(|_| scale.is_finite() && scale > 0.0) else { return false };
        // SAFETY: live plugin (`alive`, under the lock) with a created GUI; main thread.
        let applied = unsafe { f(g.plugin, scale) };
        log::debug!("clap: set_scale {scale} -> {applied}");
        applied
    }

    /// `clap_plugin_gui.get_size`, clamped to a sane window size (`None` when it reports none).
    fn size(g: &GuiInner, vt: clap_plugin_gui) -> Option<(u32, u32)> {
        let f = vt.get_size?;
        let (mut a, mut b) = (0u32, 0u32);
        // SAFETY: live plugin with a created GUI; both out-pointers are valid locals.
        (unsafe { f(g.plugin, &mut a, &mut b) } && a > 0 && b > 0).then(|| (a.min(16_384), b.min(16_384)))
    }

    /// Issue #101: the embedded editor's window moved to a monitor with other scaling (or the
    /// scaling changed). A plug-in that applies the new scale gets the size it then reports; one
    /// that ignores it scales itself and asks through `request_resize` if it needs to.
    pub(super) fn follow_scale(g: &GuiInner) {
        let (Some(win), Some(vt)) = (&g.window, GuiLink::vt(g)) else { return };
        let Some(scale) = hostwin::scale_change(win) else { return };
        if GuiLink::set_scale(g, vt, scale)
            && let Some((w, h)) = GuiLink::size(g, vt)
            && hostwin::content_size(win) != (w, h)
        {
            hostwin::resize(win.raw(), f64::from(w), f64::from(h));
        }
    }

    fn destroy(g: &mut GuiInner) {
        if g.created
            && let Some(f) = GuiLink::vt(g).and_then(|v| v.destroy)
        {
            // SAFETY: live plugin with a created GUI; destroyed exactly once.
            unsafe { f(g.plugin) };
        }
        g.created = false;
        g.visible = false;
        // The plugin's child window is gone; now its host window can go too.
        if let Some(w) = g.window.take() {
            w.close();
        }
    }

    /// `clap_plugin_state.save` into memory (`None` without the extension, on failure, or once the
    /// plugin is gone).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let g = self.lock();
        if !g.alive {
            return None;
        }
        // SAFETY: `state` is null or the plugin's static extension vtable, valid while `alive`.
        let save = unsafe { g.state.as_ref() }?.save?;
        let mut out: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: (&mut out as *mut Vec<u8>).cast::<c_void>(), write: Some(ostream_write) };
        // SAFETY: live plugin (`alive`, under the lock); the stream and its context live on this
        // frame for the call.
        unsafe { save(g.plugin, &stream) }.then_some(out)
    }

    /// Closes (destroys) the editor.
    pub fn close(&self) {
        if !self.on_main_thread() {
            return;
        }
        GuiLink::destroy(&mut self.lock());
    }

    pub fn is_open(&self) -> bool {
        let g = self.lock();
        g.alive && g.created && g.visible && g.window.as_ref().is_none_or(hostwin::HostWindow::is_visible)
    }

    /// Main-thread housekeeping: runs a requested `on_main_thread`, follows the plugin's
    /// show/hide/closed requests, and returns the parameter edits reported since the last call.
    pub fn idle(&self) -> Vec<(u32, f64)> {
        self.idle_on(self.on_main_thread())
    }

    /// [`GuiLink::idle`] for a plugin without an editor, run by the instance's owner thread (the
    /// thread that created it, which is the host's main thread for CLAP). With no GUI there is no
    /// AppKit work, so the process-main-thread gate of `idle` does not apply: it would otherwise
    /// skip `on_main_thread` whenever the owner thread is not the process main thread (the test
    /// harness on macOS).
    pub fn idle_headless(&self) -> Vec<(u32, f64)> {
        self.idle_on(true)
    }

    fn idle_on(&self, main: bool) -> Vec<(u32, f64)> {
        if !main {
            return Vec::new();
        }
        let mut g = self.lock();
        if !g.alive {
            return Vec::new();
        }
        if self.host.callback.swap(false, Ordering::Relaxed) {
            // SAFETY: `plugin` is valid while `alive`; we copy its vtable struct.
            if let Some(f) = unsafe { (*g.plugin).on_main_thread } {
                // SAFETY: live plugin, main thread, requested via `request_callback`.
                unsafe { f(g.plugin) };
            }
        }
        if g.created {
            let resize = self.host.gui_resize.swap(0, Ordering::Relaxed);
            if resize != 0
                && let Some(win) = &g.window
            {
                let (w, h) = ((resize >> 32) as u32, resize as u32);
                hostwin::resize(win.raw(), f64::from(w), f64::from(h));
                if let Some(f) = GuiLink::vt(&g).and_then(|v| v.set_size) {
                    // SAFETY: live plugin with a created embedded GUI, main thread.
                    unsafe { f(g.plugin, w, h) };
                }
            }
            if let Some(w) = g.window.as_ref().filter(|w| w.is_visible()) {
                GuiLink::follow_scale(&g);
                // The strip's changes and the size follow (issue #68).
                w.sync();
            }
            if g.window.as_ref().is_some_and(|w| !w.is_visible()) {
                // The user closed the embedded editor's host window: free the editor.
                GuiLink::destroy(&mut g);
            } else if self.host.gui_closed.swap(false, Ordering::Relaxed) {
                // The user closed the floating window (or the plugin lost its GUI): free it.
                self.host.gui_destroyed.store(false, Ordering::Relaxed);
                GuiLink::destroy(&mut g);
            } else if let Some(vt) = GuiLink::vt(&g) {
                if self.host.gui_show.swap(false, Ordering::Relaxed)
                    && let Some(f) = vt.show
                {
                    // SAFETY: live plugin with a created GUI, main thread.
                    g.visible = unsafe { f(g.plugin) };
                }
                if self.host.gui_hide.swap(false, Ordering::Relaxed)
                    && let Some(f) = vt.hide
                {
                    // SAFETY: as above.
                    unsafe { f(g.plugin) };
                    g.visible = false;
                }
            }
        }
        drop(g);
        let mut e = self.host.edits.lock().unwrap_or_else(PoisonError::into_inner);
        let out = e.clone();
        e.clear();
        out
    }

    /// Called by the instance before it destroys the plugin.
    pub(super) fn kill(&self) {
        let mut g = self.lock();
        if g.created {
            if self.on_main_thread() {
                GuiLink::destroy(&mut g);
            } else {
                log::warn!("clap: editor still open while its plugin is destroyed off the main thread");
                g.created = false;
                // Window calls are main-thread only: leave the host window rather than crash.
                g.window = None;
            }
        }
        g.alive = false;
    }
}
