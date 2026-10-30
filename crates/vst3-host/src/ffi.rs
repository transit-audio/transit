//! The FFI boundary: the only module of this crate allowed to use `unsafe`.
//!
//! It loads VST3 binaries (`libloading`), runs the platform module entry, calls the COM
//! interfaces of the factory, component, audio processor and edit controller (`vst3` raw
//! bindings), and implements the host-side COM objects (host context, parameter changes, event
//! lists, a memory stream). Everything it exposes to the rest of the crate is a safe API.
//!
//! Soundness rests on the VST3 contract (the plugin honours the COM vtable signatures and the
//! documented pointer lifetimes). A plugin that violates it can still crash the process: hosting
//! in-process can't prevent that. What this module guarantees is that *SoundCraft* never hands
//! the plugin dangling or mis-sized pointers and never reads plugin strings beyond their bounds.

use crate::{Vst3Error, scan};
use std::ffi::{c_char, c_int, c_void};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, TryLockError, mpsc};
use std::time::Duration;
use transit_dsp::Transport;
use vst3::Steinberg::Vst::{self as sv};
use vst3::Steinberg::Vst::{
    IAttributeListTrait, IAudioProcessorTrait, IComponentHandlerTrait, IComponentTrait, IConnectionPointTrait, IEditControllerTrait, IEventListTrait,
    IHostApplicationTrait, IMessageTrait, IMidiMappingTrait, IParamValueQueueTrait, IParameterChangesTrait, IUnitInfoTrait,
};
use vst3::Steinberg::{self as sb};
use vst3::Steinberg::{
    FUnknown, IBStreamTrait, IPlugFrameTrait, IPlugViewContentScaleSupportTrait, IPlugViewTrait, IPluginBaseTrait, IPluginFactory2Trait,
    IPluginFactoryTrait,
};
use vst3::com_scrape_types::{Construct, Header, MakeHeader, Wrapper};
use vst3::{Class, ComPtr, ComWrapper, Interface};

/// Most classes one factory may report (hostile-input cap).
const MAX_CLASSES: i32 = 1024;
/// Most parameters read from one controller.
pub(crate) const MAX_PARAMS: i32 = 4096;
/// Most audio buses per direction we provide buffers for.
const MAX_BUSES: i32 = 16;
/// Most channels per audio bus.
const MAX_BUS_CHANNELS: u32 = 64;
/// Capacity of the per-block event lists (preallocated: `process` never allocates).
pub(crate) const EVENT_CAP: usize = 1024;
/// Most parameter queues per block (preallocated).
pub(crate) const PARAM_QUEUE_CAP: usize = 1024;
/// Parameter points one block carries across all its queues (a preallocated slab).
pub(crate) const PARAM_POINTS: usize = 4096;
/// Event entries kept for releases (F10): one per note and pedal the mix holds open (224 + 32),
/// plus one reset. Other events are refused once only these remain.
pub(crate) const RELEASE_RESERVE: usize = 257;
/// MIDI channels the controller mapping covers.
pub(crate) const MIDI_CHANNELS: usize = 16;
// Largest plugin state we buffer (component or controller state).
use crate::preset::MAX_STATE_BYTES;
/// Most editor parameter edits buffered between two UI frames.
const EDIT_CAP: usize = 256;
// Magic of SoundCraft's state container: SCV3 + u32 LE component length + component state +
// u32 LE controller length + controller state (crate::preset::join_state).
#[cfg(test)]
use crate::preset::STATE_MAGIC;

// ---- strings -------------------------------------------------------------------------------

