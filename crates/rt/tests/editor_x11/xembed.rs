// XEmbed protocol tests. Explicit server readouts, no elapsed-time assertions or timer delivery.

struct Plug {
    c: RustConnection,
    window: Window,
    info: u32,
    xembed: u32,
    messages: Vec<[u32; 5]>,
    keys: Vec<x11rb::protocol::xproto::KeyPressEvent>,
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
                x11rb::protocol::Event::KeyPress(e) | x11rb::protocol::Event::KeyRelease(e) => self.keys.push(e),
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
    let idle_time = Instant::now();
    let content = id(win.parent());
    let mut plug = Plug::new(content);
    plug.flags(1); // Deliberately no MapWindow: GtkPlug asks the embedder to map it.
    assert_eq!(plug.state(), MapState::UNMAPPED);
    win.raise();
    // Round-trip the host connection so CreateNotify is queued before dispatch.
    assert!(win.surface_readout().is_some());
    q.dispatch(256);
    win.sync_at(idle_time);
    assert_eq!(plug.state(), MapState::UNMAPPED, "detection defers the host map");
    q.dispatch(256);
    win.sync_at(idle_time);
    assert_eq!(plug.state(), MapState::VIEWABLE, "the second dispatch's idle maps the GTK-shaped child");
    assert!(win.surface_readout().unwrap().contains("mapped_by_host=true"));
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
    let idle_time = Instant::now();
    plug.flags(1); // Added after CreateNotify was consumed: PropertyChange must be selected.
    assert!(win.surface_readout().is_some());
    q.dispatch(256);
    win.sync_at(idle_time);
    assert_eq!(plug.state(), MapState::UNMAPPED, "late info starts the same deferral");
    q.dispatch(256);
    win.sync_at(idle_time);
    assert_eq!(plug.state(), MapState::VIEWABLE, "the second dispatch after late info maps it");
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
            child: content,
            root_x: 101,
            root_y: 202,
            event_x: -3,
            event_y: 4,
            state: KeyButMask::default(),
            same_screen: true,
        };
        // A mask-based event only reaches the host if it selected the correct key mask.
        // NO_EVENT would bypass that selection and conceal a broken input path.
        let mask = if response_type == KEY_PRESS_EVENT { EventMask::KEY_PRESS } else { EventMask::KEY_RELEASE };
        plug.c.send_event(false, proxy, mask, event).unwrap();
    }
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.keys.iter().map(|e| e.response_type & 0x7f).collect::<Vec<_>>(), vec![KEY_PRESS_EVENT, KEY_RELEASE_EVENT]);
    for event in &plug.keys {
        assert_eq!(event.event, plug.window);
        assert_eq!((event.child, event.root_x, event.root_y, event.event_x, event.event_y), (0, 0, 0, 0, 0));
        assert_eq!(event.detail, 38);
    }
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

fn xembed_self_mapped_child_keeps_direct_focus_and_size() {
    use x11rb::protocol::xproto::InputFocus;
    let q = MessageQueue::attach();
    let win = EditorWindow::new("Self-mapped XEmbed", 300.0, 200.0, None).unwrap();
    let content = id(win.parent());
    let mut plug = Plug::new(content);
    plug.flags(1);
    // Already mapped when detected (the deferred-flush case has its own test below).
    plug.c.map_window(plug.window).unwrap();
    plug.c.sync().unwrap();
    assert_ne!(plug.state(), MapState::UNMAPPED);
    win.raise();
    xembed_passes(&q, &win, &mut plug);
    let root = plug.c.setup().roots[0].root;
    for _ in 0..2 {
        plug.c.set_input_focus(InputFocus::PARENT, id(win.raw()), x11rb::CURRENT_TIME).unwrap();
        plug.c.sync().unwrap();
        xembed_passes(&q, &win, &mut plug);
        assert_eq!(plug.c.get_input_focus().unwrap().reply().unwrap().focus, plug.window, "self-mapped children keep direct X focus");
        assert_eq!(geometry(&plug.c, plug.window), Some((123, 45)), "the toolkit owns its size");
        assert_eq!(plug.messages, vec![[0, 0, 0, content, 0]], "only one EMBEDDED_NOTIFY, no activation or logical focus messages");
        plug.c.set_input_focus(InputFocus::POINTER_ROOT, root, x11rb::CURRENT_TIME).unwrap();
        plug.c.sync().unwrap();
        xembed_passes(&q, &win, &mut plug);
    }
    assert!(ew::resize_content(win.raw(), 510.0, 270.0));
    plug.flags(0);
    plug.c.send_event(false, content, EventMask::NO_EVENT, ClientMessageEvent::new(32, content, plug.xembed, [0, 3, 0, 0, 0])).unwrap();
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.state(), MapState::VIEWABLE, "self-mapped children own their mapping even when the flag clears");
    assert_eq!(geometry(&plug.c, plug.window), Some((123, 45)));
    assert_eq!(plug.messages, vec![[0, 0, 0, content, 0]]);
    assert!(plug.keys.is_empty());
    win.close();
    q.dispatch(256);
}

