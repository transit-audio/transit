use super::tests::gain_instance;
use super::*;
use transit_dsp::editor_scale::{EditorScale, ScaleState};

// ---- issue #101: content scale ------------------------------------------------------------

/// Changes the window's DPI as Windows does when it moves to a monitor with other scaling.
#[cfg(windows)]
fn set_dpi(win: &window::HostWindow, dpi: usize) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_DPICHANGED};
    let mut suggested = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a live window of this thread; WM_DPICHANGED's wparam carries the DPI twice and
    // its lparam points to a suggested RECT, which outlives the call.
    unsafe { SendMessageW(win.raw().cast(), WM_DPICHANGED, (dpi << 16) | dpi, (&raw mut suggested) as isize) };
}

/// The fixture's view scales on request (as Battery 4 does): opened in a window at 144 DPI it
/// gets 1.5 before it is attached and the window takes the size it then reports; moved to 192
/// DPI it gets 2.0 again and asks for its new size through `resizeView`.
#[cfg(windows)]
#[test]
fn the_view_gets_the_windows_scale_and_the_window_follows_its_size() {
    let inst = gain_instance();
    let link = inst.editor().unwrap();
    link.open_with("scale", |w| set_dpi(w, 144)).unwrap();
    {
        let mut g = link.lock();
        let size = |g: &EditorInner| g.window.as_ref().unwrap().content_size();
        assert_eq!(size(&g), (480, 360), "320×240 at 1.5");
        set_dpi(g.window.as_ref().unwrap(), 192);
        EditorLink::follow_scale(&mut g);
        assert_eq!(size(&g), (640, 480), "the DPI change sends 2.0 and the view resizes");
        EditorLink::follow_scale(&mut g);
        assert_eq!(size(&g), (640, 480), "no change, no call");
    }
    link.close();
    assert!(!link.is_open());
}

/// N2 of the #101 review: the editor's `idle` (not a direct `follow_scale`) sends the new
/// factor after a DPI change. The window must be visible for `idle` to keep it: it is shown
/// without activation.
#[cfg(windows)]
#[test]
fn idle_sends_the_new_scale_after_a_dpi_change() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
    let inst = gain_instance();
    let link = inst.editor().unwrap();
    link.open_with("idle", |w| set_dpi(w, 144)).unwrap();
    {
        let g = link.lock();
        let win = g.window.as_ref().unwrap();
        // SAFETY: a live window of this thread.
        unsafe { ShowWindow(win.raw().cast(), SW_SHOWNOACTIVATE) };
        assert_eq!(win.content_size(), (480, 360));
        set_dpi(win, 192);
    }
    link.idle();
    assert!(link.is_open(), "idle kept the visible window");
    assert_eq!(link.lock().window.as_ref().unwrap().content_size(), (640, 480), "idle sent 2.0");
    link.close();
}

/// A view that records what the host calls; with `scales` it implements
/// `IPlugViewContentScaleSupport` and reports 200×100 times the factor, otherwise it reports
/// 200×100 always (a plug-in that scales itself, as Arturia's do).
#[cfg(windows)]
struct FakeView<const SCALES: bool> {
    calls: Arc<Mutex<Vec<String>>>,
    scale: Mutex<f32>,
    /// The frame the host set (not counted; the host keeps it alive while set).
    frame: std::sync::atomic::AtomicPtr<sb::IPlugFrame>,
    /// This view's own interface pointer (not counted), for `resizeView`.
    me: std::sync::atomic::AtomicPtr<sb::IPlugView>,
    /// How many more times `onSize` asks for a size 10 px wider through `resizeView`.
    nest: AtomicU32,
    /// What the view read as its monitor's scale when it was made (issue #153 review): 1.5 for a
    /// view that reads the DPI at `createView` and was made in a per-monitor-aware context (as on
    /// a 150% display), 1.0 otherwise. Its size is 200×100 times this, times any host factor.
    born: f32,
    /// Asks for this width (×120) through `resizeView` from inside `setFrame`, while `getSize`
    /// still reports its own size (review round 3, note 4: a size asked for before `onSize`).
    ask: Option<i32>,
    /// The size the host last gave in `onSize`: `getSize` reports it from then on (the SDK: the
    /// view takes the size `onSize` gives).
    sized: Mutex<Option<(i32, i32)>>,
}

#[cfg(windows)]
impl<const S: bool> FakeView<S> {
    fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
}

