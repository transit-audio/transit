//! Audio › Audio Devices, I/O paths section tests.

use super::*;
use transit_model::IoMap;

pub(super) fn with_window() -> T {
    let mut t = T::new();
    t.app.engine.set_io_map(IoMap::for_device("ASIO", "TASCAM Model 12", 4, 4));
    t.open_window();
    t.section("section_io");
    t
}

pub(super) fn item(t: &T, value: &str, label: &str) -> [f32; 4] {
    T::rect(&t.layout()["controls"], Some(label), value)
}

#[test]
fn the_panel_shows_the_engine_table_and_the_opened_channel_counts() {
    let t = with_window();
    let io = t.layout()["io"].clone();
    assert_eq!((io["inputs"].clone(), io["outputs"].clone(), io["main"].clone()), (json!(4), json!(4), json!("Out 1-2")));
    // Its rows are drawn: Remove is listed for every path.
    for name in ["Out 1-2", "Out 3-4", "In 1-2"] {
        item(&t, &format!("io_remove:{name}"), "Remove");
    }
}

#[test]
fn a_channel_edit_is_exactly_one_setup_io_map_command_without_an_undo_step() {
    let mut t = with_window();
    let (revision, undo, doc) = (t.app.engine.io_revision(), t.app.engine.can_undo(), t.app.engine.revision);
    // Opening the channel picker is not an edit.
    t.click(mid(item(&t, "io_channels:Out 3-4", "Channels")));
    assert_eq!(t.app.engine.io_revision(), revision);
    t.click(mid(item(&t, "io_channel:Out 3-4", "1")));
    assert_eq!(t.app.engine.io_revision(), revision + 1, "one click, one command");
    assert_eq!(t.app.engine.io_map().path("Out 3-4").unwrap().channels, [0, 2, 3]);
    assert_eq!((t.app.engine.can_undo(), t.app.engine.revision), (undo, doc), "no undo entry, no document change");
    // The panel reads the engine's table again: the overlap with Out 1-2 is said.
    assert_eq!(t.layout()["io"]["paths"].as_array().unwrap().len(), t.app.engine.io_map().paths.len());
    assert_eq!(crate::audio_devices_io::overlap_lines(t.app.engine.io_map()).len(), 1);
}

#[test]
fn a_refused_edit_reaches_the_status_line_and_changes_nothing() {
    let mut t = with_window();
    let revision = t.app.engine.io_revision();
    t.click(mid(item(&t, "io_channels:Out 3-4", "Channels")));
    t.click(mid(item(&t, "io_channel:Out 3-4", "3")));
    t.click(mid(item(&t, "io_channel:Out 3-4", "4")));
    assert_eq!(t.app.engine.io_revision(), revision + 1, "the first removal is fine, the last channel is refused");
    assert_eq!(t.app.engine.io_map().path("Out 3-4").unwrap().channels, [3]);
    t.click(mid(item(&t, "io_channel:Out 3-4", "4")));
    assert_eq!(t.app.engine.io_revision(), revision + 1);
    assert!(t.app.status.contains("1 to 16"), "{}", t.app.status);
}

