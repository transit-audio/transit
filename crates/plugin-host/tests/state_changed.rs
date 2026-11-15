//! Issue #574: a plug-in's own "state changed" notice (CLAP `mark_dirty`, VST3 `setDirty`) is taken
//! from its editor handle by the host thread's tick and handed to the UI's proxy once, so the app
//! can mark the session modified.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Fake, Log};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use transit_plugin_host::{Config, Host};

#[test]
fn a_state_changed_notice_reaches_the_proxy_once() {
    let host = Host::new(Config { inline: false, ..Config::default() });
    let log = Arc::new(Log::default());
    let l = Arc::clone(&log);
    let mut plugin = host.submit(move |ctx| ctx.adopt(Fake::make(&l, Duration::ZERO, Duration::ZERO))).wait(Duration::from_secs(5)).unwrap();
    let mut editor = plugin.editor().unwrap();
    assert!(!editor.take_state_changed(), "nothing reported yet");
    log.state_changed.store(true, Ordering::SeqCst);
    // The host thread's tick (every 15 ms) takes it from the editor; wait for that handshake.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = false;
    while !seen && Instant::now() < deadline {
        seen = editor.take_state_changed();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(seen, "the notice reached the proxy");
    assert!(!log.state_changed.load(Ordering::SeqCst), "taken from the editor");
    assert!(!editor.take_state_changed(), "once");
    drop((editor, plugin));
    assert!(host.shutdown(Duration::from_secs(5)));
}