#[cfg(windows)]
impl<const S: bool> IPlugViewTrait for FakeView<S> {
    unsafe fn isPlatformTypeSupported(&self, _t: sb::FIDString) -> sb::tresult {
        sb::kResultTrue
    }
    unsafe fn attached(&self, _p: *mut c_void, _t: sb::FIDString) -> sb::tresult {
        if TRACE.with(std::cell::Cell::get) {
            self.log(format!("attached {}", if thread_unaware() { "unaware" } else { "aware" }));
        } else {
            self.log("attached".into());
        }
        sb::kResultOk
    }
    unsafe fn removed(&self) -> sb::tresult {
        self.log("removed".into());
        sb::kResultOk
    }
    unsafe fn onWheel(&self, _d: f32) -> sb::tresult {
        sb::kResultFalse
    }
    unsafe fn onKeyDown(&self, _k: sb::char16, _c: i16, _m: i16) -> sb::tresult {
        sb::kResultFalse
    }
    unsafe fn onKeyUp(&self, _k: sb::char16, _c: i16, _m: i16) -> sb::tresult {
        sb::kResultFalse
    }
    unsafe fn getSize(&self, size: *mut sb::ViewRect) -> sb::tresult {
        if TRACE.with(std::cell::Cell::get) {
            self.log("getSize".into());
        }
        if let Some((w, h)) = *self.sized.lock().unwrap() {
            // SAFETY: the host's writable rect.
            unsafe { *size = sb::ViewRect { left: 0, top: 0, right: w, bottom: h } };
            return sb::kResultOk;
        }
        let s = *self.scale.lock().unwrap() * self.born;
        // SAFETY: the host's writable rect.
        unsafe { *size = sb::ViewRect { left: 0, top: 0, right: (200.0 * s) as i32, bottom: (100.0 * s) as i32 } };
        sb::kResultOk
    }
    unsafe fn onSize(&self, s: *mut sb::ViewRect) -> sb::tresult {
        // SAFETY: the host's rect for this call.
        let mut r = unsafe { *s };
        self.log(format!("onSize {}", r.right - r.left));
        *self.sized.lock().unwrap() = Some((r.right - r.left, r.bottom - r.top));
        let frame = self.frame.load(Ordering::Relaxed);
        if self.nest.load(Ordering::Relaxed) > 0
            // SAFETY: the frame the host set and keeps alive while set (or null).
            && let Some(f) = unsafe { vst3::ComRef::from_raw(frame) }
        {
            self.nest.fetch_sub(1, Ordering::Relaxed);
            r.right += 10;
            // SAFETY: the host's live frame; our own live view and a rect on this frame.
            let res = unsafe { f.resizeView(self.me.load(Ordering::Relaxed), &mut r) };
            self.log(format!("resizeView {} -> {res}", r.right - r.left));
        }
        sb::kResultOk
    }
    unsafe fn onFocus(&self, _s: sb::TBool) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn setFrame(&self, f: *mut sb::IPlugFrame) -> sb::tresult {
        self.frame.store(f, Ordering::Relaxed);
        if let Some(w) = self.ask
            // SAFETY: the frame the host just set (or null).
            && let Some(frame) = unsafe { vst3::ComRef::from_raw(f) }
        {
            let mut r = sb::ViewRect { left: 0, top: 0, right: w, bottom: 120 };
            // SAFETY: the host's live frame; our own live view and a rect on this frame.
            let res = unsafe { frame.resizeView(self.me.load(Ordering::Relaxed), &mut r) };
            let ctx = if thread_unaware() { "unaware" } else { "aware" };
            self.log(format!("asked {w} {ctx} -> {res}"));
        }
        sb::kResultOk
    }
    unsafe fn canResize(&self) -> sb::tresult {
        sb::kResultFalse
    }
    unsafe fn checkSizeConstraint(&self, _r: *mut sb::ViewRect) -> sb::tresult {
        sb::kResultOk
    }
}

#[cfg(windows)]
impl<const S: bool> IPlugViewContentScaleSupportTrait for FakeView<S> {
    unsafe fn setContentScaleFactor(&self, factor: f32) -> sb::tresult {
        if REFUSE.with(std::cell::Cell::get) {
            self.log(format!("scale {factor} refused"));
            return sb::kResultFalse;
        }
        self.log(format!("scale {factor}"));
        *self.scale.lock().unwrap() = factor;
        sb::kResultTrue
    }
}

#[cfg(windows)]
impl Class for FakeView<true> {
    type Interfaces = (sb::IPlugView, sb::IPlugViewContentScaleSupport);
}
#[cfg(windows)]
impl Class for FakeView<false> {
    type Interfaces = (sb::IPlugView,);
}

/// A scale state with the default setting (Auto, automatic mode), as a first open has it.
#[cfg(windows)]
fn auto_scale() -> ScaleState {
    ScaleState { stretch_available: transit_rt::editor_window::stretch_available(), ..ScaleState::default() }
}

/// How a fake view was made, by its interface pointer, so the host can make a sibling when it
/// makes the view again for a stretch (issue #153 review): scales, nest, reads the DPI, the log.
#[cfg(windows)]
type Spec = (bool, u32, bool, Arc<Mutex<Vec<String>>>);

