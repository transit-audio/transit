//! The FFI boundary: the only module in SoundCraft allowed to use `unsafe`.
//!
//! It loads CLAP binaries (`libloading`), calls the plugin entry/factory/plugin vtables
//! (`clap-sys` raw bindings) and implements the host callbacks. Everything it exposes to the rest
//! of the crate is a safe API returning `Result`.
//!
//! Soundness rests on the CLAP contract (the plugin honours the vtable signatures and the
//! documented pointer lifetimes). A plugin that violates it (e.g. dereferences garbage) can still
//! crash the process: hosting in-process can't prevent that. What this module guarantees is that
//! *SoundCraft* never hands the plugin dangling or mis-sized pointers, never calls a missing
//! (null) function pointer, and never reads plugin strings beyond their bounds.

use crate::{ClapError, scan, trace};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_EXPRESSION, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
    CLAP_EVENT_TRANSPORT, CLAP_NOTE_EXPRESSION_PRESSURE, CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE,
    CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_PLAYING, CLAP_TRANSPORT_IS_RECORDING, clap_event_header,
    clap_event_midi, clap_event_note, clap_event_note_expression, clap_event_param_value, clap_event_transport, clap_input_events,
    clap_output_events,
};
use clap_sys::ext::audio_ports::{CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_plugin_audio_ports};
use clap_sys::ext::gui::{CLAP_EXT_GUI, clap_host_gui, clap_plugin_gui, clap_window, clap_window_handle};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency, clap_plugin_latency};
use clap_sys::ext::log::{CLAP_EXT_LOG, CLAP_LOG_ERROR, CLAP_LOG_FATAL, CLAP_LOG_WARNING, clap_host_log, clap_log_severity};
use clap_sys::ext::note_ports::{CLAP_EXT_NOTE_PORTS, clap_note_port_info, clap_plugin_note_ports};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, clap_host_params, clap_param_clear_flags, clap_param_info, clap_param_rescan_flags, clap_plugin_params,
};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_host_state, clap_plugin_state};
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_host_tail, clap_plugin_tail};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{CLAP_VERSION, clap_version_is_compatible};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use transit_dsp::Transport;

/// Most plugins one factory may report (hostile-input cap).
const MAX_PLUGINS_PER_FACTORY: u32 = 1024;
/// Most parameters read from one plugin.
pub(crate) const MAX_PARAMS: u32 = 4096;
/// Most audio ports per direction we provide buffers for.
pub(crate) const MAX_PORTS: u32 = 16;
/// Most channels per audio port.
pub(crate) const MAX_PORT_CHANNELS: u32 = 64;
/// Most feature strings read from a descriptor.
const MAX_FEATURES: usize = 64;
/// Timed parameter points one block keeps (preallocated: `process` never allocates).
pub(crate) const EVENT_CAP: usize = 1024;
/// Note and MIDI events one block can hand: the whole ownership ledger at two events per entry,
/// the ordinary allowance and the safety slots (ADR 0059, Decision 1).
pub(crate) const NATIVE_EVENTS: usize = transit_dsp::plugins::instrument_queue::delivery::NATIVE_EVENTS;
/// Largest plugin state we accept from `clap_plugin_state.save` (hostile-input cap).
pub(crate) const MAX_STATE_BYTES: usize = 64 << 20;
/// Most editor parameter edits buffered between two UI frames.
const EDIT_CAP: usize = 256;

// ---- plugin strings ------------------------------------------------------------------------

/// Reads a NUL-terminated plugin string (`None` for null).
///
/// SAFETY contract for callers: `p` is null or points to a NUL-terminated string that stays valid
/// for the duration of the call (CLAP guarantees this for descriptor fields).
unsafe fn read_cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated per this function's contract.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Reads a fixed-size `char[N]` name buffer, stopping at the first NUL or the end of the array.
/// `c_char` is `i8` on x86 and `u8` on ARM/RISC-V Linux; the byte reinterpretation is the same for both.
fn read_name(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| u8::from_ne_bytes(c.to_ne_bytes())).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

mod audio;
mod events;
mod gui;
#[cfg(all(test, any(windows, target_os = "linux")))]
mod gui_diagnostics_tests;
mod gui_probe;
mod host;
mod instance;
mod module;
#[cfg(target_os = "linux")]
mod run_loop;
mod state;
#[cfg(test)]
mod storage_tests;

