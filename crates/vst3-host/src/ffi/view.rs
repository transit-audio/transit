//! The editor view: the plug frame, content scaling and the editor link.

use super::*;

// ---- editor (IPlugView) --------------------------------------------------------------------

/// The `IPlugFrame` handed to a plugin view: `resizeView` resizes our host window.
///
/// Issue #105: a `resizeView` is honored whenever it arrives once the view is attached (also
/// long after `attached`, e.g. when a plug-in applies a content scale late). One that arrives
/// before `attached` (from `setContentScaleFactor`, say) is kept and applied right after
/// `attached` returns ([`PlugFrame::attach`]). Every request is logged at info level.
pub(crate) struct PlugFrame {
    /// The host window (an `NSWindow*` on macOS), not owned; null when there is none.
    window: std::sync::atomic::AtomicPtr<c_void>,
    /// `resizeView` calls under way (see [`PlugFrame::MAX_NESTED`]).
    depth: AtomicU32,
    /// The plug-in's name, for the log.
    name: String,
    /// The view is attached to the window (between `attached` and `removed`).
    attached: AtomicBool,
    /// The last size asked for before `attached`, applied once attached.
    pending: Mutex<Option<sb::ViewRect>>,
}

impl PlugFrame {
    /// `resizeView` calls that may be under way at once: a plug-in that corrects its size from
    /// inside `onSize` gets that one more round trip (the SDK's editor host clears its guard
    /// before `onSize` too), and a plug-in that keeps asking from every `onSize` is refused at the
    /// next level instead of recursing without end.
    const MAX_NESTED: u32 = 2;

    pub(super) fn new(window: *mut c_void, name: &str) -> PlugFrame {
        PlugFrame {
            window: std::sync::atomic::AtomicPtr::new(window),
            depth: AtomicU32::new(0),
            name: name.to_owned(),
            attached: AtomicBool::new(false),
            pending: Mutex::new(None),
        }
    }

