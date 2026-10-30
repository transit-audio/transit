//! The Linux editor window (issue #147, see the parent module): a top-level X11 window and the
//! content window the plug-in parents its view in, both made by the plug-in host thread on its
//! own X11 connection. Wayland sessions get it through `XWayland`: the plug-in sees X11 either way.
//!
//! Same contract as the Windows window (`win.rs`):
//! - The calling thread owns everything: each thread that makes editor windows opens its own
//!   connection (a thread-local), and every [`EditorWindow`] method, [`resize_content`],
//!   [`group`], [`raise_window`] and [`window_state`] act only on windows of the calling thread
//!   (they do nothing otherwise). The UI thread never makes an X11 call: it posts requests to the
//!   host thread's queue (ADR 0018, ADR 0030).
//! - The host thread's [`crate::gui_thread::MessageQueue`] waits on the connection
//!   ([`connection_fd`]) and dispatches its events ([`pump`]).
//! - Closing (the window manager's close button, `WM_DELETE_WINDOW`) only hides the window; the
//!   editor link notices it in `idle` ([`EditorWindow::is_visible`]), detaches the view, then
//!   calls [`EditorWindow::close`].
//! - The size is the plug-in's, in physical pixels; the window applies no scaling of its own.
//!   [`EditorWindow::scale`] is the X11 DPI over 96 (`Xft.dpi`, else the screen's size in
//!   millimeters, in steps of 0.25), passed to plug-ins that scale on request (issues #101, #105).
//! - No header strip yet (slice 2 of #147): the strip's commands stay in the device chain and the
//!   mixer. The [`HeaderCell`] still names the window.
//!
//! No `unsafe` here: `x11rb`'s pure-Rust connection speaks the protocol over the socket.

use super::{Follow, Group, HeaderCell, WindowState, client_size, note_group};
use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::os::fd::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::properties::WmSizeHints;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, EventMask, InputFocus,
    NotifyDetail, NotifyMode, PropMode, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        UTF8_STRING,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_STATE,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_USER_TIME,
        _NET_ACTIVE_WINDOW,
    }
}

/// `WM_CLASS`: the instance and the class, each NUL-terminated (the class groups the editors with
/// transit's own windows in docks and task switchers).
const WM_CLASS: &[u8] = b"plugin-editor\0transit\0";

/// A thread's X11 connection.
struct Conn {
    c: RustConnection,
    root: Window,
    black: u32,
    atoms: Atoms,
    /// The display's DPI ([`read_dpi`]), followed while windows are open.
    dpi: Cell<u32>,
}

/// An editor window's state, on its thread.
struct Frame {
    id: Window,
    content: Window,
    cell: Option<Arc<HeaderCell>>,
    /// The plug-in content's size in pixels (what the plug-in reported or its window measures).
    size: Cell<(i32, i32)>,
    /// When the host last resized the window (the size follow leaves the child that long).
    resized: Cell<Option<Instant>>,
    follow: Cell<(Option<(i32, i32)>, u32)>,
    /// The header generation last shown (the title).
    shown_gen: Cell<u64>,
    /// Open for the user: raised and not closed by them.
    shown: Cell<bool>,
    /// Hidden by transit with the main window (issue #173), not by the user.
    parked: Cell<bool>,
    /// Mapped on the screen (`MapNotify` seen, no `UnmapNotify` since).
    mapped: Cell<bool>,
    /// Unmapped by the window manager while shown: minimized.
    minimized: Cell<bool>,
    /// Unmaps transit asked for whose `UnmapNotify` has not arrived yet (they are not a minimize).
    own_unmaps: Cell<u32>,
    /// Transit asked for the window to be mapped (and has not unmapped it since).
    requested: Cell<bool>,
    /// The DPI changed since the editor host last took the scale.
    scale_changed: Cell<bool>,
    /// The plug-in's own top window in the content window, and its size.
    child: Cell<Option<(Window, (i32, i32))>>,
    /// The tool style (no taskbar entry) is applied.
    tool: Cell<bool>,
}