pub(crate) use audio::*;
pub(crate) use events::*;
pub(crate) use gui::*;
pub(crate) use host::*;
pub(crate) use instance::*;
pub(crate) use module::*;
use state::*;

// ---- host window for embedded editors ------------------------------------------------------

/// The Windows and Linux host window for embedded editors: `transit_rt`'s editor window, a
/// top-level window on the plug-in host thread. On Windows (issue #68) its client area is a header
/// strip above the plugin's parent `HWND` and nothing else; on Linux (issue #147) it is an X11
/// window whose content window is the plugin's `x11` parent (no strip yet). Closing it only hides
/// it; the editor link notices in `idle`, destroys the plugin's GUI, then the window.
#[cfg(any(windows, target_os = "linux"))]
mod hostwin {
    pub use transit_rt::editor_window::{EditorWindow as HostWindow, resize_content as resize};

    pub const SUPPORTED: bool = true;

    /// The window's DPI scale, for `set_scale` (issue #101).
    pub fn content_scale(win: &HostWindow) -> Option<f64> {
        Some(win.scale())
    }

    /// The new scale after a DPI change, once.
    pub fn scale_change(win: &HostWindow) -> Option<f64> {
        win.take_scale_change()
    }

    /// The plug-in content's size.
    pub fn content_size(win: &HostWindow) -> (u32, u32) {
        let (w, h) = win.content_size();
        (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0))
    }

    /// A window Windows stretches (issue #153): its content container is DPI-unaware.
    #[cfg(windows)]
    pub fn new_stretched(
        title: &str,
        w: f64,
        h: f64,
        header: Option<std::sync::Arc<transit_rt::editor_window::HeaderCell>>,
    ) -> Result<HostWindow, String> {
        HostWindow::new_stretched(title, w, h, header)
    }

    /// No stretch on Linux (issue #153: only GUIs that accept a factor are scaled there).
    #[cfg(target_os = "linux")]
    pub fn new_stretched(
        _title: &str,
        _w: f64,
        _h: f64,
        _header: Option<std::sync::Arc<transit_rt::editor_window::HeaderCell>>,
    ) -> Result<HostWindow, String> {
        Err("editors are not stretched on Linux".into())
    }

    /// Runs `f` (a GUI call that may make its window) in the window's plug-in scope: the
    /// DPI-unaware thread context for a stretched window (issue #153).
    #[cfg(windows)]
    pub fn in_plugin_scope<R>(win: &HostWindow, f: impl FnOnce() -> R) -> R {
        let _scope = win.plugin_scope();
        f()
    }

    /// Runs `f`: nothing to scope on Linux.
    #[cfg(target_os = "linux")]
    pub fn in_plugin_scope<R>(_win: &HostWindow, f: impl FnOnce() -> R) -> R {
        f()
    }
}
/// The macOS host window: `transit_rt`'s `NSWindow` editor window (the one the VST3 host uses), on
/// the process main thread; its content view is the plugin's `cocoa` parent. No header strip, as on
/// Linux. Closing it only hides it, as above.
#[cfg(target_os = "macos")]
mod hostwin {
    pub use transit_rt::editor_window::{EditorWindow as HostWindow, resize_content as resize};

    pub const SUPPORTED: bool = true;

    /// None: clap gui.h on `CLAP_WINDOW_API_COCOA`: "uses logical size, don't call
    /// clap_plugin_gui->set_scale()".
    pub fn content_scale(_win: &HostWindow) -> Option<f64> {
        None
    }

    pub fn scale_change(_win: &HostWindow) -> Option<f64> {
        None
    }