#[cfg(windows)]
thread_local! {
    static SPECS: std::cell::RefCell<std::collections::HashMap<usize, Spec>> = std::cell::RefCell::new(std::collections::HashMap::new());
    /// Review round 3: the controller keeps a reference to the view it made and hands that same
    /// view back to a second `createView` while it lives.
    static SAME_VIEW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Review round 3: views made in the DPI-unaware context ask for this width from `setFrame`.
    static ASK_WHEN_UNAWARE: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
    /// Fakes log every `getSize` and the thread's DPI context at `attached` (Auto never stretches:
    /// the open's exact call sequence).
    static TRACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Fakes with the scale call answer it with `kResultFalse` and keep their size.
    static REFUSE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Makes another view like `view` (a fake made by [`make_fake`]), as the controller would; with
/// [`SAME_VIEW`] it holds `view` and gives it back.
#[cfg(windows)]
fn sibling_of(view: &ComPtr<sb::IPlugView>) -> impl FnMut() -> Option<ComPtr<sb::IPlugView>> + use<> {
    let spec = SPECS.with(|s| s.borrow().get(&(view.as_ptr() as usize)).cloned());
    let kept = SAME_VIEW.with(std::cell::Cell::get).then(|| view.clone());
    move || match &kept {
        Some(v) => Some(v.clone()),
        None => spec.as_ref().map(|(scales, nest, reads, calls)| make_fake(*scales, *nest, *reads, Arc::clone(calls))),
    }
}

/// Opens `view` in a window at `dpi` with the Scale `setting`, changes it to `then`, and returns
/// the content sizes after the open and after the change, and whether Windows stretches it.
#[cfg(windows)]
fn open_fake_with(view: &ComPtr<sb::IPlugView>, setting: EditorScale, dpi: usize, then: usize) -> ((i32, i32), (i32, i32), Option<f64>) {
    let mut scale = ScaleState { setting, ..auto_scale() };
    let header = Some(transit_rt::editor_window::HeaderCell::new());
    let mut remake = sibling_of(view);
    let (frame, win, view) =
        attach_view(view.clone(), window::PLATFORM_TYPE.unwrap(), "fake", header, &mut scale, &mut remake, |w| set_dpi(w, dpi)).unwrap();
    let opened = win.content_size();
    let stretched = win.stretch();
    set_dpi(&win, then);
    let mut g = EditorInner { view: Some(view), frame: Some(frame), window: Some(win), scale, ..EditorInner::default() };
    EditorLink::follow_scale(&mut g);
    let moved = g.window.as_ref().unwrap().content_size();
    EditorLink::teardown(&mut g);
    (opened, moved, stretched)
}

/// [`open_fake_with`] under Auto, without the stretch.
#[cfg(windows)]
fn open_fake(view: &ComPtr<sb::IPlugView>, dpi: usize, then: usize) -> ((i32, i32), (i32, i32)) {
    let (opened, moved, _) = open_fake_with(view, EditorScale::default(), dpi, then);
    (opened, moved)
}

#[cfg(windows)]
fn fake_view(scales: bool) -> (ComPtr<sb::IPlugView>, Arc<Mutex<Vec<String>>>) {
    fake_view_nesting(scales, 0)
}

/// [`fake_view`] whose `onSize` asks for a 10 px wider size through `resizeView` `nest` times.
#[cfg(windows)]
fn fake_view_nesting(scales: bool, nest: u32) -> (ComPtr<sb::IPlugView>, Arc<Mutex<Vec<String>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (make_fake(scales, nest, false, Arc::clone(&calls)), calls)
}

/// Whether this thread runs DPI-unaware now (what a view reading the DPI at `createView` sees).
#[cfg(windows)]
fn thread_unaware() -> bool {
    use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_UNAWARE, GetAwarenessFromDpiAwarenessContext, GetThreadDpiAwarenessContext};
    // SAFETY: no arguments, then the context value just returned.
    unsafe { GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext()) == DPI_AWARENESS_UNAWARE }
}

