//! ADR 0015 (amendment #431) rule 6 at the app's one reopen seam (`audio_connection::replace`):
//! Apply, both Reconnects and the automatic reopen after a reset (what a JACK quantum change asks
//! for) read the links other programs made before anything of the old device closes, restore them
//! once on the replacement after it opened, and show what did not come back. The JACK side of the
//! snapshot is tested over the fake server in `transit-audio-jack` (`links_tests.rs`).

use super::*;

const LOST: &str = "JACK: 1 link made outside transit was not restored after the reopen: in_1 from Player:out_L (the port is gone).";

/// The open fake device has links other programs made; `note` is what their restore will say.
fn with_links(t: &mut T, note: &str) {
    let fake = t.app.audio_devices.fake.as_mut().unwrap();
    fake.connection.as_mut().expect("a fake device is open").links = Some(note.to_owned());
    fake.trace.borrow_mut().clear();
}

fn trace(t: &T) -> Vec<&'static str> {
    t.app.audio_devices.fake.as_ref().unwrap().trace.borrow().clone()
}

/// The snapshot came first, before the editors closed and the old device dropped; the restore came
/// once, after the replacement opened.
fn assert_one_restore_around_the_reopen(trace: &[&'static str]) {
    let at = |step: &str| trace.iter().position(|s| *s == step).unwrap_or_else(|| panic!("no {step} in {trace:?}"));
    assert_eq!(trace.first(), Some(&"snapshot links"), "{trace:?}");
    assert!(at("snapshot links") < at("close editors") && at("close editors") < at("drop output"), "{trace:?}");
    assert!(at("drop output") < at("open output") && at("open output") < at("restore links"), "{trace:?}");
    assert_eq!(trace.iter().filter(|s| **s == "snapshot links").count(), 1, "{trace:?}");
    assert_eq!(trace.iter().filter(|s| **s == "restore links").count(), 1, "{trace:?}");
}

#[test]
fn apply_restores_the_links_and_says_what_did_not_come_back() {
    let mut t = T::new();
    with_links(&mut t, "");
    t.app.apply_audio(AudioConfig { input: Some(SCARLETT_IN.into()), ..Default::default() }).unwrap();
    assert_one_restore_around_the_reopen(&trace(&t));
    assert_eq!(t.app.audio_devices.link_note, None, "everything came back: nothing to say");

    with_links(&mut t, LOST);
    t.app.apply_audio(AudioConfig::default()).unwrap();
    assert_one_restore_around_the_reopen(&trace(&t));
    assert!(t.app.audio_devices.notes.iter().any(|n| n == LOST), "{:?}", t.app.audio_devices.notes);
    assert!(t.app.status.contains(LOST), "{}", t.app.status);
}

#[test]
fn reconnect_restores_the_links_from_the_window_and_from_the_menu() {
    let mut t = T::new();
    with_links(&mut t, LOST);
    t.app.reconnect_audio_devices();
    assert_one_restore_around_the_reopen(&trace(&t));
    assert!(t.app.status.contains(LOST), "{}", t.app.status);

    with_links(&mut t, LOST);
    t.app.status.clear();
    t.app.reconnect_audio();
    assert_one_restore_around_the_reopen(&trace(&t));
    assert_eq!(t.app.status, LOST);
}

/// A JACK quantum or rate change reaches the app as a reset request, which the automatic reopen
/// serves through the same seam.
#[test]
fn the_automatic_reopen_after_a_reset_restores_the_links() {
    let mut t = T::new();
    with_links(&mut t, LOST);
    assert!(t.app.reopen.trouble(transit_playback::reopen::ReopenCause::DriverReset, 0.0));
    t.app.last_frame = Some(transit_playback::reopen::REOPEN_BACKOFF[0]);
    t.app.step_reopen();
    assert_one_restore_around_the_reopen(&trace(&t));
    assert!(t.app.status.ends_with(LOST), "{}", t.app.status);
}

/// A device without links of other programs (every system but native JACK): nothing is read and
/// nothing restored.
#[test]
fn a_reopen_with_no_links_restores_nothing() {
    let mut t = T::new();
    t.app.audio_devices.fake.as_ref().unwrap().trace.borrow_mut().clear();
    t.app.apply_audio(AudioConfig::default()).unwrap();
    let trace = trace(&t);
    assert!(!trace.contains(&"snapshot links") && !trace.contains(&"restore links"), "{trace:?}");
    assert_eq!(t.app.audio_devices.link_note, None);
}