    /// The plug-in content's size, in points (cocoa's logical size).
    pub fn content_size(win: &HostWindow) -> (u32, u32) {
        let (w, h) = win.content_size();
        (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0))
    }

    /// No stretch on macOS (it scales editors itself, issue #153).
    pub fn new_stretched(
        _title: &str,
        _w: f64,
        _h: f64,
        _header: Option<std::sync::Arc<transit_rt::editor_window::HeaderCell>>,
    ) -> Result<HostWindow, String> {
        Err("macOS scales plug-in editors itself".into())
    }

    /// Runs `f`: nothing to scope on macOS.
    pub fn in_plugin_scope<R>(_win: &HostWindow, f: impl FnOnce() -> R) -> R {
        f()
    }
}
/// Other platforms: embedded editors are not hosted (floating ones still are).
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod hostwin {
    use std::ffi::c_void;

    pub const SUPPORTED: bool = false;

    pub struct HostWindow;

    impl HostWindow {
        pub fn new(
            _title: &str,
            _w: f64,
            _h: f64,
            _header: Option<std::sync::Arc<transit_rt::editor_window::HeaderCell>>,
        ) -> Result<HostWindow, String> {
            Err("embedded plugin editors are not hosted on this platform".into())
        }
        pub fn raw(&self) -> *mut c_void {
            std::ptr::null_mut()
        }
        pub fn parent(&self) -> *mut c_void {
            std::ptr::null_mut()
        }
        pub fn raise(&self) {}
        pub fn is_visible(&self) -> bool {
            false
        }
        pub fn sync(&self) {}
        pub fn close(self) {}
    }

    pub fn resize(_window: *mut c_void, _w: f64, _h: f64) {}

    pub fn content_scale(_win: &HostWindow) -> Option<f64> {
        None
    }

    pub fn scale_change(_win: &HostWindow) -> Option<f64> {
        None
    }

    pub fn content_size(_win: &HostWindow) -> (u32, u32) {
        (0, 0)
    }

    pub fn new_stretched(
        _title: &str,
        _w: f64,
        _h: f64,
        _header: Option<std::sync::Arc<transit_rt::editor_window::HeaderCell>>,
    ) -> Result<HostWindow, String> {
        Err("embedded plugin editors are not hosted on this platform".into())
    }

    pub fn in_plugin_scope<R>(_win: &HostWindow, f: impl FnOnce() -> R) -> R {
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn editor_thread_is_its_creator_not_a_name() {
        let link = Arc::new(GuiLink::default());
        assert!(link.on_main_thread(), "a host thread need not be named main");
        assert!(!std::thread::spawn(move || link.on_main_thread()).join().unwrap(), "another thread cannot drive this editor");
    }

    #[test]
    fn processing_failures_do_not_allocate() {
        crate::set_scan_extra_dirs_only(true);
        let descriptors = crate::scan_paths(&[fixture::unregistered_fixture_dir().to_path_buf()]);
        let d = descriptors.iter().find(|d| !d.is_instrument).unwrap();
        let mut inst = Bundle::load(&d.path).unwrap().instantiate(&d.plugin_id).unwrap();
        let mut inputs = PortBuffers::new(&[2], 64);
        let mut outputs = PortBuffers::new(&[2], 64);
        let original = inst.plugin;
        let mut vt = inst.vt();
        unsafe extern "C" fn refuse_start(_: *const clap_plugin) -> bool {
            false
        }
        unsafe extern "C" fn fail_process(_: *const clap_plugin, _: *const clap_process) -> i32 {
            CLAP_PROCESS_ERROR
        }
        for case in 0..5 {
            inst.active = case != 0;
            inst.max_frames = 64;
            inst.processing = case >= 3;
            vt.start_processing = Some(refuse_start);
            vt.process = if case == 3 { None } else { Some(fail_process) };
            // The copied vtable stays alive until the original pointer is restored below.
            inst.plugin = &vt;
            let frames = if case == 1 { 65 } else { 64 };
            let (failed, stats) = allocation_probe::measure(|| {
                let result = inst.process(frames, 0, &mut inputs, &mut outputs, &[], 0, None);
                result.is_err()
            });
            // Restore the genuine lifecycle state before any assertion can unwind.
            inst.plugin = original;
            inst.active = false;
            inst.processing = false;
            assert!(failed, "case {case}");
            assert!(stats.is_zero(), "case {case}: {stats:?}");
        }
    }

    #[test]
    fn the_transport_reaches_clap_in_fixed_point() {
        let t = Transport {
            playing: true,
            tempo: 90.0,
            numerator: 3,
            denominator: 4,
            samples: 96_000,
            seconds: 2.0,
            quarters: 3.5,
            bar_start: 3.0,
            bar: 1,
            ..Transport::default()
        };
        let e = transport_event(&t);
        assert_eq!((e.header.type_, e.header.size as usize), (CLAP_EVENT_TRANSPORT, size_of::<clap_event_transport>()));
        assert_eq!(e.flags & CLAP_TRANSPORT_IS_PLAYING, CLAP_TRANSPORT_IS_PLAYING);
        assert_eq!(e.flags & CLAP_TRANSPORT_IS_RECORDING, 0);
        assert_eq!((e.song_pos_beats, e.bar_start), (7 * CLAP_BEATTIME_FACTOR / 2, 3 * CLAP_BEATTIME_FACTOR));
        assert_eq!(e.song_pos_seconds, 2 * CLAP_SECTIME_FACTOR);
        assert_eq!((e.tempo, e.bar_number, e.tsig_num, e.tsig_denom), (90.0, 1, 3, 4));
        let nan = transport_event(&Transport { quarters: f64::NAN, ..t });
        assert_eq!(nan.song_pos_beats, 0);
    }

    #[test]
    fn the_tail_is_read_through_the_extension_and_offered_to_the_plugin() {
        unsafe extern "C" fn one_second(_: *const clap_plugin) -> u32 {
            48_000
        }
        unsafe extern "C" fn infinite(_: *const clap_plugin) -> u32 {
            u32::MAX
        }
        let finite = clap_plugin_tail { get: Some(one_second) };
        let forever = clap_plugin_tail { get: Some(infinite) };
        let no_get = clap_plugin_tail { get: None };
        // SAFETY: the fakes ignore the plugin pointer.
        unsafe {
            assert_eq!(read_tail(&finite, std::ptr::null()), Some(48_000));
            assert_eq!(read_tail(&forever, std::ptr::null()), Some(u32::MAX));
            assert_eq!(read_tail(&no_get, std::ptr::null()), None);
            assert_eq!(read_tail(std::ptr::null(), std::ptr::null()), None);
        }
        // SAFETY: a valid NUL-terminated id; the host pointer is unused.
        let ext = unsafe { host_get_extension(std::ptr::null(), CLAP_EXT_TAIL.as_ptr()) };
        assert_eq!(ext, (&HOST_TAIL as *const clap_host_tail).cast());
        // SAFETY: `changed` ignores the host pointer.
        unsafe { (HOST_TAIL.changed.unwrap())(std::ptr::null()) };
    }

    #[test]
    fn silent_inputs_set_one_constant_bit_per_channel() {
        assert_eq!(all_channels_mask(0), 0);
        assert_eq!(all_channels_mask(1), 0b1);
        assert_eq!(all_channels_mask(2), 0b11);
        assert_eq!(all_channels_mask(6), 0b11_1111);
        assert_eq!(all_channels_mask(64), u64::MAX);
        let mut b = PortBuffers::new(&[2, 1], 4);
        b.set_constant(true);
        b.refresh();
        assert_eq!(b.bufs.iter().map(|x| x.constant_mask).collect::<Vec<_>>(), vec![0b11, 0b1]);
        b.set_constant(false);
        b.refresh();
        assert!(b.bufs.iter().all(|x| x.constant_mask == 0));
    }

    // ---- issue #101: content scale ------------------------------------------------------------

    /// Changes the window's DPI as Windows does when it moves to a monitor with other scaling.
    #[cfg(windows)]
    fn set_dpi(win: &hostwin::HostWindow, dpi: usize) {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_DPICHANGED};
        let mut suggested = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: a live window of this thread; WM_DPICHANGED's wparam carries the DPI twice and
        // its lparam points to a suggested RECT, which outlives the call.
        unsafe { SendMessageW(win.raw().cast(), WM_DPICHANGED, (dpi << 16) | dpi, (&raw mut suggested) as isize) };
    }

    /// The fixture gain with an embedded GUI, after `commands` (fixture state commands).
    #[cfg(any(windows, target_os = "linux"))]
    fn embedded_gain(commands: &[&[u8]]) -> Instance {
        crate::set_scan_extra_dirs_only(true);
        let descriptors = crate::scan_paths(&[fixture::unregistered_fixture_dir().to_path_buf()]);
        let d = descriptors.iter().find(|d| !d.is_instrument).unwrap();
        let inst = Bundle::load(&d.path).unwrap().instantiate(&d.plugin_id).unwrap();
        for c in [b"CMD:gui-embedded".as_slice()].iter().chain(commands) {
            assert!(inst.load_state(c));
        }
        inst
    }

    /// Opens the embedded editor in a window at `dpi` with the Scale `setting`, then moves it to
    /// `then`: the content sizes after the open and after the move, and whether Windows stretches
    /// it (issue #153).
    #[cfg(windows)]
    fn open_scaled_with(inst: &Instance, setting: transit_dsp::EditorScale, dpi: usize, then: usize) -> ((u32, u32), (u32, u32), bool) {
        let link = inst.gui().unwrap();
        link.set_header(transit_rt::editor_window::HeaderView::default(), setting);
        link.open_with("scale", |w| set_dpi(w, dpi)).unwrap();
        let sizes = {
            let mut g = link.lock();
            let size = |g: &GuiInner| hostwin::content_size(g.window.as_ref().expect("embedded"));
            let opened = size(&g);
            let stretched = g.window.as_ref().unwrap().stretch().is_some();
            set_dpi(g.window.as_ref().unwrap(), then);
            GuiLink::follow_scale(&mut g);
            (opened, size(&g), stretched)
        };
        link.close();
        sizes
    }

    /// [`open_scaled_with`] under Auto, without the stretch.
    #[cfg(windows)]
    fn open_scaled(inst: &Instance, dpi: usize, then: usize) -> ((u32, u32), (u32, u32)) {
        let (opened, moved, _) = open_scaled_with(inst, transit_dsp::EditorScale::default(), dpi, then);
        (opened, moved)
    }

    /// Stretched by Windows, picked by the user (Auto never stretches).
    #[cfg(windows)]
    fn stretch_picked() -> transit_dsp::EditorScale {
        use transit_dsp::editor_scale::{ScaleMode, ScaleStep};
        transit_dsp::EditorScale { step: ScaleStep::Auto, mode: Some(ScaleMode::WindowsStretch) }
    }

    /// Opens the embedded editor of `inst` (after `CMD:gui-trace`) at 144 DPI with `setting` and
    /// returns the GUI calls the fixture saw during the open, and whether the window is stretched.
    #[cfg(windows)]
    fn traced_open(inst: &Instance, setting: transit_dsp::EditorScale) -> (Vec<String>, bool) {
        assert!(inst.load_state(b"CMD:gui-trace"));
        let link = inst.gui().unwrap();
        link.set_header(transit_rt::editor_window::HeaderView::default(), setting);
        link.open_with("trace", |w| set_dpi(w, 144)).unwrap();
        let stretched = link.lock().window.as_ref().expect("embedded").stretch().is_some();
        let state = link.save_state().unwrap();
        link.close();
        let trace = String::from_utf8(state[16..].to_vec()).unwrap();
        (trace.lines().map(str::to_owned).collect(), stretched)
    }

    /// `set_scale` gets the window's factor before `set_parent` (the fixture asserts a created
    /// embedded GUI), the window takes the size `get_size` then reports, and a DPI change sends
    /// the new factor and follows the new size.
    #[cfg(windows)]
    #[test]
    fn an_embedded_gui_gets_the_windows_scale_and_the_window_follows_its_size() {
        let inst = embedded_gain(&[]);
        assert_eq!(open_scaled(&inst, 144, 192), ((600, 450), (800, 600)));
    }

    /// N2 of the #101 review: the editor's `idle` (not a direct `follow_scale`) sends the new
    /// scale after a DPI change. The window must be visible for `idle` to follow it: it is shown
    /// without activation. (`idle` first applies the fixture's `request_resize` from `show`.)
    #[cfg(windows)]
    #[test]
    fn idle_sends_the_new_scale_after_a_dpi_change() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
        let inst = embedded_gain(&[]);
        let link = inst.gui().unwrap();
        link.open_with("idle", |w| set_dpi(w, 144)).unwrap();
        {
            let g = link.lock();
            let win = g.window.as_ref().expect("embedded");
            // SAFETY: a live window of this thread.
            unsafe { ShowWindow(win.raw().cast(), SW_SHOWNOACTIVATE) };
            assert_eq!(hostwin::content_size(win), (600, 450));
            set_dpi(win, 192);
        }
        let _ = link.idle();
        assert!(link.is_open(), "idle kept the visible window");
        assert_eq!(hostwin::content_size(link.lock().window.as_ref().unwrap()), (800, 600), "idle sent 2.0");
        link.close();
    }

    /// Linux (issue #147): an embedded GUI is parented in an X11 window of ours on this thread
    /// (`CLAP_WINDOW_API_X11`), takes the size it reports, and goes with the window on close.
    /// Needs an X server (CI runs these tests under `xvfb-run -a`); skipped without one.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_embedded_gui_opens_in_an_x11_window() {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("an_embedded_gui_opens_in_an_x11_window: skipped: no DISPLAY (run under `xvfb-run -a`)");
            return;
        }
        let inst = embedded_gain(&[]);
        let link = inst.gui().unwrap();
        link.open_with("x11", |_| {}).unwrap();
        {
            let g = link.lock();
            let win = g.window.as_ref().expect("embedded, not floating");
            assert!(!win.parent().is_null(), "the content window is the plug-in's parent");
            let size = hostwin::content_size(win);
            assert_eq!(Some(size), GuiLink::size(&g, GuiLink::vt(&g).unwrap()), "the window takes the size the GUI reports");
        }
        link.close();
        assert!(link.lock().window.is_none());
        assert!(!link.is_open());
    }

    /// A GUI that ignores `set_scale` keeps the size it reports in its own pixels at any DPI and
    /// gets no other factor. Auto never stretches (the maintainer's decision after #153 shipped: a
    /// GPU-drawn editor came up black stretched): under Auto it is left at 100 percent as before
    /// #153; Windows stretches it (its GUI made again in a DPI-unaware container) only when the
    /// user picks Stretched by Windows.
    #[cfg(windows)]
    #[test]
    fn a_gui_that_ignores_set_scale_keeps_its_own_drawing_under_auto_and_is_stretched_when_picked() {
        let inst = embedded_gain(&[b"CMD:gui-self-scaling"]);
        assert_eq!(open_scaled_with(&inst, transit_dsp::EditorScale::default(), 144, 192), ((400, 300), (400, 300), false));
        assert_eq!(open_scaled_with(&inst, stretch_picked(), 144, 192), ((400, 300), (400, 300), true));
    }

    /// Auto never stretches, call for call: under Auto the embedded GUI is opened exactly as
    /// before #153 (`create`, `set_scale` with the window's factor, the probe's `get_size`,
    /// `set_parent`, `show`), whether it takes the factor or ignores it: one `create`, no
    /// `destroy`, no stretched window (so no DPI-unaware scope: the scope is inert for a window
    /// that is not stretched). With Stretched by Windows picked the GUI is made again instead.
    #[cfg(windows)]
    #[test]
    fn under_auto_an_embedded_gui_is_opened_as_before_153() {
        let ignores = embedded_gain(&[b"CMD:gui-self-scaling"]);
        assert_eq!(
            traced_open(&ignores, transit_dsp::EditorScale::default()),
            (["create", "set_scale 1.5 -> false", "get_size", "set_parent", "show"].map(String::from).to_vec(), false)
        );
        let takes = embedded_gain(&[]);
        assert_eq!(
            traced_open(&takes, transit_dsp::EditorScale::default()),
            (["create", "set_scale 1.5 -> true", "get_size", "set_parent", "show"].map(String::from).to_vec(), false)
        );
        let picked = embedded_gain(&[b"CMD:gui-self-scaling"]);
        let (calls, stretched) = traced_open(&picked, stretch_picked());
        assert!(stretched, "{calls:?}");
        assert_eq!(calls.iter().filter(|c| *c == "create").count(), 2, "made again for the picked stretch: {calls:?}");
    }

    /// Issue #153 review, item 4 (Stretched by Windows picked): a resize the discarded GUI asked for (600×450, in `create`,
    /// before it refused `set_scale` and was made again for the stretch) does not reach the
    /// replacement, which reports 400×300: the next idle leaves the window at 400×300.
    #[cfg(windows)]
    #[test]
    fn a_resize_the_discarded_gui_asked_for_does_not_reach_its_replacement() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
        let inst = embedded_gain(&[b"CMD:gui-self-scaling", b"CMD:gui-resize-on-create-once", b"CMD:gui-quiet-show"]);
        let link = inst.gui().unwrap();
        link.set_header(transit_rt::editor_window::HeaderView::default(), stretch_picked());
        link.open_with("replaced", |w| set_dpi(w, 144)).unwrap();
        {
            let g = link.lock();
            let win = g.window.as_ref().expect("embedded");
            assert!(win.stretch().is_some(), "made again for the stretch");
            assert_eq!(hostwin::content_size(win), (400, 300));
            // SAFETY: a live window of this thread (idle keeps only a visible one).
            unsafe { ShowWindow(win.raw().cast(), SW_SHOWNOACTIVATE) };
        }
        let _ = link.idle();
        assert!(link.is_open());
        assert_eq!(hostwin::content_size(link.lock().window.as_ref().unwrap()), (400, 300), "the old 600×450 request was dropped");
        link.close();
    }

    /// Review round 3, note 5: a hide an earlier GUI left pending is dropped when an editor is
    /// opened again; the next idle keeps the new GUI shown.
    #[cfg(windows)]
    #[test]
    fn a_hide_left_by_an_earlier_gui_does_not_reach_the_next_one() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};
        let inst = embedded_gain(&[b"CMD:gui-quiet-show"]);
        let link = inst.gui().unwrap();
        link.host().gui_hide.store(true, Ordering::Relaxed);
        link.open_with("reopen", |_| {}).unwrap();
        {
            let g = link.lock();
            // SAFETY: a live window of this thread (idle keeps only a visible one).
            unsafe { ShowWindow(g.window.as_ref().unwrap().raw().cast(), SW_SHOWNOACTIVATE) };
        }
        let _ = link.idle();
        assert!(link.lock().visible, "the old hide was dropped");
        link.close();
    }

    /// Issue #153 through the fixture's GUI link: the user's own-zoom mode sends no `set_scale`
    /// (the GUI keeps 400×300) and nothing is stretched; a percentage while open is sent at once
    /// and the window takes the size the GUI then reports; a change to the stretch waits for the
    /// next open, and the Scale submenu says so.
    #[cfg(windows)]
    #[test]
    fn the_scale_setting_reaches_the_gui_before_the_open_and_at_once_while_open() {
        use transit_dsp::editor_scale::{EditorScale, ScaleMode, ScaleStep};
        use transit_rt::editor_window::{HeaderView, MenuItem};
        let inst = embedded_gain(&[]);
        let link = inst.gui().unwrap();
        let zoom = EditorScale { step: ScaleStep::Auto, mode: Some(ScaleMode::PluginZoom) };
        link.set_header(HeaderView::default(), zoom);
        link.open_with("zoom", |w| set_dpi(w, 144)).unwrap();
        {
            let g = link.lock();
            let w = g.window.as_ref().unwrap();
            assert_eq!((hostwin::content_size(w), w.stretch()), ((400, 300), None), "no factor, no stretch");
        }
        link.close();
        link.set_header(HeaderView::default(), EditorScale::default());
        link.open_with("auto", |w| set_dpi(w, 144)).unwrap();
        let size = || hostwin::content_size(link.lock().window.as_ref().unwrap());
        assert_eq!(size(), (600, 450), "Auto: 1.5");
        link.set_header(HeaderView::default(), EditorScale { step: ScaleStep::Percent(200), mode: None });
        assert_eq!(size(), (800, 600), "200% at once");
        link.set_header(HeaderView::default(), EditorScale { step: ScaleStep::Auto, mode: Some(ScaleMode::WindowsStretch) });
        assert_eq!(size(), (800, 600), "the stretch waits for the next open");
        assert_eq!(link.header().view().scale[1], MenuItem::Heading("Next open: stretched by Windows to 150%".into()));
        link.close();
    }
}

#[cfg(test)]
#[path = "../../rt/src/testing.rs"]
#[allow(dead_code)]
mod allocation_probe;

#[cfg(test)]
#[global_allocator]
static TEST_ALLOC: allocation_probe::CountingAlloc = allocation_probe::CountingAlloc;

#[cfg(test)]
#[path = "../tests/common/mod.rs"]
mod fixture;