/// Reads a fixed-size `char8[N]` buffer, stopping at the first NUL or the end of the array.
fn read_c8(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Reads a fixed-size UTF-16 `String128`, stopping at the first NUL or the end of the array.
fn read_c16(buf: &[u16]) -> String {
    let units: Vec<u16> = buf.iter().take_while(|c| **c != 0).copied().collect();
    String::from_utf16_lossy(&units).trim().to_string()
}

/// Writes `s` into a UTF-16 buffer, truncated and always NUL-terminated.
fn write_c16(s: &str, dst: &mut [u16]) {
    let Some(room) = dst.len().checked_sub(1) else { return };
    let mut n = 0;
    for (d, u) in dst.iter_mut().zip(s.encode_utf16().take(room)) {
        *d = u;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

fn ok(r: sb::tresult) -> bool {
    r == sb::kResultOk
}

/// VST3's numbers for the MIDI controllers that aren't CCs.
pub(crate) const AFTERTOUCH: usize = sv::ControllerNumbers_::kAfterTouch as usize;
pub(crate) const PITCH_BEND: usize = sv::ControllerNumbers_::kPitchBend as usize;

mod events;
mod host_context;
mod host_objects;
mod instance;
mod module;
#[cfg(target_os = "linux")]
mod run_loop;
mod view;

pub(crate) use events::*;
pub(crate) use host_context::*;
pub(crate) use host_objects::*;
pub(crate) use instance::*;
pub(crate) use module::*;
pub(crate) use view::*;

/// The platform host window for plugin views.
#[cfg(target_os = "macos")]
mod window {
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSFloatingWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::ffi::{c_char, c_void};

    pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeNSView);

    pub fn on_main_thread() -> bool {
        MainThreadMarker::new().is_some()
    }

    /// An `NSWindow` (titled, closable, floating above the app's windows) whose content view is
    /// the plugin view's parent. Holds one retain of the window; main thread only.
    pub struct HostWindow {
        window: *mut NSWindow,
    }

    // SAFETY: the pointer is only dereferenced on the main thread (every method checks
    // `MainThreadMarker`); the struct merely carries the retain between threads' locks.
    unsafe impl Send for HostWindow {}

    impl HostWindow {
        fn get(&self) -> Option<&NSWindow> {
            MainThreadMarker::new()?;
            // SAFETY: we hold a retain of this window (from `new`) until `close`.
            unsafe { self.window.as_ref() }
        }

        /// _header: no strip on this platform (the plug-in's view fills the window).
        pub fn new(
            title: &str,
            w: f64,
            h: f64,
            _header: Option<std::sync::Arc<transit_rt::editor_window::HeaderCell>>,
        ) -> Result<HostWindow, String> {
            let mtm = MainThreadMarker::new().ok_or_else(|| "not on the main thread".to_string())?;
            let _app = NSApplication::sharedApplication(mtm);
            let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(w, h));
            // Plugin editors are transient windows. In particular they must not be
            // minimizable: minimized NSWindows become persistent thumbnails in the macOS Dock,
            // which made every editor visited in a session look like another running app.
            let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
            // SAFETY: a freshly allocated window on the main thread; valid rect and style.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false)
            };
            // SAFETY: we keep our own retain and release it in `close`, so AppKit must not
            // release the window when the user closes it.
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str(title));
            window.setLevel(NSFloatingWindowLevel);
            window.setHidesOnDeactivate(true);
            window.setExcludedFromWindowsMenu(true);
            window.setCollectionBehavior(NSWindowCollectionBehavior::Transient | NSWindowCollectionBehavior::IgnoresCycle);
            window.center();
            Ok(HostWindow { window: Retained::into_raw(window) })
        }

        /// The raw window pointer (for the plug frame's resizes).
        pub fn raw(&self) -> *mut c_void {
            self.window.cast()
        }

        /// The content view (`NSView*`) the plugin view attaches to; null when unavailable.
        pub fn parent(&self) -> *mut c_void {
            self.get().and_then(|w| w.contentView()).map_or(std::ptr::null_mut(), |v| Retained::as_ptr(&v).cast_mut().cast())
        }

        pub fn raise(&self) {
            if let Some(w) = self.get() {
                w.makeKeyAndOrderFront(None);
            }
        }

        pub fn is_visible(&self) -> bool {
            self.get().is_some_and(|w| w.isVisible())
        }

        /// Nothing to keep in step: no strip here.
        pub fn sync(&self) {}

        pub fn close(self) {
            if let Some(w) = self.get() {
                w.orderOut(None);
                w.close();
            }
            if MainThreadMarker::new().is_some() {
                // SAFETY: balances the retain taken in `new`; nothing uses the pointer after this.
                drop(unsafe { Retained::from_raw(self.window) });
            }
        }
    }

    /// Resizes a host window's content (`window` from [`HostWindow::raw`]); main thread only.
    /// Returns whether it resized.
    pub fn resize(window: *mut c_void, w: f64, h: f64) -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        // SAFETY: null or a window the editor link keeps retained while the frame is attached.
        let Some(win) = (unsafe { window.cast::<NSWindow>().as_ref() }) else { return false };
        win.setContentSize(NSSize::new(w, h));
        true
    }

    /// No call on macOS: views work in points and read the backing scale themselves (the SDK
    /// sends `setContentScaleFactor` only where they cannot, issue #101).
    pub fn content_scale(_win: &HostWindow) -> Option<f64> {
        None
    }

    pub fn scale_change(_win: &HostWindow) -> Option<f64> {
        None
    }

    pub fn content_size(_win: &HostWindow) -> (f64, f64) {
        (0.0, 0.0)
    }
}

