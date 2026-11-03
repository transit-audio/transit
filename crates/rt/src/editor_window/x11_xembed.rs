//! `XEmbed` version 0 subset for toolkit editor windows. All calls use the owning host thread's
//! connection. See <https://specifications.freedesktop.org/xembed/latest-single/> sections 4–6.
//! No accelerator, tab traversal, modality, or drag-and-drop protocol is implemented here.

use super::*;
use x11rb::protocol::xproto::{KeyPressEvent, MapState};

const EMBEDDED_NOTIFY: u32 = 0;
const WINDOW_ACTIVATE: u32 = 1;
const WINDOW_DEACTIVATE: u32 = 2;
const REQUEST_FOCUS: u32 = 3;
const FOCUS_IN: u32 = 4;
const FOCUS_OUT: u32 = 5;

#[derive(Clone, Copy)]
pub(super) struct Plug {
    pub window: Window,
    pub version: u32,
    pub flags: u32,
    pub mapped_by_host: bool,
    pub notify_sent: bool,
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

fn map_unmapped(c: &Conn, window: Window) -> bool {
    // MapWindow succeeds even when another client already mapped the window. Serialize
    // this final read and map so that only a map we actually performed opts into XEmbed.
    // Keep the server grab limited to these requests, on the owning host thread.
    if !c.c.grab_server().is_ok_and(|cookie| cookie.check().is_ok()) {
        return false;
    }
    let mapped =
        c.c.get_window_attributes(window)
            .ok()
            .and_then(|r| r.reply().ok())
            .is_some_and(|attrs| attrs.map_state == MapState::UNMAPPED && c.c.map_window(window).is_ok_and(|cookie| cookie.check().is_ok()));
    let _ = c.c.ungrab_server();
    let _ = c.c.flush();
    mapped
}

pub(super) fn detect(c: &Conn, f: &Frame, window: Window) {
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
        state.plugs.push(Plug { window, version, flags, mapped_by_host: false, notify_sent: false });
        state.plugs.len() - 1
    });
    let plug = &mut state.plugs[index];
    let was_mapped_by_host = plug.mapped_by_host;
    plug.version = version;
    plug.flags = flags;
    if let Some(attrs) = c.c.get_window_attributes(window).ok().and_then(|r| r.reply().ok()) {
        if flags & 1 != 0 && attrs.map_state == MapState::UNMAPPED {
            plug.mapped_by_host |= map_unmapped(c, window);
        } else if plug.mapped_by_host && flags & 1 == 0 && attrs.map_state != MapState::UNMAPPED {
            let _ = c.c.unmap_window(window);
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