#[test]
fn the_channel_picker_lists_every_channel_of_a_12_in_10_out_driver_by_its_name() {
    use crate::audio_devices_io::{ChannelNames, channel_text};
    let mut t = T::new();
    t.app.engine.set_io_map(IoMap::for_device("ASIO", "Model Mixer ASIO Driver", 12, 10));
    let names = |prefix: &str, n: u16| (1..=n).map(|c| format!("{prefix} {c}")).collect();
    t.app.audio_devices.fake_channel_names = Some(ChannelNames { outputs: names("OUT", 10), inputs: names("IN", 12) });
    t.open_window();
    t.section("section_io");
    assert_eq!((t.layout()["io"]["inputs"].clone(), t.layout()["io"]["outputs"].clone()), (json!(12), json!(10)));
    t.click(mid(item(&t, "io_channels:In 1", "Channels")));
    let picks: Vec<String> = t.layout()["controls"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["value"] == "io_channel:In 1")
        .map(|c| c["label"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(picks, (1..=12).map(|c| format!("IN {c}")).collect::<Vec<_>>(), "every input, by the driver's name");
    t.click(mid(item(&t, "io_channels:Out 1-2", "Channels")));
    t.click(mid(item(&t, "io_channel:Out 1-2", "OUT 8")));
    assert_eq!(t.app.engine.io_map().path("Out 1-2").unwrap().channels, [0, 1, 7], "a named channel is a channel like any other");
    assert_eq!(channel_text(0, &["Mic/Line".into()]), "1 Mic/Line");
    assert_eq!(channel_text(2, &[]), "3", "unnamed channels keep their number");
}

#[test]
fn choosing_main_is_one_command_and_a_wasapi_host_is_noted() {
    let mut t = with_window();
    let revision = t.app.engine.io_revision();
    t.click(mid(item(&t, "io_main", "Out 1-2")));
    t.click(mid(item(&t, "io_main_entry", "Out 3-4")));
    assert_eq!(t.app.engine.io_revision(), revision + 1);
    assert_eq!(t.app.engine.io_map().main, "Out 3-4");
    // The table names ASIO: no WASAPI note.
    assert!(t.layout()["controls"].as_array().unwrap().iter().all(|c| c["value"] != "io_wasapi_note"));
    // A table with no system yet (nothing opened) takes the chosen system, WASAPI here.
    let mut t = T::new();
    t.open_window();
    t.section("section_io");
    item(&t, "io_wasapi_note", crate::audio_devices_io::WASAPI_NOTE);
}

/// An ALSA-like system (#429): the output opens the table without inputs, and the input opens as a
/// device of its own (not with the output), `inputs` wide. No device is listed or opened.
fn separate_input_rig(inputs: u16) -> (T, transit_audio_backend::fake::FakeBackend) {
    use transit_audio_backend as ab;
    let driver = ab::fake::FakeBackend::new(ab::Format::new(48_000, 2, ab::SampleFormat::F32)).named("ALSA", "pipewire").with_capture(inputs, false);
    let cfg = AudioConfig { system: Some("ALSA".into()), output: Some("pipewire".into()), ..Default::default() };
    let mut t = T::new();
    t.app.player = Some(transit_playback::Player::open_fake_native_asio(t.app.engine.session_arc(), &cfg, &driver).unwrap());
    t.app.ui.audio = cfg;
    t.app.audio_devices.fake = None;
    t.app.add_track("audio").unwrap();
    t.frames(2);
    assert_eq!(t.app.engine.io_map().inputs, 0, "the output's open knows no input count");
    (t, driver)
}

/// Arms the first track, so the app opens the input on its next frames.
fn arm(t: &mut T) {
    t.app.engine.session_mut().tracks[0].mixer.record_arm = true;
    t.frames(3);
}

/// #429: the page read "0 in" with Add disabled while the separate input ran 32 channels. The
/// input's count now reaches the table, with its default input paths.
#[test]
fn a_separate_input_gives_the_page_its_channel_count_and_input_paths() {
    let (mut t, _driver) = separate_input_rig(32);
    arm(&mut t);
    assert!(t.app.recorder.as_ref().is_some_and(|r| r.device_channels() == 32), "the input opened");
    assert_eq!(t.app.engine.io_map().inputs, 32);
    assert!(t.app.engine.io_map().path("In 1-2").is_some());
    t.open_window();
    t.section("section_io");
    assert_eq!(t.layout()["io"]["inputs"], json!(32));
}

/// #429: a failed input open read "Input open: none yet"; it now says why, on the Audio Devices
/// card and so in Copy diagnostics, which copies the card's lines.
#[test]
fn a_failed_input_open_says_why_on_the_card() {
    let (mut t, driver) = separate_input_rig(32);
    driver.fail_next_open(transit_audio_backend::OpenError::Other("snd_pcm_open: Device or resource busy".into()));
    arm(&mut t);
    assert!(t.app.recorder.is_none());
    let lines = crate::audio_devices::opened_lines(&mut t.app, 0.0);
    assert!(lines.contains(&"Input open: failed: snd_pcm_open: Device or resource busy".to_string()), "{lines:?}");
    assert!(!lines.iter().any(|l| l == crate::audio_devices::INPUT_NOT_YET), "{lines:?}");
}
