//! `XEmbed` version 0 subset for toolkit editor windows. All calls use the owning host thread's
//! connection. See <https://specifications.freedesktop.org/xembed/latest-single/> sections 4–6.
//! No accelerator, tab traversal, modality, or drag-and-drop protocol is implemented here.

use super::*;
use std::time::Duration;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::xproto::{KeyPressEvent, MapState};

const EMBEDDED_NOTIFY: u32 = 0;
const WINDOW_ACTIVATE: u32 = 1;
const WINDOW_DEACTIVATE: u32 = 2;
const REQUEST_FOCUS: u32 = 3;
const FOCUS_IN: u32 = 4;
const FOCUS_OUT: u32 = 5;

#[derive(Clone, Copy)]
struct PendingMap {
    dispatch: u64,
    since: Instant,
    retried: bool,
}

impl PendingMap {
    fn due(self, completed: u64, last_completion: Option<Instant>, now: Instant) -> bool {
        // Exclude the dispatch that detected the child. The next full dispatch lets
        // toolkit fd/timer handlers flush their own MapWindow before idle claims it.
        // Restart the 100 ms fallback whenever a dispatch completes after detection.
        let since = self.since.max(last_completion.unwrap_or(self.since));
        completed.wrapping_sub(self.dispatch) >= 2 || now.saturating_duration_since(since) >= Duration::from_millis(100)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Plug {
    pub window: Window,
    pub version: u32,
    pub flags: u32,
    pub mapped_by_host: bool,
    pub notify_sent: bool,
    pending_map: Option<PendingMap>,
}

impl Plug {
    fn finish_map(&mut self, result: Result<bool, ReplyError>) {
        let pending = self.pending_map.take();
        match result {
            Ok(mapped) => self.mapped_by_host |= mapped,
            Err(ReplyError::X11Error(e)) if e.error_kind == ErrorKind::Window => {}
            Err(e) => {
                if let Some(mut pending) = pending.filter(|p| !p.retried) {
                    pending.retried = true;
                    self.pending_map = Some(pending);
                } else {
                    log::debug!("editor windows: XEmbed map failed after one retry for window {}: {e}", self.window);
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct State {
    pub plugs: Vec<Plug>,
    active: bool,
    focused: Option<Window>,
}

fn send(c: &Conn, window: Window, message: u32, data1: u32) -> bool {
    // Version 0, FOCUS_CURRENT and unused fields are all zero. CurrentTime is permitted when
    // there is no event timestamp (section 6.3). No propagation, no event mask (section 5).
    let event = ClientMessageEvent::new(32, window, c.atoms._XEMBED, [x11rb::CURRENT_TIME, message, 0, data1, 0]);
    c.c.send_event(false, window, EventMask::NO_EVENT, event).is_ok_and(|cookie| cookie.check().is_ok())
}

fn info(c: &Conn, window: Window) -> Option<(u32, u32)> {
    let atom = c.atoms._XEMBED_INFO;
    let reply = c.c.get_property(false, window, atom, atom, 0, 2).ok()?.reply().ok()?;
    if reply.type_ != atom || reply.format != 32 || reply.value_len != 2 || reply.bytes_after != 0 {
        return None;
    }
    let mut values = reply.value32()?;
    Some((values.next()?, values.next()?))
}

struct ServerGrab<'a>(&'a Conn);

impl Drop for ServerGrab<'_> {
    fn drop(&mut self) {
        let _ = self.0.c.ungrab_server();
        let _ = self.0.c.flush();
    }
}

fn map_unmapped(c: &Conn, window: Window) -> Result<bool, ReplyError> {
    // MapWindow succeeds even when another client already mapped the window. Serialize
    // this final read and map so that only a map we actually performed opts into XEmbed.
    // Keep the server grab limited to these requests, on the owning host thread.
    // Arm before issuing the request: even a panic while checking the grab must release it.
    let _grab = ServerGrab(c);
    c.c.grab_server()?.check()?;
    if c.c.get_window_attributes(window)?.reply()?.map_state != MapState::UNMAPPED {
        return Ok(false);
    }
    c.c.map_window(window)?.check()?;
    Ok(true)
}

pub(super) fn detect(c: &Conn, f: &Frame, window: Window) {
    update(c, f, window, false);
}

pub(super) fn idle(c: &Conn, f: &Frame, now: Instant) {
    let due: Vec<Window> = f
        .xembed
        .borrow()
        .plugs
        .iter()
        .filter(|p| p.pending_map.is_some_and(|pending| pending.due(c.dispatches.get(), c.last_completion.get(), now)))
        .map(|p| p.window)
        .collect();
    for window in due {
        // Revalidate parent, property and map state: events can still be queued or stale.
        update(c, f, window, true);
    }
}

fn update(c: &Conn, f: &Frame, window: Window, mapping_due: bool) {
    // Create/property/reparent events may already be stale. Never operate on a child that
    // has left this content window. Select before reading so a late property cannot be lost.
    if c.c.query_tree(window).ok().and_then(|r| r.reply().ok()).is_none_or(|r| r.parent != f.content) {
        forget(c, f, window);
        return;
    }
    let _ = c.c.change_window_attributes(window, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE));
    let Some((version, flags)) = info(c, window) else {
        forget(c, f, window);
        return;
    };
    let mut state = f.xembed.borrow_mut();
    let index = state.plugs.iter().position(|p| p.window == window).unwrap_or_else(|| {
        state.plugs.push(Plug { window, version, flags, mapped_by_host: false, notify_sent: false, pending_map: None });
        state.plugs.len() - 1
    });
    let plug = &mut state.plugs[index];
    let was_mapped_by_host = plug.mapped_by_host;
    plug.version = version;
    plug.flags = flags;
    if let Some(attrs) = c.c.get_window_attributes(window).ok().and_then(|r| r.reply().ok()) {
        if flags & 1 != 0 && attrs.map_state == MapState::UNMAPPED {
            // Even a previously managed child retries only on the next due idle pass.
            let map_now = mapping_due || (plug.mapped_by_host && plug.pending_map.is_none());
            plug.pending_map.get_or_insert_with(|| PendingMap { dispatch: c.dispatches.get(), since: Instant::now(), retried: false });
            if map_now {
                plug.finish_map(map_unmapped(c, window));
            }
        } else {
            plug.pending_map = None;
            if plug.mapped_by_host && flags & 1 == 0 && attrs.map_state != MapState::UNMAPPED {
                let _ = c.c.unmap_window(window);
            }
        }
    }
    let newly_notified = !plug.notify_sent && send(c, window, EMBEDDED_NOTIFY, f.content);
    plug.notify_sent |= newly_notified;
    let managed = plug.mapped_by_host;
    if managed && (newly_notified || !was_mapped_by_host) && state.active {
        send(c, window, WINDOW_ACTIVATE, 0);
    }
    let focus = managed && state.active && state.focused.is_none() && flags & 1 != 0;
    let unfocus = state.focused == Some(window) && flags & 1 == 0;
    drop(state);
    resize(c, f);
    if unfocus {
        focus_on(c, f, None);
    } else if focus {
        focus_on(c, f, Some(window));
        focus_proxy(c, f);
    }
    let _ = c.c.flush();
}

pub(super) fn property(c: &Conn, window: Window) {
    // A child without the property is not in State yet. Resolve its actual direct parent.
    if let Some(parent) = c.c.query_tree(window).ok().and_then(|r| r.reply().ok()).map(|r| r.parent)
        && let Some(f) = frame_of_content(parent)
    {
        detect(c, &f, window);
    }
}

pub(super) fn forget(c: &Conn, f: &Frame, window: Window) {
    if f.xembed.borrow().focused == Some(window) {
        focus_on(c, f, None);
    }
    f.xembed.borrow_mut().plugs.retain(|p| p.window != window);
}

pub(super) fn resize(c: &Conn, f: &Frame) {
    if !f.xembed.borrow().plugs.iter().any(|p| p.mapped_by_host) {
        return;
    }
    // Read the current content geometry, not a possibly old ConfigureNotify still in the
    // queue. This also covers a window manager forcing a size and the follow-size path.
    let Some(g) = c.c.get_geometry(f.content).ok().and_then(|r| r.reply().ok()) else { return };
    for plug in f.xembed.borrow().plugs.iter().filter(|p| p.mapped_by_host) {
        let _ = c.c.configure_window(plug.window, &ConfigureWindowAux::new().x(0).y(0).width(u32::from(g.width)).height(u32::from(g.height)));
    }
    let _ = c.c.flush();
}

fn focus_on(c: &Conn, f: &Frame, window: Option<Window>) {
    let mut state = f.xembed.borrow_mut();
    if state.focused == window {
        if let Some(window) = window {
            send(c, window, FOCUS_IN, 0);
        }
        return;
    }
    if let Some(old) = state.focused.take() {
        send(c, old, FOCUS_OUT, 0);
    }
    if let Some(window) = window {
        // Logical focus does not activate a window (section 5.2). Selecting ButtonPress on
        // the foreign child would steal the toolkit's selection; REQUEST_FOCUS handles clicks.
        if send(c, window, FOCUS_IN, 0) {
            state.focused = Some(window);
        }
    }
    let _ = c.c.flush();
}

fn focus_proxy(c: &Conn, f: &Frame) {
    if f.xembed.borrow().focused.is_none() {
        return;
    }
    // A queued FocusIn can already be stale. Avoid taking focus back when the server already
    // reports another window; a race between this query and SetInputFocus still needs
    // timestamped WM_TAKE_FOCUS support (K92).
    let Some(focus) = c.c.get_input_focus().ok().and_then(|r| r.reply().ok()).map(|r| r.focus) else { return };
    if focus == f.id || focus == f.content || f.child.get().is_some_and(|(child, _)| focus == child) {
        let _ = c.c.set_input_focus(InputFocus::PARENT, f.focus_proxy, x11rb::CURRENT_TIME);
    }
}

pub(super) fn activate(c: &Conn, f: &Frame, active: bool) -> bool {
    let mut state = f.xembed.borrow_mut();
    if state.active != active {
        state.active = active;
        for plug in state.plugs.iter().filter(|p| p.mapped_by_host) {
            send(c, plug.window, if active { WINDOW_ACTIVATE } else { WINDOW_DEACTIVATE }, 0);
        }
    }
    // Self-mapped toolkits (including those advertising XEmbed) keep direct X focus.
    let has_plug = state.plugs.iter().any(|p| p.mapped_by_host);
    let target = state.plugs.iter().find(|p| p.mapped_by_host && p.flags & 1 != 0).map(|p| p.window);
    let focused = state.focused;
    drop(state);
    if active {
        if focused.is_none() {
            focus_on(c, f, target);
        }
        if has_plug {
            focus_proxy(c, f);
        }
    }
    // Section 5.2: logical focus survives top-level deactivation. Only activation changes.
    let _ = c.c.flush();
    has_plug
}

pub(super) fn hide(c: &Conn, f: &Frame) {
    focus_on(c, f, None);
    activate(c, f, false);
}

pub(super) fn request_focus(c: &Conn, event: &ClientMessageEvent) {
    if event.format != 32 || event.data.as_data32()[1] != REQUEST_FOCUS {
        return;
    }
    if let Some(f) = frame_of_content(event.window) {
        // The protocol carries no sender ID. This editor has a single logical focus site.
        let target = f.xembed.borrow().plugs.iter().find(|p| p.mapped_by_host && p.flags & 1 != 0).map(|p| p.window);
        if target.is_some() {
            focus_on(c, &f, target);
        }
    }
}

pub(super) fn key(c: &Conn, mut event: KeyPressEvent) {
    if let Some(f) = frames().into_iter().find(|f| f.focus_proxy == event.event)
        && let Some(window) = f.xembed.borrow().focused
    {
        event.event = window;
        // Match GtkSocket: pointer coordinates and the source's child are not meaningful
        // in a key event forwarded to a different window.
        event.child = 0;
        event.root_x = 0;
        event.root_y = 0;
        event.event_x = 0;
        event.event_y = 0;
        let _ = c.c.send_event(false, window, EventMask::NO_EVENT, event);
        let _ = c.c.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_map_waits_for_a_full_dispatch_or_injected_fallback() {
        let since = Instant::now();
        let pending = PendingMap { dispatch: 10, since, retried: false };
        assert!(!pending.due(10, None, since));
        assert!(!pending.due(11, Some(since), since), "the detection dispatch alone is insufficient");
        assert!(pending.due(12, Some(since), since), "one full following dispatch, without advancing time");
        assert!(!pending.due(10, None, since + Duration::from_millis(99)));
        assert!(pending.due(10, None, since + Duration::from_millis(100)), "idle without dispatch progress has a bounded fallback");
        let wrapping = PendingMap { dispatch: u64::MAX, ..pending };
        assert!(!wrapping.due(0, Some(since), since));
        assert!(wrapping.due(1, Some(since), since));
    }

    #[test]
    fn pending_map_fallback_restarts_on_dispatch_completion() {
        let before_detection = Instant::now();
        let since = before_detection + Duration::from_millis(50);
        let pending = PendingMap { dispatch: 10, since, retried: false };
        let completion = since + Duration::from_millis(140);
        let now = since + Duration::from_millis(150);
        assert!(!pending.due(11, Some(completion), now));
        assert!(pending.due(11, None, now));
        assert!(pending.due(12, Some(completion), now));
        assert!(!pending.due(11, Some(completion), completion + Duration::from_millis(99)));
        assert!(pending.due(11, Some(completion), completion + Duration::from_millis(100)));
        assert!(!pending.due(10, Some(before_detection), since + Duration::from_millis(99)));
    }

    fn pending_plug(since: Instant) -> Plug {
        Plug {
            window: 1,
            version: 0,
            flags: 1,
            mapped_by_host: false,
            notify_sent: true,
            pending_map: Some(PendingMap { dispatch: 10, since, retried: false }),
        }
    }

    fn map_error(kind: ErrorKind) -> ReplyError {
        let error_code = match kind {
            ErrorKind::Window => x11rb::protocol::xproto::WINDOW_ERROR,
            ErrorKind::Match => x11rb::protocol::xproto::MATCH_ERROR,
            _ => panic!("unsupported test error"),
        };
        ReplyError::X11Error(x11rb::x11_utils::X11Error {
            error_kind: kind,
            error_code,
            sequence: 0,
            bad_value: 1,
            minor_opcode: 0,
            major_opcode: 8,
            extension_name: None,
            request_name: Some("MapWindow"),
        })
    }

    #[test]
    fn failed_map_keeps_one_retry_then_gives_up() {
        let since = Instant::now();
        let mut plug = pending_plug(since);
        plug.finish_map(Err(map_error(ErrorKind::Match)));
        let retry = plug.pending_map.expect("first failure stays pending");
        assert!(retry.retried);
        assert!(retry.due(12, Some(since), since), "retry is eligible on the next due pass");
        assert!(!plug.mapped_by_host);
        plug.finish_map(Err(map_error(ErrorKind::Match)));
        assert!(plug.pending_map.is_none());
        assert!(!plug.mapped_by_host);
    }

    #[test]
    fn retry_success_self_map_and_departure_clear_pending() {
        let since = Instant::now();
        for result in [Ok(true), Ok(false), Err(map_error(ErrorKind::Window))] {
            let host_mapped = matches!(result, Ok(true));
            let mut plug = pending_plug(since);
            plug.finish_map(Err(map_error(ErrorKind::Match)));
            plug.finish_map(result);
            assert!(plug.pending_map.is_none());
            assert_eq!(plug.mapped_by_host, host_mapped);
        }
        let mut departed = pending_plug(since);
        departed.finish_map(Err(map_error(ErrorKind::Window)));
        assert!(departed.pending_map.is_none(), "a departed child does not get a retry");
    }
}
