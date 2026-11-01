//! Editor open requests and retries on the UI thread.

use super::*;

impl StudioApp {
    /// Ask for a plug-in editor window; it opens as soon as the plug-in is loaded.
    pub fn want_editor(&mut self, key: (TrackId, usize)) {
        if !self.ui.plugin_windows.contains(&key) {
            self.ui.plugin_windows.push(key);
        }
        if self.builtin_at(key).is_some() || self.editors.iter().any(|e| e.key == key) {
            return;
        }
        let now = self.last_frame.unwrap_or_default();
        let plugin =
            self.engine
                .session()
                .track(key.0)
                .and_then(|t| {
                    if key.1 == transit_mix::INSTRUMENT_SLOT { t.instrument.as_ref() } else { t.mixer.inserts.get(key.1).and_then(Option::as_ref) }
                })
                .map(|i| i.plugin.clone())
                .unwrap_or_default();
        let name = self
            .player
            .as_ref()
            .and_then(|p| p.live_name(key.0, key.1))
            .map(str::to_owned)
            .unwrap_or_else(|| self.plugin_name(&plugin, key.1 == transit_mix::INSTRUMENT_SLOT));
        self.editors.push(EditorWanted {
            key,
            name,
            deadline: now + 30.0,
            next_try: now,
            opened: false,
            requested: false,
            waiting_logged: false,
            settle_until: 0.0,
        });
    }

    /// Open the editors asked for once their plug-ins are loaded; forget the ones the user closed.
    pub(super) fn drive_editors(&mut self) {
        // Consume completed failures before checking the timeout in this frame.
        self.plugin_host_status();
        let now = self.last_frame.unwrap_or_default();
        let p: &dyn audio_connection::Editors = if let Some(fake) = self.audio_devices.fake.as_ref().and_then(|f| f.connection.as_ref()) {
            fake
        } else if let Some(p) = &self.player {
            p
        } else {
            self.editors.clear();
            return;
        };
        let mut keep = Vec::new();
        let mut closed = Vec::new();
        for mut w in std::mem::take(&mut self.editors) {
            if p.editor_open(w.key.0, w.key.1) && !p.editor_opening(w.key.0, w.key.1) {
                w.opened = true;
                keep.push(w);
                continue;
            }
            if w.opened && now < w.settle_until {
                keep.push(w);
                continue;
            }
            if w.opened {
                closed.push(w.key);
                continue;
            }
            if w.requested {
                if now >= w.deadline {
                    self.status = w.timed_out();
                    closed.push(w.key);
                    continue;
                }
                w.waiting("the plug-in host thread");
            } else if now >= w.next_try {
                match p.open_editor(w.key.0, w.key.1) {
                    Ok(()) => w.requested = true,
                    Err(e) if !waiting_for_host(&e) => {
                        self.status = format!("Plug-in editor: {e}");
                        closed.push(w.key);
                        continue;
                    }
                    Err(_) if now >= w.deadline => {
                        self.status = w.timed_out();
                        closed.push(w.key);
                        continue;
                    }
                    Err(e) => {
                        w.waiting(&e);
                        w.next_try = now + 0.5;
                    }
                }
            }
            keep.push(w);
        }
        self.editors = keep;
        self.ui.plugin_windows.retain(|k| !closed.contains(k));
    }
}

// Permanent format failures have already logged their warning. Retry only a pending load
// or a busy host; an inline editor failure must not emit another WARN each half second.
fn waiting_for_host(reason: &str) -> bool {
    reason == "this plugin instance is not active" || reason == "the plug-in is not responding (its host thread is stuck)"
}

impl EditorWanted {
    fn waiting(&mut self, reason: &str) {
        if !self.waiting_logged {
            log::info!("plug-in editor: {}: waiting for {reason}", self.name);
            self.waiting_logged = true;
        }
    }

    fn timed_out(&self) -> String {
        let reason = "timed out waiting for the plug-in host thread after 30 s";
        log::warn!("plug-in editor not opened: {}: {reason}", self.name);
        format!("Plug-in editor: {reason}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    thread_local! { static LOG: RefCell<Vec<(log::Level, String)>> = const { RefCell::new(Vec::new()) }; }
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
    fn setup(pending: bool) -> StudioApp {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            log::set_logger(&Capture).unwrap();
            log::set_max_level(log::LevelFilter::Info);
        });
        let mut app = crate::tests::app();
        let (request, _) = crate::ControlRequest::new(
            "audio.fake_devices",
            json!({"systems": [{"name": "WASAPI", "is_default": true, "outputs": [{"name": "Speakers", "is_default": true}], "inputs": []}]}),
        );
        crate::control::handle_for_test(&mut app, &egui::Context::default(), &request).unwrap();
        let connection = app.audio_devices.fake.as_mut().unwrap().connection.as_mut().unwrap();
        connection.editor_pending = pending;
        connection.editor_error = (!pending).then(|| "this plugin instance is not active".into());
        app.last_frame = Some(0.0);
        app.want_editor((TrackId(1), 0));
        app.editors[0].name = "Fixture Gain".into();
        LOG.with(|l| l.borrow_mut().clear());
        app
    }
    fn times_out_once(pending: bool) {
        let mut app = setup(pending);
        for stamp in [0.0, 0.5, 1.0, 10.0, 29.5] {
            app.last_frame = Some(stamp);
            app.drive_editors();
            assert_eq!(app.editors.len(), 1, "still waiting at {stamp}");
            assert!(!app.editors[0].opened, "an accepted open is not yet an opened editor");
        }
        app.last_frame = Some(30.0);
        app.drive_editors();
        assert!(app.editors.is_empty());
        app.last_frame = Some(31.0);
        app.drive_editors();
        assert_eq!(app.status, "Plug-in editor: timed out waiting for the plug-in host thread after 30 s");
        LOG.with(|l| {
            let log = l.borrow();
            assert_eq!(log.iter().filter(|(level, s)| *level == log::Level::Info && s.contains("waiting for")).count(), 1);
            let warnings = log.iter().filter(|(level, _)| *level == log::Level::Warn).map(|(_, s)| s.as_str()).collect::<Vec<_>>();
            assert_eq!(warnings, ["plug-in editor not opened: Fixture Gain: timed out waiting for the plug-in host thread after 30 s"]);
        });
    }
    #[test]
    fn refused_requests_wait_once_and_warn_once_at_the_injected_deadline() {
        times_out_once(false);
    }
    #[test]
    fn accepted_requests_still_opening_wait_once_and_warn_once_at_the_injected_deadline() {
        times_out_once(true);
    }
    #[test]
    fn a_permanent_inline_failure_is_terminal_without_retrying() {
        let mut app = setup(false);
        app.audio_devices.fake.as_mut().unwrap().connection.as_mut().unwrap().editor_error = Some("create failed".into());
        app.drive_editors();
        assert!(app.editors.is_empty());
        assert_eq!(app.status, "Plug-in editor: create failed");
        app.last_frame = Some(30.0);
        app.drive_editors();
        assert_eq!(app.status, "Plug-in editor: create failed", "no spurious timeout replaces the host's reason");
        LOG.with(|l| assert!(l.borrow().is_empty(), "the caller must not log the format host's failure again"));
    }
}