thread_local! {
    static CONN: RefCell<Option<Rc<Conn>>> = const { RefCell::new(None) };
    static FRAMES: RefCell<Vec<Rc<Frame>>> = const { RefCell::new(Vec::new()) };
    /// Frames made on this thread since the last [`take_created`].
    static CREATED: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static THREAD_ID: u32 = NEXT_THREAD.fetch_add(1, Ordering::Relaxed);
}

static NEXT_THREAD: AtomicU32 = AtomicU32::new(1);
/// Every live frame by the thread that made it, for [`hide_thread_windows`] from another thread.
static BY_THREAD: Mutex<Vec<(u32, Window)>> = Mutex::new(Vec::new());

fn current() -> Option<Rc<Conn>> {
    CONN.with(|c| c.borrow().clone())
}

/// The calling thread's connection, opened on first use.
fn conn() -> Result<Rc<Conn>, String> {
    if let Some(c) = current() {
        return Ok(c);
    }
    let (c, screen) = x11rb::connect(None).map_err(|e| {
        let display = std::env::var("DISPLAY").unwrap_or_default();
        format!("cannot open the X11 display (DISPLAY={display:?}): {e}")
    })?;
    let s = c.setup().roots.get(screen).ok_or_else(|| "the X11 display has no such screen".to_string())?;
    let (root, black) = (s.root, s.black_pixel);
    let fallback = mm_dpi(s.width_in_pixels, s.width_in_millimeters);
    let atoms = Atoms::new(&c)
        .map_err(|e| e.to_string())
        .and_then(|a| a.reply().map_err(|e| e.to_string()))
        .map_err(|e| format!("cannot set the X11 display up: {e}"))?;
    let dpi = read_dpi(&c, root).unwrap_or(fallback);
    // Follow `Xft.dpi` (the resource database on the root window) while windows are open.
    let _ = c.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE));
    let _ = c.flush();
    log::info!("editor windows: X11 display opened on this thread ({dpi} dpi)");
    let conn = Rc::new(Conn { c, root, black, atoms, dpi: Cell::new(dpi) });
    CONN.with(|c| *c.borrow_mut() = Some(Rc::clone(&conn)));
    Ok(conn)
}

/// The calling thread's X11 connection's descriptor, for the host thread's wait; `None` before
/// the thread made an editor window.
#[cfg_attr(feature = "force-fallback", allow(dead_code))]
pub(crate) fn connection_fd() -> Option<RawFd> {
    current().map(|c| c.c.stream().as_raw_fd())
}

/// `Xft.dpi` from the resource database (`RESOURCE_MANAGER` on the root window).
fn read_dpi(c: &RustConnection, root: Window) -> Option<u32> {
    let reply = c.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 20).ok()?.reply().ok()?;
    xft_dpi(&reply.value)
}

/// The `Xft.dpi` entry of an X resource database text (48 to 960; `None` without one).
fn xft_dpi(resources: &[u8]) -> Option<u32> {
    String::from_utf8_lossy(resources).lines().find_map(|line| {
        let value = line.strip_prefix("Xft.dpi:")?.trim();
        let dpi = value.parse::<f64>().ok()?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked range
        (48.0..=960.0).contains(&dpi).then(|| dpi.round() as u32)
    })
}

/// The DPI from the screen's size in millimeters, in scale steps of 0.25 from 1.0 to 4.0 (X
/// servers often report a nominal size; 96 when it reports none).
fn mm_dpi(px: u16, mm: u16) -> u32 {
    if mm == 0 {
        return 96;
    }
    let dpi = f64::from(px) * 25.4 / f64::from(mm);
    let scale = ((dpi / 96.0) * 4.0).round().clamp(4.0, 16.0) / 4.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 96..=384
    let dpi = (scale * 96.0).round() as u32;
    dpi
}

fn frame_of(id: Window) -> Option<Rc<Frame>> {
    FRAMES.with(|f| f.borrow().iter().find(|f| f.id == id).cloned())
}

fn frames() -> Vec<Rc<Frame>> {
    FRAMES.with(|f| f.borrow().clone())
}