/// The Windows and Linux host window: `transit_rt`'s editor window, a top-level window on the
/// plug-in host thread. On Windows (issue #68) its client area is a header strip above the plugin
/// view's `HWND` parent and nothing else; on Linux (issue #147) it is an X11 window whose content
/// window's id is the view's `X11EmbedWindowID` parent (no strip yet). Closing it only hides it;
/// the editor link notices in `idle`, detaches the view, then destroys the window.
#[cfg(any(windows, target_os = "linux"))]
mod window {
    use std::ffi::c_char;

    pub use transit_rt::editor_window::{EditorWindow as HostWindow, resize_content as resize};

    #[cfg(windows)]
    pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeHWND);
    #[cfg(target_os = "linux")]
    pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeX11EmbedWindowID);

    /// The window's DPI scale: a VST3 view cannot read it itself on Windows (issue #101), nor
    /// reliably on X11 (`Xft.dpi`, issue #147).
    pub fn content_scale(win: &HostWindow) -> Option<f64> {
        Some(win.scale())
    }

    /// The new scale after a DPI change, once.
    pub fn scale_change(win: &HostWindow) -> Option<f64> {
        win.take_scale_change()
    }

    /// The plug-in content's size.
    pub fn content_size(win: &HostWindow) -> (f64, f64) {
        let (w, h) = win.content_size();
        (f64::from(w), f64::from(h))
    }

    /// Windows and X11 permit GUI creation on any thread; the editor link checks its creating
    /// thread.
    pub fn on_main_thread() -> bool {
        true
    }
}
/// Other platforms: no editor windows yet (opening reports "unsupported").
#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod window {
    use std::ffi::{c_char, c_void};

    pub const PLATFORM_TYPE: Option<*const c_char> = None;

    pub fn on_main_thread() -> bool {
        true
    }

    pub struct HostWindow;

    impl HostWindow {
        pub fn new(
            _title: &str,
            _w: f64,
            _h: f64,
            _header: Option<std::sync::Arc<transit_rt::editor_window::HeaderCell>>,
        ) -> Result<HostWindow, String> {
            Err("VST3 editors are not hosted on this platform yet".into())
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

    pub fn resize(_window: *mut c_void, _w: f64, _h: f64) -> bool {
        false
    }

    pub fn content_scale(_win: &HostWindow) -> Option<f64> {
        None
    }

    pub fn scale_change(_win: &HostWindow) -> Option<f64> {
        None
    }

    pub fn content_size(_win: &HostWindow) -> (f64, f64) {
        (0.0, 0.0)
    }
}

#[cfg(test)]
#[path = "host_object_tests.rs"]
mod host_object_tests;

/// The deepest unit path kept; a deeper (or cyclic) chain stops here.
const UNIT_DEPTH_MAX: usize = 64;

/// Each unit's path from the root, from (id, parent id, name) rows: names joined by `/`, the root
/// (id 0) and unnamed units left out, a unit with no names omitted. A walk stops at an unknown
/// parent, at a unit it has already visited (a cycle in a broken table) and at `UNIT_DEPTH_MAX`.
fn unit_paths_from(raw: &[(i32, i32, String)]) -> std::collections::HashMap<i32, String> {
    use std::collections::{HashMap, HashSet};
    let mut by_id: HashMap<i32, (i32, &str)> = HashMap::with_capacity(raw.len());
    for (id, parent, name) in raw {
        by_id.entry(*id).or_insert((*parent, name.as_str()));
    }
    let mut out = HashMap::new();
    for (id, ..) in raw {
        let (mut names, mut cur) = (Vec::new(), *id);
        let mut seen = HashSet::new();
        while names.len() < UNIT_DEPTH_MAX && cur != 0 && seen.insert(cur) {
            let Some(&(parent, name)) = by_id.get(&cur) else { break };
            if !name.is_empty() {
                names.push(name);
            }
            cur = parent;
        }
        names.reverse();
        if !names.is_empty() {
            out.insert(*id, names.join("/"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    fn units(rows: &[(i32, i32, &str)]) -> Vec<(i32, i32, String)> {
        rows.iter().map(|&(i, p, n)| (i, p, n.to_string())).collect()
    }

    #[test]
    fn unit_paths_nest_from_the_root_and_stop_at_unknown_parents() {
        let m = unit_paths_from(&units(&[(0, -1, "Root"), (1, 0, "Osc"), (2, 1, "A"), (3, 99, "Lost"), (4, 1, "")]));
        assert_eq!(m.get(&1).map(String::as_str), Some("Osc"));
        assert_eq!(m.get(&2).map(String::as_str), Some("Osc/A"));
        assert_eq!(m.get(&3).map(String::as_str), Some("Lost"));
        assert_eq!(m.get(&4).map(String::as_str), Some("Osc"));
        assert!(!m.contains_key(&0));
    }

    #[test]
    fn a_cyclic_unit_table_terminates_fast_with_bounded_paths() {
        let n = 4096;
        let mut rows: Vec<(i32, i32, String)> = (1..=n).map(|i| (i, if i == n { 1 } else { i + 1 }, format!("U{i}"))).collect();
        rows.push((n + 1, n + 1, "Self".to_string()));
        let t = std::time::Instant::now();
        let m = unit_paths_from(&rows);
        let took = t.elapsed();
        eprintln!("cyclic unit table of {} units: {took:?}", rows.len());
        assert!(took < std::time::Duration::from_secs(1), "{took:?}");
        assert!(m.values().all(|p| p.split('/').count() <= UNIT_DEPTH_MAX));
        assert_eq!(m.get(&(n + 1)).map(String::as_str), Some("Self"), "a self-loop visits its unit once");
    }

    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn editor_thread_is_its_creator_not_a_name() {
        let link = Arc::new(EditorLink::default());
        assert!(link.on_main_thread(), "a host thread need not be named main");
        assert!(!std::thread::spawn(move || link.on_main_thread()).join().unwrap(), "another thread cannot drive this editor");
    }

    pub(super) fn gain_instance() -> Instance {
        crate::set_scan_extra_dirs_only(true);
        let descriptors = crate::scan_paths(&[fixture::unregistered_fixture_dir().to_path_buf()]);
        let d = descriptors.iter().find(|d| d.name == "Fixture Gain").unwrap();
        Bundle::load(&d.path).unwrap().instantiate(&crate::parse_id(&d.id).unwrap()).unwrap()
    }

    #[test]
    fn processing_failures_do_not_allocate() {
        let mut inst = gain_instance();
        let mut inputs = BusBuffers::new(&[2], 64);
        let mut outputs = BusBuffers::new(&[2], 64);
        for case in 0..4 {
            if case == 1 {
                inst.configure(48_000.0, 64, false).unwrap();
            }
            let processor = if case == 2 { inst.processor.take() } else { None };
            if case == 3 {
                assert!(inst.load_state(&(-3.0f64).to_le_bytes()));
            }
            let frames = if case == 1 { 65 } else { 64 };
            let (failed, stats) = allocation_probe::measure(|| {
                let result = inst.process(frames, &mut inputs, &mut outputs, &mut [], &[], 0, 0, None);
                result.is_err()
            });
            if case == 2 {
                inst.processor = processor;
            }
            assert!(failed, "case {case}");
            assert!(stats.is_zero(), "case {case}: {stats:?}");
        }
    }

    #[test]
    fn controller_state_rejection_is_reported() {
        let inst = gain_instance();
        assert!(!inst.load_state(&(-1.0f64).to_le_bytes()), "setComponentState refused");
        let mut blob = STATE_MAGIC.to_vec();
        blob.extend_from_slice(&8u32.to_le_bytes());
        blob.extend_from_slice(&0.5f64.to_le_bytes());
        blob.extend_from_slice(&8u32.to_le_bytes());
        blob.extend_from_slice(&(-2.0f64).to_le_bytes());
        assert!(!inst.load_state(&blob), "setState refused");
    }

    /// `IHostApplication::createInstance` as a plug-in calls it (class id = interface id).
    fn create_host_object(host: &ComWrapper<HostContext>, cid: [u8; 16], iid: [u8; 16]) -> (sb::tresult, *mut c_void) {
        let (mut cid, mut iid) = (cid.map(|b| b as c_char), iid.map(|b| b as c_char));
        let mut obj = std::ptr::dangling_mut::<c_void>();
        // SAFETY: valid TUIDs and out-pointer.
        let r = unsafe { host.createInstance(&mut cid, &mut iid, &mut obj) };
        (r, obj)
    }

    #[test]
    fn host_creates_messages_and_attribute_lists() {
        let host = ComWrapper::new(HostContext::default());
        let (r, obj) = create_host_object(&host, sv::IMessage::IID, sv::IMessage::IID);
        assert_eq!(r, sb::kResultOk);
        // SAFETY: an IMessage with one reference owned here.
        let msg = unsafe { ComPtr::from_raw(obj.cast::<sv::IMessage>()) }.expect("message");
        // SAFETY: the owned message keeps its attribute list alive; IDs/text are terminated or
        // null (rejected), and buffers have the reported sizes. Outputs are distinct and writable;
        // successful binary output is read before any replacement or release.
        unsafe {
            assert!(msg.getMessageID().is_null());
            msg.setMessageID(c"Hello".as_ptr());
            assert_eq!(std::ffi::CStr::from_ptr(msg.getMessageID()), c"Hello");
            let attrs = vst3::ComRef::from_raw(msg.getAttributes()).expect("attributes");
            let text: Vec<u16> = "abcdef".encode_utf16().chain(Some(0)).collect();
            assert_eq!(attrs.setString(c"s".as_ptr(), text.as_ptr()), sb::kResultOk);
            let mut out = [0xffffu16; 4];
            assert_eq!(attrs.getString(c"s".as_ptr(), out.as_mut_ptr(), 8), sb::kResultOk);
            assert_eq!(out, [u16::from(b'a'), u16::from(b'b'), u16::from(b'c'), 0], "truncated and terminated");
            assert_eq!(attrs.setBinary(c"b".as_ptr(), [1u8, 2, 3].as_ptr().cast(), 3), sb::kResultOk);
            let (mut p, mut n) = (std::ptr::null(), 0u32);
            assert_eq!(attrs.getBinary(c"b".as_ptr(), &mut p, &mut n), sb::kResultOk);
            assert_eq!(std::slice::from_raw_parts(p.cast::<u8>(), n as usize), [1, 2, 3]);
            let mut i = 0i64;
            assert_eq!(attrs.getInt(c"b".as_ptr(), &mut i), sb::kResultFalse, "wrong type");
            assert_eq!(attrs.getInt(std::ptr::null(), &mut i), sb::kInvalidArgument, "null id is invalid");
        }
        let (r, obj) = create_host_object(&host, sv::IAttributeList::IID, FUnknown::IID);
        assert_eq!(r, sb::kResultOk);
        // SAFETY: an FUnknown with one reference owned here.
        let list = unsafe { ComPtr::from_raw(obj.cast::<FUnknown>()) }.and_then(|u| u.cast::<sv::IAttributeList>()).expect("list");
        // SAFETY: valid list.
        unsafe {
            assert_eq!(list.setFloat(c"f".as_ptr(), 0.5), sb::kResultOk);
            let mut f = 0.0;
            assert_eq!(list.getFloat(c"f".as_ptr(), &mut f), sb::kResultOk);
            assert_eq!(f, 0.5);
        }
        // Unknown classes and interfaces the class lacks: refused, with a null object.
        for (cid, iid) in [(sv::IComponentHandler::IID, sv::IComponentHandler::IID), (sv::IMessage::IID, sv::IAttributeList::IID)] {
            assert_eq!(create_host_object(&host, cid, iid), (sb::kResultFalse, std::ptr::null_mut()));
        }
    }

    #[test]
    fn silence_flags_set_one_bit_per_channel() {
        assert_eq!(silence_flags(0), 0);
        assert_eq!(silence_flags(1), 0b1);
        assert_eq!(silence_flags(2), 0b11);
        assert_eq!(silence_flags(6), 0b11_1111);
        assert_eq!(silence_flags(64), u64::MAX);
        let mut b = BusBuffers::new(&[2, 6], 4);
        b.set_silent(true);
        b.refresh();
        assert_eq!(b.bufs.iter().map(|x| x.silenceFlags).collect::<Vec<_>>(), vec![0b11, 0b11_1111]);
        b.set_silent(false);
        b.refresh();
        assert!(b.bufs.iter().all(|x| x.silenceFlags == 0));
    }

    #[test]
    #[allow(clippy::unnecessary_cast)] // SDK enum signedness differs by target.
    fn the_transport_fills_the_process_context() {
        use sv::ProcessContext_::StatesAndFlags_::*;
        // SAFETY: `ProcessContext` is plain data (numbers and small structs); all-zero is valid.
        let mut c: sv::ProcessContext = unsafe { std::mem::zeroed() };
        let t = Transport {
            playing: true,
            tempo: 90.0,
            numerator: 3,
            denominator: 4,
            samples: 96_000,
            quarters: 3.5,
            bar_start: 3.0,
            ..Transport::default()
        };
        apply_transport(&mut c, &t);
        let playing = kPlaying as u32;
        assert_eq!(c.state & playing, playing);
        assert_eq!(c.state & kRecording as u32, 0);
        assert_eq!((c.tempo, c.timeSigNumerator, c.timeSigDenominator), (90.0, 3, 4));
        assert_eq!((c.projectTimeSamples, c.projectTimeMusic, c.barPositionMusic), (96_000, 3.5, 3.0));
        apply_transport(&mut c, &Transport { playing: false, ..t });
        assert_eq!(c.state & playing, 0);
    }

    #[test]
    fn state_container_parses_strictly() {
        let mut blob = b"SCV3".to_vec();
        blob.extend_from_slice(&3u32.to_le_bytes());
        blob.extend_from_slice(b"abc");
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(b"xy");
        assert_eq!(split_state(&blob), Some((&b"abc"[..], &b"xy"[..])));
        assert_eq!(split_state(b"raw component"), Some((&b"raw component"[..], &[][..])), "bare component state");
        assert_eq!(split_state(b""), None);
        let mut trailing = blob.clone();
        trailing.push(0);
        assert_eq!(split_state(&trailing), None);
        assert_eq!(split_state(&blob[..blob.len() - 1]), None);
        let mut huge = b"SCV3".to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(split_state(&huge), None);
        assert_eq!(split_state(b"SCV3"), None);
    }

    #[test]
    fn strings_are_bounded() {
        let mut b = [0u16; 4];
        write_c16("SoundCraft", &mut b);
        assert_eq!(read_c16(&b), "Sou");
        let full: Vec<u16> = "abcd".encode_utf16().collect();
        assert_eq!(read_c16(&full), "abcd");
        let c: Vec<c_char> = b"Fx|EQ\0junk".iter().map(|&x| x as c_char).collect();
        assert_eq!(read_c8(&c), "Fx|EQ");
        write_c16("x", &mut []);
    }

    #[test]
    fn parameter_points_keep_every_offset_in_order() {
        let pc = ParamChanges::new(8, 6);
        let pt = |time, id, class, seq, value| ParamPoint { time, id, class, seq, value, release: false };
        // Two parameters, several offsets; at offset 32 of parameter 1 a controller follows automation.
        let mut points =
            [pt(40, 1, 0, 4, 0.8), pt(0, 1, 1, 0, 1.0), pt(32, 1, 1, 3, 0.0), pt(5, 1, 0, 1, 0.2), pt(32, 1, 0, 2, 0.6), pt(9, 2, 0, 5, 0.3)];
        assert_eq!(pc.fill(&mut points, 0), 0);
        let read = |i: usize| {
            let q = &pc.queues[i];
            (q.id.load(Ordering::Relaxed), (0..q.points.load(Ordering::Relaxed)).map(|k| q.point(k).unwrap()).collect::<Vec<_>>())
        };
        assert_eq!(read(0), (1, vec![(0, 1.0), (5, 0.2), (32, 0.0), (40, 0.8)]));
        assert_eq!(read(1), (2, vec![(9, 0.3)]));
        // Exhaustion: four points for parameter 1 and one for 2 in a slab of 3 keep the last of 1.
        let small = ParamChanges::new(8, 3);
        let mut points = [pt(0, 1, 0, 0, 0.1), pt(1, 1, 0, 1, 0.2), pt(2, 1, 0, 2, 0.3), pt(3, 1, 0, 3, 0.4), pt(0, 2, 0, 4, 0.5)];
        assert_eq!(small.fill(&mut points, 0), 3);
        let q = &small.queues[0];
        assert_eq!((q.points.load(Ordering::Relaxed), q.point(0)), (1, Some((3, 0.4))));
        assert_eq!(small.queues[1].point(0), Some((0, 0.5)), "every parameter keeps a point");
    }

    #[test]
    #[allow(clippy::unnecessary_cast)] // SDK enum signedness differs by target.
    fn host_objects_behave() {
        let pc = ParamChanges::new(2, 16);
        let pt = |time, id, seq, value| ParamPoint { time, id, class: 0, seq, value, release: false };
        let mut points = [pt(2, 5, 0, 0.25), pt(2, 5, 1, 0.5), pt(2, 6, 2, 1.0), pt(2, 7, 3, 1.0)];
        assert_eq!(pc.fill(&mut points, 2), 1, "the third parameter finds no queue");
        // SAFETY: test calls through our own implementation.
        unsafe {
            assert_eq!(pc.getParameterCount(), 2, "full pool drops extra params");
            assert!(pc.getParameterData(2).is_null());
            assert!(pc.getParameterData(-1).is_null());
            let q = pc.queues[0].as_com_ref::<sv::IParamValueQueue>().unwrap();
            let (mut o, mut v) = (9, 0.0);
            assert_eq!(q.getPointCount(), 1, "one offset, the last value");
            assert_eq!(q.getPoint(0, &mut o, &mut v), sb::kResultOk);
            assert_eq!((o, v), (0, 0.5));
            assert_ne!(q.getPoint(1, &mut o, &mut v), sb::kResultOk);
            assert_ne!(q.getPoint(-1, &mut o, &mut v), sb::kResultOk);
            assert_ne!(q.getPoint(0, std::ptr::null_mut(), &mut v), sb::kResultOk);
        }
        let el = EventList::new(1);
        let mut e = NoteEvent::On { time: 5, key: 60, velocity: 1.0, channel: 3, id: 9 }.to_raw(2);
        // SAFETY: as above.
        unsafe {
            assert_eq!(el.addEvent(&mut e), sb::kResultOk);
            assert_ne!(el.addEvent(&mut e), sb::kResultOk, "bounded");
            assert_eq!(el.getEventCount(), 1);
            let mut out: sv::Event = std::mem::zeroed();
            assert_eq!(el.getEvent(0, &mut out), sb::kResultOk);
            assert_eq!(out.sampleOffset, 3);
            assert_eq!(out.__field0.noteOn.pitch, 60);
            assert_eq!((out.__field0.noteOn.channel, out.__field0.noteOn.noteId), (3, 9));
            assert_ne!(el.getEvent(1, &mut out), sb::kResultOk);
        }
        let s = MemoryStream { inner: Mutex::new((Vec::new(), 0)) };
        let mut buf = *b"hello";
        // SAFETY: as above; buffers are valid for the given sizes.
        unsafe {
            let mut n = 0;
            assert_eq!(s.write(buf.as_mut_ptr().cast(), 5, &mut n), sb::kResultOk);
            assert_eq!(n, 5);
            let mut pos = 0i64;
            assert_eq!(s.seek(1, sb::IBStream_::IStreamSeekMode_::kIBSeekSet as i32, &mut pos), sb::kResultOk);
            let mut out = [0u8; 8];
            assert_eq!(s.read(out.as_mut_ptr().cast(), 8, &mut n), sb::kResultOk);
            assert_eq!((n, &out[..4]), (4, &b"ello"[..]));
            assert_eq!(s.seek(-100, sb::IBStream_::IStreamSeekMode_::kIBSeekEnd as i32, &mut pos), sb::kResultOk);
            assert_eq!(pos, 0);
            assert_eq!(s.write(std::ptr::null_mut(), 5, &mut n), sb::kResultFalse);
        }
        let h = HostContext::default();
        let mut name: sv::String128 = [0; 128];
        // SAFETY: as above.
        unsafe {
            assert_eq!(h.getName(&mut name), sb::kResultOk);
            assert_eq!(h.restartComponent(sv::RestartFlags_::kLatencyChanged as i32), sb::kResultOk);
        }
        assert_eq!(read_c16(&name), "SoundCraft");
        assert!(h.latency_changed.load(Ordering::Relaxed));
        assert!(!h.titles_changed.load(Ordering::Relaxed) && !h.values_changed.load(Ordering::Relaxed), "latency alone leaves both");
        // SAFETY: as above.
        assert_eq!(unsafe { h.restartComponent(sv::RestartFlags_::kParamValuesChanged as i32) }, sb::kResultOk);
        assert!(h.values_changed.load(Ordering::Relaxed), "values flag recorded");
        assert!(!h.titles_changed.load(Ordering::Relaxed), "values alone leave the curves");
        // SAFETY: as above.
        assert_eq!(unsafe { h.restartComponent(sv::RestartFlags_::kParamTitlesChanged as i32) }, sb::kResultOk);
        assert!(h.titles_changed.load(Ordering::Relaxed), "titles re-sample the curves");
        assert_eq!(hex(&[0xAB; 16]), "AB".repeat(16));
    }
}

#[cfg(test)]
#[cfg(windows)]
mod view_tests;

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
