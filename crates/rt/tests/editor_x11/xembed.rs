// XEmbed protocol tests. Explicit server readouts, no elapsed-time assertions or timer delivery.

struct Plug {
    c: RustConnection,
    window: Window,
    info: u32,
    xembed: u32,
    messages: Vec<[u32; 5]>,
    keys: Vec<u8>,
}

impl Plug {
    fn new(parent: Window) -> Self {
        let c = observer();
        let window = c.generate_id().unwrap();
        c.create_window(0, window, parent, 0, 0, 123, 45, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        let info = c.intern_atom(false, b"_XEMBED_INFO").unwrap().reply().unwrap().atom;
        let xembed = c.intern_atom(false, b"_XEMBED").unwrap().reply().unwrap().atom;
        Self { c, window, info, xembed, messages: Vec::new(), keys: Vec::new() }
    }

    fn flags(&self, flags: u32) {
        self.c.change_property32(PropMode::REPLACE, self.window, self.info, self.info, &[0, flags]).unwrap();
        self.c.sync().unwrap();
    }

    fn read(&mut self) {
        self.c.sync().unwrap();
        while let Some(event) = self.c.poll_for_event().unwrap() {
            match event {
                x11rb::protocol::Event::ClientMessage(e) if e.window == self.window && e.type_ == self.xembed && e.format == 32 => {
                    self.messages.push(e.data.as_data32());
                }
                x11rb::protocol::Event::KeyPress(e) | x11rb::protocol::Event::KeyRelease(e) => self.keys.push(e.response_type & 0x7f),
                _ => {}
            }
        }
    }

    fn state(&self) -> MapState {
        self.c.get_window_attributes(self.window).unwrap().reply().unwrap().map_state
    }
}

// The host and fake plug have different X connections. Each pass dispatches a bounded batch,
// idles the host, and round-trips the server before reading the plug. 64 passes allow the finite
// create/property/configure/focus exchanges to drain; the assertions concern protocol state,
// never elapsed time. This bound catches a missing exchange without hanging the Linux gate.
fn xembed_passes(q: &MessageQueue, win: &EditorWindow, plug: &mut Plug) {
    for _ in 0..64 {
        q.dispatch(256);
        win.sync();
        plug.read();
    }
}

fn xembed_maps_notifies_unmaps_and_resizes() {
    let q = MessageQueue::attach();
    let win = EditorWindow::new("XEmbed", 300.0, 200.0, None).unwrap();
    let content = id(win.parent());
    let mut plug = Plug::new(content);
    plug.flags(1); // Deliberately no MapWindow: GtkPlug asks the embedder to map it.
    assert_eq!(plug.state(), MapState::UNMAPPED);
    win.raise();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::VIEWABLE, "XEMBED_MAPPED must be honored by the host");
    assert_eq!(plug.messages.iter().filter(|m| m[1] == 0).copied().collect::<Vec<_>>(), vec![[0, 0, 0, content, 0]]);
    assert_eq!(geometry(&plug.c, plug.window), geometry(&plug.c, content));
    assert!(ew::resize_content(win.raw(), 510.0, 270.0));
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(geometry(&plug.c, plug.window), Some((510, 270)));
    // Also exercise a content ConfigureNotify independently of resize_content's direct path.
    plug.c.configure_window(content, &x11rb::protocol::xproto::ConfigureWindowAux::new().width(620).height(310)).unwrap();
    plug.c.sync().unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(geometry(&plug.c, plug.window), Some((620, 310)));
    plug.flags(0);
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::UNMAPPED, "clearing the mapped flag unmaps");
    plug.flags(1);
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::VIEWABLE);
    assert_eq!(plug.messages.iter().filter(|m| m[1] == 0).count(), 1, "property changes do not restart the handshake");
    win.close();
    q.dispatch(256);
}

fn xembed_requires_valid_info_and_tracks_reparenting() {
    let q = MessageQueue::attach();
    let win = EditorWindow::new("Late XEmbed", 300.0, 200.0, None).unwrap();
    let content = id(win.parent());
    let mut plug = Plug::new(content);
    win.raise();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::UNMAPPED, "never map a non-XEmbed child");
    assert_eq!(plug.messages, [] as [[u32; 5]; 0]);
    // Wrong type, wrong format, short and oversized properties must not opt a toolkit in.
    plug.c.change_property32(PropMode::REPLACE, plug.window, plug.info, AtomEnum::CARDINAL, &[0, 1]).unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::UNMAPPED);
    plug.c.change_property8(PropMode::REPLACE, plug.window, plug.info, plug.info, &[0, 1]).unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::UNMAPPED);
    for values in [&[0][..], &[0, 1, 0][..]] {
        plug.c.change_property32(PropMode::REPLACE, plug.window, plug.info, plug.info, values).unwrap();
        xembed_passes(&q, &win, &mut plug);
        assert_eq!(plug.state(), MapState::UNMAPPED);
    }
    plug.flags(1); // Added after CreateNotify was consumed: PropertyChange must be selected.
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::VIEWABLE);
    let root = plug.c.setup().roots[0].root;
    plug.c.reparent_window(plug.window, root, 0, 0).unwrap();
    plug.c.sync().unwrap();
    xembed_passes(&q, &win, &mut plug);
    plug.c.unmap_window(plug.window).unwrap();
    plug.flags(1);
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::UNMAPPED, "a departed plug is no longer managed");
    plug.c.reparent_window(plug.window, content, 0, 0).unwrap();
    plug.c.sync().unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::VIEWABLE);
    assert_eq!(plug.messages.iter().filter(|m| m[1] == 0).count(), 2, "reembedding starts a new handshake");
    plug.c.destroy_window(plug.window).unwrap();
    plug.c.sync().unwrap();
    q.dispatch(256);
    assert!(ew::resize_content(win.raw(), 500.0, 250.0), "destroyed foreign windows are harmless");
    win.close();
    q.dispatch(256);
}

