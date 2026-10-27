use super::tests::gain_instance;
use super::*;

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
        let g = link.lock();
        let win = g.window.as_ref().unwrap();
        assert_eq!(win.content_size(), (480, 360), "320×240 at 1.5");
        set_dpi(win, 192);
        EditorLink::follow_scale(&g);
        assert_eq!(win.content_size(), (640, 480), "the DPI change sends 2.0 and the view resizes");
        EditorLink::follow_scale(&g);
        assert_eq!(win.content_size(), (640, 480), "no change, no call");
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
        self.log("attached".into());
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
        let s = *self.scale.lock().unwrap();
        // SAFETY: the host's writable rect.
        unsafe { *size = sb::ViewRect { left: 0, top: 0, right: (200.0 * s) as i32, bottom: (100.0 * s) as i32 } };
        sb::kResultOk
    }
    unsafe fn onSize(&self, s: *mut sb::ViewRect) -> sb::tresult {
        // SAFETY: the host's rect for this call.
        let mut r = unsafe { *s };
        self.log(format!("onSize {}", r.right - r.left));
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

/// Opens `view` in a window at `dpi`, changes it to `then`, and returns the calls the view saw
/// and the content sizes after the open and after the change.
#[cfg(windows)]
fn open_fake(view: &ComPtr<sb::IPlugView>, dpi: usize, then: usize) -> ((i32, i32), (i32, i32)) {
    let (frame, win) = attach_view(view, window::PLATFORM_TYPE.unwrap(), "fake", Some(transit_rt::editor_window::HeaderCell::new()), |w| {
        set_dpi(w, dpi);
    })
    .unwrap();
    let opened = win.content_size();
    set_dpi(&win, then);
    let mut g = EditorInner { view: Some(view.clone()), frame: Some(frame), window: Some(win), ..EditorInner::default() };
    EditorLink::follow_scale(&g);
    let moved = g.window.as_ref().unwrap().content_size();
    EditorLink::teardown(&mut g);
    (opened, moved)
}

#[cfg(windows)]
fn fake_view(scales: bool) -> (ComPtr<sb::IPlugView>, Arc<Mutex<Vec<String>>>) {
    fake_view_nesting(scales, 0)
}

/// [`fake_view`] whose `onSize` asks for a 10 px wider size through `resizeView` `nest` times.
#[cfg(windows)]
fn fake_view_nesting(scales: bool, nest: u32) -> (ComPtr<sb::IPlugView>, Arc<Mutex<Vec<String>>>) {
    use std::sync::atomic::AtomicPtr;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let (c, scale) = (Arc::clone(&calls), Mutex::new(1.0));
    let (frame, me, nest) = (AtomicPtr::new(std::ptr::null_mut()), AtomicPtr::new(std::ptr::null_mut()), AtomicU32::new(nest));
    let view = if scales {
        let w = ComWrapper::new(FakeView::<true> { calls: c, scale, frame, me, nest });
        let p = w.to_com_ptr::<sb::IPlugView>().unwrap();
        w.me.store(p.as_ptr(), Ordering::Relaxed);
        p
    } else {
        let w = ComWrapper::new(FakeView::<false> { calls: c, scale, frame, me, nest });
        let p = w.to_com_ptr::<sb::IPlugView>().unwrap();
        w.me.store(p.as_ptr(), Ordering::Relaxed);
        p
    };
    (view, calls)
}

/// Asks the host frame of a view opened by [`attach_view`] for `w`×100, as a plug-in would;
/// returns the result and the window's content width after it.
#[cfg(windows)]
fn resize_through_frame(view: &ComPtr<sb::IPlugView>, w: i32) -> (sb::tresult, i32) {
    let (frame, win) = attach_view(view, window::PLATFORM_TYPE.unwrap(), "nest", None, |_| {}).unwrap();
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

/// A view without `IPlugViewContentScaleSupport` (a plug-in that scales itself) gets no call
/// and no transform of ours: the window stays at the size it reports, at any DPI.
#[cfg(windows)]
#[test]
fn a_view_that_scales_itself_gets_no_call_and_no_transform() {
    let (view, calls) = fake_view(false);
    let (opened, moved) = open_fake(&view, 144, 192);
    assert_eq!(*calls.lock().unwrap(), ["attached", "removed"]);
    assert_eq!(opened, (200, 100));
    assert_eq!(moved, (200, 100));
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
#[ignore = "run in a child process by a_refused_scale_is_logged_and_leaves_the_window_alone"]
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

/// A view that refuses the factor (kResultFalse) is logged as such, before and after
/// `attached`, and the window keeps the size it reports.
#[cfg(windows)]
#[test]
fn a_refused_scale_is_logged_and_leaves_the_window_alone() {
    let log = scale_mode_child("scale_child_refuse", "refuse");
    for when in ["before attached", "after attached"] {
        let line = format!(
            "vst3 editor \"Fixture Gain\": content scale {when}: IPlugViewContentScaleSupport obtained, setContentScaleFactor(1.5) -> kResultFalse (1); getSize 320×240 before, 320×240 after"
        );
        assert!(log.contains(&line), "{line:?} is not in the log:\n{log}");
    }
    assert!(!log.contains("resizeView"), "{log}");
    assert!(!log.contains("window follows"), "{log}");
}