/// One fake view logging to `calls`; with `reads_dpi` it reads its monitor's scale when made
/// (1.5 unless this thread is DPI-unaware) and logs "made aware" or "made unaware".
#[cfg(windows)]
fn make_fake(scales: bool, nest: u32, reads_dpi: bool, calls: Arc<Mutex<Vec<String>>>) -> ComPtr<sb::IPlugView> {
    use std::sync::atomic::AtomicPtr;
    let born = if reads_dpi && !thread_unaware() { 1.5 } else { 1.0 };
    let ask = if thread_unaware() { ASK_WHEN_UNAWARE.with(std::cell::Cell::get) } else { None };
    if reads_dpi {
        calls.lock().unwrap().push(if born > 1.0 { "made aware".into() } else { "made unaware".into() });
    }
    let (c, scale) = (Arc::clone(&calls), Mutex::new(1.0));
    let (frame, me, nest_left) = (AtomicPtr::new(std::ptr::null_mut()), AtomicPtr::new(std::ptr::null_mut()), AtomicU32::new(nest));
    let view = if scales {
        let w = ComWrapper::new(FakeView::<true> { calls: c, scale, frame, me, nest: nest_left, born, ask, sized: Mutex::new(None) });
        let p = w.to_com_ptr::<sb::IPlugView>().unwrap();
        w.me.store(p.as_ptr(), Ordering::Relaxed);
        p
    } else {
        let w = ComWrapper::new(FakeView::<false> { calls: c, scale, frame, me, nest: nest_left, born, ask, sized: Mutex::new(None) });
        let p = w.to_com_ptr::<sb::IPlugView>().unwrap();
        w.me.store(p.as_ptr(), Ordering::Relaxed);
        p
    };
    SPECS.with(|s| s.borrow_mut().insert(view.as_ptr() as usize, (scales, nest, reads_dpi, calls)));
    view
}

/// Asks the host frame of a view opened by [`attach_view`] for `w`×100, as a plug-in would;
/// returns the result and the window's content width after it.
#[cfg(windows)]
fn resize_through_frame(view: &ComPtr<sb::IPlugView>, w: i32) -> (sb::tresult, i32) {
    let mut remake = sibling_of(view);
    let (frame, win, view) = attach_view(view.clone(), window::PLATFORM_TYPE.unwrap(), "nest", None, &mut auto_scale(), &mut remake, |_| {}).unwrap();
    let mut r = sb::ViewRect { left: 0, top: 0, right: w, bottom: 100 };
    // SAFETY: our live frame; the view is live and attached; `r` lives on this frame.
    let res = unsafe { frame.as_com_ref::<sb::IPlugFrame>().unwrap().resizeView(view.as_ptr(), &mut r) };
    let width = win.content_size().0;
    let mut g = EditorInner { view: Some(view.clone()), frame: Some(frame), window: Some(win), ..EditorInner::default() };
    EditorLink::teardown(&mut g);
    (res, width)
}

/// N3 of the #101 review: a plug-in that corrects its size once from inside `onSize` gets that
/// round trip (the SDK's editor host clears its guard before `onSize`), and the window ends at
/// the corrected size.
#[cfg(windows)]
#[test]
fn a_size_correction_from_on_size_gets_one_more_round_trip() {
    let (view, calls) = fake_view_nesting(false, 1);
    assert_eq!(resize_through_frame(&view, 300), (sb::kResultOk, 310));
    let calls = calls.lock().unwrap();
    assert_eq!(calls[1..calls.len() - 1], ["onSize 300", "onSize 310", format!("resizeView 310 -> {}", sb::kResultOk).as_str()]);
}

/// N1 of the #101 review: a plug-in that asks again from every `onSize` is refused at the
/// second nested `resizeView` instead of recursing without end.
#[cfg(windows)]
#[test]
fn a_plugin_that_keeps_resizing_from_on_size_is_refused_at_the_second_nesting() {
    let (view, calls) = fake_view_nesting(false, 10);
    assert_eq!(resize_through_frame(&view, 300), (sb::kResultOk, 310), "the first correction stands");
    let calls = calls.lock().unwrap();
    let on_size = calls.iter().filter(|c| c.starts_with("onSize")).count();
    assert_eq!(on_size, 2, "{calls:?}");
    assert!(calls.contains(&format!("resizeView 320 -> {}", sb::kResultFalse)), "{calls:?}");
}

/// The factor is the window's (DPI / 96), it comes before `attached`, again right after it
/// (issue #105), and a DPI change sends the new one; the window takes the size the view
/// reports at each.
#[cfg(windows)]
#[test]
fn the_scale_comes_before_attached_and_again_on_a_dpi_change() {
    let (view, calls) = fake_view(true);
    let (opened, moved) = open_fake(&view, 120, 144);
    assert_eq!(*calls.lock().unwrap(), ["scale 1.25", "attached", "scale 1.25", "scale 1.5", "removed"]);
    assert_eq!(opened, (250, 125));
    assert_eq!(moved, (300, 150));
}