fn xembed_focus_and_keys_are_symmetric() {
    use x11rb::protocol::xproto::{InputFocus, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, KeyButMask, KeyPressEvent};
    let q = MessageQueue::attach();
    let win = EditorWindow::new("XEmbed focus", 300.0, 200.0, None).unwrap();
    let content = id(win.parent());
    let mut plug = Plug::new(content);
    plug.flags(1);
    win.raise();
    xembed_passes(&q, &win, &mut plug);
    // Explicit focus requests, independent of a WM's activation/placement policy.
    plug.c.set_input_focus(InputFocus::PARENT, id(win.raw()), x11rb::CURRENT_TIME).unwrap();
    plug.c.sync().unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert!(plug.messages.contains(&[0, 1, 0, 0, 0]), "WINDOW_ACTIVATE");
    assert!(plug.messages.contains(&[0, 4, 0, 0, 0]), "FOCUS_IN / FOCUS_CURRENT");
    let proxy = plug.c.get_input_focus().unwrap().reply().unwrap().focus;
    assert_ne!(proxy, content, "the focus proxy must not contain embedded descendants");
    let proxy_tree = plug.c.query_tree(proxy).unwrap().reply().unwrap();
    assert_eq!(proxy_tree.parent, id(win.raw()), "the proxy and content are siblings");
    assert_eq!(proxy_tree.children, [] as [Window; 0]);
    let root = plug.c.setup().roots[0].root;
    for response_type in [KEY_PRESS_EVENT, KEY_RELEASE_EVENT] {
        let event = KeyPressEvent {
            response_type,
            detail: 38,
            sequence: 0,
            time: 0,
            root,
            event: proxy,
            child: 0,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state: KeyButMask::default(),
            same_screen: true,
        };
        // A mask-based event only reaches the host if it selected the correct key mask.
        // NO_EVENT would bypass that selection and conceal a broken input path.
        let mask = if response_type == KEY_PRESS_EVENT { EventMask::KEY_PRESS } else { EventMask::KEY_RELEASE };
        plug.c.send_event(false, proxy, mask, event).unwrap();
    }
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.keys, vec![KEY_PRESS_EVENT, KEY_RELEASE_EVENT]);
    plug.messages.clear();
    plug.c.set_input_focus(InputFocus::POINTER_ROOT, root, x11rb::CURRENT_TIME).unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.messages, vec![[0, 2, 0, 0, 0]], "deactivation preserves logical focus (spec 5.2)");
    plug.flags(0);
    xembed_passes(&q, &win, &mut plug);
    assert!(plug.messages.contains(&[0, 5, 0, 0, 0]), "unmapping leaves the logical focus site");
    plug.flags(1);
    xembed_passes(&q, &win, &mut plug);
    plug.messages.clear();
    plug.c.send_event(false, content, EventMask::NO_EVENT, ClientMessageEvent::new(32, content, plug.xembed, [0, 3, 0, 0, 0])).unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert!(plug.messages.contains(&[0, 4, 0, 0, 0]), "a toolkit click's REQUEST_FOCUS receives FOCUS_IN");
    assert_eq!(plug.c.get_input_focus().unwrap().reply().unwrap().focus, root, "an inactive request must not steal X focus");
    plug.c.set_input_focus(InputFocus::PARENT, id(win.raw()), x11rb::CURRENT_TIME).unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.c.get_input_focus().unwrap().reply().unwrap().focus, proxy, "activation restores the proxy");
    let _ = ew::group(Group::Follow(true));
    let _ = ew::group(Group::Park);
    xembed_passes(&q, &win, &mut plug);
    assert!(plug.messages.contains(&[0, 5, 0, 0, 0]), "hiding sends FOCUS_OUT");
    win.close();
    let _ = ew::group(Group::Unpark);
    q.dispatch(256);
}