fn px(v: i32) -> u16 {
    u16::try_from(v.clamp(1, i32::from(u16::MAX))).unwrap_or(1)
}

fn dim(v: i32) -> u32 {
    u32::try_from(v.max(1)).unwrap_or(1)
}

/// Resizes the frame so its content is `w`×`h` and fixes that size for the window manager (this
/// window has no user resizing; the plug-in sets its size).
fn set_content(c: &Conn, f: &Frame, w: i32, h: i32) {
    let (cw, ch) = client_size(w, h, 0);
    f.size.set((cw, ch));
    f.resized.set(Some(Instant::now()));
    let hints = WmSizeHints { min_size: Some((cw, ch)), max_size: Some((cw, ch)), ..WmSizeHints::default() };
    // The hints first, so a window manager that clamps to them takes the new size.
    let _ = hints.set_normal_hints(&c.c, f.id);
    let _ = c.c.configure_window(f.id, &ConfigureWindowAux::new().width(dim(cw)).height(dim(ch)));
    let _ = c.c.configure_window(f.content, &ConfigureWindowAux::new().x(0).y(0).width(dim(cw)).height(dim(ch)));
    let _ = c.c.flush();
}

fn set_title(c: &Conn, f: &Frame, title: &str) {
    let _ = c.c.change_property8(PropMode::REPLACE, f.id, AtomEnum::WM_NAME, c.atoms.UTF8_STRING, title.as_bytes());
    let _ = c.c.change_property8(PropMode::REPLACE, f.id, c.atoms._NET_WM_NAME, c.atoms.UTF8_STRING, title.as_bytes());
}

