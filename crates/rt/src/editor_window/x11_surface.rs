//! Two observations after a successful embedded open (#289). X11 queries run only on the
//! window's host thread, from editor idle. They cannot flush a plug-in's separate connection.

use super::*;
use std::fmt::Write as _;
use std::time::Duration;
use x11rb::protocol::xproto::MapState;

pub(super) struct Probe {
    name: String,
    opened: Instant,
    timers: u64,
    events: u64,
    next: usize,
}

impl Probe {
    // Explicit elapsed input: tests never wait for the sampling deadlines.
    fn due(&mut self, elapsed: Duration) -> Option<u64> {
        let seconds = *[1, 5].get(self.next)?;
        if elapsed < Duration::from_secs(seconds) {
            return None;
        }
        self.next += 1;
        Some(seconds)
    }
}

impl EditorWindow {
    /// Starts the Linux surface observations at a successful editor open. Repeated raises of
    /// the same editor do not restart them. `timers` is this instance's delivered callback count.
    pub fn start_surface_probe(&self, name: &str, timers: u64) {
        let Some((c, f)) = self.live() else { return };
        f.surface.borrow_mut().get_or_insert_with(|| Probe {
            name: name.to_owned(),
            opened: Instant::now(),
            timers,
            events: c.events.get(),
            next: 0,
        });
    }

    /// Samples at the first idle at/after 1 s and 5 s. `registrations` is evaluated only when
    /// due; it describes this instance's current format run-loop registrations. Counts are
    /// relative to open; X events cover this host connection, not the plug-in's own connection.
    pub fn sample_surface(&self, timers: u64, registrations: impl FnOnce() -> String) {
        self.sample_surface_at(timers, registrations, Instant::now());
    }

    fn sample_surface_at(&self, timers: u64, registrations: impl FnOnce() -> String, now: Instant) {
        let Some((c, f)) = self.live() else { return };
        let (name, elapsed, at, callbacks, events) = {
            let mut probe = f.surface.borrow_mut();
            let Some(p) = probe.as_mut() else { return };
            let elapsed = now.saturating_duration_since(p.opened);
            let Some(at) = p.due(elapsed) else { return };
            (p.name.clone(), elapsed, at, timers.saturating_sub(p.timers), c.events.get().saturating_sub(p.events))
        };
        // Release the RefCell before calling caller-supplied code or the logger: either may
        // reenter the diagnostic. The deadline has already been consumed.
        let snapshot = snapshot(&c, &f);
        log::info!(
            "editor surface: {name}: at={at}s elapsed_ms={} {} timer_callbacks={callbacks} x_events={events} {}",
            elapsed.as_millis(),
            snapshot.text,
            registrations()
        );
        if at == 5 && snapshot.no_mapped_child == Some(true) && callbacks == 0 {
            log::warn!(
                "plug-in editor shows nothing: {name}: editor surface: no mapped child and no timer callback since open; candidates: deferred child creation, missing timer registration/delivery, XEmbed/toolkit initialization, or drawing directly into the parent (this observation does not prove blank pixels)"
            );
        }
    }
}

struct Snapshot {
    text: String,
    // Unknown on query failure: a vanished child is not evidence of no mapped child.
    no_mapped_child: Option<bool>,
}

fn describe(c: &Conn, id: Window) -> Option<(String, bool)> {
    let geometry = c.c.get_geometry(id).ok()?.reply().ok()?;
    let attributes = c.c.get_window_attributes(id).ok()?.reply().ok()?;
    let class =
        c.c.setup().roots.iter().flat_map(|s| &s.allowed_depths).flat_map(|d| &d.visuals).find(|v| v.visual_id == attributes.visual).map(|v| v.class);
    let position = c.c.translate_coordinates(id, c.root, 0, 0).ok().and_then(|r| r.reply().ok()).map(|p| (p.dst_x, p.dst_y));
    Some((
        format!(
            "id=0x{id:x} size={}x{} depth={} visual=0x{:x} class={class:?} colormap=0x{:x} map={:?} root_xy={position:?} override_redirect={}",
            geometry.width,
            geometry.height,
            geometry.depth,
            attributes.visual,
            attributes.colormap,
            attributes.map_state,
            attributes.override_redirect
        ),
        attributes.map_state != MapState::UNMAPPED,
    ))
}

