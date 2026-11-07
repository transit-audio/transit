//! ADR 0046 in Audio Devices: the effective backend drives the native controls for `Auto` and
//! former cpal users, a saved device opens the same endpoint, a missing one is said at startup and
//! on apply, and a native open failure is said with the device left closed.

use super::*;
use crate::audio_devices::startup_notes;

/// A config saved with `"backend": "cpal"` loads as `Auto` on Windows, and on WASAPI shows the
/// exclusive-mode checkbox and Paired input with no Backend row. A cpal value set in memory (over
/// the control channel) shows them too: the effective backend decides, not the saved value.
/// Control: reverting a site to the saved value (`draft.backend == Backend::NativeWasapi`) hides
/// them here.
#[test]
fn a_loaded_cpal_config_on_wasapi_shows_the_native_controls() {
    let loaded: AudioConfig = serde_json::from_str(r#"{"backend":"cpal","system":"WASAPI"}"#).unwrap();
    let mut t = T::new();
    t.app.apply_audio(loaded).unwrap();
    t.open_window();
    if !transit_playback::devices::NATIVE_WASAPI_COMPILED {
        assert!(!has_control(&t, "share_device") && !has_control(&t, "paired_input"), "cpal has neither");
        return;
    }
    assert_eq!(t.app.audio_devices.draft.backend, Backend::Auto, "loaded as Auto");
    assert!(!has_control(&t, "backend") && has_control(&t, "share_device") && has_control(&t, "paired_input"));
    t.app.apply_audio(AudioConfig { backend: Backend::Cpal, system: Some("WASAPI".into()), ..Default::default() }).unwrap();
    t.frames(5);
    assert_eq!(t.app.audio_devices.draft.backend, Backend::Cpal);
    assert!(has_control(&t, "share_device") && has_control(&t, "paired_input"), "the effective backend is native WASAPI");
}

/// ADR 0046, decision 7: a config saved by the current build (system WASAPI, the friendly name of
/// the third endpoint, `backend: "cpal"`) resolves to that endpoint's id over a fake list of three in
/// default-first order, and loads as `Auto`. Controls: a broken matcher that always answers the
/// first endpoint fails the main assertion; a missing device resolves to index 0, the default, and
/// the note says so.
#[cfg(windows)]
#[test]
fn a_saved_device_opens_the_same_endpoint_when_it_is_not_the_first() {
    use transit_playback::backend::native::wasapi_saved_endpoint as resolve;
    let list = [
        ("Speakers (Realtek)", "{0.0.0.00000000}.{aaaa}"),
        ("Headphones (USB Audio)", "{0.0.0.00000000}.{bbbb}"),
        ("Speakers (Focusrite USB Audio)", "{0.0.0.00000000}.{cccc}"),
    ];
    let cfg: AudioConfig = serde_json::from_str(r#"{"backend":"cpal","system":"WASAPI","output":"Speakers (Focusrite USB Audio)"}"#).unwrap();
    assert_eq!(cfg.backend, Backend::Auto, "a saved cpal backend loads as Auto");
    let id = |at: Option<usize>| at.map(|i| list[i].1);
    assert_eq!(id(resolve(&list, cfg.output.as_deref())), Some("{0.0.0.00000000}.{cccc}"), "the saved endpoint, at index 2");
    assert_eq!(resolve(&list, Some("{0.0.0.00000000}.{cccc}")), Some(2), "its id names it too");
    let broken = |_: &[(&str, &str)], _: Option<&str>| Some(0usize);
    assert_ne!(id(broken(&list, cfg.output.as_deref())), Some("{0.0.0.00000000}.{cccc}"), "control: an always-first matcher fails it");
    let opened = |at: Option<usize>| Opened { output: at.map(|i| list[i].0.to_owned()), ..Default::default() };
    assert!(fallback_notes(&cfg, None, &opened(resolve(&list, cfg.output.as_deref()))).is_empty(), "the saved device opened: nothing to say");
    let missing = AudioConfig { output: Some("Gone Interface".into()), ..cfg };
    let at = resolve(&list, missing.output.as_deref());
    assert_eq!(at, Some(0), "control: a missing device opens the default endpoint");
    assert_eq!(fallback_notes(&missing, None, &opened(at)), ["Output “Gone Interface” is not available; using “Speakers (Realtek)”."]);
}

/// ADR 0046, decision 1: a saved output and input that are not there open the defaults and are said
/// on apply and at startup, on the status line and in Audio Devices. Before slice 3 the startup check
/// said only the output. Control: listed choices say nothing at startup.
#[test]
fn a_missing_saved_output_and_input_are_said_at_startup_and_on_apply() {
    let out = "Output “Gone Interface” is not available; using “Speakers (Realtek)”.";
    let input = "Input “Gone Input” is not available; recording uses the default input.";
    let mut t = T::new();
    let want = AudioConfig { output: Some("Gone Interface".into()), input: Some("Gone Input".into()), ..Default::default() };
    t.app.apply_audio(want).unwrap();
    assert!(t.app.status.contains(out) && t.app.status.contains(input), "on apply: {}", t.app.status);
    // Startup: the saved choices are open (the stand-in connection); the startup check says both.
    t.app.status.clear();
    t.app.audio_devices.notes.clear();
    startup_notes(&mut t.app);
    assert!(t.app.status.contains(out) && t.app.status.contains(input), "at startup: {}", t.app.status);
    assert!(t.app.status.ends_with("Choose another in Audio › Audio Devices."), "{}", t.app.status);
    assert_eq!(t.app.audio_devices.notes, [out, input]);
    t.open_window();
    assert_eq!(t.layout()["notes"], json!([out, input]), "the window shows both");
    let present = AudioConfig { output: Some("Speakers (Focusrite USB Audio)".into()), input: Some(SCARLETT_IN.into()), ..Default::default() };
    t.app.apply_audio(present).unwrap();
    t.app.status.clear();
    t.app.audio_devices.notes.clear();
    startup_notes(&mut t.app);
    assert!(t.app.status.is_empty() && t.app.audio_devices.notes.is_empty(), "control: {} {:?}", t.app.status, t.app.audio_devices.notes);
}

/// ADR 0046, decision 7: a native open that fails leaves the device closed (a silent Player) and is
/// said as "The audio device did not open: <reason>." on the status line and in Audio Devices, at
/// startup as on Reconnect; Reconnect opens it again once the driver answers.
#[test]
fn a_native_open_failure_is_said_with_the_device_closed_until_reconnect() {
    let driver = scarlett_driver();
    let again = driver.clone();
    let mut t = asio_rig(&driver, move |session, cfg| transit_playback::Player::with_fake_native_asio(session, cfg, &again));
    let reason = "ASIO driver \"Focusrite USB ASIO\": start failed";
    driver.fail_next_open(transit_audio_backend::OpenError::DeviceNotAvailable(reason.into()));
    t.app.reconnect_audio_devices();
    let said = format!("The audio device did not open: {reason}.");
    assert!(t.app.player.as_ref().is_some_and(|p| p.silent), "the device stays closed");
    assert!(t.app.status.contains(&said), "{}", t.app.status);
    assert!(t.app.audio_devices.notes.contains(&said), "{:?}", t.app.audio_devices.notes);
    t.app.status.clear();
    t.app.audio_devices.notes.clear();
    startup_notes(&mut t.app);
    assert!(t.app.status.contains(&said) && t.app.audio_devices.notes.contains(&said), "at startup: {}", t.app.status);
    t.app.reconnect_audio_devices();
    assert!(t.app.player.as_ref().is_some_and(|p| !p.silent), "Reconnect opens it");
    assert!(!t.app.status.contains("did not open"), "{}", t.app.status);
}