/// Gives the frame the tool style (`tool`: no taskbar or pager entry, issue #173) or takes it
/// away: a property while unmapped, a request to the window manager while mapped (EWMH).
fn restyle(c: &Conn, f: &Frame, tool: bool) -> bool {
    if f.tool.replace(tool) == tool {
        return false;
    }
    let states = [c.atoms._NET_WM_STATE_SKIP_TASKBAR, c.atoms._NET_WM_STATE_SKIP_PAGER];
    if f.mapped.get() {
        // _NET_WM_STATE_ADD (1) or _REMOVE (0), two properties, source: an application (1).
        let data = [u32::from(tool), states[0], states[1], 1, 0];
        let ev = ClientMessageEvent::new(32, f.id, c.atoms._NET_WM_STATE, data);
        let _ = c.c.send_event(false, c.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
    } else if tool {
        let _ = c.c.change_property32(PropMode::REPLACE, f.id, c.atoms._NET_WM_STATE, AtomEnum::ATOM, &states);
    } else {
        let _ = c.c.delete_property(f.id, c.atoms._NET_WM_STATE);
    }
    true
}

/// Maps the frame; `activate` also asks the window manager to focus it, otherwise it asks it not
/// to (`_NET_WM_USER_TIME` 0).
fn show(c: &Conn, f: &Frame, activate: bool) {
    f.minimized.set(false);
    f.requested.set(true);
    if !activate {
        let _ = c.c.change_property32(PropMode::REPLACE, f.id, c.atoms._NET_WM_USER_TIME, AtomEnum::CARDINAL, &[0]);
    }
    let _ = c.c.map_window(f.id);
    let _ = c.c.configure_window(f.id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    if activate {
        // Source: an application (1), no timestamp, no active window of ours.
        let ev = ClientMessageEvent::new(32, f.id, c.atoms._NET_ACTIVE_WINDOW, [1, 0, 0, 0, 0]);
        let _ = c.c.send_event(false, c.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev);
    }
    let _ = c.c.flush();
}

fn hide(c: &Conn, f: &Frame) {
    f.minimized.set(false);
    if f.requested.replace(false) {
        f.own_unmaps.set(f.own_unmaps.get().saturating_add(1));
    }
    let _ = c.c.unmap_window(f.id);
    let _ = c.c.flush();
}

/// A plug-in editor window (see the parent module). Owns the frame; destroyed by
/// [`EditorWindow::close`] on its thread.
pub struct EditorWindow {
    id: Window,
    thread: std::thread::ThreadId,
}

impl EditorWindow {
    /// Makes a hidden window for plug-in content of `w`×`h` pixels titled `title` (the header's
    /// title once it has one). Call [`EditorWindow::raise`] to show it once the plug-in attached.
    ///
    /// # Errors
    /// When the X11 display cannot be opened (no `DISPLAY`) or the windows cannot be created.
    #[allow(clippy::cast_possible_truncation)] // pixel sizes, clamped
    pub fn new(title: &str, w: f64, h: f64, header: Option<Arc<HeaderCell>>) -> Result<EditorWindow, String> {
        let c = conn()?;
        let fail = |e: &dyn std::fmt::Display| format!("cannot create the editor window: {e}");
        let id = c.c.generate_id().map_err(|e| fail(&e))?;
        let content = c.c.generate_id().map_err(|e| fail(&e))?;
        let (cw, ch) = client_size(w as i32, h as i32, 0);
        let frame_aux = CreateWindowAux::new().background_pixel(c.black).event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE);
        // A top-level window (the root is its parent) with no owner and no transient-for link
        // (ADR 0018 section 7), created unmapped.
        c.c.create_window(0, id, c.root, 0, 0, px(cw), px(ch), 0, WindowClass::INPUT_OUTPUT, 0, &frame_aux).map_err(|e| fail(&e))?;
        let content_aux = CreateWindowAux::new().background_pixel(c.black).event_mask(EventMask::SUBSTRUCTURE_NOTIFY);
        if let Err(e) = c.c.create_window(0, content, id, 0, 0, px(cw), px(ch), 0, WindowClass::INPUT_OUTPUT, 0, &content_aux) {
            let _ = c.c.destroy_window(id);
            return Err(fail(&e));
        }
        let _ = c.c.map_window(content);
        let atoms = &c.atoms;
        let _ = c.c.change_property32(PropMode::REPLACE, id, atoms.WM_PROTOCOLS, AtomEnum::ATOM, &[atoms.WM_DELETE_WINDOW]);
        let _ = c.c.change_property8(PropMode::REPLACE, id, AtomEnum::WM_CLASS, AtomEnum::STRING, WM_CLASS);
        let _ = c.c.change_property32(PropMode::REPLACE, id, atoms._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()]);
        let _ = c.c.change_property32(PropMode::REPLACE, id, atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM, &[atoms._NET_WM_WINDOW_TYPE_NORMAL]);
        let shown = header.as_ref().map(|h| h.view().title).filter(|t| !t.is_empty());
        let frame = Rc::new(Frame {
            id,
            content,
            cell: header,
            size: Cell::new((cw, ch)),
            resized: Cell::new(None),
            follow: Cell::new((None, 0)),
            shown_gen: Cell::new(u64::MAX),
            shown: Cell::new(false),
            parked: Cell::new(false),
            mapped: Cell::new(false),
            minimized: Cell::new(false),
            own_unmaps: Cell::new(0),
            requested: Cell::new(false),
            scale_changed: Cell::new(false),
            child: Cell::new(None),
            tool: Cell::new(false),
        });
        set_title(&c, &frame, shown.as_deref().unwrap_or(title));
        // Editors that follow the main window (issue #173) have no taskbar entry.
        restyle(&c, &frame, super::follows());
        set_content(&c, &frame, cw, ch);
        FRAMES.with(|v| v.borrow_mut().push(Rc::clone(&frame)));
        CREATED.with(|v| {
            let mut v = v.borrow_mut();
            if v.len() < 64 {
                v.push(id as usize);
            }
        });
        BY_THREAD.lock().unwrap_or_else(PoisonError::into_inner).push((current_thread_id(), id));
        let win = EditorWindow { id, thread: std::thread::current().id() };
        win.sync();
        Ok(win)
    }

    fn live(&self) -> Option<(Rc<Conn>, Rc<Frame>)> {
        if std::thread::current().id() != self.thread {
            return None;
        }
        Some((current()?, frame_of(self.id)?))
    }

    /// The frame's window id as a pointer-sized value (for the plug-in's resize requests,
    /// [`resize_content`]).
    #[must_use]
    pub fn raw(&self) -> *mut c_void {
        std::ptr::without_provenance_mut(self.id as usize)
    }

    /// The window the plug-in makes its window in (the content window's id as a pointer-sized
    /// value, as VST3's `kPlatformTypeX11EmbedWindowID` and CLAP's `x11` handle take it); null
    /// once the window is gone or on another thread.
    #[must_use]
    pub fn parent(&self) -> *mut c_void {
        self.live().map_or(std::ptr::null_mut(), |(_, f)| std::ptr::without_provenance_mut(f.content as usize))
    }

    /// Shows the window and brings it to the front. While the editors are parked with a minimized
    /// main window (issue #173) it is parked instead and comes back with the others.
    pub fn raise(&self) {
        let Some((c, f)) = self.live() else { return };
        f.shown.set(true);
        if super::follows() && super::parked() {
            f.parked.set(true);
            return;
        }
        f.parked.set(false);
        show(&c, &f, true);
    }

    /// The window's state ([`window_state`]); [`WindowState::Hidden`] once it is gone or on
    /// another thread.
    #[must_use]
    pub fn state(&self) -> WindowState {
        window_state(self.id as usize).unwrap_or(WindowState::Hidden)
    }

    /// The window's content scale: the display's DPI over 96 (1.0 once the window is gone or on
    /// another thread). What a plug-in that scales on request is told (issue #101).
    #[must_use]
    pub fn scale(&self) -> f64 {
        self.live().map_or(1.0, |(c, _)| f64::from(c.dpi.get()) / 96.0)
    }

    /// The new [`EditorWindow::scale`] when the display's DPI changed since the last call
    /// (`Xft.dpi` changed); `None` otherwise.
    #[must_use]
    pub fn take_scale_change(&self) -> Option<f64> {
        let (c, f) = self.live()?;
        f.scale_changed.replace(false).then(|| f64::from(c.dpi.get()) / 96.0)
    }

    /// The plug-in content's size in pixels; `(0, 0)` once the window is gone or on another
    /// thread.
    #[must_use]
    pub fn content_size(&self) -> (i32, i32) {
        self.live().map_or((0, 0), |(_, f)| f.size.get())
    }

    /// Whether the window is open for the user: shown (minimized included), or parked with the
    /// main window (issue #173). False once the user closed it (closing only hides it; the editor
    /// then tears it down).
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.live().is_some_and(|(_, f)| f.parked.get() || f.shown.get())
    }

    /// Housekeeping on the window's thread, from the editor's `idle`: shows the header's title
    /// and follows the size of the plug-in's own window ([`Follow`]).
    pub fn sync(&self) {
        let Some((c, f)) = self.live() else { return };
        if let Some(cell) = &f.cell {
            let generation = cell.generation();
            if generation != f.shown_gen.replace(generation) {
                let title = cell.view().title;
                if !title.is_empty() {
                    set_title(&c, &f, &title);
                    let _ = c.c.flush();
                }
            }
        }
        let child = f.child.get().map(|(_, size)| size);
        let (seen, count) = f.follow.get();
        let mut follow = Follow { seen, count };
        let to = follow.observe(child, f.size.get(), super::since(f.resized.get()));
        f.follow.set((follow.seen, follow.count));
        if let Some((w, h)) = to {
            log::debug!("editor window: the plug-in's view is {w}×{h}, not {:?}; the window follows it", f.size.get());
            set_content(&c, &f, w, h);
        }
    }

    /// Destroys the window (after the plug-in's view was detached from it).
    pub fn close(self) {
        let Some((c, f)) = self.live() else { return };
        let _ = c.c.destroy_window(f.id);
        let _ = c.c.flush();
        FRAMES.with(|v| v.borrow_mut().retain(|x| x.id != f.id));
        BY_THREAD.lock().unwrap_or_else(PoisonError::into_inner).retain(|&(_, id)| id != f.id);
    }
}

