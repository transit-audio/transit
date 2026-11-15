//! GtkPlug-style fixture: set the map flag, never map ourselves, record the host's handshake.
//!
//! The plug also records the structure events the server reports for it and for the content
//! window (`StructureNotify` on both), so a readout can tell the host's own requests from
//! another client's. The host only maps, unmaps and configures the plug, always at (0, 0) to
//! the content's current size (it reads that size on its own connection just before), so in
//! the server's event order every configure of the plug carries the latest content size. A
//! window manager acting on the plug through a reused window id (E74) shows up as a reparent or
//! a configure the host did not ask for, and the readout prints ids, parent, geometry, events.

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, MapState, PropMode, Window, WindowClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// A structure event, as the fixture's connection received it.
#[derive(Clone, Copy)]
enum Seen {
    /// The plug configured; `content` is the content's size the server had reported before it.
    Configure {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
        content: (u16, u16),
    },
    Reparent {
        parent: Window,
        x: i16,
        y: i16,
    },
    Map,
    Unmap,
    /// The content window resized (only the host does that).
    Content {
        width: u16,
        height: u16,
    },
}

impl Seen {
    /// Whether a host request explains the event (see the module docs).
    fn hosted(self) -> bool {
        match self {
            Seen::Configure { x, y, width, height, content } => (x, y, (width, height)) == (0, 0, content),
            Seen::Reparent { .. } => false,
            Seen::Map | Seen::Unmap | Seen::Content { .. } => true,
        }
    }
}

/// Written out by hand: dead-code analysis ignores a derived `Debug`, and the failure report is
/// the only reader of most fields.
impl std::fmt::Debug for Seen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Seen::Configure { x, y, width, height, content: (cw, ch) } => {
                write!(f, "plug configured {width}x{height} at ({x}, {y}), content {cw}x{ch}")
            }
            Seen::Reparent { parent, x, y } => write!(f, "plug reparented to {parent:#x} at ({x}, {y})"),
            Seen::Map => write!(f, "plug mapped"),
            Seen::Unmap => write!(f, "plug unmapped"),
            Seen::Content { width, height } => write!(f, "content configured {width}x{height}"),
        }
    }
}

pub struct Plug {
    c: RustConnection,
    window: Window,
    parent: Window,
    info: u32,
    xembed: u32,
    messages: Vec<[u32; 5]>,
    seen: Vec<Seen>,
    content: (u16, u16),
}

impl Plug {
    pub fn new(parent: u64) -> Option<Self> {
        let parent = Window::try_from(parent).ok()?;
        let c = x11rb::connect(None).ok()?.0;
        // Select on the content before reading its size, so no resize falls between the two.
        // Event masks are per client: this does not change what the host receives.
        c.change_window_attributes(parent, &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY)).ok()?.check().ok()?;
        let g = c.get_geometry(parent).ok()?.reply().ok()?;
        let window = c.generate_id().ok()?;
        let aux = CreateWindowAux::new().event_mask(EventMask::STRUCTURE_NOTIFY);
        c.create_window(0, window, parent, 0, 0, 123, 45, 0, WindowClass::INPUT_OUTPUT, 0, &aux).ok()?.check().ok()?;
        let info = c.intern_atom(false, b"_XEMBED_INFO").ok()?.reply().ok()?.atom;
        let xembed = c.intern_atom(false, b"_XEMBED").ok()?.reply().ok()?.atom;
        c.change_property32(PropMode::REPLACE, window, info, info, &[0, 1]).ok()?.check().ok()?;
        Some(Self { c, window, parent, info, xembed, messages: Vec::new(), seen: Vec::new(), content: (g.width, g.height) })
    }

    /// The plug's state for a failure message: ids, actual parent, both geometries, events.
    fn report(&self, what: &str) {
        let tree = self.c.query_tree(self.window).ok().and_then(|r| r.reply().ok()).map(|r| r.parent);
        let child = self.c.get_geometry(self.window).ok().and_then(|r| r.reply().ok()).map(|g| (g.x, g.y, g.width, g.height));
        let parent = self.c.get_geometry(self.parent).ok().and_then(|r| r.reply().ok()).map(|g| (g.x, g.y, g.width, g.height));
        let base = self.c.setup().resource_id_base;
        eprintln!(
            "xembed fixture: {what}: plug {:#x} (connection id base {base:#x}), parent now {tree:#x?}, content {:#x}; \
             plug geometry {child:?}, content geometry {parent:?}; events in order {:?}; xembed messages {:?}",
            self.window, self.parent, self.seen, self.messages
        );
    }

    fn drain(&mut self) {
        self.c.sync().unwrap();
        while let Some(event) = self.c.poll_for_event().unwrap() {
            let seen = match event {
                Event::ClientMessage(e) if e.window == self.window && e.type_ == self.xembed && e.format == 32 => {
                    self.messages.push(e.data.as_data32());
                    continue;
                }
                Event::ConfigureNotify(e) if e.window == self.parent => {
                    self.content = (e.width, e.height);
                    Seen::Content { width: e.width, height: e.height }
                }
                Event::ConfigureNotify(e) if e.window == self.window => {
                    Seen::Configure { x: e.x, y: e.y, width: e.width, height: e.height, content: self.content }
                }
                Event::ReparentNotify(e) if e.window == self.window => Seen::Reparent { parent: e.parent, x: e.x, y: e.y },
                Event::MapNotify(e) if e.window == self.window => Seen::Map,
                Event::UnmapNotify(e) if e.window == self.window => Seen::Unmap,
                _ => continue,
            };
            self.seen.push(seen);
        }
    }

    pub fn command(&mut self, command: &str) -> Option<bool> {
        self.drain();
        Some(match command {
            // Only the host acted on the plug: it is still in the content window and every event
            // is one a host request explains. Read before the state readouts, so interference
            // fails here with its cause printed, not later as a wrong map state or size.
            "xembed-hosted" => {
                let parent = self.c.query_tree(self.window).unwrap().reply().unwrap().parent;
                let hosted = parent == self.parent && self.seen.iter().all(|s| s.hosted());
                if !hosted {
                    self.report("another client moved, resized or reparented the plug");
                }
                hosted
            }
            // Mapped by the host: VIEWABLE, or UNVIEWABLE while a window manager has not yet
            // mapped the editor's top-level (a compositor's XWM maps it asynchronously).
            "xembed-mapped" => self.c.get_window_attributes(self.window).unwrap().reply().unwrap().map_state != MapState::UNMAPPED,
            "xembed-unmapped" => self.c.get_window_attributes(self.window).unwrap().reply().unwrap().map_state == MapState::UNMAPPED,
            "xembed-notified" => self.messages.iter().filter(|m| m[1] == 0).copied().collect::<Vec<_>>() == vec![[0, 0, 0, self.parent, 0]],
            "xembed-sized" => {
                let child = self.c.get_geometry(self.window).unwrap().reply().unwrap();
                let parent = self.c.get_geometry(self.parent).unwrap().reply().unwrap();
                let sized = (child.x, child.y, child.width, child.height) == (0, 0, parent.width, parent.height);
                if !sized {
                    self.report("the plug does not fill the content window");
                }
                sized
            }
            "xembed-unmap" => {
                self.c.change_property32(PropMode::REPLACE, self.window, self.info, self.info, &[0, 0]).unwrap().check().unwrap();
                true
            }
            _ => return None,
        })
    }

    pub fn close(self) {
        self.c.destroy_window(self.window).unwrap().check().unwrap();
    }
}
