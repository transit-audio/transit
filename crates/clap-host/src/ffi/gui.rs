//! The editor (`clap.gui`): the main-thread check, the GUI link and its window handling.

use super::*;
use transit_dsp::editor_scale::{Applies, EditorScale, ScaleEntry, ScaleState};

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

/// The embedded editor's parent for `set_parent`: the content window (`HWND` on Win32, the X11
/// window id on Linux, the window's content `NSView` on macOS).
#[cfg(windows)]
fn window_handle(win: &hostwin::HostWindow) -> clap_window_handle {
    clap_window_handle { win32: win.parent() }
}
#[cfg(target_os = "linux")]
fn window_handle(win: &hostwin::HostWindow) -> clap_window_handle {
    clap_window_handle { x11: std::ffi::c_ulong::try_from(win.parent().addr()).unwrap_or(0) }
}
#[cfg(target_os = "macos")]
fn window_handle(win: &hostwin::HostWindow) -> clap_window_handle {
    clap_window_handle { cocoa: win.parent() }
}
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn window_handle(win: &hostwin::HostWindow) -> clap_window_handle {
    clap_window_handle { ptr: win.parent() }
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
    /// The editor's Scale setting, its last answer and the plan in use (issue #153).
    pub(super) scale: ScaleState,
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
                scale: ScaleState::default(),
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

    pub(super) fn vt(g: &GuiInner) -> Option<clap_plugin_gui> {
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
        let mut probe = super::gui_probe::Probe::default();
        let result = self.try_open(title, prepare, &mut probe);
        probe.log(title);
        if let Err(reason) = &result {
            log::warn!("plug-in editor not opened: {title}: {reason}");
        } else {
            log::info!("plug-in editor opened: {title}");
            #[cfg(target_os = "linux")]
            if let Some(w) = &self.lock().window {
                w.start_surface_probe(title, self.host.timer_callbacks());
            }
        }
        result
    }

    fn try_open(&self, title: &str, prepare: impl FnOnce(&hostwin::HostWindow), probe: &mut super::gui_probe::Probe) -> Result<(), String> {
        if !self.on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let mut g = self.lock();
        let vt = GuiLink::vt(&g).ok_or_else(|| "no gui extension".to_string())?;
        let api = window_api().ok_or_else(|| "plugin editors are not supported on this platform".to_string())?;
        let (Some(supported), Some(create)) = (vt.is_api_supported, vt.create) else { return Err("create failed".into()) };
        // Embedded where this host has its own window (Windows; Linux on X11, issue #147; macOS): the
        // Windows window carries the header strip (issue #68). Floating otherwise, when the plug-in offers nothing else, or when
        // embedding fails and the plug-in also offers a floating editor (as before #68).
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread.
        let floating = unsafe { supported(g.plugin, api.as_ptr(), true) };
        // SAFETY: as above.
        let embedded = unsafe { supported(g.plugin, api.as_ptr(), false) };
        probe.support(&g, vt, api, embedded, floating);
        if g.created {
            in_gui_scope(&g, || probe.created(&g, vt, g.window.as_ref().and_then(hostwin::content_scale), g.window.is_none()));
            if let Some(show) = vt.show {
                // SAFETY: live plugin with a created GUI; main thread (checked); in the window's
                // plug-in scope when it is stretched (review round 3).
                if !in_gui_scope(&g, || unsafe { show(g.plugin) }) {
                    return Err("show failed".into());
                }
            }
            if let Some(w) = &g.window {
                w.raise();
            }
            g.visible = true;
            return Ok(());
        }
        if hostwin::SUPPORTED && embedded {
            match self.open_embedded(&mut g, vt, api, title, prepare, probe) {
                Ok(()) => return Ok(()),
                Err(e) if floating => log::info!("clap: the embedded editor failed ({e}); opening the floating one"),
                Err(e) => return Err(e),
            }
        }
        if !floating {
            return Err(format!("{} embedding not supported (neither)", api.to_string_lossy()));
        }
        // What an earlier GUI left pending goes, show and hide included (review round 3).
        self.host.forget_gui_requests();
        // SAFETY: as above; no GUI exists yet.
        if !unsafe { create(g.plugin, api.as_ptr(), true) } {
            return Err("create failed".into());
        }
        g.created = true;
        probe.created(&g, vt, None, true);
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
            return Err("show failed".into());
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
    /// call was ignored, or the scaling could not be applied." The window takes the size
    /// `get_size` reports (physical pixels on Win32).
    ///
    /// Issue #153: under Auto (the default setting) the open is the one before #153 and never
    /// stretched. For a setting the user picked, the factor the editor's plan asks for is the one
    /// `set_scale` gets (none for the plug-in's own zoom or the Windows stretch). A plan that
    /// stretches (only ever the user's choice) makes the window again with a DPI-unaware content
    /// container, and the GUI again inside its plug-in scope (some GUIs make their window in
    /// `create`), with no `set_scale`: a factor and the stretch are never both in use.
    fn open_embedded(
        &self,
        g: &mut GuiInner,
        vt: clap_plugin_gui,
        api: &CStr,
        title: &str,
        prepare: impl FnOnce(&hostwin::HostWindow),
        probe: &mut super::gui_probe::Probe,
    ) -> Result<(), String> {
        let Some(create) = vt.create else { return Err("create failed".into()) };
        let Some(set_parent) = vt.set_parent else { return Err("set_parent failed".into()) };
        // What an earlier GUI left pending goes, show and hide included (review round 3).
        self.host.forget_gui_requests();
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread; no GUI yet.
        if !unsafe { create(g.plugin, api.as_ptr(), false) } {
            return Err("create failed".into());
        }
        g.created = true;
        let shown = self.header.view().title;
        let title = if shown.is_empty() { title } else { shown.as_str() };
        let header = Some(Arc::clone(&self.header));
        let mut win = match hostwin::HostWindow::new(title, 640.0, 480.0, header.clone()) {
            Ok(win) => win,
            Err(e) => {
                GuiLink::destroy(g);
                return Err(format!("host window failed: {e}"));
            }
        };
        prepare(&win);
        let display = hostwin::content_scale(&win);
        g.scale.stretch_available = transit_rt::editor_window::stretch_available();
        let first = g.scale.before_open(display.unwrap_or(1.0), vt.set_scale.is_some());
        let mut answer = None;
        if g.scale.auto() {
            // Auto never stretches: the open is the one before #153, call for call (`set_scale`
            // with the window's scale, whatever the GUI answered last time, and no `get_size`
            // around it; the answer only says what is in use). Nothing below makes the GUI again
            // or enters a DPI-unaware scope: the window is not a stretched one.
            if let Some(factor) = display {
                answer = Some((GuiLink::set_scale(g, vt, factor, title), false));
            }
        } else if let (Some(factor), Some(_)) = (first.factor, display) {
            let before = GuiLink::size(g, vt);
            let accepted = GuiLink::set_scale(g, vt, factor, title);
            answer = Some((accepted, before == GuiLink::size(g, vt) && (factor - 1.0).abs() > 0.005));
        }
        if g.scale.opened(answer).stretch.is_some() {
            match hostwin::new_stretched(title, 640.0, 480.0, header) {
                Ok(stretched) => {
                    win.close();
                    win = stretched;
                    if let Some(f) = vt.destroy {
                        // SAFETY: live plugin with a created GUI that has no parent yet; destroyed
                        // once here and created again below.
                        unsafe { f(g.plugin) };
                    }
                    g.created = false;
                    // Issue #153 review: what the discarded GUI asked for (a resize, show, hide,
                    // closed) must not reach its replacement.
                    self.host.forget_gui_requests();
                    // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread; no
                    // GUI now. Inside the stretched window's plug-in scope (DPI-unaware context).
                    if !hostwin::in_plugin_scope(&win, || unsafe { create(g.plugin, api.as_ptr(), false) }) {
                        win.close();
                        g.scale.closed();
                        return Err("create failed (stretched)".into());
                    }
                    g.created = true;
                }
                Err(e) => {
                    log::warn!("clap editor \"{title}\": cannot stretch the editor ({e}); it keeps its own drawing");
                    g.scale.stretch_failed();
                }
            }
        }
        // Every call into a stretched GUI runs in the window's plug-in scope (review round 3).
        hostwin::in_plugin_scope(&win, || probe.created(g, vt, display, false));
        let Some((w, h)) = probe.size else {
            win.close();
            GuiLink::destroy(g);
            return Err("zero size".into());
        };
        hostwin::resize(win.raw(), f64::from(w), f64::from(h));
        let parent = clap_window { api: api.as_ptr(), specific: window_handle(&win) };
        // SAFETY: live plugin with a created embedded GUI; `parent` names our live window and
        // outlives the call. A stretched window's GUI parents itself in the DPI-unaware context.
        if !hostwin::in_plugin_scope(&win, || unsafe { set_parent(g.plugin, &parent) }) {
            win.close();
            GuiLink::destroy(g);
            return Err("set_parent failed".into());
        }
        let shown = match vt.show {
            // SAFETY: live plugin with a created, parented GUI; in the plug-in scope as above.
            Some(f) => hostwin::in_plugin_scope(&win, || unsafe { f(g.plugin) }),
            None => false,
        };
        g.window = Some(win);
        if !shown {
            GuiLink::destroy(g);
            return Err("show failed".into());
        }
        g.visible = true;
        log::info!("clap editor \"{title}\": scale: {}", g.scale.log_line());
        self.show_scale_menu(g);
        Ok(())
    }

    /// Puts the Scale submenu for the current state into the header strip's view (issue #153).
    fn show_scale_menu(&self, g: &mut GuiInner) {
        let mut view = self.header.view();
        view.scale = scale_rows(g.scale.menu());
        self.header.set(&view);
    }

    /// The header the app sent (issue #68) with this plug-in's Scale setting (issue #153): a new
    /// setting reaches an open embedded editor at once where the plug-in allows (a new factor for
    /// a GUI that takes one) and otherwise on the next open, and the log says which.
    pub fn set_header(&self, mut view: transit_rt::editor_window::HeaderView, setting: EditorScale) {
        let mut g = self.lock();
        let (applies, wanted) = g.scale.set_setting(setting);
        let open = g.created && g.window.is_some();
        let name = view.title.clone();
        match (applies, GuiLink::vt(&g)) {
            (Applies::Now, Some(vt)) if open && self.on_main_thread() => {
                if let Some(factor) = wanted.factor {
                    let before = GuiLink::size(&g, vt);
                    let accepted = GuiLink::set_scale(&g, vt, factor, &name);
                    let after = GuiLink::size(&g, vt);
                    if accepted
                        && let (Some(win), Some((w, h))) = (&g.window, after)
                        && hostwin::content_size(win) != (w, h)
                    {
                        hostwin::resize(win.raw(), f64::from(w), f64::from(h));
                        log::info!("clap editor \"{name}\": window resized to {w}×{h} after the Scale change");
                    }
                    g.scale.applied(accepted, before == after && (factor - 1.0).abs() > 0.005);
                }
                log::info!("clap editor \"{name}\": Scale {setting:?} applied at once; scale: {}", g.scale.log_line());
            }
            (Applies::Now | Applies::NextOpen, _) if open => {
                log::info!("clap editor \"{name}\": Scale {setting:?} applies the next time the editor opens");
            }
            _ => {}
        }
        view.scale = scale_rows(g.scale.menu());
        self.header.set(&view);
    }

    /// The Scale setting the menu entry at `index` stores (issue #153).
    pub fn scale_pick(&self, index: usize) -> Option<EditorScale> {
        self.lock().scale.pick(index)
    }

    /// `clap_plugin_gui.set_scale` (an embedded GUI on Win32): whether the plug-in applied it.
    /// Logged at info for the plug-in `name` (issue #153).
    fn set_scale(g: &GuiInner, vt: clap_plugin_gui, scale: f64, name: &str) -> bool {
        let Some(f) = vt.set_scale.filter(|_| scale.is_finite() && scale > 0.0) else {
            log::info!("clap editor \"{name}\": no set_scale, factor {scale} not sent");
            return false;
        };
        // SAFETY: live plugin (`alive`, under the lock) with a created GUI; main thread.
        let applied = unsafe { f(g.plugin, scale) };
        log::info!("clap editor \"{name}\": set_scale({scale}) -> {applied}");
        applied
    }

    /// `clap_plugin_gui.get_size`, clamped to a sane window size (`None` when it reports none).
    pub(super) fn size(g: &GuiInner, vt: clap_plugin_gui) -> Option<(u32, u32)> {
        let f = vt.get_size?;
        let (mut a, mut b) = (0u32, 0u32);
        // SAFETY: live plugin with a created GUI; both out-pointers are valid locals.
        (unsafe { f(g.plugin, &mut a, &mut b) } && a > 0 && b > 0).then(|| (a.min(16_384), b.min(16_384)))
    }

    /// Issue #101: the embedded editor's window moved to a monitor with other scaling (or the
    /// scaling changed). A plug-in that applies the new scale gets the size it then reports; one
    /// that ignores it scales itself and asks through `request_resize` if it needs to. Issue #153:
    /// only a GUI drawn at the host's factor under Auto gets the new one. Returns whether the DPI
    /// changed.
    pub(super) fn follow_scale(g: &mut GuiInner) -> bool {
        let Some(scale) = g.window.as_ref().and_then(hostwin::scale_change) else { return false };
        let send = g.scale.dpi_changed(scale);
        let (Some(win), Some(vt)) = (&g.window, GuiLink::vt(g)) else { return true };
        log::info!("clap: the editor window's display scale is now {scale}");
        let Some(factor) = send else { return true };
        let before = GuiLink::size(g, vt);
        let accepted = GuiLink::set_scale(g, vt, factor, "on a DPI change");
        let after = GuiLink::size(g, vt);
        if accepted
            && let Some((w, h)) = after
            && hostwin::content_size(win) != (w, h)
        {
            hostwin::resize(win.raw(), f64::from(w), f64::from(h));
            log::info!("clap: editor window resized to {w}×{h} after the DPI change");
        }
        // Issue #153 review: the answer is recorded; a refusal keeps the factor the GUI had.
        g.scale.dpi_answer(accepted, before == after && (factor - 1.0).abs() > 0.005);
        true
    }

    fn destroy(g: &mut GuiInner) {
        g.scale.closed();
        if g.created
            && let Some(f) = GuiLink::vt(g).and_then(|v| v.destroy)
        {
            // SAFETY: live plugin with a created GUI; destroyed exactly once; scoped as above.
            in_gui_scope(g, || unsafe { f(g.plugin) });
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
        let saved = unsafe { save(g.plugin, &stream) };
        trace::record(self.host.trace_id, trace::Call::StateSave, u32::from(saved), 0.0, out.len() as u64);
        saved.then_some(out)
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
                log::info!("clap: editor window resized to {w}×{h} (request_resize)");
                if let Some(f) = GuiLink::vt(&g).and_then(|v| v.set_size) {
                    // SAFETY: live plugin with a created embedded GUI, main thread; in the
                    // stretched window's plug-in scope (review round 3).
                    hostwin::in_plugin_scope(win, || unsafe { f(g.plugin, w, h) });
                }
            }
            if g.window.as_ref().is_some_and(hostwin::HostWindow::is_visible) && GuiLink::follow_scale(&mut g) {
                self.show_scale_menu(&mut g);
            }
            if let Some(w) = g.window.as_ref().filter(|w| w.is_visible()) {
                // The strip's changes and the size follow (issue #68).
                w.sync();
                #[cfg(target_os = "linux")]
                w.sample_surface(self.host.timer_callbacks(), || self.host.surface_registrations());
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
                    // SAFETY: live plugin with a created GUI, main thread; scoped as above.
                    g.visible = in_gui_scope(&g, || unsafe { f(g.plugin) });
                }
                if self.host.gui_hide.swap(false, Ordering::Relaxed)
                    && let Some(f) = vt.hide
                {
                    // SAFETY: as above.
                    in_gui_scope(&g, || unsafe { f(g.plugin) });
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
        self.lock().scale.closed();
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

/// Runs `f` (a call into the GUI) in its window's plug-in scope: the DPI-unaware context for a
/// stretched window (review round 3), nothing otherwise or without a window.
fn in_gui_scope<R>(g: &GuiInner, f: impl FnOnce() -> R) -> R {
    match &g.window {
        Some(w) => hostwin::in_plugin_scope(w, f),
        None => f(),
    }
}

/// The Scale submenu's rows as the editor window shows them (issue #153).
fn scale_rows(entries: &[ScaleEntry]) -> Vec<transit_rt::editor_window::MenuItem> {
    use transit_rt::editor_window::{MenuItem, scale_item};
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| match e {
            ScaleEntry::Info(t) => MenuItem::Heading(t.clone()),
            ScaleEntry::Separator => MenuItem::Separator,
            ScaleEntry::Choice { label, checked, enabled, .. } => scale_item(i, label, *checked, *enabled),
        })
        .collect()
}