    /// Called right after the view's `attached` returned: from now on `resizeView` resizes the
    /// window, and a size the view asked for before is applied now (issue #105).
    pub(super) fn attach(&self, view: &ComPtr<sb::IPlugView>) {
        self.attached.store(true, Ordering::Relaxed);
        let pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(mut r) = pending {
            self.depth.fetch_add(1, Ordering::Relaxed);
            self.apply(Some(view.as_com_ref()), &mut r, " (asked before attached)");
            self.depth.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Called before the view's `removed`: requests from now on are only kept.
    pub(super) fn detach(&self) {
        self.attached.store(false, Ordering::Relaxed);
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Resizes the window's content to `r` and, when that worked, tells `view` through `onSize`
    /// (the SDK: "Afterwards the host has to call IPlugView::onSize()"). Returns whether the
    /// window was resized. The size is the one the plug-in asked for: `canResize` and
    /// `checkSizeConstraint` are the SDK's checks for sizes the host or the user propose (a
    /// window edge dragged), not for the plug-in's own `resizeView`, and this window has no
    /// user resizing.
    fn apply(&self, view: Option<vst3::ComRef<'_, sb::IPlugView>>, r: &mut sb::ViewRect, when: &str) -> bool {
        let (w, h) = rect_size(r);
        let honored = window::resize(self.window.load(Ordering::Relaxed), w, h);
        if honored && let Some(v) = view {
            // SAFETY: a live view of the plug-in (it passed it, or the editor link holds it) on
            // its UI thread; `r` is a writable rect that outlives the call. The SDK has the host
            // call `onSize` after it resized for `resizeView`.
            unsafe { v.onSize(r) };
        }
        let outcome = if honored { "honored, window resized" } else { "not honored (no live window on this thread)" };
        log::info!("vst3 editor \"{}\": resizeView {w}×{h}{when}: {outcome}", self.name);
        honored
    }
}

impl Class for PlugFrame {
    type Interfaces = (sb::IPlugFrame,);
}

impl IPlugFrameTrait for PlugFrame {
    // Steinberg, `IPlugFrame::resizeView`: "Called to inform the host about the resize of a given
    // view. Afterwards the host has to call IPlugView::onSize()." It is also how a plug-in answers
    // `setContentScaleFactor` (issue #101, see `set_content_scale`).
    unsafe fn resizeView(&self, view: *mut sb::IPlugView, new_size: *mut sb::ViewRect) -> sb::tresult {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(r) = (unsafe { new_size.as_mut() }) else {
            log::info!("vst3 editor \"{}\": resizeView without a size: refused", self.name);
            return sb::kInvalidArgument;
        };
        let (w, h) = rect_size(r);
        if !self.attached.load(Ordering::Relaxed) {
            // Issue #105: not attached yet (a view answering `setContentScaleFactor` before
            // `attached`): kept and applied once attached (`attach`), not dropped.
            *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(*r);
            log::info!("vst3 editor \"{}\": resizeView {w}×{h} before attached: kept, applied once attached", self.name);
            return sb::kResultTrue;
        }
        if self.depth.fetch_add(1, Ordering::Relaxed) >= PlugFrame::MAX_NESTED {
            self.depth.fetch_sub(1, Ordering::Relaxed);
            log::info!("vst3 editor \"{}\": resizeView {w}×{h}: refused (nested in another resize)", self.name);
            return sb::kResultFalse;
        }
        // SAFETY: the plugin passes its own live view (or null, handled), valid for this call.
        let v = unsafe { vst3::ComRef::from_raw(view) };
        let honored = self.apply(v, r, "");
        self.depth.fetch_sub(1, Ordering::Relaxed);
        if honored { sb::kResultTrue } else { sb::kResultFalse }
    }
}

/// Issue #101: passes the host window's content scale to a view that scales on request. Returns
/// whether the view took it. A view without the interface gets no call and keeps the size it
/// reports: plug-ins that scale themselves (they read the monitor's DPI) are never scaled twice,
/// and the host applies no transform of its own either way.
///
/// Steinberg, `IPlugViewContentScaleSupport`: "This interface communicates the content scale
/// factor from the host to the plug-in view on systems where plug-ins cannot get this information
/// directly like Microsoft Windows. The host calls setContentScaleFactor directly before or after
/// the plug-in view is attached and when the scale factor changes while the view is attached
/// (system change or window moved to another screen with different scaling settings). [...] When a
/// plug-in handles this (by returning kResultTrue), it needs to scale the width and height of its
/// view by the scale factor and inform the host via a IPlugFrame::resizeView(). The host will then
/// call IPlugView::onSize()." The host picks no size here, so `checkSizeConstraint` (for sizes the
/// host proposes) does not apply: the window takes the size the view reports.
///
/// Issue #105: one info line per call for the plug-in `name`, at `when` ("before attached", "after
/// attached", "on a DPI change"): whether the interface was obtained, the factor, the `tresult`,
/// and `getSize` before and after the call.
fn set_content_scale(view: &ComPtr<sb::IPlugView>, scale: f64, name: &str, when: &str) -> bool {
    if !scale.is_finite() || scale <= 0.0 {
        return false;
    }
    let before = size_text(view_size(view));
    // `cast` is `queryInterface`: `None` when the view does not implement the interface.
    let Some(support) = view.cast::<sb::IPlugViewContentScaleSupport>() else {
        log::info!("vst3 editor \"{name}\": content scale {when}: no IPlugViewContentScaleSupport, factor {scale} not sent; getSize {before}");
        return false;
    };
    #[allow(clippy::cast_possible_truncation)]
    let factor = scale as f32;
    // SAFETY: a valid interface of the live view, held (counted) for the call; main thread (the
    // callers run on the editor's thread). `ScaleFactor` is an `f32` in the SDK.
    let r = unsafe { support.setContentScaleFactor(factor) };
    let after = size_text(view_size(view));
    log::info!(
        "vst3 editor \"{name}\": content scale {when}: IPlugViewContentScaleSupport obtained, setContentScaleFactor({factor}) -> {} ({r}); getSize {before} before, {after} after",
        tresult_name(r)
    );
    r == sb::kResultTrue
}

/// A view size for the log.
fn size_text(size: Option<(f64, f64)>) -> String {
    size.map_or_else(|| "failed".to_owned(), |(w, h)| format!("{w}×{h}"))
}

/// The SDK's name of a `tresult`, for the log.
fn tresult_name(r: sb::tresult) -> &'static str {
    match r {
        x if x == sb::kResultOk => "kResultTrue",
        x if x == sb::kResultFalse => "kResultFalse",
        x if x == sb::kNotImplemented => "kNotImplemented",
        x if x == sb::kInvalidArgument => "kInvalidArgument",
        x if x == sb::kNoInterface => "kNoInterface",
        x if x == sb::kInternalError => "kInternalError",
        x if x == sb::kNotInitialized => "kNotInitialized",
        x if x == sb::kOutOfMemory => "kOutOfMemory",
        _ => "unknown",
    }
}

/// The window takes the size `view` reports, when it differs from the content's (after the view
/// took a content scale without asking through `resizeView`).
fn follow_view_size(view: &ComPtr<sb::IPlugView>, win: &window::HostWindow, name: &str, when: &str) {
    if let Some((w, h)) = view_size(view)
        && window::content_size(win) != (w, h)
    {
        let honored = window::resize(win.raw(), w, h);
        log::info!("vst3 editor \"{name}\": window follows getSize {w}×{h} {when}: {}", if honored { "resized" } else { "not resized" });
    }
}

/// The size `view` reports (`None` when it reports none).
fn view_size(view: &ComPtr<sb::IPlugView>) -> Option<(f64, f64)> {
    let mut r = sb::ViewRect { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: valid view; `r` is writable.
    ok(unsafe { view.getSize(&mut r) }).then(|| rect_size(&r))
}

/// Makes the host window for `view` (`prepare` sees it first; tests change its DPI there), sets
/// our frame, passes the window's content scale (issue #101) and attaches the view. On success the
/// caller keeps the frame and window and detaches the view before dropping them.
pub(super) fn attach_view(
    view: &ComPtr<sb::IPlugView>,
    platform: sb::FIDString,
    title: &str,
    header: Option<Arc<transit_rt::editor_window::HeaderCell>>,
    prepare: impl FnOnce(&window::HostWindow),
) -> Result<(ComWrapper<PlugFrame>, window::HostWindow), String> {
    // On failure the default size stays.
    let (w, h) = view_size(view).unwrap_or((400.0, 300.0));
    let win = window::HostWindow::new(title, w, h, header)?;
    prepare(&win);
    let frame = ComWrapper::new(PlugFrame::new(win.raw(), title));
    if let Some(f) = frame.as_com_ref::<sb::IPlugFrame>() {
        // SAFETY: valid view; the frame outlives the view's attachment (the caller keeps it, in
        // `g.frame`, and detaches it in `teardown`).
        unsafe { view.setFrame(f.as_ptr()) };
    }
    // Issue #101: the window's scale before `attached`, then the size the view reports at
    // that scale (a `resizeView` it sends now is kept until it is attached, issue #105).
    let scale = window::content_scale(&win);
    if let Some(scale) = scale
        && set_content_scale(view, scale, title, "before attached")
        && let Some((w, h)) = view_size(view)
    {
        window::resize(win.raw(), w, h);
    }
    // SAFETY: valid view; the parent is the live content view of `win`, of the type named.
    if !ok(unsafe { view.attached(win.parent(), platform) }) {
        // SAFETY: valid view; detach our frame again.
        unsafe { view.setFrame(std::ptr::null_mut()) };
        win.close();
        return Err("the editor could not attach to its window".into());
    }
    frame.attach(view);
    // Issue #105: the same factor again once attached (the SDK allows "before or after"; some
    // plug-ins apply it only to an attached view). A view that scales asks through `resizeView`
    // (honored now); one that only reports its new size gets the window at that size.
    if let Some(scale) = scale
        && set_content_scale(view, scale, title, "after attached")
    {
        follow_view_size(view, &win, title, "after attached");
    }
    Ok((frame, win))
}

/// Width and height of a view rect, clamped to a sane window size.
fn rect_size(r: &sb::ViewRect) -> (f64, f64) {
    let w = r.right.saturating_sub(r.left).clamp(1, 16_384);
    let h = r.bottom.saturating_sub(r.top).clamp(1, 16_384);
    (f64::from(w), f64::from(h))
}

#[derive(Default)]
pub(super) struct EditorInner {
    /// `None` once the instance is gone.
    pub(super) component: Option<ComPtr<sv::IComponent>>,
    /// `None` once the instance is gone, or without an edit controller.
    pub(super) controller: Option<ComPtr<sv::IEditController>>,
    pub(super) view: Option<ComPtr<sb::IPlugView>>,
    pub(super) frame: Option<ComWrapper<PlugFrame>>,
    pub(super) window: Option<window::HostWindow>,
}

/// The link between an instance and its editor and state handles. The instance may process on
/// the audio thread while the editor (the edit controller's `IPlugView`) is driven and the state
/// read from the main thread, which is VST3's own threading model (controller, views and
/// `getState` on the UI thread). All such calls happen under this lock, and the instance closes
/// the view and drops the component and controller references under the same lock before it
/// terminates, so a handle never calls into a dead plugin.
pub(crate) struct EditorLink {
    /// GUI/controller calls belong to the instance's creating thread, regardless of its name.
    pub(super) main: std::thread::ThreadId,
    pub(super) inner: Mutex<EditorInner>,
    /// The editor window's header strip (issue #68): kept across opens, read by the window.
    pub(super) header: Arc<transit_rt::editor_window::HeaderCell>,
}

impl Default for EditorLink {
    fn default() -> Self {
        Self { main: std::thread::current().id(), inner: Mutex::new(EditorInner::default()), header: transit_rt::editor_window::HeaderCell::new() }
    }
}

impl EditorLink {
    pub(super) fn on_main_thread(&self) -> bool {
        self.main == std::thread::current().id() && window::on_main_thread()
    }

    pub(super) fn lock(&self) -> std::sync::MutexGuard<'_, EditorInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn has_controller(&self) -> bool {
        self.lock().controller.is_some()
    }

    /// The header strip's model (any thread).
    pub fn header(&self) -> &transit_rt::editor_window::HeaderCell {
        &self.header
    }

    fn create_view(g: &EditorInner) -> Result<ComPtr<sb::IPlugView>, String> {
        let c = g.controller.as_ref().ok_or_else(|| "the plugin is gone".to_string())?;
        // SAFETY: valid controller; the name is a static NUL-terminated string.
        let raw = unsafe { c.createView(c"editor".as_ptr()) };
        // SAFETY: a non-null view comes with one reference owned by the caller.
        unsafe { ComPtr::from_raw(raw) }.ok_or_else(|| "this plugin has no editor".to_string())
    }

    /// Creates the view, checks it supports this platform's window type and reads its size,
    /// without attaching it anywhere. For diagnostics and tests.
    pub fn probe(&self) -> Result<(u32, u32), String> {
        if !self.on_main_thread() {
            return Err("plugin editor belongs to another thread".into());
        }
        let g = self.lock();
        let view = EditorLink::create_view(&g)?;
        let platform = window::PLATFORM_TYPE.ok_or_else(|| "plugin editors are not supported on this platform".to_string())?;
        // SAFETY: valid view; `platform` is a static NUL-terminated string.
        if unsafe { view.isPlatformTypeSupported(platform) } != sb::kResultTrue {
            return Err("the editor does not support this platform's windows".into());
        }
        let mut r = sb::ViewRect { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: valid view; `r` is writable.
        if !ok(unsafe { view.getSize(&mut r) }) {
            return Err("the editor reports no size".into());
        }
        let (w, h) = rect_size(&r);
        Ok((w as u32, h as u32))
    }

    /// Opens (or raises) the editor in a host window. Main thread only.
    pub fn open(&self, title: &str) -> Result<(), String> {
        self.open_with(title, |_| {})?;
        if let Some(w) = &self.lock().window {
            w.raise();
        }
        Ok(())
    }

    /// [`EditorLink::open`] without showing a new window; `prepare` sees the window before the
    /// view is scaled and attached (tests change its DPI there).
    pub(super) fn open_with(&self, title: &str, prepare: impl FnOnce(&window::HostWindow)) -> Result<(), String> {
        if !self.on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let platform = window::PLATFORM_TYPE.ok_or_else(|| "VST3 editors are not hosted on this platform yet".to_string())?;
        let mut g = self.lock();
        if let Some(w) = &g.window {
            if w.is_visible() {
                w.raise();
                return Ok(());
            }
            // The user closed the window: start over.
            EditorLink::teardown(&mut g);
        }
        let view = EditorLink::create_view(&g)?;
        // SAFETY: valid view; `platform` is a static NUL-terminated string.
        if unsafe { view.isPlatformTypeSupported(platform) } != sb::kResultTrue {
            return Err("the editor does not support this platform's windows".into());
        }
        let shown = self.header.view().title;
        let title = if shown.is_empty() { title } else { shown.as_str() };
        let (frame, win) = attach_view(&view, platform, title, Some(Arc::clone(&self.header)), prepare)?;
        g.view = Some(view);
        g.frame = Some(frame);
        g.window = Some(win);
        Ok(())
    }

    pub(super) fn teardown(g: &mut EditorInner) {
        if let Some(f) = &g.frame {
            f.detach();
        }
        if let Some(view) = g.view.take() {
            // SAFETY: valid, attached view; main thread (callers check). `removed` before the
            // window goes, and the frame is detached before it is released.
            unsafe {
                view.removed();
                view.setFrame(std::ptr::null_mut());
            }
        }
        g.frame = None;
        if let Some(w) = g.window.take() {
            w.close();
        }
    }

    /// Hands a host-set value to the controller before a state save (`setParamNormalized`,
    /// under the lock like [`EditorLink::save_state`]). No-op once the instance is gone.
    pub fn sync_param(&self, id: u32, n: f64) {
        if !n.is_finite() {
            return;
        }
        let g = self.lock();
        if let Some(c) = &g.controller {
            // SAFETY: valid controller (the instance clears it under this lock before it terminates).
            unsafe { c.setParamNormalized(id, n.clamp(0.0, 1.0)) };
        }
    }

    /// The component state plus the controller state, in SoundCraft's container (see
    /// `STATE_MAGIC`). `None` when the component refuses `getState` or the instance is gone.
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let g = self.lock();
        let component = g.component.as_ref()?;
        // SAFETY: valid component (the instance clears it under this lock before it terminates);
        // the stream lives in `read_state` for the whole call.
        let comp = Instance::read_state(|s| unsafe { component.getState(s) })?;
        let ctrl = match &g.controller {
            // SAFETY: valid controller; as above.
            Some(c) => Instance::read_state(|s| unsafe { c.getState(s) }).unwrap_or_default(),
            None => Vec::new(),
        };
        crate::preset::join_state(&comp, &ctrl)
    }

    /// Closes the editor. Main thread only (no-op elsewhere).
    pub fn close(&self) {
        if self.on_main_thread() {
            EditorLink::teardown(&mut self.lock());
        }
    }

    pub fn is_open(&self) -> bool {
        let g = self.lock();
        match &g.window {
            Some(w) if self.on_main_thread() => w.is_visible(),
            Some(_) => true,
            None => false,
        }
    }

    /// Main-thread housekeeping: tears the view down when the user closed its window.
    pub fn idle(&self) {
        if !self.on_main_thread() {
            return;
        }
        let mut g = self.lock();
        match &g.window {
            Some(w) if !w.is_visible() => EditorLink::teardown(&mut g),
            Some(w) => {
                EditorLink::follow_scale(&g);
                // The strip's changes and the size follow (issue #68).
                w.sync();
            }
            None => {}
        }
    }

    /// Issue #101: the window moved to a monitor with other scaling (or the scaling changed):
    /// the view gets the new factor, and the window the size it then reports (normally the view
    /// already asked for it through `resizeView`).
    pub(super) fn follow_scale(g: &EditorInner) {
        let (Some(w), Some(view)) = (&g.window, &g.view) else { return };
        let Some(scale) = window::scale_change(w) else { return };
        let name = g.frame.as_ref().map_or("", |f| f.name.as_str());
        if set_content_scale(view, scale, name, "on a DPI change") {
            follow_view_size(view, w, name, "on a DPI change");
        }
    }

    /// Normalized → plain through the controller (main thread).
    pub fn to_plain(&self, id: u32, n: f64) -> Option<f64> {
        let g = self.lock();
        let c = g.controller.as_ref()?;
        // SAFETY: valid controller.
        let v = unsafe { c.normalizedParamToPlain(id, n.clamp(0.0, 1.0)) };
        v.is_finite().then_some(v)
    }

    /// Plain → normalized through the controller (main thread).
    pub fn to_normalized(&self, id: u32, plain: f64) -> Option<f64> {
        let g = self.lock();
        let c = g.controller.as_ref()?;
        if !plain.is_finite() {
            return None;
        }
        // SAFETY: valid controller.
        let v = unsafe { c.plainParamToNormalized(id, plain) };
        v.is_finite().then(|| v.clamp(0.0, 1.0))
    }

    /// Shows a value change in the editor (`setParamNormalized`, main thread).
    pub fn set_normalized(&self, id: u32, n: f64) {
        if !self.on_main_thread() || !n.is_finite() {
            return;
        }
        let g = self.lock();
        if let Some(c) = &g.controller {
            // SAFETY: valid controller, main thread.
            unsafe { c.setParamNormalized(id, n.clamp(0.0, 1.0)) };
        }
    }

    /// Called by the instance before it terminates: closes the editor and cuts the handles off.
    /// Returns whether plugin objects had to be leaked.
    pub(super) fn kill(&self) -> bool {
        let mut g = self.lock();
        let mut leaked = false;
        if g.view.is_some() || g.window.is_some() {
            if self.on_main_thread() {
                EditorLink::teardown(&mut g);
            } else {
                // Window calls are main-thread only; leak the view and window rather than crash.
                log::warn!("vst3: editor still open while its plugin is destroyed off the main thread");
                if let Some(v) = g.view.take() {
                    std::mem::forget(v);
                    leaked = true;
                }
                // `HostWindow` has no destructor: dropping it leaks its retain on purpose.
                g.window = None;
                if let Some(f) = g.frame.take() {
                    std::mem::forget(f);
                }
            }
        }
        g.component = None;
        g.controller = None;
        leaked
    }
}