/// Resizes the plug-in content of the editor window `window` ([`EditorWindow::raw`]) to `w`×`h`.
/// Does nothing for a window that is not a live editor window of the calling thread. Returns
/// whether it resized (a plug-in's `resizeView` is honored only then, issue #105).
#[allow(clippy::cast_possible_truncation)] // pixel sizes, clamped
pub fn resize_content(window: *mut c_void, w: f64, h: f64) -> bool {
    let Ok(id) = Window::try_from(window.addr()) else { return false };
    let (Some(c), Some(f)) = (current(), frame_of(id)) else { return false };
    set_content(&c, &f, w as i32, h as i32);
    true
}

/// Handles the X11 events of the calling thread's editor windows that have arrived, at most
/// `max`, and returns how many. Runs no plug-in code. A connection that failed (the display went
/// away) is dropped with its windows.
#[cfg_attr(feature = "force-fallback", allow(dead_code))]
pub(crate) fn pump(max: usize) -> usize {
    let Some(c) = current() else { return 0 };
    let mut done = 0;
    while done < max {
        match c.c.poll_for_event() {
            Ok(Some(ev)) => {
                done += 1;
                handle(&c, ev);
            }
            Ok(None) => break,
            Err(e) => {
                log::warn!("editor windows: the X11 connection failed ({e}); this thread's editor windows are gone");
                let gone: Vec<Window> = frames().iter().map(|f| f.id).collect();
                BY_THREAD.lock().unwrap_or_else(PoisonError::into_inner).retain(|(_, id)| !gone.contains(id));
                FRAMES.with(|v| v.borrow_mut().clear());
                CONN.with(|v| *v.borrow_mut() = None);
                break;
            }
        }
    }
    done
}