fn xembed_juce_deferred_flush_keeps_direct_focus_and_size() {
    use x11rb::protocol::xproto::InputFocus;
    let q = MessageQueue::attach();
    let win = EditorWindow::new("JUCE-shaped XEmbed", 300.0, 200.0, None).unwrap();
    let idle_time = Instant::now();
    let content = id(win.parent());
    win.raise();
    let mut plug = Plug::new(content);
    // JUCE creates + advertises + flushes, but its MapWindow reaches the server only
    // when a later fd/timer handler flushes. Detection must leave that opportunity open.
    plug.flags(1);
    assert!(win.surface_readout().is_some()); // Host round trip queues CreateNotify.
    q.dispatch(256);
    win.sync_at(idle_time);
    let after_detection = plug.state();
    // A dispatch without any toolkit budget is not a toolkit turn. Freeze the idle
    // readout before detection so neither these passes nor a loaded host can expire
    // the product's fallback and mask a broken dispatch counter.
    for _ in 0..2 {
        q.dispatch(0);
        win.sync_at(idle_time);
    }
    let after_empty_dispatches = plug.state();
    plug.c.map_window(plug.window).unwrap();
    plug.c.set_input_focus(InputFocus::PARENT, id(win.raw()), x11rb::CURRENT_TIME).unwrap();
    plug.c.sync().unwrap();
    assert!(win.surface_readout().is_some()); // Queue the explicit frame activation.
    q.dispatch(256);
    win.sync_at(idle_time);
    plug.read();
    // Keep focus first: on 935d0001 the first dispatch already claimed the child,
    // so this assertion sees the proxy, even though the client's MapWindow succeeds.
    assert_eq!(plug.c.get_input_focus().unwrap().reply().unwrap().focus, plug.window, "a deferred self-map keeps direct X focus");
    assert_eq!(after_detection, MapState::UNMAPPED, "the detection dispatch must not map");
    assert_eq!(after_empty_dispatches, MapState::UNMAPPED, "zero toolkit budget does not advance the deferral");
    assert_eq!(geometry(&plug.c, plug.window), Some((123, 45)), "the toolkit owns its size");
    let readout = win.surface_readout().unwrap();
    assert!(readout.contains("xembed=[0, 1, mapped_by_host=false, notify_sent=true]"), "{readout}");
    assert_eq!(plug.messages, vec![[0, 0, 0, content, 0]], "only one EMBEDDED_NOTIFY");
    xembed_passes(&q, &win, &mut plug);
    assert_eq!(plug.c.get_input_focus().unwrap().reply().unwrap().focus, plug.window);
    assert_eq!(geometry(&plug.c, plug.window), Some((123, 45)));
    assert_eq!(plug.messages, vec![[0, 0, 0, content, 0]]);
    assert!(win.surface_readout().unwrap().contains("mapped_by_host=false"));
    win.close();
    q.dispatch(256);
}
