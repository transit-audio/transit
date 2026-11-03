//! GtkPlug-style fixture: set the map flag, never map ourselves, record the host's handshake.

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, MapState, PropMode, Window, WindowClass};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub struct Plug {
    c: RustConnection,
    window: Window,
    parent: Window,
    info: u32,
    xembed: u32,
    messages: Vec<[u32; 5]>,
}

impl Plug {
    pub fn new(parent: u64) -> Option<Self> {
        let parent = Window::try_from(parent).ok()?;
        let c = x11rb::connect(None).ok()?.0;
        let window = c.generate_id().ok()?;
        c.create_window(0, window, parent, 0, 0, 123, 45, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).ok()?.check().ok()?;
        let info = c.intern_atom(false, b"_XEMBED_INFO").ok()?.reply().ok()?.atom;
        let xembed = c.intern_atom(false, b"_XEMBED").ok()?.reply().ok()?.atom;
        c.change_property32(PropMode::REPLACE, window, info, info, &[0, 1]).ok()?.check().ok()?;
        Some(Self { c, window, parent, info, xembed, messages: Vec::new() })
    }

    pub fn command(&mut self, command: &str) -> Option<bool> {
        self.c.sync().unwrap();
        while let Some(event) = self.c.poll_for_event().unwrap() {
            if let Event::ClientMessage(event) = event
                && event.window == self.window
                && event.type_ == self.xembed
                && event.format == 32
            {
                self.messages.push(event.data.as_data32());
            }
        }
        Some(match command {
            // Mapped by the host: VIEWABLE, or UNVIEWABLE while a window manager has not yet
            // mapped the editor's top-level (a compositor's XWM maps it asynchronously).
            "xembed-mapped" => self.c.get_window_attributes(self.window).unwrap().reply().unwrap().map_state != MapState::UNMAPPED,
            "xembed-unmapped" => self.c.get_window_attributes(self.window).unwrap().reply().unwrap().map_state == MapState::UNMAPPED,
            "xembed-notified" => self.messages.iter().filter(|m| m[1] == 0).copied().collect::<Vec<_>>() == vec![[0, 0, 0, self.parent, 0]],
            "xembed-sized" => {
                let child = self.c.get_geometry(self.window).unwrap().reply().unwrap();
                let parent = self.c.get_geometry(self.parent).unwrap().reply().unwrap();
                (child.width, child.height) == (parent.width, parent.height)
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