fn handle(c: &Conn, ev: Event) {
    match ev {
        Event::ClientMessage(e) if e.format == 32 && e.type_ == c.atoms.WM_PROTOCOLS && e.data.as_data32()[0] == c.atoms.WM_DELETE_WINDOW => {
            // Closing only hides: the editor link notices in `idle` and tears the view down first.
            if let Some(f) = frame_of(e.window) {
                f.shown.set(false);
                f.parked.set(false);
                hide(c, &f);
            }
        }
        Event::MapNotify(e) => {
            if let Some(f) = frame_of(e.window) {
                f.mapped.set(true);
                f.minimized.set(false);
            }
        }
        Event::UnmapNotify(e) => {
            if let Some(f) = frame_of(e.window) {
                f.mapped.set(false);
                // Unmapped while open, not parked and not hidden by transit: the window manager
                // minimized it.
                let own = f.own_unmaps.get();
                if own > 0 {
                    f.own_unmaps.set(own - 1);
                } else {
                    f.minimized.set(f.shown.get() && !f.parked.get());
                }
            }
        }
        Event::ConfigureNotify(e) => {
            if let Some(f) = frame_of(e.window) {
                // A window manager that forces another size (a tiling one) gets the content laid
                // out in it; the plug-in's size is unchanged.
                let _ = c.c.configure_window(f.content, &ConfigureWindowAux::new().width(u32::from(e.width.max(1))).height(u32::from(e.height.max(1))));
            } else if let Some(f) = frame_of_content(e.event)
                && f.child.get().is_some_and(|(w, _)| w == e.window)
            {
                f.child.set(Some((e.window, (i32::from(e.width), i32::from(e.height)))));
            }
        }
        Event::CreateNotify(e) => {
            if let Some(f) = frame_of_content(e.parent) {
                f.child.set(Some((e.window, (i32::from(e.width), i32::from(e.height)))));
            }
        }
        Event::ReparentNotify(e) => {
            if let Some(f) = frame_of_content(e.event) {
                if e.parent == f.content {
                    // A plug-in that made its window elsewhere and moved it in: measure it once.
                    let size = c.c.get_geometry(e.window).ok().and_then(|g| g.reply().ok()).map(|g| (i32::from(g.width), i32::from(g.height)));
                    f.child.set(size.map(|s| (e.window, s)));
                } else if f.child.get().is_some_and(|(w, _)| w == e.window) {
                    f.child.set(None);
                }
            }
        }
        Event::DestroyNotify(e) => {
            if let Some(f) = frame_of_content(e.event)
                && f.child.get().is_some_and(|(w, _)| w == e.window)
            {
                f.child.set(None);
            }
        }
        Event::FocusIn(e) if e.mode == NotifyMode::NORMAL && e.detail != NotifyDetail::INFERIOR => {
            // The window manager focused the frame: the keyboard goes to the plug-in's window.
            if let Some(f) = frame_of(e.event)
                && let Some((child, _)) = f.child.get()
            {
                let _ = c.c.set_input_focus(InputFocus::PARENT, child, x11rb::CURRENT_TIME);
                let _ = c.c.flush();
            }
        }
        Event::PropertyNotify(e) if e.window == c.root && e.atom == u32::from(AtomEnum::RESOURCE_MANAGER) => {
            if let Some(dpi) = read_dpi(&c.c, c.root)
                && c.dpi.replace(dpi) != dpi
            {
                log::info!("editor windows: the display's DPI is now {dpi}");
                for f in frames() {
                    f.scale_changed.set(true);
                }
            }
        }
        Event::Error(e) => log::debug!("editor windows: X11 error {:?} on resource {}", e.error_kind, e.bad_value),
        _ => {}
    }
}