// Read only: a GTK plug requests mapping through this property; the embedder is responsible
// for honoring it. An unmapped child with flags & 1 set distinguishes that from a hidden GUI.
fn xembed_info(c: &Conn, id: Window) -> String {
    let atom = c.c.intern_atom(true, b"_XEMBED_INFO").ok().and_then(|r| r.reply().ok());
    let Some(atom) = atom else { return "query-failed".into() };
    if atom.atom == 0 {
        return "absent".into();
    }
    let property = c.c.get_property(false, id, atom.atom, atom.atom, 0, 2).ok().and_then(|r| r.reply().ok());
    match property {
        Some(p) if p.type_ == 0 => "absent".into(),
        Some(p) => p.value32().map_or_else(|| "invalid-format".into(), |v| format!("{:?}", v.collect::<Vec<_>>())),
        None => "query-failed".into(),
    }
}

fn snapshot(c: &Conn, f: &Frame) -> Snapshot {
    let content = describe(c, f.content).map_or_else(|| format!("id=0x{:x} query=failed", f.content), |(s, _)| s);
    let frame = describe(c, f.id).map_or_else(|| format!("id=0x{:x} query=failed", f.id), |(s, _)| s);
    let screen = c.c.setup().roots.iter().find(|s| s.root == c.root).map(|s| (s.width_in_pixels, s.height_in_pixels));
    let mut text = format!(
        "content=[{content}] frame=[{frame}] shown={} parked={} screen={screen:?} session={:?} wayland_display={:?}",
        f.shown.get(),
        f.parked.get(),
        std::env::var("XDG_SESSION_TYPE").ok(),
        std::env::var("WAYLAND_DISPLAY").ok()
    );
    let tree = c.c.query_tree(f.content).ok().and_then(|r| r.reply().ok());
    let Some(tree) = tree else {
        text.push_str(" children=unknown (query failed)");
        return Snapshot { text, no_mapped_child: None };
    };
    let _ = write!(text, " content_parent=0x{:x} children={}", tree.parent, tree.children.len());
    let mut no_mapped_child = Some(true);
    for child in tree.children {
        if let Some((description, mapped)) = describe(c, child) {
            let _ = write!(text, " child=[{description} _XEMBED_INFO={}]", xembed_info(c, child));
            if mapped {
                no_mapped_child = Some(false);
            }
        } else {
            let _ = write!(text, " child=[id=0x{child:x} query=failed]");
            if no_mapped_child != Some(false) {
                no_mapped_child = None;
            }
        }
    }
    Snapshot { text, no_mapped_child }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    thread_local! {
        static LOG: RefCell<Vec<(log::Level, String)>> = const { RefCell::new(Vec::new()) };
    }
    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, r: &log::Record<'_>) {
            LOG.with(|l| l.borrow_mut().push((r.level(), r.args().to_string())));
        }
        fn flush(&self) {}
    }
    fn capture() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            log::set_logger(&Capture).unwrap();
            log::set_max_level(log::LevelFilter::Info);
        });
        LOG.with(|l| l.borrow_mut().clear());
    }

    #[test]
    fn surface_logs_twice_with_relative_counts_and_warns_only_without_a_pump_or_child() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let win = EditorWindow::new("surface probe", 300.0, 200.0, None).unwrap();
        let (c, f) = win.live().unwrap();
        c.events.set(40);
        win.start_surface_probe("test", 100);
        let opened = f.surface.borrow().as_ref().unwrap().opened;
        // Repeated opens/raises must not reset either baseline or the deadlines.
        win.start_surface_probe("replacement", 101);
        capture();
        win.sample_surface_at(100, String::new, opened + Duration::from_millis(999));
        assert!(LOG.with(|l| l.borrow().is_empty()));
        c.events.set(43);
        win.sample_surface_at(100, || "format=CLAP timers=0".into(), opened + Duration::from_secs(1));
        win.sample_surface_at(100, String::new, opened + Duration::from_secs(5));
        win.sample_surface_at(100, String::new, opened + Duration::from_secs(10));
        LOG.with(|l| {
            let l = l.borrow();
            assert_eq!(l.len(), 3);
            assert!(l[0].1.starts_with("editor surface: test: at=1s elapsed_ms=1000"));
            assert!(l[0].1.contains("timer_callbacks=0 x_events=3 format=CLAP timers=0"));
            assert!(l[1].1.starts_with("editor surface: test: at=5s"));
            assert_eq!(l[2].0, log::Level::Warn);
            assert!(l[2].1.starts_with("plug-in editor shows nothing: test: editor surface:"));
        });
        // Same empty surface, but a delivered callback: absence of a child alone must not WARN.
        f.surface.borrow_mut().as_mut().unwrap().next = 1;
        capture();
        win.sample_surface_at(101, String::new, opened + Duration::from_secs(5));
        LOG.with(|l| {
            let l = l.borrow();
            assert_eq!(l.len(), 1);
            assert!(l[0].1.contains("timer_callbacks=1 x_events=3"));
        });
        f.surface.borrow_mut().as_mut().unwrap().next = 1;
        win.sample_surface_at(
            101,
            || {
                win.start_surface_probe("reentrant", 999);
                win.sample_surface_at(101, String::new, opened + Duration::from_secs(5));
                String::new()
            },
            opened + Duration::from_secs(5),
        );
        win.close();
    }

    #[test]
    fn surface_deadlines_are_once_each_and_do_not_depend_on_events() {
        let mut p = Probe { name: String::new(), opened: Instant::now(), timers: 9, events: 4, next: 0 };
        assert_eq!(p.due(Duration::from_millis(999)), None);
        assert_eq!(p.due(Duration::from_secs(1)), Some(1));
        assert_eq!(p.due(Duration::from_secs(1)), None);
        assert_eq!(p.due(Duration::from_secs(5)), Some(5));
        assert_eq!(p.due(Duration::from_secs(50)), None);
    }

    #[test]
    fn surface_queries_distinguish_absent_unmapped_and_mapped_children() {
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        let win = EditorWindow::new("surface probe", 300.0, 200.0, None).unwrap();
        let (c, f) = win.live().unwrap();
        let empty = snapshot(&c, &f);
        assert_eq!(empty.no_mapped_child, Some(true));
        assert!(empty.text.contains("children=0"));
        let child = c.c.generate_id().unwrap();
        c.c.create_window(0, child, f.content, 0, 0, 123, 45, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap().check().unwrap();
        let hidden = snapshot(&c, &f);
        assert_eq!(hidden.no_mapped_child, Some(true));
        assert!(hidden.text.contains("children=1"));
        assert!(hidden.text.contains("size=123x45"));
        let atom = c.c.intern_atom(false, b"_XEMBED_INFO").unwrap().reply().unwrap().atom;
        c.c.change_property32(PropMode::REPLACE, child, atom, atom, &[0, 1]).unwrap().check().unwrap();
        assert!(snapshot(&c, &f).text.contains("_XEMBED_INFO=[0, 1]"), "a request to map, not proof of a mapped child");
        c.c.map_window(child).unwrap().check().unwrap();
        // Frame is still hidden: mapped child is UNVIEWABLE, not UNMAPPED.
        let mapped = snapshot(&c, &f);
        assert_eq!(mapped.no_mapped_child, Some(false));
        assert!(mapped.text.contains("UNVIEWABLE"));
        let screen = c.c.setup().roots.iter().find(|s| s.root == c.root).unwrap();
        assert!(mapped.text.contains(&format!("depth={} visual=0x{:x}", screen.root_depth, screen.root_visual)));
        win.close();
    }
}