/// A view without `IPlugViewContentScaleSupport` gets no call and keeps the size it reports in its
/// own pixels, at any DPI. Auto never stretches (the maintainer's decision after #153 shipped: a
/// GPU-drawn editor came up black stretched), so under Auto it is left at its own drawing as
/// before #153 (K74); Windows stretches it to the display's scale only when the user picks
/// Stretched by Windows.
#[cfg(windows)]
#[test]
fn a_view_without_the_scale_call_keeps_its_own_drawing_under_auto_and_is_stretched_when_picked() {
    let (view, calls) = fake_view(false);
    let (opened, moved, stretched) = open_fake_with(&view, EditorScale::default(), 144, 192);
    assert_eq!(*calls.lock().unwrap(), ["attached", "removed"]);
    assert_eq!((opened, moved, stretched), ((200, 100), (200, 100), None), "its own pixels, not stretched");
    let (view, calls) = fake_view(false);
    let (opened, _, stretched) = open_fake_with(&view, setting(ScaleStep::Auto, Some(ScaleMode::WindowsStretch)), 144, 192);
    assert_eq!(*calls.lock().unwrap(), ["attached", "removed"]);
    assert_eq!((opened, stretched), ((200, 100), Some(1.5)), "stretched at the display's scale (the simulated 144 DPI)");
}

/// Auto never stretches, call for call: on a per-monitor-aware thread at 144 DPI, a view that
/// reads the DPI when made (300×150) and refuses the factor, and one without the scale call, are
/// opened under Auto exactly as before #153: no second `createView` (one "made" line), `attached`
/// outside any DPI-unaware scope, the factor before and again after `attached` (issue #105), and
/// no stretched window.
#[cfg(windows)]
#[test]
fn under_auto_a_view_that_does_not_take_the_factor_is_opened_as_before_153() {
    let _pm = PerMonitor::enter();
    TRACE.with(|t| t.set(true));
    REFUSE.with(|r| r.set(true));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let view = make_fake(true, 0, true, Arc::clone(&calls));
    let (opened, moved, stretched) = open_fake_with(&view, EditorScale::default(), 144, 144);
    let refusing = calls.lock().unwrap().clone();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let view = make_fake(false, 0, true, Arc::clone(&calls));
    let (plain_opened, _, plain_stretched) = open_fake_with(&view, EditorScale::default(), 144, 144);
    let plain = calls.lock().unwrap().clone();
    TRACE.with(|t| t.set(false));
    REFUSE.with(|r| r.set(false));
    assert_eq!(
        refusing,
        [
            "made aware",
            "getSize",
            "getSize",
            "scale 1.5 refused",
            "getSize",
            "attached aware",
            "getSize",
            "scale 1.5 refused",
            "getSize",
            "getSize",
            "removed"
        ]
    );
    assert_eq!((opened, moved, stretched), ((300, 150), (300, 150), None), "its own drawing, never stretched");
    assert_eq!(plain, ["made aware", "getSize", "getSize", "attached aware", "getSize", "getSize", "removed"]);
    assert_eq!((plain_opened, plain_stretched), ((300, 150), None));
}

// ---- issue #105: a scale applied after `attached`, and `resizeView` at any time -------------

/// A `resizeView` that arrives before `attached` is kept (not dropped, not applied to a view
/// that is not attached yet) and applied right after `attached`, with the view's `onSize`.
#[cfg(windows)]
#[test]
fn a_resize_asked_before_attached_is_applied_once_attached() {
    let (view, calls) = fake_view(false);
    let win = window::HostWindow::new("early", 200.0, 100.0, None).unwrap();
    let frame = ComWrapper::new(PlugFrame::new(win.raw(), "early"));
    let f = frame.as_com_ref::<sb::IPlugFrame>().unwrap();
    let mut r = sb::ViewRect { left: 0, top: 0, right: 300, bottom: 150 };
    // SAFETY: our live frame; the view is live; `r` lives on this frame.
    assert_eq!(unsafe { f.resizeView(view.as_ptr(), &mut r) }, sb::kResultTrue);
    assert_eq!(win.content_size(), (200, 100), "kept until attached");
    assert!(calls.lock().unwrap().is_empty(), "no onSize before attached");
    frame.attach(&view);
    assert_eq!(win.content_size(), (300, 150), "applied once attached");
    assert_eq!(*calls.lock().unwrap(), ["onSize 300"]);
    let mut r = sb::ViewRect { left: 0, top: 0, right: 320, bottom: 160 };
    // SAFETY: as above.
    assert_eq!(unsafe { f.resizeView(view.as_ptr(), &mut r) }, sb::kResultTrue);
    assert_eq!(win.content_size(), (320, 160), "honored at once while attached");
    frame.detach();
    win.close();
}

/// Writes info-level log lines to stderr (a child process's log, for its parent test).
#[cfg(windows)]
struct StderrLog;

#[cfg(windows)]
impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata<'_>) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record<'_>) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

#[cfg(windows)]
static STDERR_LOG: StderrLog = StderrLog;