fn frame_of_content(content: Window) -> Option<Rc<Frame>> {
    FRAMES.with(|f| f.borrow().iter().find(|f| f.content == content).cloned())
}

/// Records `request` for the whole process ([`note_group`]) and applies it to every editor window
/// of the calling thread ([`apply`]): where the thread that records it owns the windows (inline
/// mode, the tests). Returns how many windows it changed.
#[must_use = "the count tells the tests what changed; `let _ =` where it does not matter"]
pub fn group(request: Group) -> usize {
    note_group(request);
    apply(request)
}

/// Applies `request` to every editor window of the calling thread without recording anything: what
/// the plug-in host thread runs from its queue, after the UI recorded the request when it posted
/// it. Each window is brought to the state the process is in now ([`follows`], [`parked`]), not
/// the one the request was posted for (the same as the Windows version). Park, Unpark and Front do
/// nothing while editors do not follow the main window. X11's means: unmapped and mapped again
/// (without focus), restacked, and the taskbar entry dropped through `_NET_WM_STATE`. Returns how
/// many windows it changed.
///
/// [`follows`]: crate::editor_window::follows
/// [`parked`]: crate::editor_window::parked
#[must_use = "the count tells the tests what changed; `let _ =` where it does not matter"]
pub fn apply(request: Group) -> usize {
    let Some(c) = current() else { return 0 };
    let follow = super::follows();
    let parked = super::parked();
    let mut changed = 0;
    let frames = frames();
    let request = match request {
        Group::Follow(_) => Group::Follow(follow),
        Group::Park if !parked => return 0,
        Group::Unpark if parked => return 0,
        r => r,
    };
    match request {
        Group::Park if follow => {
            for f in &frames {
                if f.shown.get() && !f.parked.get() {
                    f.parked.set(true);
                    hide(&c, f);
                    changed += 1;
                }
            }
        }
        Group::Unpark | Group::Follow(false) => {
            for f in &frames {
                if f.parked.replace(false) {
                    show(&c, f, false);
                    changed += 1;
                }
            }
            if request == Group::Follow(false) {
                for f in &frames {
                    restyle(&c, f, false);
                }
            }
        }
        Group::Front if follow => {
            for f in &frames {
                if f.shown.get() && !f.parked.get() && !f.minimized.get() {
                    let _ = c.c.configure_window(f.id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
                    changed += 1;
                }
            }
        }
        Group::Follow(true) => {
            for f in &frames {
                restyle(&c, f, true);
                changed += 1;
            }
        }
        Group::Park | Group::Front => {}
    }
    let _ = c.c.flush();
    changed
}

/// Brings the editor window `window` (an id from [`take_created`]) to the front without
/// activating it (issue #173). A parked window stays parked, and one the user closed stays
/// closed. False when `window` is not a live editor window of the calling thread.
#[must_use = "false: the window was not raised"]
pub fn raise_window(window: usize) -> bool {
    let Ok(id) = Window::try_from(window) else { return false };
    let (Some(c), Some(f)) = (current(), frame_of(id)) else { return false };
    if f.parked.get() || !f.shown.get() {
        return false;
    }
    show(&c, &f, false);
    true
}

/// The state of the editor window `window` (an id from [`take_created`]); `None` when it is not a
/// live editor window of the calling thread.
#[must_use]
pub fn window_state(window: usize) -> Option<WindowState> {
    let f = frame_of(Window::try_from(window).ok()?)?;
    Some(if f.parked.get() {
        WindowState::Parked
    } else if f.minimized.get() {
        WindowState::Minimized
    } else if f.shown.get() {
        WindowState::Shown
    } else {
        WindowState::Hidden
    })
}

/// The editor windows made on the calling thread since the last call (window ids as numbers, for
/// [`raise_window`] and [`window_state`]).
#[must_use]
pub fn take_created() -> Vec<usize> {
    CREATED.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// A number for the calling thread, unique in the process (never 0), for [`hide_thread_windows`]
/// from another thread.
#[must_use]
pub fn current_thread_id() -> u32 {
    THREAD_ID.with(|id| *id)
}

/// Asks every editor window of `thread` to hide, without waiting for that thread: issue #205's
/// "Give up" on a plug-in host thread stuck inside a plug-in. Returns how many windows were asked.
///
/// X11 lets any client unmap any window, so a short-lived helper thread opens its own connection
/// and unmaps them; neither the calling (UI) thread nor the stuck thread's connection is used, so
/// nothing waits for the stuck thread or makes an X11 call on the UI thread. The window manager
/// hides them at once, also while their thread stays stuck.
pub fn hide_thread_windows(thread: u32) -> usize {
    let windows: Vec<Window> = BY_THREAD.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|&&(t, _)| t == thread).map(|&(_, id)| id).collect();
    if windows.is_empty() {
        return 0;
    }
    let n = windows.len();
    let spawned = std::thread::Builder::new().name("editor-hide".into()).spawn(move || {
        let Ok((c, _)) = x11rb::connect(None) else { return };
        for id in windows {
            let _ = c.unmap_window(id);
        }
        let _ = c.flush();
        // A round trip, so the requests reach the server before the connection closes.
        let _ = c.get_input_focus().map(x11rb::cookie::Cookie::reply);
    });
    if spawned.is_err() { 0 } else { n }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dpi_comes_from_xft_dpi_or_the_screens_size() {
        assert_eq!(xft_dpi(b"Xft.antialias:\t1\nXft.dpi:\t144\nXft.hinting:\t1\n"), Some(144));
        assert_eq!(xft_dpi(b"Xft.dpi: 120.4"), Some(120));
        assert_eq!(xft_dpi(b"Xft.dpi:\tlots"), None);
        assert_eq!(xft_dpi(b"Xft.dpi:\t9000"), None, "out of range");
        assert_eq!(xft_dpi(b"Xcursor.size: 24"), None);
        // 1920 px over 508 mm is 96 dpi; 3840 px over the same is 192; a bogus tiny size clamps.
        assert_eq!(mm_dpi(1920, 508), 96);
        assert_eq!(mm_dpi(3840, 508), 192);
        assert_eq!(mm_dpi(2560, 508), 120, "1.33 rounds to 1.25");
        assert_eq!(mm_dpi(1920, 0), 96, "no size reported");
        assert_eq!(mm_dpi(1920, 2000), 96, "below 1.0 reads as 1.0");
        assert_eq!(mm_dpi(30_000, 10), 384, "at most 4.0");
    }

    #[test]
    fn a_thread_id_is_unique_and_never_zero() {
        let here = current_thread_id();
        let there = std::thread::spawn(current_thread_id).join().unwrap_or(here);
        assert_ne!(here, 0);
        assert_ne!(here, there);
        assert_eq!(hide_thread_windows(u32::MAX), 0, "no windows on an unknown thread");
    }
}
