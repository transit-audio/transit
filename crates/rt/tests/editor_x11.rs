//! The Linux editor window on a real X server (issue #147): created, resized, followed, closed by
//! the window manager's close request, parked and destroyed on a host thread that pumps its
//! message queue as the plug-in host thread does (ADR 0018), with a fake plug-in that makes its
//! own window in ours on its own X11 connection and drives itself through the run loop (a timer
//! and its connection's descriptor), as VST3 `IRunLoop` and CLAP `timer-support` and
//! `posix-fd-support` plug-ins do.
//!
//! Needs an X server. The runner starts a private `Xvfb` when one is installed (so no window
//! flashes on a developer's desktop and the desktop's resource database is never touched), and
//! runs this binary again on it; without `Xvfb` it uses `DISPLAY` (CI runs it under
//! `xvfb-run -a`), and with neither it skips with a message.

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("editor_x11: X11 editor windows are Linux only; nothing to test here");
}

#[cfg(target_os = "linux")]
fn main() {
    linux::main();
}

#[cfg(target_os = "linux")]
mod linux {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::cell::Cell;
    use std::io::{BufRead, BufReader};
    use std::os::fd::AsRawFd;
    use std::process::{Command, Stdio};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use transit_rt::editor_window::{self as ew, EditorWindow, Group, HeaderCell, HeaderView, WindowState};
    use transit_rt::gui_thread::MessageQueue;
    use transit_rt::run_loop::{self, Ready};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, MapState, PropMode, Window, WindowClass,
    };
    use x11rb::rust_connection::RustConnection;
    use x11rb::wrapper::ConnectionExt as _;

    /// Set in the run on the private `Xvfb`.
    const CHILD: &str = "TRANSIT_EDITOR_X11_ON_PRIVATE_XVFB";

    pub fn main() {
        if std::env::var_os(CHILD).is_none() && run_under_own_xvfb() {
            return;
        }
        if std::env::var_os(CHILD).is_none() {
            if std::env::var_os("DISPLAY").is_none() {
                println!("editor_x11: skipped: no DISPLAY and no Xvfb (install xvfb, or run under `xvfb-run -a`)");
                return;
            }
            println!("editor_x11: no Xvfb; running on DISPLAY={:?}", std::env::var("DISPLAY").unwrap_or_default());
        }
        let tests: &[(&str, fn())] = &[
            ("create_resize_follow_and_destroy_with_a_fake_plugin", create_resize_follow_and_destroy_with_a_fake_plugin),
            ("the_close_button_hides_and_close_destroys", the_close_button_hides_and_close_destroys),
            ("another_thread_never_touches_the_window", another_thread_never_touches_the_window),
            ("park_and_unpark_follow_the_main_window", park_and_unpark_follow_the_main_window),
            ("a_stuck_threads_windows_hide_without_it", a_stuck_threads_windows_hide_without_it),
            ("a_dpi_change_reaches_the_window", a_dpi_change_reaches_the_window),
        ];
        let filter: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
        let mut ran = 0;
        for (name, test) in tests {
            if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
                continue;
            }
            println!("test {name} ...");
            test();
            println!("test {name} ... ok");
            ran += 1;
        }
        println!("test result: ok. {ran} passed; 0 failed");
    }

    /// Runs this binary again on a private `Xvfb`; false when there is no `Xvfb`. It listens on
    /// the abstract socket only (`-nolisten unix`): `WSLg` mounts `/tmp/.X11-unix` read-only.
    fn run_under_own_xvfb() -> bool {
        let xvfb = Command::new("Xvfb")
            .args(["-displayfd", "1", "-nolisten", "tcp", "-nolisten", "unix", "-screen", "0", "1280x1024x24"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut xvfb) = xvfb else { return false };
        let mut line = String::new();
        let read = BufReader::new(xvfb.stdout.take().unwrap()).read_line(&mut line);
        let display = line.trim().to_owned();
        if read.is_err() || display.is_empty() {
            let _ = xvfb.kill();
            panic!("Xvfb did not report its display");
        }
        println!("editor_x11: running on a private Xvfb at :{display}");
        let status = Command::new(std::env::current_exe().unwrap())
            .args(std::env::args().skip(1))
            .env("DISPLAY", format!(":{display}"))
            .env(CHILD, "1")
            .env_remove("WAYLAND_DISPLAY")
            .status();
        let _ = xvfb.kill();
        let _ = xvfb.wait();
        assert!(status.unwrap().success(), "the tests failed on the private Xvfb");
        true
    }

    /// Another client of the display: what the window manager and the user see.
    fn observer() -> RustConnection {
        x11rb::connect(None).expect("the X server").0
    }

    fn id(raw: *mut std::ffi::c_void) -> Window {
        Window::try_from(raw.addr()).unwrap()
    }

    fn geometry(c: &RustConnection, w: Window) -> Option<(u16, u16)> {
        c.get_geometry(w).ok()?.reply().ok().map(|g| (g.width, g.height))
    }

    fn mapped(c: &RustConnection, w: Window) -> bool {
        c.get_window_attributes(w).ok().and_then(|r| r.reply().ok()).is_some_and(|a| a.map_state == MapState::VIEWABLE)
    }

    /// Pumps the host thread's queue (as its loop does: wait, dispatch, the editor's idle) until
    /// `done` or 5 s pass; returns whether `done` held.
    fn pump_until(q: &MessageQueue, win: Option<&EditorWindow>, mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            q.dispatch(256);
            if let Some(w) = win {
                w.sync();
            }
            if done() {
                return true;
            }
            q.wait(Duration::from_millis(15));
        }
        false
    }

    /// The plug-in's side: its own connection, a window made in the parent we pass it (as in
    /// `IPlugView::attached` or `clap_plugin_gui.set_parent`), and its run-loop registrations.
    struct FakePlugin {
        conn: Rc<RustConnection>,
        window: Window,
        tokens: Vec<run_loop::Token>,
        timer_calls: Rc<Cell<u32>>,
        events_seen: Rc<Cell<u32>>,
    }

    impl FakePlugin {
        fn attach(parent: Window, width: u16, height: u16) -> FakePlugin {
            let conn = Rc::new(x11rb::connect(None).unwrap().0);
            let window = conn.generate_id().unwrap();
            let aux = CreateWindowAux::new().background_pixel(0x0030_6090).event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::EXPOSURE);
            conn.create_window(0, window, parent, 0, 0, width, height, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
            conn.map_window(window).unwrap();
            conn.flush().unwrap();
            let timer_calls = Rc::new(Cell::new(0));
            let events_seen = Rc::new(Cell::new(0));
            let calls = Rc::clone(&timer_calls);
            let timer = run_loop::add_timer(
                Duration::from_millis(10),
                Rc::new(move || {
                    calls.set(calls.get() + 1);
                    true
                }),
            )
            .unwrap();
            let (own, seen) = (Rc::clone(&conn), Rc::clone(&events_seen));
            let fd = run_loop::add_fd(
                conn.stream().as_raw_fd(),
                Ready::READ,
                Rc::new(move |_, _| {
                    while let Ok(Some(_)) = own.poll_for_event() {
                        seen.set(seen.get() + 1);
                    }
                    true
                }),
            )
            .unwrap();
            FakePlugin { conn, window, tokens: vec![timer, fd], timer_calls, events_seen }
        }

        /// As a plug-in's `removed` / `destroy`: its window goes, its registrations too.
        fn detach(self) {
            for t in &self.tokens {
                assert!(run_loop::remove(*t));
            }
            self.conn.destroy_window(self.window).unwrap();
            self.conn.flush().unwrap();
        }
    }

    fn create_resize_follow_and_destroy_with_a_fake_plugin() {
        let q = MessageQueue::attach();
        let obs = observer();
        let header = HeaderCell::new();
        header.set(&HeaderView { title: "Fake Synth".into(), ..HeaderView::default() });
        let win = EditorWindow::new("ignored", 300.0, 200.0, Some(Arc::clone(&header))).unwrap();
        let (frame, content) = (id(win.raw()), id(win.parent()));
        assert_ne!(content, 0, "the content window the plug-in parents in");
        assert_eq!(ew::take_created(), vec![frame as usize], "the window an open made");
        assert_eq!(geometry(&obs, frame), Some((300, 200)));
        assert!(win.scale() >= 1.0);
        let title = obs.get_property(false, frame, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 64).unwrap().reply().unwrap().value;
        assert_eq!(title, b"Fake Synth", "the header's title names the window");
        let class = obs.get_property(false, frame, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64).unwrap().reply().unwrap().value;
        assert_eq!(class, b"plugin-editor\0transit\0");

        // Shown on the host thread.
        win.raise();
        assert!(win.is_visible());
        assert!(pump_until(&q, Some(&win), || mapped(&obs, frame)), "mapped");
        assert_eq!(win.state(), WindowState::Shown);

        // The plug-in attaches with the size it reported, asks for another (`resizeView`,
        // `request_resize`) and runs on the host thread's run loop.
        let plugin = FakePlugin::attach(content, 300, 200);
        assert!(ew::resize_content(win.raw(), 400.0, 250.0), "honored on the window's thread");
        assert_eq!(win.content_size(), (400, 250));
        assert_eq!(geometry(&obs, frame), Some((400, 250)));
        assert_eq!(geometry(&obs, content), Some((400, 250)));
        assert!(pump_until(&q, Some(&win), || plugin.timer_calls.get() >= 3 && plugin.events_seen.get() > 0), "the timer fired and the descriptor handler ran");

        // A plug-in whose window is not the size it reported: the window follows it once it is
        // stable (`Follow`, the grace after the host's own resize included).
        plugin.conn.configure_window(plugin.window, &x11rb::protocol::xproto::ConfigureWindowAux::new().width(520).height(330)).unwrap();
        plugin.conn.flush().unwrap();
        assert!(pump_until(&q, Some(&win), || win.content_size() == (520, 330)), "followed the plug-in's window: {:?}", win.content_size());
        assert_eq!(geometry(&obs, frame), Some((520, 330)));

        // Teardown in the editor link's order: the view goes, then the window.
        plugin.detach();
        assert_eq!(run_loop::counts(), (0, 0));
        win.close();
        obs.sync().unwrap();
        assert_eq!(geometry(&obs, frame), None, "destroyed");
        assert_eq!(ew::window_state(frame as usize), None);
        q.dispatch(256);
    }

    fn the_close_button_hides_and_close_destroys() {
        let q = MessageQueue::attach();
        let obs = observer();
        let win = EditorWindow::new("Close me", 200.0, 100.0, None).unwrap();
        let frame = id(win.raw());
        win.raise();
        assert!(pump_until(&q, Some(&win), || mapped(&obs, frame)));
        // What the window manager sends for its close button (ICCCM WM_DELETE_WINDOW).
        let protocols = obs.intern_atom(false, b"WM_PROTOCOLS").unwrap().reply().unwrap().atom;
        let delete = obs.intern_atom(false, b"WM_DELETE_WINDOW").unwrap().reply().unwrap().atom;
        obs.send_event(false, frame, EventMask::NO_EVENT, ClientMessageEvent::new(32, frame, protocols, [delete, 0, 0, 0, 0])).unwrap();
        obs.flush().unwrap();
        assert!(pump_until(&q, Some(&win), || !win.is_visible()), "closing hides");
        assert_eq!(win.state(), WindowState::Hidden);
        assert!(pump_until(&q, None, || !mapped(&obs, frame)));
        assert!(geometry(&obs, frame).is_some(), "hidden, not destroyed: the editor link tears the view down first");
        win.close();
        obs.sync().unwrap();
        assert_eq!(geometry(&obs, frame), None);
    }

    fn another_thread_never_touches_the_window() {
        let q = MessageQueue::attach();
        let win = EditorWindow::new("Mine", 200.0, 100.0, None).unwrap();
        let raw = win.raw().addr();
        let (win, seen) = std::thread::spawn(move || {
            let raw = std::ptr::without_provenance_mut(raw);
            let seen = (win.parent().is_null(), win.content_size(), win.is_visible(), ew::resize_content(raw, 50.0, 50.0), ew::window_state(raw.addr()));
            win.raise();
            (win, seen)
        })
        .join()
        .unwrap();
        assert_eq!(seen, (true, (0, 0), false, false, None), "another thread sees no window and changes nothing");
        assert_eq!(win.content_size(), (200, 100));
        assert!(!win.is_visible(), "the raise from the other thread did nothing");
        win.close();
        q.dispatch(256);
    }

    fn park_and_unpark_follow_the_main_window() {
        let q = MessageQueue::attach();
        let obs = observer();
        assert_eq!(ew::group(Group::Follow(true)), 0, "no window yet");
        let win = EditorWindow::new("Follows", 200.0, 100.0, None).unwrap();
        let frame = id(win.raw());
        let state = obs.intern_atom(false, b"_NET_WM_STATE").unwrap().reply().unwrap().atom;
        let skip = obs.intern_atom(false, b"_NET_WM_STATE_SKIP_TASKBAR").unwrap().reply().unwrap().atom;
        let atoms = obs.get_property(false, frame, state, AtomEnum::ATOM, 0, 8).unwrap().reply().unwrap();
        assert!(atoms.value32().unwrap().any(|a| a == skip), "a following editor has no taskbar entry");
        win.raise();
        assert!(pump_until(&q, Some(&win), || mapped(&obs, frame)));
        assert_eq!(ew::group(Group::Park), 1);
        assert_eq!(win.state(), WindowState::Parked);
        assert!(win.is_visible(), "a parked editor still counts as open");
        assert!(pump_until(&q, Some(&win), || !mapped(&obs, frame)), "hidden with the main window");
        assert!(!ew::raise_window(frame as usize), "a parked window stays parked");
        assert_eq!(ew::group(Group::Unpark), 1);
        assert!(pump_until(&q, Some(&win), || mapped(&obs, frame)), "back with the main window");
        assert_eq!(win.state(), WindowState::Shown);
        assert_eq!(ew::group(Group::Front), 1);
        assert!(ew::raise_window(frame as usize));
        assert_eq!(ew::group(Group::Follow(false)), 0, "nothing parked");
        let _ = obs.change_property32(PropMode::REPLACE, frame, state, AtomEnum::ATOM, &[]);
        win.close();
        q.dispatch(256);
    }

    /// Issue #205's Give up: the windows of a host thread that is held (not pumping) are hidden
    /// from another thread, which waits for nothing.
    fn a_stuck_threads_windows_hide_without_it() {
        let (to_host, held) = std::sync::mpsc::channel::<()>();
        let (ready_tx, ready) = std::sync::mpsc::channel::<(u32, Window)>();
        let host = std::thread::spawn(move || {
            let q = MessageQueue::attach();
            let obs = observer();
            let win = EditorWindow::new("Stuck", 200.0, 100.0, None).unwrap();
            let frame = id(win.raw());
            win.raise();
            assert!(pump_until(&q, Some(&win), || mapped(&obs, frame)));
            ready_tx.send((ew::current_thread_id(), frame)).unwrap();
            // Held inside "a plug-in call": no dispatch, no wait.
            held.recv().unwrap();
            win.close();
        });
        let (thread, frame) = ready.recv().unwrap();
        let obs = observer();
        let start = Instant::now();
        assert_eq!(ew::hide_thread_windows(thread), 1);
        assert!(start.elapsed() < Duration::from_millis(500), "the call waits for nothing");
        let start = Instant::now();
        while mapped(&obs, frame) && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!mapped(&obs, frame), "hidden while its thread is still held");
        to_host.send(()).unwrap();
        host.join().unwrap();
    }

    /// `Xft.dpi` changes (the desktop's scale setting): the window reports the new scale once.
    /// The root's resource database is restored afterwards.
    fn a_dpi_change_reaches_the_window() {
        let q = MessageQueue::attach();
        let obs = observer();
        let root = obs.setup().roots[0].root;
        let before = obs.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 20).unwrap().reply().unwrap();
        let win = EditorWindow::new("Scale", 200.0, 100.0, None).unwrap();
        assert_eq!(win.take_scale_change(), None);
        obs.change_property8(PropMode::REPLACE, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, b"Xft.dpi:\t192\n").unwrap();
        obs.flush().unwrap();
        let mut change = None;
        assert!(pump_until(&q, Some(&win), || {
            change = change.or(win.take_scale_change());
            change.is_some()
        }));
        assert_eq!(change, Some(2.0));
        assert_eq!(win.scale(), 2.0);
        assert_eq!(win.take_scale_change(), None, "once");
        if before.type_ == u32::from(AtomEnum::NONE) {
            obs.delete_property(root, AtomEnum::RESOURCE_MANAGER.into()).unwrap();
        } else {
            obs.change_property8(PropMode::REPLACE, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, &before.value).unwrap();
        }
        obs.flush().unwrap();
        win.close();
        q.dispatch(256);
    }
}