/// The fixture's scale mode (`VST3_FIXTURE_SCALE`) is read per process, so the parent test
/// runs the ignored test `child` of this module in a child process with `mode` set and
/// returns its output (the child's log included).
#[cfg(windows)]
fn scale_mode_child(child: &str, mode: &str) -> String {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("ffi::view_tests::{child}"), "--ignored", "--nocapture", "--test-threads=1"])
        .env("VST3_FIXTURE_SCALE", mode)
        .output()
        .unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed"), "the child ran: {text}");
    text
}

/// Opens Fixture Gain's editor in a window at 144 DPI (scale 1.5) with the log on stderr and
/// returns its content size.
#[cfg(windows)]
fn open_gain_at_144_dpi(mode: &str) -> (i32, i32) {
    assert_eq!(std::env::var("VST3_FIXTURE_SCALE").as_deref(), Ok(mode), "run by its parent test");
    if log::set_logger(&STDERR_LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    let inst = gain_instance();
    let link = inst.editor().unwrap();
    link.open_with("Fixture Gain", |w| set_dpi(w, 144)).unwrap();
    let size = link.lock().window.as_ref().unwrap().content_size();
    link.close();
    size
}

#[cfg(windows)]
#[test]
#[ignore = "run in a child process by the_window_takes_a_scale_the_view_applies_only_after_attached"]
fn scale_child_after_attach() {
    assert_eq!(open_gain_at_144_dpi("after-attach"), (480, 360), "320×240 at 1.5, applied after attached");
}

#[cfg(windows)]
#[test]
#[ignore = "run in a child process by a_refused_scale_is_logged_and_the_editor_keeps_its_own_drawing_under_auto"]
fn scale_child_refuse() {
    assert_eq!(open_gain_at_144_dpi("refuse"), (320, 240), "no scale, no resize");
}

/// The suspected Battery 4 behavior: the view accepts the factor before `attached` but applies
/// it only to one that arrives after, then asks for its scaled size through `resizeView`. The
/// factor sent again after `attached` reaches it and the window ends at the scaled size; the
/// log shows each step.
#[cfg(windows)]
#[test]
fn the_window_takes_a_scale_the_view_applies_only_after_attached() {
    let log = scale_mode_child("scale_child_after_attach", "after-attach");
    for line in [
        "vst3 editor \"Fixture Gain\": content scale before attached: IPlugViewContentScaleSupport obtained, setContentScaleFactor(1.5) -> kResultTrue (0); getSize 320×240 before, 320×240 after",
        "vst3 editor \"Fixture Gain\": resizeView 480×360: honored, window resized",
        "vst3 editor \"Fixture Gain\": content scale after attached: IPlugViewContentScaleSupport obtained, setContentScaleFactor(1.5) -> kResultTrue (0); getSize 320×240 before, 480×360 after",
    ] {
        assert!(log.contains(line), "{line:?} is not in the log:\n{log}");
    }
}

/// A view that refuses the factor (kResultFalse) is logged as such before `attached` and keeps the
/// size it reports in its own pixels. Auto never stretches: it is asked again after `attached` as
/// before #153 (issue #105), is not stretched, and the editor's scale line says so.
#[cfg(windows)]
#[test]
fn a_refused_scale_is_logged_and_the_editor_keeps_its_own_drawing_under_auto() {
    let log = scale_mode_child("scale_child_refuse", "refuse");
    let before = "vst3 editor \"Fixture Gain\": content scale before attached: IPlugViewContentScaleSupport obtained, setContentScaleFactor(1.5) -> kResultFalse (1); getSize 320×240 before, 320×240 after";
    assert!(log.contains(before), "{before:?} is not in the log:\n{log}");
    let after = "vst3 editor \"Fixture Gain\": content scale after attached: IPlugViewContentScaleSupport obtained, setContentScaleFactor(1.5) -> kResultFalse (1)";
    assert!(log.contains(after), "asked again after attached, as before #153:\n{log}");
    let line = "vst3 editor \"Fixture Gain\": scale: accepts a factor: no; factor in use: none; Windows stretch: none; in use: the plug-in's own drawing (100%)";
    assert!(log.contains(line), "{line:?} is not in the log:\n{log}");
    assert!(!log.contains("resizeView"), "{log}");
    assert!(!log.contains("window follows"), "{log}");
}

// ---- issue #153: the Scale setting through the fake views and the fixture -------------------

#[cfg(windows)]
use transit_dsp::editor_scale::{ScaleMode, ScaleStep};
#[cfg(windows)]
use transit_rt::editor_window::{HeaderClick, HeaderView, MenuItem};

#[cfg(windows)]
fn setting(step: ScaleStep, mode: Option<ScaleMode>) -> EditorScale {
    EditorScale { step, mode }
}

/// Stretched by Windows, picked by the user (Auto never stretches).
#[cfg(windows)]
fn stretch_picked() -> EditorScale {
    setting(ScaleStep::Auto, Some(ScaleMode::WindowsStretch))
}

/// The third kind of editor (it takes the factor; whether it draws at it is not known): with the
/// user's mode set to the Windows stretch it gets no scale call at all and is stretched; with its
/// own zoom it gets neither, not even on a DPI change.
#[cfg(windows)]
#[test]
fn a_view_that_takes_the_factor_can_be_stretched_or_left_to_its_own_zoom() {
    let (view, calls) = fake_view(true);
    let (opened, _, stretched) = open_fake_with(&view, setting(ScaleStep::Auto, Some(ScaleMode::WindowsStretch)), 144, 144);
    assert_eq!(*calls.lock().unwrap(), ["attached", "removed"], "no factor");
    assert_eq!((opened, stretched), ((200, 100), Some(1.5)));
    let (view, calls) = fake_view(true);
    let (opened, moved, stretched) = open_fake_with(&view, setting(ScaleStep::Auto, Some(ScaleMode::PluginZoom)), 144, 192);
    assert_eq!(*calls.lock().unwrap(), ["attached", "removed"], "no factor, not even on a DPI change");
    assert_eq!((opened, moved, stretched), ((200, 100), (200, 100), None));
}

/// A chosen percentage is the factor sent (before and after `attached`) and stays on a DPI change.
#[cfg(windows)]
#[test]
fn a_chosen_percentage_is_sent_and_kept_on_a_dpi_change() {
    let (view, calls) = fake_view(true);
    let (opened, moved, stretched) = open_fake_with(&view, setting(ScaleStep::Percent(200), None), 144, 192);
    assert_eq!(*calls.lock().unwrap(), ["scale 2", "attached", "scale 2", "removed"]);
    assert_eq!((opened, moved, stretched), ((400, 200), (400, 200), None));
}

/// No factor is ever applied twice: for every setting, on a view that takes the factor and on one
/// without the call, a stretched window never goes with a factor the view took.
#[cfg(windows)]
#[test]
fn no_factor_is_applied_twice_through_the_fake_views() {
    let steps = [ScaleStep::Auto, ScaleStep::Percent(50), ScaleStep::Percent(100), ScaleStep::Percent(150), ScaleStep::Percent(200)];
    let modes = [None, Some(ScaleMode::HostFactor), Some(ScaleMode::PluginZoom), Some(ScaleMode::WindowsStretch)];
    for scales in [true, false] {
        for step in steps {
            for mode in modes {
                let (view, calls) = fake_view(scales);
                let (_, _, stretched) = open_fake_with(&view, setting(step, mode), 144, 192);
                let calls = calls.lock().unwrap();
                // The fake that scales takes every factor it gets.
                let took = calls.iter().any(|c| c.starts_with("scale"));
                assert!(!(stretched.is_some() && took), "{scales} {step:?} {mode:?}: {calls:?} stretched {stretched:?}");
                assert!(scales || !took, "a view without the call got one: {calls:?}");
                // Auto never stretches: a view without the call keeps its own drawing unless the
                // user picked the stretch (the mode, or the display's 150%).
                let own = !scales && matches!(mode, None | Some(ScaleMode::HostFactor)) && step != ScaleStep::Percent(150);
                assert!(
                    stretched.is_some() || took || own || mode == Some(ScaleMode::PluginZoom) || step == ScaleStep::Percent(100),
                    "{scales} {step:?} {mode:?}: neither"
                );
                if setting(step, mode).is_default() {
                    assert_eq!(stretched, None, "Auto stretched: {calls:?}");
                }
            }
        }
    }
}

/// Through the fixture's editor link: the Scale submenu is in the header strip's view; a new
/// percentage applies at once; a change to the stretch applies on the next open and the menu says
/// so; a click on a Scale entry maps to the setting it stores; reopened, the editor is stretched
/// and gets no factor.
#[cfg(windows)]
#[test]
fn a_scale_change_applies_at_once_or_on_the_next_open_through_the_editor_link() {
    let inst = gain_instance();
    let link = inst.editor().unwrap();
    link.open_with("menu", |w| set_dpi(w, 144)).unwrap();
    let size = || link.lock().window.as_ref().unwrap().content_size();
    let rows = || link.header().view().scale;
    assert_eq!(size(), (480, 360));
    assert_eq!(rows()[0], MenuItem::Heading("In use: drawn by the plug-in at 150%".into()));
    link.set_header(HeaderView::default(), setting(ScaleStep::Percent(200), None));
    assert_eq!(size(), (640, 480), "200% applied at once");
    assert_eq!(rows()[0], MenuItem::Heading("In use: drawn by the plug-in at 200%".into()));
    link.set_header(HeaderView::default(), setting(ScaleStep::Auto, Some(ScaleMode::WindowsStretch)));
    assert_eq!(size(), (640, 480), "unchanged until the next open");
    assert_eq!(rows()[1], MenuItem::Heading("Next open: stretched by Windows to 150%".into()));
    let auto = rows().iter().position(|m| matches!(m, MenuItem::Item { label, .. } if label.starts_with("Automatic"))).unwrap();
    link.header().click(HeaderClick::Scale(auto));
    assert_eq!(link.header().take_clicks(), [HeaderClick::Scale(auto)]);
    assert_eq!(link.scale_pick(auto), Some(EditorScale::default()));
    link.close();
    link.open_with("menu", |w| set_dpi(w, 144)).unwrap();
    {
        let g = link.lock();
        let w = g.window.as_ref().unwrap();
        assert!(w.stretch().is_some(), "stretched now");
        assert_eq!(w.content_size(), (320, 240), "its own pixels: the view got no factor");
    }
    link.close();
}

/// Issue #153 review, item 5: a view that reads its monitor's DPI when it is made (made in the
/// per-monitor-aware context it reports 1.5 times its size, 300×150) and has no scale call is,
/// with Stretched by Windows picked, made again inside the stretched window's DPI-unaware scope before it is attached:
/// the attached view reads 96 DPI and reports 200×100, so Windows' 1.5 stretch is the only scale
/// (not 1.5 drawn and stretched again to 2.25).
#[cfg(windows)]
#[test]
fn a_view_that_reads_the_dpi_when_made_is_made_again_inside_the_unaware_scope() {
    let pm = PerMonitor::enter();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let view = make_fake(false, 0, true, Arc::clone(&calls));
    let (opened, moved, stretched) = open_fake_with(&view, stretch_picked(), 144, 144);
    drop(pm);
    assert_eq!(*calls.lock().unwrap(), ["made aware", "made unaware", "attached", "removed"], "made again, the second one attached");
    assert_eq!((opened, moved, stretched), ((200, 100), (200, 100), Some(1.5)), "its 100% size, stretched once");
}

/// The thread per-monitor aware for one test, as transit's are (a test binary is not).
#[cfg(windows)]
struct PerMonitor(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT);

#[cfg(windows)]
impl PerMonitor {
    fn enter() -> PerMonitor {
        use windows_sys::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
        // SAFETY: a predefined context value; this thread's context only, put back on drop.
        PerMonitor(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

#[cfg(windows)]
impl Drop for PerMonitor {
    fn drop(&mut self) {
        // SAFETY: the context this thread had, on this thread.
        unsafe { windows_sys::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(self.0) };
    }
}

/// Review round 3, finding 2 (Stretched by Windows picked): a controller that hands back its
/// existing view while that view lives (it kept a reference) gives the view that read 150% when it
/// was made. The host sees the
/// same object, does not stretch it, and attaches it to a window of the normal kind: drawn at its
/// own 150% (300×150), never 150% stretched again.
#[cfg(windows)]
#[test]
fn a_controller_that_gives_back_the_same_view_is_not_stretched() {
    let _pm = PerMonitor::enter();
    SAME_VIEW.with(|s| s.set(true));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let view = make_fake(false, 0, true, Arc::clone(&calls));
    let (opened, _, stretched) = open_fake_with(&view, stretch_picked(), 144, 144);
    SAME_VIEW.with(|s| s.set(false));
    assert_eq!(*calls.lock().unwrap(), ["made aware", "attached", "removed"]);
    assert_eq!((opened, stretched), ((300, 150), None), "its own drawing, not stretched on top");
}

/// Review round 3, note 4 (Stretched by Windows picked): the new view asks for 260×120 from `setFrame`, before it is attached,
/// while it still reports 200×100; that request is kept and applied once it is attached, with its
/// `onSize`.
#[cfg(windows)]
#[test]
fn a_size_the_new_view_asks_for_before_attached_is_applied() {
    let _pm = PerMonitor::enter();
    ASK_WHEN_UNAWARE.with(|a| a.set(Some(260)));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let view = make_fake(false, 0, true, Arc::clone(&calls));
    let (opened, _, stretched) = open_fake_with(&view, stretch_picked(), 144, 144);
    ASK_WHEN_UNAWARE.with(|a| a.set(None));
    let calls = calls.lock().unwrap().clone();
    assert!(stretched.is_some());
    assert_eq!(opened, (260, 120), "{calls:?}");
    assert!(calls.contains(&"onSize 260".to_owned()), "{calls:?}");
    // Review round 3, note 3: `setFrame` on the stretched view runs in the DPI-unaware scope.
    assert!(calls.contains(&format!("asked 260 unaware -> {}", sb::kResultTrue)), "{calls:?}");
}
